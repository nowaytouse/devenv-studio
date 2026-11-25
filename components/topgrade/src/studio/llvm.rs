use std::env;
use std::ffi::OsStr;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::Command;

use color_eyre::eyre::{Result, WrapErr, ensure};

use super::{Step, account_home, check_dir, exists, open_file, resolve_tool, state_dir, tool_command, user_only};
use crate::{command::CommandExt as _, error::PartialStep};

// This is the update lock used by Homebrew's cmd/update.sh and utils/lock.sh.
// Take it before resolving the run's formula and hold it through the build
// child, which reloads that same tap path.
const WORKER: &str = "studio-llvm-worker";

fn worker_script(state: &Path) -> Result<tempfile::TempDir> {
    let directory = tempfile::Builder::new()
        .prefix(".llvm-update-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(state)?;
    check_dir(directory.path(), true)?;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(directory.path().join(format!("brew-{WORKER}.rb")))?
        .write_all(include_bytes!("llvm.rb"))?;
    Ok(directory)
}

fn homebrew_update_lock(prefix: &Path) -> Result<std::path::PathBuf> {
    let parent = prefix.join("var/homebrew");
    let path = parent.join("locks/update");
    ensure!(
        parent.canonicalize()? == parent,
        "Non-canonical Homebrew state directory"
    );
    let directory = parent.join("locks");
    if let Err(error) = DirBuilder::new().mode(0o755).create(&directory)
        && error.kind() != ErrorKind::AlreadyExists
    {
        return Err(error.into());
    }
    check_dir(&directory, false)?;
    // Homebrew creates this file on demand. Keep an existing lock inode intact.
    if let Err(error) = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)
        && error.kind() != ErrorKind::AlreadyExists
    {
        return Err(error.into());
    }
    let metadata = fs::symlink_metadata(&path)?;
    ensure!(
        path.canonicalize()? == path
            && metadata.is_file()
            && metadata.uid() == nix::unistd::getuid().as_raw()
            && metadata.mode() & 0o022 == 0
            && metadata.nlink() == 1,
        "Unexpected Homebrew update lock: {}",
        path.display()
    );
    Ok(path)
}

fn command(
    action: &str,
    step: Step,
    brew: &Path,
    prefix: &Path,
    worker: &Path,
    make: impl FnOnce(&Path) -> Result<Command>,
) -> Result<Command> {
    ensure!(step.manual_llvm(), "Invalid reviewed LLVM operation");
    user_only()?;
    let mut command = if action != "plan" {
        let path = homebrew_update_lock(prefix)
            .wrap_err_with(|| format!("Could not prepare Homebrew update lock under {}", prefix.display()))?;
        let mut command = make(Path::new("/usr/bin/lockf"))?;
        command.args(["-kn", "-t", "0"]).arg(path).arg(brew);
        command
    } else {
        make(brew)?
    };
    let original_path = command
        .get_envs()
        .find(|(key, _)| *key == "PATH")
        .and_then(|(_, value)| value.map(OsStr::to_os_string))
        .or_else(|| env::var_os("PATH"))
        .unwrap_or_default();
    let path = env::join_paths(std::iter::once(worker.to_path_buf()).chain(env::split_paths(&original_path)))?;
    // Homebrew's external Ruby command keeps the bottle prefix metadata loaded by brew.
    command
        .env("PATH", path)
        .env("HOMEBREW_NO_INSTALL_FROM_API", "1")
        .env("HOMEBREW_NO_AUTO_UPDATE", "1")
        .env("HOMEBREW_NO_INSTALL_CLEANUP", "1")
        .env("HOMEBREW_NO_ANALYTICS", "1")
        .env("HOMEBREW_NO_ASK", "1")
        .args([WORKER, action, step.id()]);
    Ok(command)
}

