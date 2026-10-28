//! Exact, local Topgrade configuration/script archives. No contents are executed.
//! Restore creates a review directory; external tools, binaries and dependencies need separate installation.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, File, Metadata, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const MAX_FILES: usize = 128;
const MAX_FILE_BYTES: usize = 256 * 1024;
const MAX_TOTAL_BYTES: usize = 512 * 1024;
const MAX_PROFILE_BYTES: usize = 1024 * 1024;
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub path: String,
    pub contents: String,
    /// Original Unix metadata only; extracted files never receive executable bits.
    pub executable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub schema_version: u32,
    pub files: Vec<Entry>,
}

impl Profile {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == 1,
            "Unsupported Topgrade archive schema"
        );
        ensure!(
            !self.files.is_empty() && self.files.len() <= MAX_FILES,
            "Topgrade archive must contain 1–128 files"
        );
        let mut paths = BTreeSet::new();
        let mut total = 0usize;
        for entry in &self.files {
            validate_relative(&entry.path)?;
            let folded = entry.path.to_lowercase();
            ensure!(paths.insert(folded), "Duplicate Topgrade archive path");
            ensure!(
                entry.contents.len() <= MAX_FILE_BYTES && !entry.contents.contains('\0'),
                "Topgrade archive file is not bounded text"
            );
            total += entry.contents.len();
        }
        ensure!(
            total <= MAX_TOTAL_BYTES,
            "Topgrade archive exceeds 512 KiB of text"
        );
        for path in &paths {
            for (end, _) in path.match_indices('/') {
                ensure!(
                    !paths.contains(&path[..end]),
                    "Topgrade archive has a file/directory path conflict"
                );
            }
        }
        ensure!(
            serde_json::to_vec_pretty(self)?.len() < MAX_PROFILE_BYTES,
            "Encoded Topgrade archive exceeds 1 MiB"
        );
        Ok(())
    }
}

pub fn default_home() -> Result<PathBuf> {
    let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let home = PathBuf::from(std::env::var_os(name).context("Home directory is unavailable")?);
    validate_absolute(&home)?;
    Ok(home)
}

/// Read standard configuration files and only explicitly supplied attachments.
/// Includes, commands and scripts are never interpreted or followed.
pub fn capture(home: &Path, attachments: &[PathBuf]) -> Result<Profile> {
    validate_absolute(home)?;
    inspect_path(home)?.context("Topgrade home directory does not exist")?;
    ensure!(
        attachments.len() <= MAX_FILES,
        "Too many Topgrade attachments"
    );
    let mut sources = Vec::new();
    for relative in [".config/topgrade.toml", ".config/topgrade/topgrade.toml"] {
        let path = home.join(relative);
        if inspect_path(&path)?.is_some() {
            sources.push(path);
        }
    }
    for relative in [".config/topgrade.d", ".config/topgrade/topgrade.d"] {
        let path = home.join(relative);
        let Some(metadata) = inspect_path(&path)? else {
            continue;
        };
        ensure!(
            metadata.is_dir(),
            "Topgrade drop-in path must be a directory"
        );
        for entry in fs::read_dir(path).context("Read Topgrade drop-in directory")? {
            let path = entry.context("Read Topgrade drop-in entry")?.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "toml")
            {
                sources.push(path);
                ensure!(
                    sources.len() <= MAX_FILES,
                    "Too many Topgrade configuration files"
                );
            }
        }
    }
    ensure!(
        !sources.is_empty() || !attachments.is_empty(),
        "No standard Topgrade configuration files or explicit attachments found"
    );
    sources.extend_from_slice(attachments);
    ensure!(
        sources.len() <= MAX_FILES,
        "Too many Topgrade archive files"
    );
    sources.sort();
    let mut files = Vec::new();
    let mut total = 0usize;
    for path in sources {
        validate_absolute(&path)?;
        let relative = path
            .strip_prefix(home)
            .context("Topgrade attachment is outside the selected home")?;
        let relative = relative.to_str().context("Topgrade paths must be UTF-8")?;
        let relative = relative.replace(std::path::MAIN_SEPARATOR, "/");
        validate_relative(&relative)?;
        let (bytes, metadata) = read_regular(&path, MAX_FILE_BYTES)?;
        total += bytes.len();
        ensure!(
            total <= MAX_TOTAL_BYTES,
            "Topgrade capture exceeds 512 KiB of text"
        );
        let contents = String::from_utf8(bytes).context("Topgrade attachment is not UTF-8 text")?;
        #[cfg(unix)]
        let executable = {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode() & 0o111 != 0
        };
        #[cfg(not(unix))]
        let executable = {
            let _ = metadata;
            false
        };
        files.push(Entry {
            path: relative,
            contents,
            executable,
        });
    }
    let profile = Profile {
        schema_version: 1,
        files,
    };
    profile.validate()?;
    for entry in &profile.files {
        ensure!(
            read_regular(&home.join(&entry.path), MAX_FILE_BYTES)?.0 == entry.contents.as_bytes(),
            "Topgrade source changed during capture; capture again"
        );
    }
    Ok(profile)
}

