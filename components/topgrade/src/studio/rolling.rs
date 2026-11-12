//! Local project dependency maintenance. Package managers receive explicit arguments.
mod files;
mod plan;
mod process;
mod specialized;

use std::{
    collections::BTreeSet,
    fs::{File, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::Mutex,
    thread,
    time::Instant,
};

use chrono::Local;
use color_eyre::eyre::{Context, Result, ensure};
use nix::libc::{O_NOFOLLOW, O_NONBLOCK};
use serde::{Deserialize, Serialize};

use self::plan::{Action, Surface};
use crate::error::PartialStep;

static CARGO_LOCK: Mutex<()> = Mutex::new(());

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Config {
    roots: Vec<PathBuf>,
    max_depth: usize,
    policy: Option<String>,
    execution: Execution,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            roots: vec!["~/Downloads/GitHub".into()],
            max_depth: 4,
            policy: None,
            execution: Execution::default(),
        }
    }
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Execution {
    jobs: usize,
}

impl Default for Execution {
    fn default() -> Self {
        Self { jobs: 3 }
    }
}

impl Config {
    fn read(home: &Path) -> Result<Self> {
        let path = home.join(".config/project-deps.json");
        let config: Self = if super::exists(&path)? {
            serde_json::from_slice(&files::read(&path, 32 * 1024)?.bytes)
                .wrap_err("Invalid project-deps.json configuration")?
        } else {
            Self::default()
        };
        ensure!(
            config.roots.len() <= 64
                && config.max_depth <= 32
                && (1..=32).contains(&config.execution.jobs)
                && matches!(config.policy.as_deref(), None | Some("edge" | "compatible")),
            "Invalid rolling dependency policy or scan limits"
        );
        Ok(config)
    }

    fn repos(&self, home: &Path) -> Result<Vec<PathBuf>> {
        let mut repos = BTreeSet::new();
        for configured in &self.roots {
            let root = if let Ok(rest) = configured.strip_prefix("~") {
                home.join(rest)
            } else {
                ensure!(
                    configured.is_absolute(),
                    "Dependency roots must be absolute paths or relative to ~/."
                );
                configured.clone()
            };
            if !super::exists(&root)? {
                continue;
            }
            repos.extend(files::discover(&root.canonicalize()?, self.max_depth)?);
        }
        Ok(repos.into_iter().collect())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum Status {
    Current,
    Rolled,
    Failed,
    Partial,
    Breaking,
    Discovered,
    UpstreamMissing,
    Retryable,
    PolicySkipped,
    Skipped,
}

impl Status {
    fn failed(self) -> bool {
        matches!(self, Self::Failed | Self::Partial | Self::Breaking | Self::Retryable)
    }

    fn deferred(self) -> bool {
        matches!(self, Self::Skipped | Self::UpstreamMissing)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Current => "CURRENT",
            Self::Rolled => "ROLLED",
            Self::Failed => "FAILED",
            Self::Partial => "PARTIAL",
            Self::Breaking => "BREAKING",
            Self::Discovered => "DISCOVERY NEEDS REVIEW",
            Self::UpstreamMissing => "UPSTREAM_MISSING",
            Self::Retryable => "RETRYABLE",
            Self::PolicySkipped => "SKIPPED BY POLICY",
            Self::Skipped => "SKIPPED",
        }
    }
}

#[derive(Serialize)]
struct Record<'a> {
    time: String,
    run: &'a str,
    repo: &'a Path,
    surface: &'static str,
    level: &'static str,
    status: Status,
    changed: Vec<PathBuf>,
    failures: Vec<String>,
    duration_s: f64,
}

impl Record<'_> {
    fn persist(&self, history: &Mutex<File>) -> Result<()> {
        let mut history = history
            .lock()
            .map_err(|_| color_eyre::eyre::eyre!("Rolling history lock was poisoned"))?;
        let mut entry = serde_json::to_vec(self)?;
        entry.push(b'\n');
        history.write_all(&entry)?;
        history.sync_data()?;
        Ok(())
    }
}

