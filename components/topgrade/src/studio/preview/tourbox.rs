use color_eyre::eyre::{ContextCompat, Result, ensure};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{self, IsTerminal, Read, Seek},
    os::unix::fs::OpenOptionsExt,
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
}

fn numbers(version: &str) -> Result<Vec<u32>> {
    Ok(version
        .split('.')
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()?)
}

fn discover(page: &str) -> Result<Release> {
    static ARCHIVE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(https://cdn\.tourboxtech\.com/prod/console/TourBoxInstall([0-9]{1,5}(?:\.[0-9]{1,5}){2})Beta([0-9]{1,5}(?:\.[0-9]{1,5}){0,4})_arm64_([0-9]{1,18})\.zip)")
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
            (numbers(&matched[2])?, numbers(&matched[3])?, matched[4].parse::<u64>()?),
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

fn executable_digest(app: &Path, info: &Value) -> Result<String> {
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
    super::super::sha256(file)
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
    let version = field(&info, "CFBundleShortVersionString")?;
    let build = field(&info, "CFBundleVersion")?;
    let executable = executable_digest(&app, &info)?;
    signature(&app, None)?;
    let expected_team = team(&app)?;
    let path = private_path(&home, ".local/state")?.join("tourbox-beta.json");
    let original = state(&path)?;
    let receipt = original.as_deref().map(serde_json::from_slice::<Receipt>).transpose()?;
    println!(
        "TourBox: installed {version} ({build}), official target {} Beta {}",
        release.version, release.beta
    );
    if receipt.as_ref().is_some_and(|receipt| {
        receipt.url == release.url
            && receipt.app_id == APP_ID
            && receipt.app_version == version
            && receipt.app_build == build
            && receipt.team == expected_team
            && super::super::is_hex(&receipt.sha256, 64)
    }) {
        println!("TourBox: CURRENT");
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
    ensure!(
        io::stdin().is_terminal(),
        "TourBox Beta installation requires a Terminal for sudo"
    );
    ensure!(
        !running(&app, "/tourbox console")?,
        "Close TourBox Console before installing the Beta"
    );
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
    backup_presets(&home)?;
    let checked = bundle(&app, APP_ID)?;
    ensure!(
        field(&checked, "CFBundleShortVersionString")? == version
            && field(&checked, "CFBundleVersion")? == build
            && executable_digest(&app, &checked)? == executable
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
    let installed = bundle(&app, APP_ID)?;
    signature(&app, None)?;
    ensure!(
        team(&app)? == expected_team,
        "Installed TourBox publisher differs from the verified PKG"
    );
    let app_version = field(&installed, "CFBundleShortVersionString")?;
    let app_build = field(&installed, "CFBundleVersion")?;
    let matches_target = app_version.strip_prefix(&release.version).is_some_and(|suffix| {
        suffix
            .as_bytes()
            .first()
            .is_none_or(|byte| !byte.is_ascii_digit() && *byte != b'.')
    });
    ensure!(matches_target, "Installed TourBox does not report the expected version");
    ensure!(
        app_version != version || app_build != build || executable_digest(&app, &installed)? != executable,
        "TourBox installer exited successfully but the application did not change"
    );
    ensure!(
        state(&path)? == original,
        "TourBox receipt changed while installation ran"
    );
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
    };
    serde_json::to_writer_pretty(&mut pending, &receipt)?;
    pending.as_file().sync_all()?;
    pending.persist(&path)?;
    File::open(parent)?.sync_all()?;
    println!("TourBox: INSTALLED — {} Beta {}", receipt.version, receipt.beta);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
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
}
