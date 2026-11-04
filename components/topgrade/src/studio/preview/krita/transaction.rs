use color_eyre::eyre::{Context, ContextCompat, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    ffi::CString,
    fs::{self, File},
    io::{Read, Seek, Write},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

use super::{Fingerprint, fingerprint};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Journal {
    pub(super) before: Fingerprint,
    pub(super) next: Fingerprint,
    pub(super) staged: PathBuf,
    pub(super) before_state: String,
    pub(super) next_state: String,
}

#[derive(Debug, PartialEq, Eq)]
enum Recovery {
    Abandoned,
    Revert,
    Committed,
}

impl Journal {
    fn recovery(&self, app: &Fingerprint, staged: &Fingerprint, state: &str) -> Result<Recovery> {
        if state == self.before_state && app == &self.before && staged == &self.next {
            return Ok(Recovery::Abandoned);
        }
        if app == &self.next && staged == &self.before {
            if state == self.before_state {
                return Ok(Recovery::Revert);
            }
            if state == self.next_state {
                return Ok(Recovery::Committed);
            }
        }
        bail!("Krita app, retained stage or receipt changed independently; review the pending journal before retrying");
    }
}

pub(super) fn read_private(path: &Path, limit: u64) -> Result<Option<Vec<u8>>> {
    if !super::super::super::exists(path)? {
        return Ok(None);
    }
    let file = super::super::super::open_file(path, true, limit)?;
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "Krita state exceeds its limit");
    Ok(Some(bytes))
}

pub(super) fn state_digest(bytes: &[u8]) -> Result<String> {
    let mut file = tempfile::tempfile()?;
    file.write_all(bytes)?;
    file.rewind()?;
    super::super::super::sha256(file)
}

pub(super) fn write_private(path: &Path, bytes: &[u8], replace: bool) -> Result<()> {
    let parent = path.parent().context("Missing Krita state directory")?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    if replace {
        file.persist(path)?;
    } else {
        file.persist_noclobber(path)?;
    }
    File::open(parent)?.sync_all()?;
    Ok(())
}

pub(super) fn swap(left: &Path, right: &Path) -> Result<()> {
    let left_name = CString::new(left.as_os_str().as_bytes())?;
    let right_name = CString::new(right.as_os_str().as_bytes())?;
    // APFS exchanges both names atomically; neither application is removed.
    let result = unsafe { nix::libc::renamex_np(left_name.as_ptr(), right_name.as_ptr(), nix::libc::RENAME_SWAP) };
    ensure!(
        result == 0,
        "Could not exchange Krita bundles: {}",
        std::io::Error::last_os_error()
    );
    sync_app_dirs(left, right)
}

fn sync_app_dirs(app: &Path, staged: &Path) -> Result<()> {
    for parent in [app.parent(), staged.parent()] {
        File::open(parent.context("Missing Krita app parent")?)?.sync_all()?;
    }
    Ok(())
}

pub(super) fn recover(app: &Path, state: &Path, journal_path: &Path) -> Result<()> {
    let Some(bytes) = read_private(journal_path, 64 * 1024)? else {
        return Ok(());
    };
    let journal: Journal = serde_json::from_slice(&bytes)?;
    let root = journal.staged.parent().context("Invalid Krita stage in journal")?;
    ensure!(
        root.parent() == app.parent()
            && root
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".krita.preview-"))
            && journal.staged.file_name().is_some_and(|name| name == "Krita.app"),
        "Unsafe Krita stage in recovery journal"
    );
    super::super::super::check_dir(root, true)?;
    ensure!(root.canonicalize()? == root, "Unsafe Krita stage directory");
    let before_state = read_private(state, super::MAX_STATE)?.context("Krita recovery receipt is missing")?;
    let current = fingerprint(app)?;
    let staged = fingerprint(&journal.staged)?;
    match journal.recovery(&current, &staged, &state_digest(&before_state)?)? {
        Recovery::Abandoned => println!("Krita: recovered an uncommitted staging transaction"),
        Recovery::Revert => {
            ensure!(
                !super::super::running(app, "/krita")?,
                "Close Krita before recovering an interrupted update"
            );
            swap(app, &journal.staged)?;
            ensure!(fingerprint(app)? == journal.before, "Krita rollback readback failed");
            println!("Krita: restored the previous app after interruption");
        }
        Recovery::Committed => println!("Krita: finalized an already committed update receipt"),
    }
    ensure!(
        read_private(journal_path, 64 * 1024)?.as_deref() == Some(bytes.as_slice()),
        "Krita recovery journal changed independently"
    );
    fs::remove_file(journal_path).wrap_err("Retire Krita recovery journal")?;
    File::open(journal_path.parent().context("Missing Krita journal directory")?)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
#[test]
fn interrupted_exchange_and_state_commit_have_distinct_recovery_paths() -> Result<()> {
    let identity = |inode| Fingerprint {
        inode,
        version: "6.1.0".into(),
        executable_sha256: format!("exec-{inode}"),
        info_plist_sha256: format!("plist-{inode}"),
        team: None,
    };
    let before = identity(101);
    let next = identity(102);
    let journal = Journal {
        before: before.clone(),
        next: next.clone(),
        staged: PathBuf::new(),
        before_state: "before".into(),
        next_state: "next".into(),
    };
    assert_eq!(journal.recovery(&before, &next, "before")?, Recovery::Abandoned);
    assert_eq!(journal.recovery(&next, &before, "before")?, Recovery::Revert);
    assert_eq!(journal.recovery(&next, &before, "next")?, Recovery::Committed);
    assert!(journal.recovery(&next, &before, "external").is_err());
    assert!(journal.recovery(&identity(103), &before, "before").is_err());
    Ok(())
}

#[cfg(test)]
#[test]
fn exchanging_and_reverting_retains_both_application_trees() -> Result<()> {
    let root = tempfile::tempdir()?;
    let installed = root.path().join("installed");
    let staged = root.path().join("staged");
    fs::create_dir(&installed)?;
    fs::create_dir(&staged)?;
    fs::write(installed.join("payload"), b"prior")?;
    fs::write(staged.join("payload"), b"candidate")?;
    swap(&installed, &staged)?;
    assert_eq!(fs::read(installed.join("payload"))?, b"candidate");
    assert_eq!(fs::read(staged.join("payload"))?, b"prior");
    swap(&installed, &staged)?;
    assert_eq!(fs::read(installed.join("payload"))?, b"prior");
    assert_eq!(fs::read(staged.join("payload"))?, b"candidate");
    Ok(())
}
