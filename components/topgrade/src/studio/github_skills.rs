//! Update only verified GitHub CLI-owned skills and protect the shared NPX lock.
use color_eyre::eyre::{Context, ContextCompat, Result, bail, ensure};
use regex::Regex;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{LazyLock, mpsc},
    thread,
    time::{Duration, Instant},
};

mod lock;
pub(super) mod ownership;
mod remote;

#[cfg(test)]
mod tests;

use crate::command::CommandExt as _;
use lock::{Lock, Snapshot};
use ownership::{Metadata, directory, metadata, preflight};

const COMMAND_LIMIT: Duration = Duration::from_secs(600);
const OUTPUT_LIMIT: u64 = 16 * 1024 * 1024;

struct Item {
    path: PathBuf,
    alias: String,
    repo: String,
    meta: Metadata,
    expected: String,
}

impl Item {
    fn keys(&self) -> [&str; 2] {
        [&self.alias, &self.meta.name]
    }

    fn skill_path(&self) -> String {
        if matches!(self.meta.directory.as_str(), "" | ".") {
            "SKILL.md".into()
        } else {
            format!("{}/SKILL.md", self.meta.directory)
        }
    }

    fn changed(&self) -> bool {
        self.meta.tree != self.expected
    }
}

fn hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

struct Output {
    success: bool,
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn gh(executable: &Path, args: &[&str]) -> Result<Output> {
    let mut child = super::tool_command(executable)?
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn_checked()
        .context("Could not start GitHub CLI")?;

    fn read(mut pipe: impl Read) -> Result<String> {
        let mut bytes = Vec::new();
        (&mut pipe).take(OUTPUT_LIMIT + 1).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= OUTPUT_LIMIT,
            "GitHub CLI output exceeded its verification limit"
        );
        String::from_utf8(bytes).context("GitHub CLI output was not valid UTF-8")
    }

    let (out_send, out_read) = mpsc::channel();
    let (err_send, err_read) = mpsc::channel();
    let result = (|| {
        let stdout = child.stdout.take().context("Missing GitHub CLI stdout")?;
        let stderr = child.stderr.take().context("Missing GitHub CLI stderr")?;
        thread::spawn(move || {
            let _ = out_send.send(read(stdout));
        });
        thread::spawn(move || {
            let _ = err_send.send(read(stderr));
        });
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            ensure!(
                started.elapsed() < COMMAND_LIMIT,
                "GitHub CLI did not complete before its deadline"
            );
            thread::sleep(Duration::from_millis(25));
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        let stdout = out_read
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .context("GitHub CLI stdout did not close")??;
        let stderr = err_read
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .context("GitHub CLI stderr did not close")??;
        Ok(Output {
            success: status.success(),
            code: status.code(),
            stdout,
            stderr,
        })
    })();

    if result.is_err() && !matches!(child.try_wait(), Ok(Some(_))) {
        let termination = child.kill();
        let reaped = child.wait();
        reaped.context("Could not reap GitHub CLI")?;
        termination.context("Could not terminate GitHub CLI")?;
    }
    result
}

fn accepted(output: &Output, group: &[&Item]) -> Result<()> {
    ensure!(output.success, "Native GH skill update failed (exit {:?})", output.code);
    static ANSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").expect("static ANSI regex"));
    static WARNING: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?im)!|✗|✘|^\s*Error:|\bSkipping\b").expect("static warning regex"));
    static EMPTY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\bno (?:installed |updatable )?skills\b|none of the specified skills")
            .expect("static no-skills regex")
    });

    let text = format!("{}\n{}", output.stdout, output.stderr);
    let text = ANSI.replace_all(&text, "");
    ensure!(
        !WARNING.is_match(&text),
        "Native GH skill updater reported a warning or error despite exit 0"
    );
    ensure!(
        !EMPTY.is_match(&text),
        "Native GH updater did not select the verified skills"
    );
    if text.lines().any(|line| line.trim() == "All skills are up to date.") {
        return Ok(());
    }
    let mut updates = text.lines().filter_map(|line| line.strip_prefix("Updated ")).peekable();
    ensure!(
        updates.peek().is_some() && updates.all(|name| group.iter().any(|item| item.keys().contains(&name.trim()))),
        "Native GH updater returned an unrecognized success summary"
    );
    Ok(())
}

fn resolve_expected(items: &mut [Item], run: &mut impl FnMut(&[&str]) -> Result<Output>) -> Result<()> {
    let mut directories = BTreeMap::<&str, BTreeSet<&str>>::new();
    for item in items.iter() {
        directories.entry(&item.repo).or_default().insert(&item.meta.directory);
    }
    let mut expected = BTreeMap::new();
    let mut missing = Vec::new();
    for (repo, paths) in directories {
        expected.insert(repo.to_owned(), remote::expectations(run, repo, &paths, &mut missing)?);
    }
    ensure!(
        missing.is_empty(),
        "Remote skill preflight failed before any updates:\n{}",
        missing.join("\n")
    );
    for item in items {
        item.expected = expected[&item.repo][directory(&item.meta.directory)?].clone();
    }
    Ok(())
}