pub fn save(profile: &Profile, path: &Path) -> Result<()> {
    profile.validate()?;
    validate_absolute(path)?;
    let parent = path
        .parent()
        .context("Topgrade archive needs a parent directory")?;
    ensure!(
        inspect_path(parent)?.is_some_and(|metadata| metadata.is_dir()),
        "Topgrade archive parent does not exist"
    );
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let staging = parent.join(format!(
        ".topgrade-export-{}-{sequence}.tmp",
        std::process::id()
    ));
    let mut bytes = serde_json::to_vec_pretty(profile)?;
    bytes.push(b'\n');
    let mut file = create_private(&staging)?;
    let result = (|| -> Result<()> {
        file.write_all(&bytes).context("Write Topgrade archive")?;
        file.sync_all().context("Flush Topgrade archive")?;
        ensure!(
            read_regular(&staging, MAX_PROFILE_BYTES)?.0 == bytes,
            "Topgrade archive staging readback failed"
        );
        fs::hard_link(&staging, path)
            .context("Publish Topgrade archive; existing files are never replaced")?;
        ensure!(
            read_regular(path, MAX_PROFILE_BYTES)?.0 == bytes,
            "Topgrade archive readback failed"
        );
        #[cfg(unix)]
        File::open(parent)?
            .sync_all()
            .context("Flush Topgrade archive directory")?;
        Ok(())
    })();
    drop(file);
    let cleanup = fs::remove_file(&staging).context("Remove Topgrade archive staging file");
    match (result, cleanup) {
        (Ok(()), cleanup) => cleanup,
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup)) => Err(error.context(format!(
            "Topgrade archive staging cleanup failed: {cleanup}"
        ))),
    }
}

pub fn load(path: &Path) -> Result<Profile> {
    validate_absolute(path)?;
    let (bytes, _) = read_regular(path, MAX_PROFILE_BYTES)?;
    let profile: Profile = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("Invalid Topgrade archive JSON"))?;
    profile.validate()?;
    Ok(profile)
}

/// Materialize reviewed files beneath a newly reserved directory, never into live settings.
pub fn restore(profile: &Profile, selected: &BTreeSet<String>, destination: &Path) -> Result<()> {
    profile.validate()?;
    validate_absolute(destination)?;
    ensure!(
        !selected.is_empty(),
        "Select Topgrade archive files before restoring"
    );
    ensure!(
        selected
            .iter()
            .all(|path| profile.files.iter().any(|entry| &entry.path == path)),
        "Unknown Topgrade restore selection"
    );
    let parent = destination
        .parent()
        .context("Topgrade restore needs a parent directory")?;
    ensure!(
        inspect_path(parent)?.is_some_and(|metadata| metadata.is_dir()),
        "Topgrade restore parent does not exist"
    );
    create_private_dir(destination)
        .context("Reserve a new Topgrade review directory; existing paths are never replaced")?;
    let reserved = fs::symlink_metadata(destination)?;
    let result = (|| -> Result<()> {
        for entry in &profile.files {
            if !selected.contains(&entry.path) {
                continue;
            }
            let path = destination.join(&entry.path);
            let mut directory = destination.to_path_buf();
            let components: Vec<_> = Path::new(&entry.path).components().collect();
            for component in &components[..components.len() - 1] {
                directory.push(component.as_os_str());
                match inspect_path(&directory)? {
                    Some(metadata) => {
                        ensure!(metadata.is_dir(), "Topgrade review directory conflict")
                    }
                    None => create_private_dir(&directory)?,
                }
            }
            let mut file = create_private(&path)?;
            file.write_all(entry.contents.as_bytes())
                .context("Write Topgrade review file")?;
            file.sync_all().context("Flush Topgrade review file")?;
            ensure!(
                read_regular(&path, MAX_FILE_BYTES)?.0 == entry.contents.as_bytes(),
                "Topgrade review file readback failed"
            );
        }
        Ok(())
    })();
    if let Err(error) = result {
        let cleanup = (|| -> Result<()> {
            let current = fs::symlink_metadata(destination)?;
            ensure!(
                same_file(&reserved, &current),
                "Topgrade review directory changed; preserving it"
            );
            fs::remove_dir_all(destination).context("Remove incomplete Topgrade review directory")
        })();
        return match cleanup {
            Ok(()) => Err(error),
            Err(cleanup) => {
                Err(error.context(format!("Topgrade review cleanup failed: {cleanup}")))
            }
        };
    }
    Ok(())
}

