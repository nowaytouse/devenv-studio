use super::{FILE_LIMIT, check_dir, command, exists, is_hex, open_file, read_small, sync_dir};
use crate::command::CommandExt as _;
use color_eyre::eyre::{Context, ContextCompat, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fs, io::Write, os::unix::fs::PermissionsExt, path::Path};

const ACTIVE: &str = "cli-active";
const JOURNAL: &str = "journal.json";
const OLD: [&str; 2] = ["original-bin", "original-revision"];

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    original: [Option<String>; 2],
    candidate: [String; 2],
}

fn copy(src: &Path, dst: &Path) -> Result<()> {
    command(Path::new("/bin/cp"))
        .arg("-p")
        .arg(src)
        .arg(dst)
        .status_checked()?;
    open_file(dst, false, FILE_LIMIT)?.sync_all()?;
    Ok(())
}

fn digest(path: &Path) -> Result<Option<String>> {
    exists(path)?
        .then(|| super::super::sha256(open_file(path, false, FILE_LIMIT)?))
        .transpose()
}

fn replace(path: &Path, from: &Path) -> Result<()> {
    let parent = path.parent().context("SQLite destination has no parent")?;
    let temp = tempfile::NamedTempFile::new_in(parent)?;
    copy(from, temp.path())?;
    temp.persist(path)?;
    sync_dir(parent)
}

fn finish(active: &Path, state: &Path) -> Result<()> {
    let retired = tempfile::Builder::new().prefix(".sqlite-retired.").tempdir_in(state)?;
    let retired = retired.keep();
    fs::rename(active, retired.join(ACTIVE))?;
    sync_dir(&retired)?;
    sync_dir(state)?;
    fs::remove_dir_all(retired)?;
    Ok(())
}

pub(super) fn recover(state: &Path, binary: &Path) -> Result<()> {
    let active = state.join(ACTIVE);
    if !exists(&active)? {
        return Ok(());
    }
    check_dir(&active, true)?;
    let journal: Journal = serde_json::from_slice(&read_small(&active.join(JOURNAL), 2048)?)?;
    ensure!(
        journal
            .original
            .iter()
            .flatten()
            .chain(&journal.candidate)
            .all(|hash| is_hex(hash, 64)),
        "Invalid SQLite CLI recovery journal"
    );
    let receipt = state.join("revision");
    let destinations = [binary, receipt.as_path()];
    for (index, path) in destinations.iter().enumerate() {
        let actual = digest(path)?;
        ensure!(
            actual == journal.original[index] || actual.as_ref() == Some(&journal.candidate[index]),
            "SQLite CLI recovery stopped at a changed file: {}",
            path.display()
        );
        ensure!(
            digest(&active.join(OLD[index]))? == journal.original[index],
            "SQLite CLI recovery snapshot is incomplete or has changed"
        );
    }
    for (index, path) in destinations.iter().enumerate() {
        let actual = digest(path)?;
        if actual == journal.original[index] {
            continue;
        }
        ensure!(
            actual.as_ref() == Some(&journal.candidate[index]),
            "SQLite CLI file changed during recovery: {}",
            path.display()
        );
        if journal.original[index].is_some() {
            replace(path, &active.join(OLD[index]))?;
        } else {
            fs::remove_file(path)?;
            sync_dir(path.parent().context("SQLite destination has no parent")?)?;
        }
        ensure!(
            digest(path)? == journal.original[index],
            "SQLite CLI recovery readback failed"
        );
    }
    finish(&active, state)
}

fn stage(state: &Path, binary: &Path, candidate: &Path, revision: &[u8]) -> Result<Journal> {
    let pending = tempfile::Builder::new().prefix(".sqlite-pending.").tempdir_in(state)?;
    let pending_dir = pending.path();
    fs::set_permissions(pending_dir, fs::Permissions::from_mode(0o700))?;
    let active = state.join(ACTIVE);
    ensure!(!exists(&active)?, "An existing SQLite CLI transaction needs recovery");
    let receipt = state.join("revision");
    let original = [digest(binary)?, digest(&receipt)?];
    for (index, path) in [binary, receipt.as_path()].into_iter().enumerate() {
        if original[index].is_some() {
            let snapshot = pending_dir.join(OLD[index]);
            copy(path, &snapshot)?;
            ensure!(
                digest(&snapshot)? == original[index],
                "SQLite CLI changed while snapshotting"
            );
        }
    }
    let mut candidate_receipt = super::super::create_file(&pending_dir.join("new-revision"))?;
    candidate_receipt.write_all(revision)?;
    candidate_receipt.sync_all()?;
    let journal = Journal {
        original,
        candidate: [
            digest(candidate)?.context("Missing SQLite candidate")?,
            digest(&pending_dir.join("new-revision"))?.context("Missing candidate revision receipt")?,
        ],
    };
    let mut journal_file = super::super::create_file(&pending_dir.join(JOURNAL))?;
    serde_json::to_writer(&mut journal_file, &journal)?;
    journal_file.sync_all()?;
    sync_dir(pending_dir)?;
    fs::rename(pending_dir, &active)?;
    let _ = pending.keep();
    sync_dir(state)?;
    Ok(journal)
}

