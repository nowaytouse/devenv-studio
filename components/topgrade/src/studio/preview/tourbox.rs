use color_eyre::eyre::{ContextCompat, Result, ensure};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{self, IsTerminal, Read, Seek},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::LazyLock,
    time::Duration,
};
use zip::ZipArchive;

use super::{
    APPLICATIONS, bounded_output, bundle, download_to, field, private_path, running, signature, team, tool_command,
};
use crate::command::CommandExt as _;

const APP_ID: &str = "com.tourbox.ui.launch";
const PAGE: &str = "https://www.tourboxtech.com/jp/downloads/macos/";
const HOSTS: &[&str] = &["www.tourboxtech.com", "tourboxtech.com", "cdn.tourboxtech.com"];
const MAX_ARCHIVE: u64 = 1 << 30;
const MAX_EXPANDED: u64 = 8 << 30;
const MAX_TREE_ENTRIES: usize = 100_000;
const MAX_TREE_DEPTH: usize = 128;
const BUNDLE_VERIFICATION: &str = "signed-package-bundle-sha256-v1";

struct Release {
    version: String,
    beta: String,
    url: String,
}

#[derive(Deserialize, Serialize)]
struct Receipt {
    version: String,
    beta: String,
    url: String,
    sha256: String,
    app_id: String,
    app_version: String,
    app_build: String,
    team: String,
    // Old receipts recorded only version labels, which cannot prove package identity.
    #[serde(default)]
    bundle_sha256: Option<String>,
    #[serde(default)]
    verification: Option<String>,
}

fn numbers(version: &str) -> Result<Vec<u32>> {
    Ok(version
        .split('.')
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()?)
}

fn discover(page: &str) -> Result<Release> {
    static ARCHIVE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(https://cdn\.tourboxtech\.com/prod/console/TourBoxInstall([0-9]{1,5}(?:\.[0-9]{1,5}){2})Beta((?:[0-9]{1,5}(?:\.[0-9]{1,5}){0,4})?)_arm64_([0-9]{1,18})\.zip)")
            .expect("valid TourBox archive pattern")
    });
    let page = page.replace("\\/", "/").replace("&#47;", "/").replace("&#x2f;", "/");
    let mut candidates = Vec::new();
    for matched in ARCHIVE.captures_iter(&page) {
        let whole = matched.get(1).expect("archive group");
        if page
            .as_bytes()
            .get(whole.end())
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/'))
        {
            continue;
        }
        candidates.push((
            (
                numbers(&matched[2])?,
                if matched[3].is_empty() {
                    Vec::new()
                } else {
                    numbers(&matched[3])?
                },
                matched[4].parse::<u64>()?,
            ),
            Release {
                version: matched[2].into(),
                beta: matched[3].into(),
                url: whole.as_str().into(),
            },
        ));
    }
    candidates
        .into_iter()
        .max_by(|left, right| left.0.cmp(&right.0))
        .map(|(_, release)| release)
        .context("Official TourBox page has no ARM64 Beta ZIP")
}

fn package_team(report: &str) -> Result<String> {
    let mut lines = report.lines().map(str::trim);
    let status = lines
        .find_map(|line| line.strip_prefix("Status:"))
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase();
    ensure!(
        matches!(
            status.as_str(),
            "signed by a developer certificate issued by apple for distribution"
                | "signed by a certificate trusted by mac os x"
                | "signed by a certificate trusted by macos"
        ),
        "TourBox PKG did not report a trusted Apple signing status"
    );
    ensure!(
        report.lines().any(|line| line
            .trim()
            .eq_ignore_ascii_case("Notarization: trusted by the Apple notary service")),
        "TourBox PKG did not pass Apple notarization"
    );
    static TEAM: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^\s*1\.\s*Developer ID Installer:[^\n]*\(([A-Z0-9]{10})\)\s*$")
            .expect("valid Developer ID Installer pattern")
    });
    let ids: Vec<_> = TEAM.captures_iter(report).map(|record| record[1].to_owned()).collect();
    ensure!(ids.len() == 1, "TourBox PKG has no unique Developer ID Installer team");
    Ok(ids.into_iter().next().expect("verified one team"))
}

