//! SQLite trunk updates for the managed macOS profile.
mod cli_transaction;
mod python;

use super::{account_home, check_dir, check_file, command, exists, is_hex, open_file, tool_command, user_only};
use crate::command::CommandExt as _;
use color_eyre::eyre::{Context, Result, ensure};
use nix::libc::{O_NOFOLLOW, O_NONBLOCK};
use std::{
    fs::{DirBuilder, File, OpenOptions},
    io::{self, Read},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

const SOURCE: &str = "Developer/sqlite-git";
const BIN: &str = ".local/bin/sqlite3-trunk";
const STATE: &str = ".local/state/sqlite-trunk";
const OFFICIAL: &str = "https://github.com/sqlite/sqlite.git";
const FILE_LIMIT: u64 = 128 * 1024 * 1024;
const SQL: &str = "CREATE TABLE t(id INTEGER PRIMARY KEY); INSERT INTO t VALUES(1); SELECT id FROM t; PRAGMA quick_check; SELECT sqlite_source_id();";
static SYNCED: OnceLock<String> = OnceLock::new();

pub(super) fn run(python: bool) -> Result<()> {
    user_only()?;
    let home = account_home()?;
    let state = owned_directories(&home, Path::new(STATE), true)?;
    let bin_dir = owned_directories(&home, Path::new(".local/bin"), true)?;
    let lock_path = state.join("update.lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(&lock_path)?;
    check_file(&lock, &lock_path, true, 0)?;
    lock.try_lock().wrap_err("Another SQLite trunk update is active")?;
    cli_transaction::recover(&state, &bin_dir.join("sqlite3-trunk"))?;
    if python {
        python::run(&home, &state)?;
    } else {
        ensure_cli(&home, &state)?;
    }
    Ok(())
}

fn owned_directories(root: &Path, relative: &Path, create: bool) -> Result<PathBuf> {
    let mut path = root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            color_eyre::eyre::bail!("Unsafe SQLite directory: {}", relative.display());
        };
        path.push(name);
        if create
            && let Err(error) = DirBuilder::new().mode(0o700).create(&path)
            && error.kind() != io::ErrorKind::AlreadyExists
        {
            return Err(error).wrap_err_with(|| format!("Create SQLite directory {}", path.display()));
        }
        check_dir(&path, false)?;
    }
    Ok(path)
}

fn read_small(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    open_file(path, false, limit)?.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "SQLite state exceeds its limit");
    Ok(bytes)
}

fn git(source: &Path) -> Result<Command> {
    let mut git = tool_command(Path::new("/usr/bin/git"))?;
    git.args([
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "http.sslVerify=true",
        "-c",
        "http.https://github.com/.sslVerify=true",
        "-c",
        "http.followRedirects=false",
        "-c",
        "http.https://github.com/.followRedirects=false",
        "-C",
    ])
    .arg(source)
    .env("GIT_TERMINAL_PROMPT", "0")
    .env("GIT_HTTP_LOW_SPEED_LIMIT", "1")
    .env("GIT_HTTP_LOW_SPEED_TIME", "30");
    Ok(git)
}

fn git_text(source: &Path, args: &[&str]) -> Result<String> {
    Ok(git(source)?.args(args).output_checked_utf8()?.stdout.trim().to_owned())
}

fn clean_source(source: &Path) -> Result<()> {
    ensure!(
        git_text(source, &["symbolic-ref", "--quiet", "--short", "HEAD"])? == "master",
        "SQLite source is not on master"
    );
    ensure!(
        git_text(source, &["remote", "get-url", "origin"])? == OFFICIAL
            && git_text(source, &["ls-remote", "--get-url", OFFICIAL])? == OFFICIAL,
        "SQLite source origin is not the approved HTTPS mirror"
    );
    ensure!(
        git_text(source, &["status", "--porcelain=v1", "--untracked-files=all"])?.is_empty(),
        "SQLite source has local modifications; update stopped"
    );
    Ok(())
}

