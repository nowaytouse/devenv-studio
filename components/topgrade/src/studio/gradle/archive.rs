//! Extract only bounded regular Gradle distribution entries into private staging.
use color_eyre::eyre::{Result, ensure, eyre};
use std::{
    collections::HashSet,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{self, Read, Seek},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
use zip::ZipArchive;

const MAX_FILES: usize = 8192;
const MAX_EXPANDED: u64 = 2 * 1024 * 1024 * 1024;

fn entry_path(name: &[u8], directory: bool, expected_root: &str) -> Result<PathBuf> {
    let name = std::str::from_utf8(name)?;
    let path = if directory {
        name.strip_suffix('/')
            .ok_or_else(|| eyre!("Gradle ZIP directory is missing its separator"))?
    } else {
        name
    };
    ensure!(
        path.len() <= 1024
            && path
                .bytes()
                .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'\\' | b':'))
            && path.split('/').next() == Some(expected_root)
            && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
            && (directory || path.contains('/')),
        "Unsafe Gradle ZIP entry path"
    );
    Ok(PathBuf::from(path))
}

pub(super) fn extract(source: impl Read + Seek, directory: &Path, version: &str) -> Result<PathBuf> {
    let mut archive = ZipArchive::new(source)?;
    ensure!(
        !archive.is_empty() && archive.len() <= MAX_FILES,
        "Unexpected Gradle ZIP entry count"
    );
    archive.validate_header_consistency()?;
    DirBuilder::new().mode(0o700).create(directory)?;
    let expected_root = format!("gradle-{version}");
    let mut paths = HashSet::new();
    let mut remaining = MAX_EXPANDED;
    for index in 0..archive.len() {
        let mut member = archive.by_index(index)?;
        let is_dir = member.is_dir();
        let expected_type = if is_dir { nix::libc::S_IFDIR } else { nix::libc::S_IFREG };
        ensure!(
            !member.is_symlink()
                && !member.encrypted()
                && member.unix_mode().is_none_or(|mode| {
                    let kind = mode & u32::from(nix::libc::S_IFMT);
                    kind == 0 || kind == u32::from(expected_type)
                }),
            "Gradle ZIP contains a symbolic link, encrypted data or special entry"
        );
        let path = entry_path(member.name_raw(), is_dir, &expected_root)?;
        ensure!(
            paths.insert(path.to_string_lossy().to_ascii_lowercase()),
            "Gradle ZIP contains conflicting entry names"
        );
        let dest = directory.join(&path);
        let size = member.size();
        ensure!(size <= remaining, "Gradle ZIP exceeds the 2 GiB extraction limit");
        if is_dir {
            ensure!(
                size == 0 && member.read(&mut [0u8; 1])? == 0,
                "Gradle ZIP contains a directory with data"
            );
            fs::create_dir_all(&dest)?;
            continue;
        }
        fs::create_dir_all(dest.parent().ok_or_else(|| eyre!("Gradle ZIP entry has no parent"))?)?;
        let mode = if path.ends_with("bin/gradle") { 0o755 } else { 0o644 };
        let mut file = OpenOptions::new().write(true).create_new(true).mode(mode).open(&dest)?;
        let expanded = io::copy(&mut member.take(size + 1), &mut file)?;
        ensure!(expanded == size, "Gradle ZIP entry differs from its declared size");
        file.sync_all()?;
        remaining -= expanded;
    }
    let root = directory.join(expected_root);
    File::open(&root)?.sync_all()?;
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::{ZipWriter, write::SimpleFileOptions};

    const VERSION: &str = "9.10.0-20261104002831+0000";

    #[test]
    fn stages_regular_files_and_rejects_escapes_and_symbolic_links() -> Result<()> {
        let root = format!("gradle-{VERSION}");
        let base = tempfile::tempdir()?;
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default();
        writer.start_file(format!("{root}/bin/gradle"), opts)?;
        writer.write_all(b"launcher")?;
        writer.add_directory(format!("{root}/lib/"), opts)?;
        let verified = extract(writer.finish()?, &base.path().join("valid"), VERSION)?;
        assert_eq!(fs::read(verified.join("bin/gradle"))?, b"launcher");
        assert!(verified.join("lib").is_dir());

        assert!(entry_path(format!("{root}/../escape").as_bytes(), false, &root).is_err());
        assert!(entry_path(format!("/{root}/bin/gradle").as_bytes(), false, &root).is_err());
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer.add_symlink(format!("{root}/escape"), "/tmp", opts)?;
        let result = extract(writer.finish()?, &base.path().join("invalid"), VERSION);
        assert!(result.is_err());
        assert!(!base.path().join("invalid").join(root).join("escape").exists());
        Ok(())
    }

    #[test]
    fn rejects_crc_mismatch_while_streaming() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        writer.start_file(format!("gradle-{VERSION}/bin/gradle"), opts)?;
        let content = b"fixture-launcher-content";
        writer.write_all(content)?;
        let mut bytes = writer.finish()?.into_inner();
        let payload = bytes
            .windows(content.len())
            .position(|window| window == content)
            .unwrap();
        bytes[payload] ^= 1;
        assert!(extract(Cursor::new(bytes), &directory.path().join("bad-crc"), VERSION).is_err());
        Ok(())
    }
}