fn extract_archive(file: impl Read + Seek, root: &Path) -> Result<PathBuf> {
    let mut archive = ZipArchive::new(file)?;
    ensure!(
        !archive.is_empty() && archive.len() <= 4096,
        "Invalid TourBox ZIP entry count"
    );
    let mut paths = BTreeSet::new();
    let mut packages = BTreeSet::new();
    let mut expanded = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name()?;
        let plain = name.trim_end_matches('/');
        ensure!(
            !plain.is_empty()
                && !plain.contains(['\\', ':'])
                && plain
                    .split('/')
                    .all(|part| !part.is_empty() && !matches!(part, "." | ".."))
                && !plain.chars().any(char::is_control),
            "Unsafe TourBox ZIP path"
        );
        let path = Path::new(plain);
        ensure!(
            path.components()
                .all(|component| matches!(component, Component::Normal(_))),
            "Unsafe TourBox ZIP path"
        );
        ensure!(paths.insert(plain.to_ascii_lowercase()), "Duplicate TourBox ZIP path");
        let kind = entry.unix_mode().unwrap_or(0) & 0o170000;
        let directory = entry.is_dir();
        ensure!(
            kind == 0 || if directory { kind == 0o040000 } else { kind == 0o100000 },
            "Unsafe TourBox ZIP entry type"
        );
        let mut parent = root.to_path_buf();
        for part in path.components() {
            let Component::Normal(part) = part else {
                unreachable!("validated path")
            };
            parent.push(part);
            if parent.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("pkg")) {
                packages.insert(parent.clone());
                break;
            }
        }
        let target = root.join(path);
        if directory {
            fs::create_dir_all(target)?;
        } else {
            let size = entry.size();
            expanded = expanded.checked_add(size).context("TourBox ZIP size overflow")?;
            ensure!(expanded <= MAX_ARCHIVE, "Expanded TourBox ZIP exceeds its limit");
            fs::create_dir_all(target.parent().context("Missing TourBox extraction directory")?)?;
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&target)?;
            ensure!(
                io::copy(&mut io::Read::take(&mut entry, size + 1), &mut output)? == size,
                "TourBox ZIP entry has a wrong length"
            );
        }
    }
    ensure!(
        packages.len() == 1,
        "Expected exactly one TourBox PKG in the official ZIP"
    );
    Ok(packages.pop_first().expect("validated package count"))
}

fn backup_presets(home: &Path) -> Result<()> {
    let parent = home.join("Library/Application Support");
    let source = parent.join("TourBox Console");
    if !super::super::exists(&source)? {
        println!("TourBox: no default presets directory to back up");
        return Ok(());
    }
    ensure!(
        fs::symlink_metadata(&source)?.is_dir() && source.canonicalize()? == source,
        "Unsafe TourBox presets directory"
    );
    let directory = private_path(home, ".local/share/tourbox-beta/backups")?;
    let target = tempfile::Builder::new()
        .prefix("TourBox-")
        .suffix(".tar.gz")
        .tempfile_in(directory)?;
    tool_command(Path::new("/usr/bin/tar"))?
        .args(["-czf", "-", "-C"])
        .arg(parent)
        .arg("TourBox Console")
        .stdout(Stdio::from(target.as_file().try_clone()?))
        .status_checked()?;
    target.as_file().sync_all()?;
    tool_command(Path::new("/usr/bin/tar"))?
        .arg("-tzf")
        .arg(target.path())
        .stdout(Stdio::null())
        .status_checked()?;
    let (_, path) = target.keep()?;
    println!("TourBox preset backup: {}", path.display());
    Ok(())
}

fn state(path: &Path) -> Result<Option<Vec<u8>>> {
    if !super::super::exists(path)? {
        return Ok(None);
    }
    let file = super::super::open_file(path, true, 128 * 1024)?;
    let mut bytes = Vec::new();
    io::Read::read_to_end(&mut io::Read::take(file, 128 * 1024 + 1), &mut bytes)?;
    ensure!(bytes.len() <= 128 * 1024, "TourBox receipt is too large");
    Ok(Some(bytes))
}