struct Source {
    dir: PathBuf,
    revision: String,
    fossil: String,
}

impl Source {
    fn check(&self) -> Result<()> {
        clean_source(&self.dir)?;
        ensure!(
            git_text(&self.dir, &["rev-parse", "HEAD"])? == self.revision
                && String::from_utf8(read_small(&self.dir.join("manifest.uuid"), 128)?)?.trim() == self.fossil,
            "SQLite source changed during the update"
        );
        Ok(())
    }

    fn resolve(home: &Path) -> Result<Self> {
        let dir = owned_directories(home, Path::new(SOURCE), false)?;
        check_dir(&dir.join(".git"), false)?;
        clean_source(&dir)?;
        let revision = if let Some(revision) = SYNCED.get() {
            ensure!(
                git_text(&dir, &["rev-parse", "HEAD"])? == *revision,
                "SQLite source changed since this Topgrade run first verified it"
            );
            revision.clone()
        } else {
            git(&dir)?
                .args([
                    "fetch",
                    "--no-tags",
                    "--no-recurse-submodules",
                    OFFICIAL,
                    "refs/heads/master",
                ])
                .status_checked()?;
            let revision = git_text(&dir, &["rev-parse", "--verify", "FETCH_HEAD^{commit}"])?;
            ensure!(is_hex(&revision, 40), "Invalid SQLite Git revision");
            clean_source(&dir)?;
            git(&dir)?
                .args(["merge-base", "--is-ancestor", "HEAD", &revision])
                .status_checked()
                .wrap_err("SQLite master cannot fast-forward to the official revision")?;
            git(&dir)?.args(["merge", "--ff-only", &revision]).status_checked()?;
            clean_source(&dir)?;
            ensure!(
                git_text(&dir, &["rev-parse", "HEAD"])? == revision,
                "SQLite source revision drifted"
            );
            revision
        };
        let fossil = String::from_utf8(read_small(&dir.join("manifest.uuid"), 128)?)?
            .trim()
            .to_owned();
        ensure!(is_hex(&fossil, 64), "Invalid SQLite Fossil source identity");
        ensure!(
            String::from_utf8(read_small(&dir.join("manifest.uuid"), 128)?)?
                == git_text(&dir, &["show", "HEAD:manifest.uuid"])? + "\n",
            "SQLite manifest identity differs from the official revision"
        );
        Ok(Self { dir, revision, fossil })
    }

    fn build(&self, work: &Path, target: &str) -> Result<()> {
        self.check()?;
        let configure = self.dir.join("configure");
        let file = open_file(&configure, false, FILE_LIMIT)?;
        ensure!(
            file.metadata()?.permissions().mode() & 0o111 != 0,
            "SQLite configure is not executable"
        );
        tool_command(&configure)?
            .arg("--enable-all")
            .current_dir(work)
            .status_checked()?;
        let jobs = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
        tool_command(Path::new("/usr/bin/make"))?
            .arg(format!("-j{jobs}"))
            .arg(target)
            .current_dir(work)
            .status_checked()?;
        self.check()
    }
}

fn cli_source_id(binary: &Path, fossil: &str) -> Result<String> {
    let file = open_file(binary, false, FILE_LIMIT)?;
    ensure!(
        file.metadata()?.permissions().mode() & 0o111 != 0,
        "SQLite CLI is not executable"
    );
    let output = command(binary)
        .args(["-batch", "-bail", "-noheader", "-init", "/dev/null", ":memory:", SQL])
        .output_checked_utf8()?;
    ensure!(
        output.stderr.is_empty(),
        "SQLite CLI produced unexpected diagnostic output"
    );
    verified_source_id(&output.stdout, fossil)
}