fn execute(surface: &Surface, log: &mut File) -> Result<(Status, Vec<String>)> {
    match &surface.action {
        Action::Commands(commands) => {
            for args in commands {
                let pretty = shell_words::join(args);
                writeln!(log, "$ {pretty}")?;
                let Some(executable) = process::find_tool(&args[0])? else {
                    return Ok((Status::Skipped, vec![format!("missing command: {}", args[0])]));
                };
                let _cargo = if args[0] == "cargo" {
                    Some(
                        CARGO_LOCK
                            .lock()
                            .map_err(|_| color_eyre::eyre::eyre!("Cargo update lock was poisoned"))?,
                    )
                } else {
                    None
                };
                let mut child = process::command(&executable, &surface.root);
                child.args(&args[1..]);
                if args[0] == "pinact" {
                    process::pinact_auth(&mut child, &surface.root)?;
                }
                let output = process::capture(child, process::UPDATE_TIMEOUT)?;
                writeln!(log, "[exit {}]", output.status)?;
                if !output.status.success() {
                    let breaking = args[0] == "cargo"
                        && output.contains_any(&["failed to select a version", "could not resolve this conflict"]);
                    return Ok((
                        if breaking { Status::Breaking } else { Status::Failed },
                        vec![format!("{pretty} → {}", output.status)],
                    ));
                }
            }
            Ok((Status::Current, Vec::new()))
        }
        Action::Specialized(kind, edge) => specialized::run(*kind, &surface.root, *edge, log),
    }
}

fn process_repo(
    repo: &Path,
    surfaces: &[Surface],
    run: &Path,
    run_id: &str,
    history: &Mutex<File>,
) -> Result<(bool, usize)> {
    if surfaces.is_empty() {
        return Ok((true, 0));
    }
    let stem = files::repo_stem(repo)?;
    let before = if files::git_clean(repo, "initial")? {
        let snapshot = files::snapshot(repo)?;
        files::git_clean(repo, "after dependency snapshot")?.then_some(snapshot)
    } else {
        None
    };
    let mut log = super::create_file(&run.join(format!("{stem}.log")))?;
    writeln!(log, "# initial worktree clean: {}", before.is_some())?;
    let Some(before) = before else {
        writeln!(
            log,
            "# {} dependency surfaces deferred: uncommitted project changes",
            surfaces.len()
        )?;
        log.sync_data()?;
        for surface in surfaces {
            Record {
                time: Local::now().to_rfc3339(),
                run: run_id,
                repo,
                surface: surface.name,
                level: surface.level,
                status: Status::Skipped,
                changed: Vec::new(),
                failures: vec!["uncommitted project changes".into()],
                duration_s: 0.0,
            }
            .persist(history)?;
        }
        return Ok((true, surfaces.len()));
    };
    let backup = files::backup(repo, &before, run, &stem)?;
    writeln!(log, "# repository: {}\n# backup: {}", repo.display(), backup.display())?;
    let mut current = before;
    let mut verified = true;
    let mut deferred = 0;
    for surface in surfaces {
        ensure!(
            !crate::ctrlc::interrupted(),
            "Rolling dependency maintenance interrupted"
        );
        ensure!(
            files::snapshot(repo)? == current,
            "Dependency files changed before starting {} in {}",
            surface.name,
            repo.display()
        );
        writeln!(
            log,
            "\n## {} @ {}\nlevel={}",
            surface.name,
            surface.root.display(),
            surface.level
        )?;
        log.sync_data()?;
        let started = Instant::now();
        let (mut status, failures) =
            execute(surface, &mut log).unwrap_or_else(|error| (Status::Failed, vec![format!("{error:#}")]));
        let after = files::snapshot(repo)?;
        let changed = files::changed(&current, &after);
        current = after;
        if !changed.is_empty() {
            status = match status {
                Status::Current => Status::Rolled,
                Status::Failed | Status::Skipped | Status::PolicySkipped | Status::UpstreamMissing => Status::Partial,
                other => other,
            };
        }
        writeln!(log, "[{}; {} dependency files changed]", status.label(), changed.len())?;
        for failure in &failures {
            writeln!(log, "{failure}")?;
        }
        log.sync_data()?;
        println!(
            "{} / {}: {} [{}]",
            repo.display(),
            surface.name,
            status.label(),
            surface.level
        );
        Record {
            time: Local::now().to_rfc3339(),
            run: run_id,
            repo,
            surface: surface.name,
            level: surface.level,
            status,
            changed,
            failures,
            duration_s: started.elapsed().as_secs_f64(),
        }
        .persist(history)?;
        verified &= !status.failed();
        deferred += usize::from(status.deferred());
        if !verified {
            break;
        }
    }
    let clean = files::git_clean(repo, "final")?;
    writeln!(log, "# final worktree clean: {clean}")?;
    log.sync_all()?;
    Ok((verified, deferred))
}

