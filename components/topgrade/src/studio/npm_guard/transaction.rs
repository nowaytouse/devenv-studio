use super::{NAMES, Tree, bytes, check_dir, exists, json, leaf, receipt, replace_leaf, tree};
use crate::command::CommandExt as _;
use color_eyre::eyre::{Context, ContextCompat, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    ffi::CString,
    fs::{self, File},
    io::{self, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, symlink},
    },
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

const PENDING: &str = "npm-security-patch.pending.json";
const RECEIPT: &str = "npm-security-patch-receipt.json";
const RECEIPT_LIMIT: u64 = 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    version: u8,
    stage: String,
    previous: Tree,
    candidates: [Tree; 2],
    previous_receipt: Option<Vec<u8>>,
    installed_receipt: Vec<u8>,
}

fn directory_sync(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

fn write(path: &Path, data: &[u8], replace: bool) -> Result<()> {
    let parent = path.parent().context("Missing npm transaction parent")?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    staged.write_all(data)?;
    staged.as_file().sync_all()?;
    if replace {
        staged.persist(path)?;
    } else {
        staged.persist_noclobber(path)?;
    }
    directory_sync(parent)
}

fn exchange(stage: &Path, target: &Path) -> Result<()> {
    let source = CString::new(stage.as_os_str().as_bytes())?;
    let destination = CString::new(target.as_os_str().as_bytes())?;
    // Darwin's same-volume directory exchange preserves both complete trees.
    if unsafe { nix::libc::renamex_np(source.as_ptr(), destination.as_ptr(), nix::libc::RENAME_SWAP) } != 0 {
        return Err(io::Error::last_os_error()).wrap_err("Exchange verified npm leaves");
    }
    directory_sync(stage.parent().context("Missing staged leaf parent")?)?;
    directory_sync(target.parent().context("Missing npm leaf parent")?)?;
    Ok(())
}

fn copy_leaf(source: &Path, destination: &Path) -> Result<()> {
    let mut directories = Vec::new();
    for entry in WalkDir::new(source).follow_links(false) {
        let entry = entry?;
        let target = destination.join(entry.path().strip_prefix(source)?);
        if entry.file_type().is_dir() {
            fs::create_dir(&target)?;
            directories.push((target, fs::metadata(entry.path())?.permissions()));
        } else if entry.file_type().is_symlink() {
            symlink(fs::read_link(entry.path())?, target)?;
        } else {
            ensure!(entry.file_type().is_file(), "Non-file in npm patch leaf");
            fs::copy(entry.path(), &target)?;
            File::open(&target)?.sync_all()?;
        }
    }
    for (directory, mode) in directories.into_iter().rev() {
        fs::set_permissions(&directory, mode)?;
        directory_sync(&directory)?;
    }
    Ok(())
}

fn snapshot_receipt(base: &Path) -> Result<Option<Vec<u8>>> {
    let path = base.join(RECEIPT);
    exists(&path)?.then(|| bytes(&path, RECEIPT_LIMIT)).transpose()
}

fn finish(base: &Path) -> Result<()> {
    fs::remove_file(base.join(PENDING))?;
    directory_sync(base)
}

fn stage_path(base: &Path, pending: &Pending) -> Result<PathBuf> {
    ensure!(
        pending.version == 1
            && pending
                .stage
                .strip_prefix("npm-patch-switch.")
                .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|c| c.is_ascii_alphanumeric())),
        "Invalid npm recovery stage identity"
    );
    let stage = base.join(&pending.stage);
    check_dir(&stage, true)?;
    Ok(stage)
}

pub(super) fn recover(base: &Path, live: &Path) -> Result<()> {
    let journal = base.join(PENDING);
    if !exists(&journal)? {
        return Ok(());
    }
    let pending: Pending = json(&journal)?;
    let stage = stage_path(base, &pending)?;
    let mut mixed = pending.previous.clone();
    let mut switched = [false; 2];
    for (index, name) in NAMES.into_iter().enumerate() {
        let original = leaf(&pending.previous, name);
        let candidate = &pending.candidates[index];
        ensure!(
            !original.is_empty() && !candidate.is_empty() && original != *candidate,
            "Invalid npm leaf recovery references"
        );
        let current = tree(&live.join("node_modules").join(name))?;
        let saved = tree(&stage.join(name))?;
        if current == *candidate && saved == original {
            switched[index] = true;
            replace_leaf(&mut mixed, name, candidate);
        } else {
            ensure!(
                current == original && saved == *candidate,
                "npm leaf {name} changed during the transaction"
            );
        }
    }
    ensure!(
        tree(live)? == mixed,
        "Unrelated npm files changed during the transaction"
    );
    let receipt = snapshot_receipt(base)?;
    ensure!(
        receipt == pending.previous_receipt || receipt.as_ref() == Some(&pending.installed_receipt),
        "npm patch receipt changed during the transaction"
    );
    for (index, name) in NAMES.into_iter().enumerate().rev() {
        if switched[index] {
            let target = live.join("node_modules").join(name);
            let saved = stage.join(name);
            ensure!(
                tree(&target)? == pending.candidates[index] && tree(&saved)? == leaf(&pending.previous, name),
                "npm leaf changed before rollback"
            );
            exchange(&saved, &target)?;
        }
    }
    ensure!(
        tree(live)? == pending.previous,
        "npm patch rollback verification failed"
    );
    if receipt != pending.previous_receipt {
        if let Some(previous) = &pending.previous_receipt {
            write(&base.join(RECEIPT), previous, true)?;
        } else {
            fs::remove_file(base.join(RECEIPT))?;
            directory_sync(base)?;
        }
    }
    ensure!(
        snapshot_receipt(base)? == pending.previous_receipt,
        "npm receipt rollback failed"
    );
    finish(base)?;
    println!(
        "Recovered an interrupted npm patch; retained files in {}",
        stage.display()
    );
    Ok(())
}

