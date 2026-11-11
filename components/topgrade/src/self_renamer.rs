use color_eyre::eyre::Result;
use std::{env::current_exe, fs, path::PathBuf};
use tracing::{debug, error};

pub struct SelfRenamer {
    exe_path: PathBuf,
    temp_path: PathBuf,
    // Keep the executable's temporary directory alive until Drop restores it.
    _temp_dir: tempfile::TempDir,
}

impl SelfRenamer {
    pub fn create() -> Result<Self> {
        Self::create_for_path(current_exe()?)
    }

    fn create_for_path(exe_path: PathBuf) -> Result<Self> {
        let tempdir = tempfile::tempdir()?;
        let temp_path = tempdir.path().join("topgrade.exe");

        debug!("Current exe in {:?}. Moving it to {:?}", exe_path, temp_path);

        fs::rename(&exe_path, &temp_path)?;

        Ok(SelfRenamer {
            exe_path,
            temp_path,
            _temp_dir: tempdir,
        })
    }
}

impl Drop for SelfRenamer {
    fn drop(&mut self) {
        if self.exe_path.exists() {
            debug!("{:?} exists. Topgrade was probably upgraded", self.exe_path);
            return;
        }

        match fs::rename(&self.temp_path, &self.exe_path) {
            Ok(_) => debug!("Moved Topgrade back from {:?} to {:?}", self.temp_path, self.exe_path),
            Err(e) => {
                self._temp_dir.disable_cleanup(true);
                error!(
                    "Could not restore Topgrade; recovery executable retained at {}: {}",
                    self.temp_path.display(),
                    e
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_temporary_executable_alive_until_restored() -> Result<()> {
        let install_dir = tempfile::tempdir()?;
        let exe_path = install_dir.path().join("topgrade.exe");
        fs::write(&exe_path, b"original executable")?;

        let renamer = SelfRenamer::create_for_path(exe_path.clone())?;
        let temp_path = renamer.temp_path.clone();
        let temp_dir = temp_path.parent().unwrap().to_owned();
        assert!(!exe_path.exists());
        assert_eq!(fs::read(&temp_path)?, b"original executable");

        drop(renamer);

        assert_eq!(fs::read(&exe_path)?, b"original executable");
        assert!(!temp_dir.exists());
        Ok(())
    }

    #[test]
    fn preserves_replacement_executable_and_cleans_temporary_directory() -> Result<()> {
        let install_dir = tempfile::tempdir()?;
        let exe_path = install_dir.path().join("topgrade.exe");
        fs::write(&exe_path, b"original executable")?;

        let renamer = SelfRenamer::create_for_path(exe_path.clone())?;
        let temp_dir = renamer.temp_path.parent().unwrap().to_owned();
        fs::write(&exe_path, b"replacement executable")?;

        drop(renamer);

        assert_eq!(fs::read(&exe_path)?, b"replacement executable");
        assert!(!temp_dir.exists());
        Ok(())
    }
}