pub(super) fn install(
    state: &Path,
    binary: &Path,
    candidate: &Path,
    revision: &[u8],
    verify: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let journal = stage(state, binary, candidate, revision)?;
    let active = state.join(ACTIVE);
    let receipt = state.join("revision");
    let result = (|| {
        for (index, (path, from)) in [(binary, candidate), (&receipt, &active.join("new-revision"))]
            .into_iter()
            .enumerate()
        {
            ensure!(
                digest(path)? == journal.original[index] && digest(from)?.as_ref() == Some(&journal.candidate[index]),
                "SQLite CLI changed before installation: {}",
                path.display()
            );
            replace(path, from)?;
            ensure!(
                digest(path)?.as_ref() == Some(&journal.candidate[index]),
                "SQLite CLI installation readback failed"
            );
        }
        verify()
    })();
    if let Err(error) = result {
        recover(state, binary).wrap_err_with(|| format!("SQLite CLI install failed: {error:#}; recovery stopped"))?;
        return Err(error);
    }
    finish(&active, state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupted_install_restores_pair_and_recovery_is_repeatable() {
        let dir = tempfile::tempdir().unwrap();
        let state = &dir.path().canonicalize().unwrap();
        let binary = state.join("sqlite");
        let candidate = state.join("candidate");
        fs::write(&binary, "old binary").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o751)).unwrap();
        let before = fs::metadata(&binary).unwrap();
        fs::write(state.join("revision"), "old revision").unwrap();
        fs::write(&candidate, "candidate").unwrap();
        stage(state, &binary, &candidate, b"new revision").unwrap();
        replace(&binary, &candidate).unwrap();
        recover(state, &binary).unwrap();
        recover(state, &binary).unwrap();
        assert_eq!(fs::read(&binary).unwrap(), b"old binary");
        assert_eq!(fs::read(state.join("revision")).unwrap(), b"old revision");
        let after = fs::metadata(&binary).unwrap();
        assert_eq!(after.permissions().mode(), before.permissions().mode());
        assert_eq!(after.modified().unwrap(), before.modified().unwrap());
    }

    #[test]
    fn recovery_validates_both_paths_before_touching_either() {
        let dir = tempfile::tempdir().unwrap();
        let state = &dir.path().canonicalize().unwrap();
        let binary = state.join("sqlite");
        let candidate = state.join("candidate");
        fs::write(&binary, "old binary").unwrap();
        fs::write(state.join("revision"), "old revision").unwrap();
        fs::write(&candidate, "candidate").unwrap();
        stage(state, &binary, &candidate, b"new revision").unwrap();
        replace(&binary, &candidate).unwrap();
        fs::write(state.join("revision"), "external edit").unwrap();
        assert!(recover(state, &binary).is_err());
        assert_eq!(fs::read(&binary).unwrap(), b"candidate");
        assert!(state.join(ACTIVE).is_dir());
    }

    #[test]
    fn failed_runtime_readback_rolls_back_first_install() {
        let dir = tempfile::tempdir().unwrap();
        let state = &dir.path().canonicalize().unwrap();
        let binary = state.join("sqlite");
        let candidate = state.join("candidate");
        fs::write(&candidate, "candidate").unwrap();
        assert!(
            install(state, &binary, &candidate, b"new revision", || {
                color_eyre::eyre::bail!("injected runtime failure")
            })
            .is_err()
        );
        assert!(!binary.exists());
        assert!(!state.join("revision").exists());
        assert!(!state.join(ACTIVE).exists());
    }
}