fn apply_group(
    home: &Path,
    parent: &Path,
    group: &[&Item],
    snapshot: &Snapshot,
    protected: bool,
    run: &mut impl FnMut(&[&str]) -> Result<Output>,
) -> Result<Snapshot> {
    let before = Snapshot::read(home)?;
    ensure!(&before == snapshot, "Shared skill lock changed after preflight");
    for item in group {
        ensure!(
            metadata(&item.path)? == item.meta,
            "Skill metadata changed after preflight: {}",
            item.path.display()
        );
    }
    if protected {
        // All remote hashes were already verified current. A native call could select a newer ref
        // and write away fields that the current GitHub CLI cannot preserve.
        return Ok(before);
    }

    let parent = parent.to_str().context("Non-UTF-8 GitHub skill root")?;
    let mut args = vec!["skill", "update", "--dir", parent, "--all"];
    let mut names = group.iter().map(|item| item.meta.name.as_str()).collect::<Vec<_>>();
    names.sort_unstable();
    args.extend(names);
    let native = run(&args).and_then(|output| accepted(&output, group));
    let after = Snapshot::read(home)
        .and_then(|after| {
            before.verify_after(&after, group, native.is_ok())?;
            Ok(after)
        })
        .context("Shared lock verification failed; restore the verified pre-update backup before retrying")?;
    if let Err(error) = native {
        ensure!(
            after == before,
            "{error}; shared lock changed; restore the verified pre-update backup before retrying"
        );
        return Err(error);
    }
    for item in group {
        let after = metadata(&item.path)
            .context("Read GitHub skill metadata after update; verify the pre-update backup before retrying")?;
        ensure!(
            after.name == item.meta.name
                && after.source == item.meta.source
                && after.directory == item.meta.directory
                && after.tree == item.expected,
            "GitHub skill update verification failed: {}; restore the verified pre-update backup before retrying",
            item.path.display()
        );
    }
    Ok(after)
}

#[derive(Debug, PartialEq, Eq)]
struct Summary {
    github: usize,
    npx: usize,
    cursor: usize,
    updated: usize,
}

fn update(home: &Path, run: &mut impl FnMut(&[&str]) -> Result<Output>) -> Result<Summary> {
    let listing = run(&["skill", "list", "--json", "skillName,sourceURL,path,pinned,scope"])?;
    ensure!(
        listing.success,
        "GitHub CLI skill discovery failed (exit {:?})",
        listing.code
    );
    let rows = serde_json::from_str(&listing.stdout)
        .map_err(|_| color_eyre::eyre::eyre!("Unsupported GitHub CLI skill discovery schema"))?;
    let mut snapshot = Snapshot::read(home)?;
    let (mut selected, delegated) = preflight(rows, home, &snapshot.lock)?;
    resolve_expected(&mut selected, run)?;
    snapshot.lock.collisions(&selected)?;
    let planned = selected.iter().filter(|item| item.changed()).count();
    let protected = snapshot.lock.protected();
    ensure!(
        !protected || planned == 0,
        "{planned} GH skill update(s) pending; native GH cannot preserve NPX lock metadata. No skills or lock entries were changed."
    );
    let mut groups = BTreeMap::<&Path, Vec<&Item>>::new();
    for item in &selected {
        let parent = item.path.parent().context("GitHub skill directory has no parent")?;
        groups.entry(parent).or_default().push(item);
    }
    for (parent, group) in groups {
        snapshot = apply_group(home, parent, &group, &snapshot, protected, run)?;
    }
    Ok(Summary {
        github: selected.len(),
        npx: delegated.npx,
        cursor: delegated.cursor,
        updated: planned,
    })
}

pub(super) fn run() -> Result<()> {
    ensure!(
        std::env::var_os("XDG_STATE_HOME").is_none_or(|value| value.is_empty()),
        "Non-default shared skill lock ownership needs review"
    );
    let home = super::account_home()?;
    let executable = Path::new("/opt/homebrew/bin/gh")
        .canonicalize()
        .context("GitHub CLI is unavailable")?;
    let metadata = std::fs::metadata(&executable)?;
    use std::os::unix::fs::PermissionsExt as _;
    if !executable.starts_with("/opt/homebrew/Cellar/gh")
        || !metadata.is_file()
        || metadata.permissions().mode() & 0o111 == 0
        || metadata.permissions().mode() & 0o022 != 0
    {
        bail!("GitHub CLI does not resolve to the expected Homebrew executable");
    }
    let summary = update(&home, &mut |args| gh(&executable, args))?;
    println!(
        "Verified {} GitHub skills ({} updated); delegated {} to NPX and {} to Cursor.",
        summary.github, summary.updated, summary.npx, summary.cursor,
    );
    Ok(())
}