fn validate_executable(app: &Path, info: &Value) -> Result<()> {
    let name = field(info, "CFBundleExecutable")?;
    ensure!(
        !name.contains('/') && !matches!(name.as_str(), "." | ".."),
        "Unsafe TourBox executable name"
    );
    let path = app.join("Contents/MacOS").join(name);
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
        .open(&path)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() <= 256 * 1024 * 1024 && path.canonicalize()? == path,
        "Unsafe TourBox application executable"
    );
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct TreeStamp {
    device: u64,
    inode: u64,
    length: u64,
    mode: u32,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl TreeStamp {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            length: metadata.len(),
            mode: metadata.mode(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct TreeEntry {
    relative: PathBuf,
    kind: u8,
    stamp: TreeStamp,
    target: Option<PathBuf>,
}

fn internal_symlink(relative: &Path, target: &Path) -> bool {
    let mut depth = relative.parent().map_or(0, |parent| parent.components().count());
    for component in target.components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir if depth > 0 => depth -= 1,
            _ => return false,
        }
    }
    !target.as_os_str().is_empty()
}

// Never follow links while enumerating either an expanded package or an app bundle.
// The same bounded inventory is taken again after hashing to catch concurrent edits.
fn tree_entries(root: &Path) -> Result<Vec<TreeEntry>> {
    ensure!(fs::symlink_metadata(root)?.is_dir(), "Unsafe TourBox tree root");
    let mut entries = Vec::new();
    let mut bytes = 0u64;
    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .max_open(32)
        .max_depth(MAX_TREE_DEPTH + 1)
    {
        let entry = entry?;
        ensure!(entry.depth() <= MAX_TREE_DEPTH, "TourBox tree is too deeply nested");
        ensure!(entries.len() < MAX_TREE_ENTRIES, "TourBox tree has too many entries");
        let relative = entry.path().strip_prefix(root)?.to_path_buf();
        ensure!(
            relative.as_os_str().as_bytes().len() <= 4096,
            "TourBox tree path is too long"
        );
        let metadata = fs::symlink_metadata(entry.path())?;
        let (kind, target) = if metadata.is_dir() {
            (b'd', None)
        } else if metadata.is_file() {
            bytes = bytes
                .checked_add(metadata.len())
                .context("TourBox expanded size overflow")?;
            ensure!(bytes <= MAX_EXPANDED, "Expanded TourBox content exceeds its limit");
            (b'f', None)
        } else {
            ensure!(metadata.is_symlink(), "Unsupported TourBox tree entry type");
            let target = fs::read_link(entry.path())?;
            ensure!(
                target.as_os_str().as_bytes().len() <= 4096
                    && internal_symlink(&relative, &target)
                    && entry.path().canonicalize()?.starts_with(root),
                "TourBox symlink escapes its tree"
            );
            (b'l', Some(target))
        };
        entries.push(TreeEntry {
            relative,
            kind,
            stamp: TreeStamp::from_metadata(&metadata),
            target,
        });
    }
    entries.sort_by(|left, right| {
        left.relative
            .as_os_str()
            .as_bytes()
            .cmp(right.relative.as_os_str().as_bytes())
    });
    Ok(entries)
}

fn hash_bytes(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_be_bytes());
    hash.update(bytes);
}

fn bundle_digest(app: &Path) -> Result<String> {
    ensure!(fs::symlink_metadata(app)?.is_dir(), "Unsafe TourBox application path");
    let root = app.canonicalize()?;
    let entries = tree_entries(&root)?;
    let mut hash = Sha256::new();
    hash.update(b"TourBox bundle tree SHA-256 v1\0");
    for entry in &entries {
        hash.update([entry.kind]);
        hash_bytes(&mut hash, entry.relative.as_os_str().as_bytes());
        // Installer legitimately changes ownership, permissions, timestamps and
        // quarantine attributes (TourBox's signed postinstall runs chmod +x).
        // Compare signed content: every relative path, entry type, file byte and
        // symlink target. Code-signature verification remains independently required.
        if let Some(target) = &entry.target {
            hash_bytes(&mut hash, target.as_os_str().as_bytes());
        } else if entry.kind == b'f' {
            let path = root.join(&entry.relative);
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
                .open(&path)?;
            ensure!(
                TreeStamp::from_metadata(&file.metadata()?) == entry.stamp && path.canonicalize()? == path,
                "TourBox content changed before hashing"
            );
            hash.update(entry.stamp.length.to_be_bytes());
            let mut input = file.take(entry.stamp.length + 1);
            let mut buffer = [0u8; 64 * 1024];
            let mut read = 0u64;
            loop {
                let count = input.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                read += count as u64;
                hash.update(&buffer[..count]);
            }
            ensure!(
                read == entry.stamp.length && TreeStamp::from_metadata(&input.get_ref().metadata()?) == entry.stamp,
                "TourBox content changed while hashing"
            );
        }
    }
    ensure!(tree_entries(&root)? == entries, "TourBox tree changed while hashing");
    Ok(format!("{:x}", hash.finalize()))
}