pub(super) fn run(dry_run: bool) -> Result<()> {
    super::user_only()?;
    let home = super::account_home()?;
    let config = Config::read(&home)?;
    let mut planned = Vec::new();
    for repo in config.repos(&home)? {
        let edge = files::repository_edge(&repo, config.policy.as_deref() != Some("compatible"))?;
        let surfaces = plan::detect(&repo, edge)?;
        if dry_run {
            for surface in &surfaces {
                println!("{} / {}: DRY-RUN [{}]", repo.display(), surface.name, surface.level);
                if let Action::Commands(commands) = &surface.action {
                    for command in commands {
                        println!("  {}", shell_words::join(command));
                    }
                }
            }
        }
        planned.push((repo, surfaces));
    }
    if dry_run || planned.iter().all(|(_, surfaces)| surfaces.is_empty()) {
        println!("Rolling Project Dependencies: {} repositories inspected", planned.len());
        return Ok(());
    }

    let cache = files::cache_dir(&home)?;
    let lock_path = cache.join("native.lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(&lock_path)?;
    super::check_file(&lock, &lock_path, true, 0)?;
    lock.try_lock()
        .wrap_err("Another rolling dependency update is active")?;
    let history_path = cache.join("history.jsonl");
    let history = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(&history_path)?;
    super::check_file(&history, &history_path, false, u64::MAX)?;
    let history = Mutex::new(history);
    let run_id = format!("{}-{}", Local::now().format("%Y%m%d-%H%M%S"), std::process::id());
    let run = cache.join(&run_id);
    files::create_private_dir(&run)?;
    File::open(&cache)?.sync_all()?;
    let mut verified = true;
    let mut deferred = Vec::new();
    for chunk in planned.chunks(config.execution.jobs) {
        let outcomes = thread::scope(|scope| {
            let handles = chunk
                .iter()
                .map(|(repo, surfaces)| {
                    let (run, run_id, history) = (&run, &run_id, &history);
                    scope.spawn(move || process_repo(repo, surfaces, run, run_id, history))
                })
                .collect::<Vec<_>>();
            handles.into_iter().map(|handle| handle.join()).collect::<Vec<_>>()
        });
        for ((repo, _), outcome) in chunk.iter().zip(outcomes) {
            match outcome {
                Ok(Ok((ok, pending))) => {
                    verified &= ok;
                    if pending > 0 {
                        deferred.push((repo, pending));
                    }
                }
                Ok(Err(error)) => {
                    verified = false;
                    eprintln!("{}: {error:#}", repo.display());
                }
                Err(_) => {
                    verified = false;
                    eprintln!("{}: rolling dependency worker panicked", repo.display());
                }
            }
        }
        if crate::ctrlc::interrupted() {
            break;
        }
    }
    println!("Rolling Project Dependencies evidence: {}", run.display());
    ensure!(
        verified && !crate::ctrlc::interrupted(),
        "One or more rolling dependency updates did not complete"
    );
    if !deferred.is_empty() {
        let total: usize = deferred.iter().map(|(_, count)| count).sum();
        let projects = deferred
            .iter()
            .map(|(repo, count)| format!("{} ({count})", repo.display()))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(PartialStep(format!(
            "{total} dependency surfaces were deferred across {} repositories: {projects}. See the rolling evidence above.",
            deferred.len()
        ))
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subprocess_failure_retains_auditable_before_and_after_state() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        let repo = root.join("fixture");
        let run = root.join("run");
        files::create_private_dir(&repo)?;
        files::create_private_dir(&run)?;
        let git =
            process::find_tool("git")?.ok_or_else(|| color_eyre::eyre::eyre!("Git fixture dependency missing"))?;
        let mut initialize = process::command(&git, &repo);
        initialize.args(["init", "--quiet"]);
        ensure!(
            process::capture(initialize, process::QUERY_TIMEOUT)?.status.success(),
            "Could not initialize fixture Git repository"
        );
        std::fs::write(repo.join("Cargo.lock"), b"initial")?;
        std::fs::write(repo.join("notes.txt"), b"unrelated local work")?;
        for args in [
            &["add", "--", "Cargo.lock", "notes.txt"][..],
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.test",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "--quiet",
                "-m",
                "fixture",
            ][..],
        ] {
            let mut prepare = process::command(&git, &repo);
            prepare.args(args);
            ensure!(
                process::capture(prepare, process::QUERY_TIMEOUT)?.status.success(),
                "Could not prepare Git fixture"
            );
        }
        let history_path = root.join("history.jsonl");
        let history = Mutex::new(super::super::create_file(&history_path)?);
        let surface = |name, fixture| Surface {
            name,
            root: repo.clone(),
            level: "fixture",
            action: Action::Commands(vec![vec!["/bin/sh".into(), "-c".into(), String::from(fixture)]]),
        };
        let missing_run = root.join("missing-tool");
        files::create_private_dir(&missing_run)?;
        let missing = Surface {
            name: "missing tool",
            root: repo.clone(),
            level: "fixture",
            action: Action::Commands(vec![vec![root.join("nonexistent-tool").to_string_lossy().into_owned()]]),
        };
        assert_eq!(
            process_repo(&repo, &[missing], &missing_run, "missing-tool-fixture", &history)?,
            (true, 1)
        );
        let surfaces = [
            surface("partial fixture", "printf changed > Cargo.lock; exit 42"),
            surface("next fixture", "printf should-not-run > notes.txt"),
        ];
        assert_eq!(
            process_repo(&repo, &surfaces, &run, "fixture-run", &history)?,
            (false, 0)
        );
        assert_eq!(std::fs::read(repo.join("Cargo.lock"))?, b"changed");
        assert_eq!(std::fs::read(repo.join("notes.txt"))?, b"unrelated local work");
        let backup = run.join("before").join(files::repo_stem(&repo)?);
        assert_eq!(std::fs::read(backup.join("files/Cargo.lock"))?, b"initial");
        let retry = root.join("retry");
        files::create_private_dir(&retry)?;
        let unrelated_dependency = repo.join("Package.resolved");
        std::os::unix::fs::symlink(root.join("missing"), &unrelated_dependency)?;
        assert!(files::snapshot(&repo).is_err());
        assert_eq!(
            process_repo(&repo, &surfaces[1..], &retry, "dirty-fixture", &history)?,
            (true, 1)
        );
        assert_eq!(std::fs::read(repo.join("notes.txt"))?, b"unrelated local work");
        assert!(!retry.join("before").exists());
        assert!(
            std::fs::symlink_metadata(&unrelated_dependency)?
                .file_type()
                .is_symlink()
        );
        let history = std::fs::read_to_string(history_path)?;
        let records = history
            .lines()
            .map(serde_json::from_str::<serde_json::Value>)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let [missing, record, dirty] = records.as_slice() else {
            panic!("Expected a missing-tool, a partial-write and a dirty-worktree record")
        };
        assert_eq!(missing["run"], "missing-tool-fixture");
        assert_eq!(missing["status"], "SKIPPED");
        assert_eq!(dirty["run"], "dirty-fixture");
        assert_eq!(dirty["status"], "SKIPPED");
        assert_eq!(dirty["failures"], serde_json::json!(["uncommitted project changes"]));
        assert_eq!(record["run"], "fixture-run");
        assert_eq!(record["status"], "PARTIAL");
        assert_eq!(record["changed"], serde_json::json!(["Cargo.lock"]));
        assert!(
            record["failures"][0]
                .as_str()
                .is_some_and(|failure| failure.ends_with("42"))
        );
        Ok(())
    }

    #[test]
    fn unsafe_dependency_path_stops_before_subprocess_or_history_write() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        let repo = root.join("fixture");
        let run = root.join("run");
        files::create_private_dir(&repo)?;
        files::create_private_dir(&run)?;
        let outside = root.join("outside");
        std::fs::write(&outside, b"preserve")?;
        std::os::unix::fs::symlink(&outside, repo.join("Cargo.lock"))?;
        assert!(files::snapshot(&repo).is_err());
        let history_path = root.join("history.jsonl");
        let history = Mutex::new(super::super::create_file(&history_path)?);
        let surface = Surface {
            name: "must not start",
            root: repo.clone(),
            level: "fixture",
            action: Action::Commands(vec![vec![
                "/bin/sh".into(),
                "-c".into(),
                "printf changed > Cargo.lock".into(),
            ]]),
        };
        assert!(process_repo(&repo, &[surface], &run, "fixture-run", &history).is_err());
        assert_eq!(std::fs::read(outside)?, b"preserve");
        assert_eq!(std::fs::metadata(history_path)?.len(), 0);
        assert_eq!(std::fs::read_dir(run)?.count(), 0);
        Ok(())
    }
}