fn validate_absolute(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute()
            && !path
                .components()
                .any(|component| matches!(component, Component::ParentDir | Component::CurDir)),
        "Topgrade paths must be absolute without traversal"
    );
    Ok(())
}

fn validate_relative(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty()
            && path.len() <= 512
            && !path.starts_with('/')
            && !path.contains(['\\', ':', '<', '>', '\"', '|', '?', '*'])
            && !path.chars().any(char::is_control),
        "Invalid Topgrade archive path"
    );
    for component in path.split('/') {
        ensure!(
            !component.is_empty()
                && component != "."
                && component != ".."
                && component.len() <= 128
                && !component.ends_with(['.', ' ']),
            "Invalid Topgrade archive path component"
        );
        let stem = component
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        ensure!(
            !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                && !(stem.len() == 4
                    && (stem.starts_with("COM") || stem.starts_with("LPT"))
                    && matches!(stem.as_bytes()[3], b'1'..=b'9')),
            "Reserved Topgrade archive path component"
        );
    }
    Ok(())
}

/// Inspect every component so neither files nor directories are followed through symlinks.
fn inspect_path(path: &Path) -> Result<Option<Metadata>> {
    let mut prefix = PathBuf::new();
    let mut last = None;
    for component in path.components() {
        prefix.push(component.as_os_str());
        let metadata = match fs::symlink_metadata(&prefix) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("Inspect Topgrade path"),
        };
        ensure!(
            !metadata.file_type().is_symlink(),
            "Topgrade paths cannot contain symlinks"
        );
        last = Some(metadata);
    }
    Ok(last)
}

fn read_regular(path: &Path, limit: usize) -> Result<(Vec<u8>, Metadata)> {
    let before = inspect_path(path)?.context("Topgrade file does not exist")?;
    ensure!(
        before.is_file() && before.len() <= limit as u64,
        "Topgrade file must be a bounded regular file"
    );
    let file = File::open(path).context("Open Topgrade file")?;
    let opened = file.metadata()?;
    ensure!(
        opened.is_file() && same_file(&before, &opened),
        "Topgrade file changed while opening"
    );
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .context("Read Topgrade file")?;
    let after = inspect_path(path)?.context("Topgrade file disappeared while reading")?;
    ensure!(
        bytes.len() <= limit
            && same_file(&before, &after)
            && before.len() == after.len()
            && before.modified()? == after.modified()?,
        "Topgrade file changed or exceeded its limit while reading"
    );
    Ok((bytes, after))
}

fn same_file(left: &Metadata, right: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        left.dev() == right.dev() && left.ino() == right.ino()
    }
    #[cfg(not(unix))]
    {
        left.is_dir() == right.is_dir()
            && left.len() == right.len()
            && left.created().ok() == right.created().ok()
    }
}

fn create_private(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).context("Create private Topgrade file")
}