fn find_payload(expanded: &Path) -> Result<PathBuf> {
    let root = expanded.canonicalize()?;
    let mut apps = Vec::new();
    for entry in tree_entries(&root)? {
        if entry.kind == b'd' && entry.relative.ends_with("Payload/Applications/TourBox Console.app") {
            let app = root.join(entry.relative);
            let component = app
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent)
                .context("Missing TourBox package component")?;
            let metadata = fs::symlink_metadata(component.join("PackageInfo"))?;
            ensure!(
                metadata.is_file() && metadata.len() <= 4 * 1024 * 1024,
                "Unsafe TourBox package metadata"
            );
            apps.push(app);
        }
    }
    ensure!(
        apps.len() == 1,
        "Expected exactly one TourBox application in the verified PKG payload"
    );
    Ok(apps.pop().expect("validated payload count"))
}

fn verify_payload_match(
    installed: &Value,
    payload: &Value,
    installed_sha256: &str,
    payload_sha256: &str,
) -> Result<()> {
    ensure!(
        installed_sha256 == payload_sha256
            && field(installed, "CFBundleShortVersionString")? == field(payload, "CFBundleShortVersionString")?
            && field(installed, "CFBundleVersion")? == field(payload, "CFBundleVersion")?,
        "Installed TourBox does not exactly match the verified signed application payload"
    );
    Ok(())
}

fn receipt_matches(
    receipt: &Receipt,
    release: &Release,
    version: &str,
    build: &str,
    publisher: &str,
    digest: &str,
) -> bool {
    receipt.version == release.version
        && receipt.beta == release.beta
        && receipt.url == release.url
        && receipt.app_id == APP_ID
        && receipt.app_version == version
        && receipt.app_build == build
        && receipt.team == publisher
        && super::super::is_hex(&receipt.sha256, 64)
        && receipt.verification.as_deref() == Some(BUNDLE_VERIFICATION)
        && receipt.bundle_sha256.as_deref() == Some(digest)
}

