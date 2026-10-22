//! User-interface preferences are independent of Windows configuration profiles.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
const MAX_BYTES: u64 = 64 * 1024;
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    #[serde(rename = "zh-CN")]
    ZhCn,
    #[serde(rename = "en-US")]
    EnUs,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    System,
    Light,
    Dark,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Accent {
    Blue,
    Teal,
    Violet,
    Rose,
    Amber,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    Comfortable,
    Compact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub schema_version: u32,
    pub language: Language,
    pub appearance: Appearance,
    pub accent: Accent,
    pub scale_percent: u16,
    pub density: Density,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            schema_version: 1,
            language: Language::ZhCn,
            appearance: Appearance::System,
            accent: Accent::Blue,
            scale_percent: 100,
            density: Density::Comfortable,
        }
    }
}
impl Preferences {
    fn validate(&self) -> Result<()> {
        ensure!(self.schema_version == 1, "Unsupported preferences version");
        ensure!(
            [90, 100, 110, 125].contains(&self.scale_percent),
            "Unsupported interface scale"
        );
        Ok(())
    }
}
pub fn path() -> Result<PathBuf> {
    let directory = studio_platform::data_directory()?;
    ensure!(
        directory.is_absolute(),
        "Application data directory must be absolute"
    );
    Ok(directory.join("preferences.json"))
}
pub fn load() -> Result<Preferences> {
    load_at(&path()?)
}
pub fn save(preferences: &Preferences) -> Result<()> {
    save_at(&path()?, preferences)
}

fn load_at(path: &Path) -> Result<Preferences> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "Preferences must be a regular file"
            );
            ensure!(metadata.len() <= MAX_BYTES, "Preferences file is too large");
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Preferences::default());
        }
        Err(error) => return Err(error).context("Cannot inspect preferences"),
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .context("Cannot open preferences")?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("Cannot read preferences")?;
    ensure!(
        bytes.len() as u64 <= MAX_BYTES,
        "Preferences file grew beyond the size limit"
    );
    let preferences: Preferences = serde_json::from_slice(&bytes).context(
        "Invalid preferences; the existing file was preserved. Move it aside before saving new preferences")?;
    preferences.validate()?;
    Ok(preferences)
}
fn save_at(path: &Path, preferences: &Preferences) -> Result<()> {
    preferences.validate()?;
    // Reject corrupt or future-version files instead of overwriting them with defaults.
    load_at(path)?;
    let directory = path.parent().context("Preferences path has no parent")?;
    fs::create_dir_all(directory).context("Cannot create preferences directory")?;
    let mut temporary = None;
    for _ in 0..16 {
        let candidate = directory.join(format!(
            ".preferences-{}-{}.tmp",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&candidate) {
            Ok(file) => {
                temporary = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("Cannot stage preferences"),
        }
    }
    let (candidate, mut file) = temporary.context("Cannot reserve a preferences staging file")?;
    let result = (|| -> Result<()> {
        serde_json::to_writer_pretty(&mut file, preferences)
            .context("Cannot encode preferences")?;
        file.write_all(b"\n").context("Cannot finish preferences")?;
        file.sync_all()
            .context("Cannot persist staged preferences")?;
        drop(file);
        // Recheck just before publication; a failed write leaves the previous settings intact.
        load_at(path)?;
        fs::rename(&candidate, path).context("Cannot publish preferences")?;
        Ok(())
    })();
    if let Err(error) = result {
        if let Err(cleanup) = fs::remove_file(&candidate)
            && cleanup.kind() != std::io::ErrorKind::NotFound
        {
            bail!("{error:#}; staging cleanup failed: {cleanup}");
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "ltsc-prefs-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&dir).unwrap();
            Self(dir)
        }
        fn file(&self) -> PathBuf {
            self.0.join("preferences.json")
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    #[test]
    fn missing_defaults_and_changed_preferences_survive_reload() {
        let directory = Directory::new();
        let path = directory.file();
        assert_eq!(load_at(&path).unwrap(), Preferences::default());
        let preferences = Preferences {
            language: Language::EnUs,
            appearance: Appearance::Dark,
            accent: Accent::Teal,
            scale_percent: 125,
            density: Density::Compact,
            ..Preferences::default()
        };
        save_at(&path, &preferences).unwrap();
        assert_eq!(load_at(&path).unwrap(), preferences);
        save_at(&path, &Preferences::default()).unwrap();
        assert_eq!(load_at(&path).unwrap(), Preferences::default());
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
    }
    #[test]
    fn corrupt_and_future_preferences_are_preserved() {
        let directory = Directory::new();
        for bytes in [b"{broken".as_slice(), br#"{"schema_version":2,"language":"zh-CN","appearance":"system","accent":"blue","scale_percent":100,"density":"comfortable"}"#] {
            fs::write(directory.file(), bytes).unwrap();
            assert!(load_at(&directory.file()).is_err());
            assert!(save_at(&directory.file(), &Preferences::default()).is_err());
            assert_eq!(fs::read(directory.file()).unwrap(), bytes);
        }
    }
    #[test]
    fn invalid_scale_does_not_replace_previous_preferences() {
        let directory = Directory::new();
        save_at(&directory.file(), &Preferences::default()).unwrap();
        let bytes = fs::read(directory.file()).unwrap();
        let invalid = Preferences {
            scale_percent: 400,
            ..Preferences::default()
        };
        assert!(save_at(&directory.file(), &invalid).is_err());
        assert_eq!(fs::read(directory.file()).unwrap(), bytes);
    }
    #[test]
    fn oversized_file_and_occupied_destination_are_errors() {
        let directory = Directory::new();
        fs::write(directory.file(), vec![b' '; MAX_BYTES as usize + 1]).unwrap();
        assert!(load_at(&directory.file()).is_err());
        let occupied = directory.0.join("occupied");
        fs::create_dir(&occupied).unwrap();
        assert!(save_at(&occupied, &Preferences::default()).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn symlink_preferences_do_not_overwrite_the_target() {
        let directory = Directory::new();
        let real = directory.0.join("original");
        fs::write(&real, b"personal data").unwrap();
        std::os::unix::fs::symlink(&real, directory.file()).unwrap();
        assert!(load_at(&directory.file()).is_err());
        assert!(save_at(&directory.file(), &Preferences::default()).is_err());
        assert_eq!(fs::read(real).unwrap(), b"personal data");
    }
}