fn create_private_dir(path: &Path) -> Result<()> {
    let builder = fs::DirBuilder::new();
    #[cfg(unix)]
    let mut builder = builder;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .context("Create private Topgrade directory")
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
                "studio-topgrade-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn fixture(directory: &Path) -> Profile {
        fs::create_dir_all(directory.join(".config/topgrade.d")).unwrap();
        fs::create_dir_all(directory.join(".local/bin")).unwrap();
        fs::write(
            directory.join(".config/topgrade.toml"),
            "[misc]\nonly = [\"custom_commands\"]\n",
        )
        .unwrap();
        fs::write(
            directory.join(".config/topgrade.d/local.toml"),
            "[commands]\nexample = \"never run\"",
        )
        .unwrap();
        fs::write(
            directory.join(".local/bin/example"),
            "#!/bin/sh\nprintf 'not run'",
        )
        .unwrap();
        capture(directory, &[directory.join(".local/bin/example")]).unwrap()
    }

    #[test]
    fn capture_save_restore_preserves_exact_selected_bytes_and_never_clobbers() {
        let home = Directory::new();
        let profile = fixture(&home.0);
        let archive = home.0.join("topgrade.json");
        save(&profile, &archive).unwrap();
        assert_eq!(load(&archive).unwrap(), profile);
        assert!(save(&profile, &archive).is_err());
        let selected = BTreeSet::from([".local/bin/example".into()]);
        let destination = home.0.join("review");
        restore(&profile, &selected, &destination).unwrap();
        assert_eq!(
            fs::read(destination.join(".local/bin/example")).unwrap(),
            b"#!/bin/sh\nprintf 'not run'"
        );
        assert!(!destination.join(".config").exists());
        assert!(restore(&profile, &selected, &destination).is_err());
        assert!(restore(&profile, &BTreeSet::new(), &home.0.join("empty")).is_err());
        assert!(
            restore(
                &profile,
                &BTreeSet::from(["unknown".into()]),
                &home.0.join("unknown")
            )
            .is_err()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&archive).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(destination.join(".local/bin/example"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn explicit_configuration_root_can_be_captured_without_standard_layout() {
        let home = Directory::new();
        assert!(capture(&home.0, &[]).is_err());
        let custom = home.0.join("custom-topgrade.toml");
        fs::write(&custom, "[misc]\n").unwrap();
        let profile = capture(&home.0, &[custom]).unwrap();
        assert_eq!(profile.files.len(), 1);
        assert_eq!(profile.files[0].path, "custom-topgrade.toml");
        assert_eq!(profile.files[0].contents, "[misc]\n");
    }

    #[test]
    fn archives_reject_ambiguous_paths_traversal_oversize_binary_and_unknown_json() {
        let home = Directory::new();
        let original = fixture(&home.0);
        for invalid in [
            "../escape",
            "/absolute",
            ".config/../escape",
            "a\\b",
            "C:/drive",
            "a//b",
            "NUL.txt",
            "a.",
            "wild*card",
            "question?mark",
            "file|pipe",
            "quote\"name",
            "angle<name",
        ] {
            let mut profile = original.clone();
            profile.files[0].path = invalid.into();
            assert!(profile.validate().is_err(), "{invalid}");
        }
        let mut duplicate = original.clone();
        duplicate.files.push(duplicate.files[0].clone());
        assert!(duplicate.validate().is_err());
        let mut conflict = original.clone();
        conflict.files[0].path = ".local".into();
        assert!(conflict.validate().is_err());
        let mut oversized = original.clone();
        oversized.files[0].contents = "x".repeat(MAX_FILE_BYTES + 1);
        assert!(oversized.validate().is_err());
        oversized.files[0].contents = "\0binary".into();
        assert!(oversized.validate().is_err());
        assert!(capture(&home.0, &[home.0.join("../outside")]).is_err());
        let invalid = home.0.join("invalid.json");
        fs::write(
            &invalid,
            br#"{"schema_version":1,"files":[],"secret":"PRIVATE"}"#,
        )
        .unwrap();
        let error = load(&invalid).unwrap_err().to_string();
        assert!(!error.contains("PRIVATE"));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_rejected_at_sources_ancestors_archives_and_destinations() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let home = Directory::new();
        fixture(&home.0);
        fs::set_permissions(
            home.0.join(".local/bin/example"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let profile = capture(&home.0, &[home.0.join(".local/bin/example")]).unwrap();
        assert!(
            profile
                .files
                .iter()
                .find(|entry| entry.path == ".local/bin/example")
                .unwrap()
                .executable
        );
        let dangling = home.0.join("dangling");
        symlink(home.0.join("missing"), &dangling).unwrap();
        assert!(save(&profile, &dangling).is_err());
        assert!(load(&dangling).is_err());
        assert!(
            restore(
                &profile,
                &BTreeSet::from([".local/bin/example".into()]),
                &dangling
            )
            .is_err()
        );
        symlink(home.0.join(".local/bin"), home.0.join("linked-bin")).unwrap();
        assert!(capture(&home.0, &[home.0.join("linked-bin/example")]).is_err());
        fs::remove_file(home.0.join(".config/topgrade.toml")).unwrap();
        symlink(
            home.0.join(".config/topgrade.d/local.toml"),
            home.0.join(".config/topgrade.toml"),
        )
        .unwrap();
        assert!(capture(&home.0, &[]).is_err());
    }
}