pub(super) fn install(base: &Path, probe: &Path, live: &Path, previous: &Tree, after: &Tree) -> Result<()> {
    let staging = tempfile::Builder::new().prefix("npm-patch-switch.").tempdir_in(base)?;
    ensure!(
        staging.path().metadata()?.dev() == live.metadata()?.dev(),
        "npm leaves require same-volume exchange"
    );
    let desired_receipt = receipt(probe, staging.path())?;
    let mut candidates: [Tree; 2] = std::array::from_fn(|_| Tree::new());
    let mut desired = previous.clone();
    for (index, name) in NAMES.into_iter().enumerate() {
        let expected = leaf(after, name);
        let source = probe.join("node_modules").join(name);
        let destination = staging.path().join(name);
        ensure!(
            !expected.is_empty() && expected != leaf(previous, name) && tree(&source)? == expected,
            "npm patch leaf differs from its reviewed contents: {name}"
        );
        copy_leaf(&source, &destination)?;
        ensure!(
            tree(&destination)? == expected,
            "Staged npm patch failed readback: {name}"
        );
        replace_leaf(&mut desired, name, &expected);
        candidates[index] = expected;
    }
    ensure!(
        super::matches_reference(&desired, after) && tree(live)? == *previous,
        "Unexpected npm candidate or concurrent change"
    );
    let stage = staging.keep();
    directory_sync(base)?;
    let pending = Pending {
        version: 1,
        stage: stage
            .file_name()
            .and_then(|name| name.to_str())
            .context("Missing npm stage identity")?
            .into(),
        previous: previous.clone(),
        candidates,
        previous_receipt: snapshot_receipt(base)?,
        installed_receipt: desired_receipt,
    };
    write(&base.join(PENDING), &serde_json::to_vec(&pending)?, false)?;
    let update = (|| -> Result<()> {
        for (index, name) in NAMES.into_iter().enumerate() {
            let target = live.join("node_modules").join(name);
            let staged = stage.join(name);
            ensure!(
                tree(&target)? == leaf(previous, name) && tree(&staged)? == pending.candidates[index],
                "npm leaf changed before activation"
            );
            exchange(&staged, &target)?;
        }
        ensure!(tree(live)? == desired, "Promoted npm differs from reviewed candidate");
        let output = super::tool_command(Path::new("/opt/homebrew/bin/node"))?
            .arg(live.join("bin/npm-cli.js"))
            .arg("--version")
            .output_checked_utf8()?;
        ensure!(
            output.stdout.trim() == "12.2.1" && output.stderr.is_empty(),
            "Unexpected patched npm runtime"
        );
        ensure!(
            snapshot_receipt(base)? == pending.previous_receipt && tree(live)? == desired,
            "npm tree or receipt changed before commit"
        );
        write(&base.join(RECEIPT), &pending.installed_receipt, true)?;
        ensure!(
            snapshot_receipt(base)?.as_ref() == Some(&pending.installed_receipt),
            "npm receipt failed readback"
        );
        Ok(())
    })();
    if let Err(error) = update {
        if let Err(recovery) = recover(base, live) {
            return Err(error.wrap_err(format!("npm recovery failed: {recovery:#}")));
        }
        return Err(error);
    }
    finish(base)?;
    println!(
        "PASS promoted only verified undici 6.29.1 and ip-address 10.7.5; previous leaves retained in {}",
        stage.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupted_exchange_restores_exact_previous_tree_and_keeps_the_stage() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let root = fixture.path().canonicalize()?;
        let live = root.join("live");
        let stage = root.join("npm-patch-switch.fixture");
        for name in NAMES {
            let package = live.join("node_modules").join(name);
            let staged = stage.join(name);
            fs::create_dir_all(&package)?;
            fs::create_dir_all(&staged)?;
            fs::write(package.join("index"), format!("old {name}"))?;
            fs::write(staged.join("index"), format!("new {name}"))?;
        }
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&stage, fs::Permissions::from_mode(0o700))?;
        let before = tree(&live)?;
        let pending = Pending {
            version: 1,
            stage: "npm-patch-switch.fixture".into(),
            previous: before.clone(),
            candidates: [tree(&stage.join(NAMES[0]))?, tree(&stage.join(NAMES[1]))?],
            previous_receipt: None,
            installed_receipt: b"new receipt".to_vec(),
        };
        write(&root.join(PENDING), &serde_json::to_vec(&pending)?, false)?;
        exchange(&stage.join(NAMES[0]), &live.join("node_modules").join(NAMES[0]))?;
        fs::write(live.join("unrelated"), b"user edit")?;
        assert!(recover(&root, &live).is_err());
        assert!(root.join(PENDING).exists());
        fs::remove_file(live.join("unrelated"))?;
        write(&root.join(RECEIPT), &pending.installed_receipt, false)?;
        recover(&root, &live)?;
        assert_eq!(tree(&live)?, before);
        assert!(!root.join(RECEIPT).exists());
        assert!(stage.is_dir());
        recover(&root, &live)?;
        Ok(())
    }
}
