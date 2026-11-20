//! Suspend only the exact active stable LLVM pin while Homebrew resolves the
//! synchronized ggml/llama.cpp upgrade. The sibling symlink is the durable
//! recovery record: unlike `brew pin`, it never chooses a different HEAD keg.
use std::{
    ffi::CString,
    fs::{self, File},
    io,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

use color_eyre::eyre::{Context, Result, ensure};
use nix::libc;
use serde_json::Value;

struct Paths {
    parent: PathBuf,
    pin: PathBuf,
    held: PathBuf,
    opt: PathBuf,
    rack: PathBuf,
}

impl Paths {
    fn new(prefix: &Path) -> Result<Self> {
        ensure!(prefix.is_absolute(), "Homebrew prefix is not absolute");
        let prefix = prefix.canonicalize()?;
        let parent = prefix.join("var/homebrew/pinned");
        ensure!(
            fs::symlink_metadata(&parent)?.is_dir() && parent.canonicalize()? == parent,
            "Unsafe Homebrew pin directory"
        );
        let rack = prefix.join("Cellar/llvm").canonicalize()?;
        ensure!(rack.is_dir(), "Homebrew LLVM Cellar is missing");
        Ok(Self {
            pin: parent.join("llvm"),
            held: parent.join(".llvm.topgrade-headsync-suspended"),
            opt: prefix.join("opt/llvm"),
            parent,
            rack,
        })
    }

    fn occupied(path: &Path) -> Result<bool> {
        match fs::symlink_metadata(path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    fn stable_target(&self, link: &Path) -> Result<PathBuf> {
        ensure!(
            fs::symlink_metadata(link)?.file_type().is_symlink(),
            "Expected Homebrew LLVM symlink: {}",
            link.display()
        );
        let target = link.canonicalize()?;
        ensure!(
            target.is_dir() && target.parent() == Some(self.rack.as_path()),
            "LLVM pin target is outside its Cellar"
        );
        let name = target.file_name().and_then(|part| part.to_str()).unwrap_or_default();
        ensure!(
            name.as_bytes().first().is_some_and(u8::is_ascii_digit),
            "LLVM pin target is not a stable keg"
        );
        let receipt = target.join("INSTALL_RECEIPT.json");
        let mut file = super::super::open_file(&receipt, false, super::super::MAX_JSON)?;
        let data: Value = serde_json::from_slice(&super::super::read_json_bytes(&mut file)?)?;
        ensure!(
            data.pointer("/source/spec").and_then(Value::as_str) == Some("stable"),
            "LLVM pin receipt does not identify a stable build"
        );
        Ok(target)
    }

    fn finish(&self) -> Result<()> {
        let saved = self.stable_target(&self.held)?;
        ensure!(
            !Self::occupied(&self.pin)?,
            "Another process created an LLVM pin during HEAD-sync; the original pin was preserved at {}",
            self.held.display()
        );
        move_exclusive(&self.held, &self.pin)?;
        File::open(&self.parent)?.sync_all()?;
        ensure!(
            self.stable_target(&self.pin)? == saved,
            "Restored LLVM pin differs from its recorded stable target"
        );
        ensure!(
            self.stable_target(&self.opt)? == saved,
            "LLVM opt changed during HEAD-sync. The original stable pin was restored; inspect LLVM linkage before another update"
        );
        Ok(())
    }
}

fn move_exclusive(source: &Path, dest: &Path) -> Result<()> {
    let source = CString::new(source.as_os_str().as_bytes())?;
    let dest = CString::new(dest.as_os_str().as_bytes())?;
    let rc = unsafe { libc::renamex_np(source.as_ptr(), dest.as_ptr(), libc::RENAME_EXCL) };
    ensure!(
        rc == 0,
        "Could not move LLVM pin without overwriting another file: {}",
        io::Error::last_os_error()
    );
    Ok(())
}

pub(super) fn recover(prefix: &Path) -> Result<()> {
    let paths = Paths::new(prefix)?;
    if !Paths::occupied(&paths.held)? {
        return Ok(());
    }
    println!("Recovering a suspended LLVM pin before Homebrew maintenance");
    paths.finish().wrap_err("Recovery of suspended LLVM pin failed")
}

pub(super) struct Suspension {
    paths: Paths,
    active: bool,
}

pub(super) fn suspend(prefix: &Path) -> Result<Suspension> {
    let paths = Paths::new(prefix)?;
    ensure!(
        !Paths::occupied(&paths.held)?,
        "An earlier suspended LLVM pin exists; recover it before another upgrade"
    );
    ensure!(
        paths.stable_target(&paths.pin)? == paths.stable_target(&paths.opt)?,
        "LLVM opt and pin no longer identify the same stable keg"
    );
    move_exclusive(&paths.pin, &paths.held)?;
    let guard = Suspension { paths, active: true };
    File::open(&guard.paths.parent)?.sync_all()?;
    println!("Temporarily suspending the exact stable LLVM pin for the provider upgrade");
    Ok(guard)
}

impl Suspension {
    pub(super) fn restore(mut self) -> Result<()> {
        let result = self.paths.finish();
        if !Paths::occupied(&self.paths.held)? {
            // The durable pin was restored even if a subsequent opt check failed.
            self.active = false;
        }
        if result.is_ok() {
            println!("Restored the exact stable LLVM pin");
        }
        result
    }
}

impl Drop for Suspension {
    fn drop(&mut self) {
        if self.active
            && let Err(error) = self.paths.finish()
        {
            eprintln!("LLVM pin restoration incomplete: {error:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn sample() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path();
        let keg = base.join("Cellar/llvm/23.1.5_1");
        fs::create_dir_all(&keg).unwrap();
        fs::write(keg.join("INSTALL_RECEIPT.json"), br#"{"source":{"spec":"stable"}}"#).unwrap();
        fs::create_dir_all(base.join("opt")).unwrap();
        fs::create_dir_all(base.join("var/homebrew/pinned")).unwrap();
        symlink(&keg, base.join("opt/llvm")).unwrap();
        symlink(&keg, base.join("var/homebrew/pinned/llvm")).unwrap();
        // macOS resolves /var through /private/var; compare canonical paths.
        (temp, keg.canonicalize().unwrap())
    }

    #[test]
    fn suspension_restores_the_exact_original_pin() -> Result<()> {
        let (temp, keg) = sample();
        let pin = temp.path().join("var/homebrew/pinned/llvm");
        let held = pin.with_file_name(".llvm.topgrade-headsync-suspended");
        let suspended = suspend(temp.path())?;
        assert!(!Paths::occupied(&pin)?);
        assert_eq!(held.canonicalize()?, keg);
        suspended.restore()?;
        assert_eq!(pin.canonicalize()?, keg);
        assert!(!Paths::occupied(&held)?);
        Ok(())
    }

    #[test]
    fn killed_process_can_restore_a_suspended_pin_on_next_run() -> Result<()> {
        let (temp, keg) = sample();
        let pin = temp.path().join("var/homebrew/pinned/llvm");
        let suspended = suspend(temp.path())?;
        std::mem::forget(suspended);
        recover(temp.path())?;
        assert_eq!(pin.canonicalize()?, keg);
        Ok(())
    }

    #[test]
    fn foreign_pin_blocks_recovery_without_clobbering_either_link() -> Result<()> {
        let (temp, keg) = sample();
        let pin = temp.path().join("var/homebrew/pinned/llvm");
        let held = pin.with_file_name(".llvm.topgrade-headsync-suspended");
        let suspended = suspend(temp.path())?;
        std::mem::forget(suspended);
        symlink(&keg, &pin)?;
        assert!(recover(temp.path()).is_err());
        assert!(Paths::occupied(&pin)?);
        assert!(Paths::occupied(&held)?);
        Ok(())
    }

    #[test]
    fn changed_opt_causes_explicit_failure_after_pin_is_restored() -> Result<()> {
        let (temp, keg) = sample();
        let opt = temp.path().join("opt/llvm");
        let pin = temp.path().join("var/homebrew/pinned/llvm");
        let suspended = suspend(temp.path())?;
        fs::remove_file(&opt)?;
        let other = temp.path().join("Cellar/llvm/23.1.4");
        fs::create_dir_all(&other)?;
        fs::write(other.join("INSTALL_RECEIPT.json"), br#"{"source":{"spec":"stable"}}"#)?;
        symlink(&other, &opt)?;
        assert!(suspended.restore().is_err());
        assert_eq!(pin.canonicalize()?, keg);
        assert!(!Paths::occupied(
            &pin.with_file_name(".llvm.topgrade-headsync-suspended")
        )?);
        Ok(())
    }
}
