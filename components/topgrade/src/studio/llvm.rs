use std::ffi::OsStr;
use std::fs::{self, DirBuilder};
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::Path;
use std::process::Command;

use color_eyre::eyre::{Result, ensure};

use super::{Step, account_home, check_dir, resolve_tool, state_dir, tool_command, user_only};
use crate::command::CommandExt as _;

// This is the update lock used by Homebrew's cmd/update.sh and utils/lock.sh.
// Take it before resolving the run's formula and hold it through the build
// child, which reloads that same tap path.
const BREW_UPDATE_LOCK: &str = "/opt/homebrew/var/homebrew/locks/update";

fn command(action: &str, step: Step) -> Result<Command> {
    ensure!(step.manual_llvm(), "Invalid reviewed LLVM operation");
    user_only()?;
    let brew = resolve_tool("brew")?;
    let mut command = if action == "run" {
        let path = Path::new(BREW_UPDATE_LOCK);
        let metadata = fs::symlink_metadata(path)?;
        ensure!(
            path.canonicalize()? == path
                && metadata.is_file()
                && metadata.uid() == nix::unistd::getuid().as_raw()
                && metadata.mode() & 0o022 == 0
                && metadata.nlink() == 1,
            "Unexpected Homebrew update lock"
        );
        let mut command = tool_command(Path::new("/usr/bin/lockf"))?;
        command.args(["-kn", "-t", "0", BREW_UPDATE_LOCK]).arg(brew);
        command
    } else {
        tool_command(&brew)?
    };
    command
        .env("HOMEBREW_NO_INSTALL_FROM_API", "1")
        .env("HOMEBREW_NO_INSTALL_CLEANUP", "1")
        .env("HOMEBREW_NO_ANALYTICS", "1")
        .env("HOMEBREW_NO_ASK", "1")
        .args(["ruby", "-e", include_str!("llvm.rb"), "--", action, step.id()]);
    Ok(command)
}

pub(super) fn plan(id: &OsStr) -> Result<()> {
    let step = id
        .to_str()
        .and_then(Step::find)
        .ok_or_else(|| color_eyre::eyre::eyre!("Invalid reviewed LLVM operation"))?;
    command("plan", step)?.status_checked()
}

pub(super) fn run(step: Step, digest: &str, dir: &Path) -> Result<()> {
    let mut command = command("run", step)?;
    if step == Step::LlvmHead {
        let cache = state_dir(&account_home()?)?.join("llvm-head-cache");
        if let Err(error) = DirBuilder::new().mode(0o700).create(&cache)
            && error.kind() != ErrorKind::AlreadyExists
        {
            return Err(error.into());
        }
        check_dir(&cache, true)?;
        command.env("HOMEBREW_CACHE", cache);
    }
    command.arg(digest).arg(dir.join("llvm-before.json")).status_checked()
}

pub(super) fn check_restoration(dir: &Path) -> Result<()> {
    ensure!(
        !super::exists(&dir.join("llvm-before.json"))? || super::exists(&dir.join("llvm-before.json.done"))?,
        "LLVM restoration was incomplete. Review llvm-before.json and the Homebrew links before acknowledging this interruption."
    );
    Ok(())
}