pub(in crate::studio) fn tourbox_beta() -> Result<()> {
    super::super::user_only()?;
    let home = super::super::account_home()?;
    let _lock = super::lock(&home, "tourbox-beta.lock")?;
    let workspace = tempfile::tempdir()?;
    let page = workspace.path().join("release.html");
    download_to(PAGE, HOSTS, &page, 8 * 1024 * 1024, 60, false, true)?;
    let release = discover(&fs::read_to_string(page)?)?;
    let app = Path::new(APPLICATIONS).join("TourBox Console.app");
    let info = bundle(&app, APP_ID)?;
    validate_executable(&app, &info)?;
    let version = field(&info, "CFBundleShortVersionString")?;
    let build = field(&info, "CFBundleVersion")?;
    signature(&app, None)?;
    let expected_team = team(&app)?;
    let original_bundle = bundle_digest(&app)?;
    let path = private_path(&home, ".local/state")?.join("tourbox-beta.json");
    let original = state(&path)?;
    let receipt = original.as_deref().map(serde_json::from_slice::<Receipt>).transpose()?;
    println!(
        "TourBox: installed bundle {version} ({build}), official target {} Beta{}",
        release.version, release.beta
    );
    if receipt
        .as_ref()
        .is_some_and(|receipt| receipt_matches(receipt, &release, &version, &build, &expected_team, &original_bundle))
    {
        println!("TourBox: CURRENT — full signed bundle matches the previously verified official application payload");
        return Ok(());
    }
    let local_base = version
        .split(|character: char| !character.is_ascii_digit() && character != '.')
        .next()
        .unwrap_or_default();
    if numbers(local_base)? > numbers(&release.version)? {
        println!("TourBox: SKIPPED — local version is newer");
        return Ok(());
    }
    let archive = workspace.path().join("TourBoxBeta.zip");
    download_to(&release.url, HOSTS, &archive, MAX_ARCHIVE, 1800, false, false)?;
    ensure!(
        fs::metadata(&archive)?.len() >= 100_000,
        "Official TourBox ZIP is unexpectedly small"
    );
    let digest = super::super::sha256(File::open(&archive)?)?;
    let extracted = workspace.path().join("extracted");
    fs::create_dir(&extracted)?;
    let package = extract_archive(File::open(&archive)?, &extracted)?;
    let (status, stdout, stderr) = bounded_output(
        tool_command(Path::new("/usr/sbin/pkgutil"))?
            .env("LC_ALL", "C")
            .arg("--check-signature")
            .arg(&package),
        Duration::from_secs(120),
        128 * 1024,
    )?;
    ensure!(status.success(), "TourBox PKG signature check failed");
    ensure!(
        package_team(&format!("{stdout}\n{stderr}"))? == expected_team,
        "TourBox PKG publisher does not match the installed application"
    );
    // Expand only after verifying the package's Apple signature, notarization and team.
    // pkgutil does not execute installer scripts here; all content is inspected as the user.
    let expanded = workspace.path().join("expanded-pkg");
    let (status, _, _) = bounded_output(
        tool_command(Path::new("/usr/sbin/pkgutil"))?
            .arg("--expand-full")
            .arg(&package)
            .arg(&expanded),
        Duration::from_secs(600),
        128 * 1024,
    )?;
    ensure!(status.success(), "Could not expand the verified TourBox PKG");
    let payload_app = find_payload(&expanded)?;
    let payload_info = bundle(&payload_app, APP_ID)?;
    validate_executable(&payload_app, &payload_info)?;
    signature(&payload_app, None)?;
    ensure!(
        team(&payload_app)? == expected_team,
        "TourBox PKG payload publisher differs from its verified installer"
    );
    let payload_version = field(&payload_info, "CFBundleShortVersionString")?;
    let payload_build = field(&payload_info, "CFBundleVersion")?;
    let payload_digest = bundle_digest(&payload_app)?;
    println!(
        "TourBox: official {} Beta{} contains signed bundle {payload_version} ({payload_build}); verifying all bundle content",
        release.version, release.beta
    );
    let install = original_bundle != payload_digest;
    if install {
        ensure!(
            io::stdin().is_terminal(),
            "TourBox Beta installation requires a Terminal for sudo"
        );
        ensure!(
            !running(&app, "/tourbox console")?,
            "Close TourBox Console before installing the Beta"
        );
        backup_presets(&home)?;
        let checked = bundle(&app, APP_ID)?;
        validate_executable(&app, &checked)?;
        signature(&app, None)?;
        ensure!(
            field(&checked, "CFBundleShortVersionString")? == version
                && field(&checked, "CFBundleVersion")? == build
                && bundle_digest(&app)? == original_bundle
                && team(&app)? == expected_team
                && state(&path)? == original
                && !running(&app, "/tourbox console")?,
            "TourBox or its receipt changed after review"
        );
        tool_command(Path::new("/usr/bin/sudo"))?
            .args(["/usr/sbin/installer", "-pkg"])
            .arg(&package)
            .args(["-target", "/"])
            .stdin(Stdio::inherit())
            .status_checked()?;
    }
    let installed = bundle(&app, APP_ID)?;
    validate_executable(&app, &installed)?;
    signature(&app, None)?;
    ensure!(
        team(&app)? == expected_team,
        "Installed TourBox publisher differs from the verified PKG"
    );
    let app_version = field(&installed, "CFBundleShortVersionString")?;
    let app_build = field(&installed, "CFBundleVersion")?;
    let installed_digest = bundle_digest(&app)?;
    verify_payload_match(&installed, &payload_info, &installed_digest, &payload_digest)?;
    ensure!(state(&path)? == original, "TourBox receipt changed during the update");
    let parent = path.parent().context("Missing TourBox receipt directory")?;
    let mut pending = tempfile::NamedTempFile::new_in(parent)?;
    let receipt = Receipt {
        version: release.version,
        beta: release.beta,
        url: release.url,
        sha256: digest,
        app_id: APP_ID.into(),
        app_version,
        app_build,
        team: expected_team,
        bundle_sha256: Some(installed_digest),
        verification: Some(BUNDLE_VERIFICATION.into()),
    };
    serde_json::to_writer_pretty(&mut pending, &receipt)?;
    pending.as_file().sync_all()?;
    pending.persist(&path)?;
    File::open(parent)?.sync_all()?;
    println!(
        "TourBox: {} — official {} Beta{}; exact signed application payload verified (bundle {} / build {})",
        if install {
            "INSTALLED"
        } else {
            "CURRENT — no installation needed"
        },
        receipt.version,
        receipt.beta,
        receipt.app_version,
        receipt.app_build
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use std::os::unix::fs::{PermissionsExt, symlink};
    use zip::{ZipWriter, write::SimpleFileOptions};

    #[test]
    fn only_official_arm64_archive_and_notarized_package_are_admitted() -> Result<()> {
        let html = r#""https:\/\/cdn.tourboxtech.com\/prod\/console\/TourBoxInstall5.12.0Beta9_arm64_250.zip" "https://cdn.tourboxtech.com/prod/console/TourBoxInstall5.12.0Beta10.2_arm64_100.zip" "https://cdn.tourboxtech.com/prod/console/TourBoxInstall6.0.0Beta10_x86_64_300.zip""#;
        assert_eq!(discover(html)?.beta, "10.2");
        assert!(
            discover("https://cdn.tourboxtech.com/prod/console/TourBoxInstall5.12.0Beta10_arm64_100.zip.exe").is_err()
        );
        let signed = "Status: signed by a developer certificate issued by Apple for distribution\nNotarization: trusted by the Apple notary service\n1. Developer ID Installer: Example (ABCDEFGHIJ)\n";
        assert_eq!(package_team(signed)?, "ABCDEFGHIJ");
        assert!(package_team(&signed.replace("trusted by the Apple notary service", "not notarized")).is_err());
        Ok(())
    }

    #[test]
    fn zip_extracts_one_regular_package_and_refuses_symlinks() -> Result<()> {
        let root = tempfile::tempdir()?;
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer.start_file("installer/TourBox.pkg", SimpleFileOptions::default())?;
        writer.write_all(b"pkg fixture")?;
        let package = extract_archive(writer.finish()?, &root.path().join("good"))?;
        assert_eq!(fs::read(package)?, b"pkg fixture");
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer.add_symlink("TourBox.pkg", "/tmp/unknown", SimpleFileOptions::default())?;
        assert!(extract_archive(writer.finish()?, &root.path().join("unsafe")).is_err());
        assert!(!root.path().join("unsafe/TourBox.pkg").exists());
        Ok(())
    }

    fn fixture_bundle(root: &Path) -> Result<PathBuf> {
        let app = root.join("TourBox Console.app");
        fs::create_dir_all(app.join("Contents/MacOS"))?;
        fs::create_dir_all(app.join("Contents/Resources/app/TourBox.app/Contents"))?;
        fs::write(app.join("Contents/MacOS/TourBox Console"), b"unchanged launcher")?;
        fs::write(app.join("Contents/Info.plist"), b"outer version 5.12.1")?;
        fs::write(
            app.join("Contents/Resources/app/TourBox.app/Contents/Info.plist"),
            b"nested version 6.0.0",
        )?;
        Ok(app)
    }

    #[test]
    fn bundle_digest_covers_nested_resources_and_file_names() -> Result<()> {
        let root = tempfile::tempdir()?;
        let app = fixture_bundle(root.path())?;
        let original = bundle_digest(&app)?;
        let resource = app.join("Contents/Resources/app/TourBox.app/Contents/Info.plist");
        fs::write(&resource, b"nested version 6.0.1")?;
        assert_ne!(bundle_digest(&app)?, original);
        fs::write(&resource, b"nested version 6.0.0")?;
        assert_eq!(bundle_digest(&app)?, original);
        fs::rename(&resource, resource.with_extension("renamed"))?;
        assert_ne!(bundle_digest(&app)?, original);
        Ok(())
    }

    #[test]
    fn main_executable_requires_a_regular_canonical_file_with_a_basename() -> Result<()> {
        let root = tempfile::tempdir()?;
        let app = fixture_bundle(root.path())?.canonicalize()?;
        let info = serde_json::json!({"CFBundleExecutable": "TourBox Console"});
        validate_executable(&app, &info)?;
        let traversal = serde_json::json!({"CFBundleExecutable": "../TourBox Console"});
        assert!(validate_executable(&app, &traversal).is_err());
        let executable = app.join("Contents/MacOS/TourBox Console");
        fs::rename(&executable, executable.with_file_name("real-launcher"))?;
        symlink("real-launcher", &executable)?;
        assert!(validate_executable(&app, &info).is_err());
        Ok(())
    }

    #[test]
    fn bundle_digest_is_independent_of_root_and_creation_order() -> Result<()> {
        let left = tempfile::tempdir()?;
        let right = tempfile::tempdir()?;
        let app = fixture_bundle(left.path())?;
        let copy = fixture_bundle(right.path())?;
        fs::write(app.join("Contents/Resources/z"), b"z")?;
        fs::write(app.join("Contents/Resources/a"), b"a")?;
        fs::write(copy.join("Contents/Resources/a"), b"a")?;
        fs::write(copy.join("Contents/Resources/z"), b"z")?;
        assert_eq!(bundle_digest(&app)?, bundle_digest(&copy)?);
        fs::write(copy.join("Contents/Resources/extra"), b"")?;
        assert_ne!(bundle_digest(&app)?, bundle_digest(&copy)?);
        fs::remove_file(copy.join("Contents/Resources/extra"))?;
        fs::remove_file(copy.join("Contents/Resources/a"))?;
        assert_ne!(bundle_digest(&app)?, bundle_digest(&copy)?);
        Ok(())
    }

    #[test]
    fn bundle_digest_allows_installer_permission_changes_and_covers_symlink_targets() -> Result<()> {
        let root = tempfile::tempdir()?;
        let app = fixture_bundle(root.path())?;
        let executable = app.join("Contents/MacOS/TourBox Console");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o744))?;
        let original = bundle_digest(&app)?;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))?;
        assert_eq!(bundle_digest(&app)?, original);
        let link = app.join("Contents/Resources/current");
        symlink("app", &link)?;
        let linked = bundle_digest(&app)?;
        fs::remove_file(&link)?;
        symlink("app/TourBox.app", &link)?;
        assert_ne!(bundle_digest(&app)?, linked);
        fs::remove_file(&link)?;
        symlink("../../../outside", &link)?;
        assert!(bundle_digest(&app).is_err());
        fs::remove_file(&link)?;
        symlink("/Applications/Other.app", &link)?;
        assert!(bundle_digest(&app).is_err());
        Ok(())
    }

    #[test]
    fn bundle_digest_rejects_links_that_escape_through_another_link() -> Result<()> {
        let root = tempfile::tempdir()?;
        let app = fixture_bundle(root.path())?;
        fs::write(root.path().join("outside"), b"outside the bundle")?;
        symlink("../..", app.join("Contents/Resources/up"))?;
        symlink("up/../outside", app.join("Contents/Resources/escape"))?;
        assert!(bundle_digest(&app).is_err());
        Ok(())
    }

    #[test]
    fn bundle_digest_rejects_oversized_or_special_entries() -> Result<()> {
        let root = tempfile::tempdir()?;
        let app = fixture_bundle(root.path())?;
        let large = app.join("Contents/Resources/large");
        File::create(&large)?.set_len(MAX_EXPANDED + 1)?;
        assert!(bundle_digest(&app).is_err());
        fs::remove_file(&large)?;
        let fifo = std::ffi::CString::new(app.join("Contents/pipe").as_os_str().as_bytes())?;
        // SAFETY: CString supplies a valid, NUL-terminated path for this test fixture.
        let status = unsafe { nix::libc::mkfifo(fifo.as_ptr(), 0o600) };
        assert_eq!(
            status,
            0,
            "Could not create FIFO fixture: {}",
            io::Error::last_os_error()
        );
        assert!(bundle_digest(&app).is_err());
        Ok(())
    }

    #[test]
    fn payload_selection_requires_one_exact_application_location() -> Result<()> {
        let root = tempfile::tempdir()?;
        let component = root.path().join("TourBox.pkg");
        fs::create_dir_all(component.join("Payload/Applications"))?;
        fs::write(component.join("PackageInfo"), b"package metadata")?;
        let app = fixture_bundle(&component.join("Payload/Applications"))?;
        fixture_bundle(&component.join("Resources"))?;
        assert_eq!(find_payload(root.path())?, app.canonicalize()?);
        let duplicate = root.path().join("Other.pkg");
        fs::create_dir_all(duplicate.join("Payload/Applications"))?;
        fs::write(duplicate.join("PackageInfo"), b"other package metadata")?;
        fixture_bundle(&duplicate.join("Payload/Applications"))?;
        assert!(find_payload(root.path()).is_err());
        Ok(())
    }

    #[test]
    fn payload_verification_accepts_stale_metadata_only_with_identical_full_content() -> Result<()> {
        // The advertised release is 5.13.0 Beta; its signed outer bundle is 5.12.1.
        let payload = serde_json::json!({"CFBundleShortVersionString": "5.12.1", "CFBundleVersion": "5.12.1"});
        let sha256 = "a".repeat(64);
        verify_payload_match(&payload, &payload, &sha256, &sha256)?;
        assert!(verify_payload_match(&payload, &payload, &"b".repeat(64), &sha256).is_err());
        let changed = serde_json::json!({"CFBundleShortVersionString": "5.13.0", "CFBundleVersion": "5.13.0"});
        assert!(verify_payload_match(&changed, &payload, &sha256, &sha256).is_err());
        Ok(())
    }

    #[test]
    fn legacy_receipt_cannot_skip_full_payload_verification() -> Result<()> {
        let release =
            discover("https://cdn.tourboxtech.com/prod/console/TourBoxInstall5.13.0Beta_arm64_261117174511.zip")?;
        let mut receipt: Receipt = serde_json::from_value(serde_json::json!({
            "version": "5.13.0", "beta": "", "url": release.url,
            "sha256": "a".repeat(64), "app_id": APP_ID,
            "app_version": "5.12.1", "app_build": "5.12.1", "team": "8B253ZL4W9"
        }))?;
        let digest = "b".repeat(64);
        assert!(!receipt_matches(
            &receipt,
            &release,
            "5.12.1",
            "5.12.1",
            "8B253ZL4W9",
            &digest
        ));
        receipt.bundle_sha256 = Some(digest.clone());
        receipt.verification = Some(BUNDLE_VERIFICATION.into());
        assert!(receipt_matches(
            &receipt,
            &release,
            "5.12.1",
            "5.12.1",
            "8B253ZL4W9",
            &digest
        ));
        assert!(!receipt_matches(
            &receipt,
            &release,
            "5.12.1",
            "5.12.1",
            "8B253ZL4W9",
            &"c".repeat(64)
        ));
        receipt.beta = "99".into();
        assert!(!receipt_matches(
            &receipt,
            &release,
            "5.12.1",
            "5.12.1",
            "8B253ZL4W9",
            &digest
        ));
        Ok(())
    }
}

#[cfg(test)]
#[test]
fn tourbox_unnumbered_beta_discovery() {
    let url = "https://cdn.tourboxtech.com/prod/console/TourBoxInstall5.13.0Beta_arm64_261117174511.zip";
    let release = discover(url).expect("unnumbered Beta must be discovered");
    assert_eq!(release.version, "5.13.0");
    assert_eq!(release.beta, "");
    assert_eq!(release.url, url);

    let older = "https://cdn.tourboxtech.com/prod/console/TourBoxInstall5.13.0Beta2_arm64_261117174511.zip";
    assert_eq!(discover(older).unwrap().beta, "2");
}