fn reviewed_command(action: &str, step: Step, worker: &Path) -> Result<Command> {
    command(
        action,
        step,
        &resolve_tool("brew")?,
        Path::new("/opt/homebrew"),
        worker,
        tool_command,
    )
}

pub(super) fn plan(id: &OsStr) -> Result<()> {
    let step = id
        .to_str()
        .and_then(Step::find)
        .ok_or_else(|| color_eyre::eyre::eyre!("Invalid reviewed LLVM operation"))?;
    let worker = worker_script(&env::temp_dir())?;
    reviewed_command("plan", step, worker.path())?.status_checked()
}

pub(super) fn run(step: Step, digest: &str, dir: &Path) -> Result<()> {
    let worker = worker_script(dir)?;
    let mut command = reviewed_command("run", step, worker.path())?;
    if step == Step::LlvmHead {
        attach_head_cache(&mut command, &account_home()?)?;
    }
    command.arg(digest).arg(dir.join("llvm-before.json")).status_checked()
}

fn attach_head_cache(command: &mut Command, home: &Path) -> Result<()> {
    let cache = state_dir(home)?.join("llvm-head-cache");
    if let Err(error) = DirBuilder::new().mode(0o700).create(&cache)
        && error.kind() != ErrorKind::AlreadyExists
    {
        return Err(error.into());
    }
    check_dir(&cache, true)?;
    command.env("HOMEBREW_CACHE", cache);
    Ok(())
}

pub(super) fn automatic_journal(state: &Path, step: Step) -> Result<std::path::PathBuf> {
    let journal = state.join(format!("{}-before.json", step.id()));
    let done = journal.with_extension("json.done");
    let pending = exists(&journal)?;
    let completed = exists(&done)?;
    ensure!(
        !pending || completed,
        "LLVM restoration was incomplete. Review {} before another LLVM update.",
        journal.display()
    );
    if completed {
        let mut file = open_file(&done, true, super::MAX_JSON)?;
        let receipt: serde_json::Value = serde_json::from_slice(&super::read_json_bytes(&mut file)?)?;
        ensure!(
            receipt.get("restored").and_then(serde_json::Value::as_bool) == Some(true)
                || receipt
                    .get("verified")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|path| Path::new(path).is_absolute()),
            "Invalid LLVM completion receipt: {}",
            done.display()
        );
        if pending {
            open_file(&journal, true, super::MAX_JSON)?;
            fs::remove_file(&journal)?;
            fs::File::open(state)?.sync_all()?;
        }
        fs::remove_file(done)?;
        fs::File::open(state)?.sync_all()?;
    }
    Ok(journal)
}

pub(super) fn automatic(
    step: Step,
    brew: &Path,
    prefix: &Path,
    state: &Path,
    make: impl FnOnce(&Path) -> Result<Command>,
    execute: impl FnOnce(&mut Command) -> Result<()>,
) -> Result<()> {
    let state = state.canonicalize()?;
    let journal = automatic_journal(&state, step)?;
    let worker = worker_script(&state)?;
    let mut command = command("automatic", step, brew, prefix, worker.path(), make)?;
    if step == Step::LlvmHead {
        let home =
            std::env::home_dir().ok_or_else(|| color_eyre::eyre::eyre!("LLVM HEAD home directory is unavailable"))?;
        attach_head_cache(&mut command, &home)?;
    }
    let result = execute(command.arg(journal));
    automatic_journal(&state, step)?;
    if step == Step::LlvmHead {
        return result.map_err(|error| {
            PartialStep(format!(
                "LLVM preview update did not complete; restoration check passed: {error:#}"
            ))
            .into()
        });
    }
    result
}

pub(super) fn check_restoration(dir: &Path) -> Result<()> {
    ensure!(
        !super::exists(&dir.join("llvm-before.json"))? || super::exists(&dir.join("llvm-before.json.done"))?,
        "LLVM restoration was incomplete. Review llvm-before.json and the Homebrew links before acknowledging this interruption."
    );
    Ok(())
}