fn verified_source_id(stdout: &str, fossil: &str) -> Result<String> {
    let lines = stdout.lines().collect::<Vec<_>>();
    ensure!(
        matches!(lines.as_slice(), ["1", "ok", id] if id.split_ascii_whitespace().count() == 3 && id.split_ascii_whitespace().last() == Some(fossil)),
        "SQLite CLI failed SQL, quick-check, or exact Fossil source verification"
    );
    Ok(lines[2].into())
}

fn ensure_cli(home: &Path, state: &Path) -> Result<(Source, String)> {
    let binary = home.join(BIN);
    let receipt = state.join("revision");
    let source = Source::resolve(home)?;
    let expected = format!("{}\n", source.revision);
    let source_id = if exists(&receipt)? && read_small(&receipt, 128)? == expected.as_bytes() && exists(&binary)? {
        let source_id = cli_source_id(&binary, &source.fossil)?;
        println!("SQLite trunk is current and verified: {}", source.revision);
        source_id
    } else {
        println!("Building SQLite trunk: {}", source.revision);
        let work = tempfile::Builder::new()
            .prefix(".sqlite-build.")
            .tempdir_in(home.join("Developer"))?;
        source.build(work.path(), "sqlite3")?;
        let candidate = work.path().join("sqlite3");
        let source_id = cli_source_id(&candidate, &source.fossil)?;
        cli_transaction::install(state, &binary, &candidate, expected.as_bytes(), || {
            ensure!(
                cli_source_id(&binary, &source.fossil)? == source_id,
                "Installed SQLite source differs from the candidate"
            );
            ensure!(
                read_small(&receipt, 128)? == expected.as_bytes(),
                "SQLite revision receipt failed readback"
            );
            source.check()
        })?;
        println!("SQLite trunk installed and verified: {}", source.revision);
        source_id
    };
    let _ = SYNCED.set(source.revision.clone());
    Ok((source, source_id))
}

fn sync_dir(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
#[test]
fn cli_verification_requires_all_sql_results_and_complete_source_id() {
    let fossil = "f".repeat(64);
    let identity = format!("2026-11-03 20:30:33 {fossil}");
    assert_eq!(
        verified_source_id(&format!("1\nok\n{identity}\n"), &fossil).unwrap(),
        identity
    );
    assert!(verified_source_id(&format!("1\nok\n{identity}a\n"), &fossil).is_err());
    assert!(verified_source_id(&format!("1\nwrong\n{identity}\n"), &fossil).is_err());
    assert!(verified_source_id(&format!("1\nok\n{identity}\nextra\n"), &fossil).is_err());
}

#[cfg(test)]
#[test]
fn source_guard_rejects_drift_dirty_worktree_and_unapproved_origin() {
    let repo = tempfile::tempdir().unwrap();
    let dir = repo.path();
    git(dir)
        .unwrap()
        .args(["init", "-q", "-b", "master"])
        .status_checked()
        .unwrap();
    git(dir)
        .unwrap()
        .args(["remote", "add", "origin", OFFICIAL])
        .status_checked()
        .unwrap();
    clean_source(dir).unwrap();
    std::fs::write(dir.join("dirty"), "user changes").unwrap();
    assert!(
        clean_source(dir)
            .unwrap_err()
            .to_string()
            .contains("local modifications")
    );
    std::fs::remove_file(dir.join("dirty")).unwrap();
    git(dir)
        .unwrap()
        .args(["symbolic-ref", "HEAD", "refs/heads/other"])
        .status_checked()
        .unwrap();
    assert!(clean_source(dir).unwrap_err().to_string().contains("not on master"));
    git(dir)
        .unwrap()
        .args(["symbolic-ref", "HEAD", "refs/heads/master"])
        .status_checked()
        .unwrap();
    git(dir)
        .unwrap()
        .args(["remote", "set-url", "origin", "https://example.invalid/sqlite.git"])
        .status_checked()
        .unwrap();
    assert!(clean_source(dir).unwrap_err().to_string().contains("approved HTTPS"));
}
