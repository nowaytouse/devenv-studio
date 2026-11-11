mod source;
mod transaction;

use color_eyre::eyre::{Context, ContextCompat, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    fs::{self, File, OpenOptions},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

use super::{APPLICATIONS, bounded_output, bundle, field, private_path, running, signature, tool_command};
use crate::command::CommandExt as _;
use source::{Relation, Release};
use transaction::{Journal, read_private, recover, state_digest, swap, write_private};

const ID: &str = "org.krita";
const MAX_STATE: u64 = 256 * 1024;
const MIN_QT: [u32; 3] = [6, 11, 3];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Fingerprint {
    inode: u64,
    version: String,
    executable_sha256: String,
    info_plist_sha256: String,
    team: Option<String>,
}

#[derive(Deserialize)]
struct Prior {
    sha: String,
    inode: u64,
    version: String,
    executable_sha256: String,
    info_plist_sha256: Option<String>,
    team: Option<String>,
    installed_utc: Option<String>,
    #[serde(default)]
    allow_unsigned: bool,
}

impl Prior {
    fn identifies(&self, installed: &Fingerprint) -> bool {
        source::valid_sha(&self.sha)
            && self.inode == installed.inode
            && self.version == installed.version
            && self.executable_sha256 == installed.executable_sha256
            && self.team == installed.team
            && self
                .info_plist_sha256
                .as_ref()
                .is_none_or(|sha| sha == &installed.info_plist_sha256)
    }
}

fn rebuilt_since_install(prior: &Prior, release: &Release) -> Result<bool> {
    let (Some(installed), Some(published)) = (&prior.installed_utc, &release.modified_utc) else {
        return Ok(false);
    };
    let installed =
        chrono::DateTime::parse_from_rfc3339(installed).wrap_err("Invalid Krita receipt installation timestamp")?;
    let published =
        chrono::DateTime::parse_from_rfc3339(published).wrap_err("Invalid official Krita publication timestamp")?;
    Ok(published > installed)
}

fn app_info(app: &Path) -> Result<Value> {
    match bundle(app, ID) {
        Ok(info) => Ok(info),
        Err(original) => {
            // Keep the legacy native parser fallback for vendor plists rejected by plutil.
            let info = app.join("Contents/Info.plist");
            bounded_file(&info, super::PLIST_LIMIT)?;
            let mut recovered = Map::new();
            for key in [
                "CFBundleIdentifier",
                "CFBundleExecutable",
                "CFBundleShortVersionString",
                "CFBundleVersion",
            ] {
                let (status, stdout, _) = bounded_output(
                    tool_command(Path::new("/usr/libexec/PlistBuddy"))?
                        .args(["-c", &format!("Print :{key}")])
                        .arg(&info),
                    Duration::from_secs(15),
                    super::PLIST_LIMIT,
                )?;
                ensure!(status.success(), "Could not read Krita property list: {original:#}");
                recovered.insert(key.into(), Value::String(stdout.trim().into()));
            }
            ensure!(
                recovered.get("CFBundleIdentifier").and_then(Value::as_str) == Some(ID),
                "Unexpected Krita bundle identifier"
            );
            Ok(Value::Object(recovered))
        }
    }
}

fn bounded_file(path: &Path, limit: u64) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
        .open(path)?;
    ensure!(
        file.metadata()?.is_file() && file.metadata()?.len() <= limit && path.canonicalize()? == path,
        "Unsafe Krita file: {}",
        path.display()
    );
    Ok(file)
}

fn parse_qt_version(version: &str) -> Result<[u32; 3]> {
    let version = version.trim();
    ensure!(
        version.bytes().all(|byte| byte.is_ascii_digit() || byte == b'.'),
        "Invalid Krita Qt CFBundleVersion"
    );
    let parts = version
        .split('.')
        .map(str::parse)
        .collect::<std::result::Result<Vec<u32>, _>>()
        .wrap_err("Invalid Krita Qt CFBundleVersion")?;
    ensure!(
        parts.len() == 3,
        "Krita Qt CFBundleVersion must include major, minor and patch"
    );
    Ok([parts[0], parts[1], parts[2]])
}

fn qt_version(app: &Path) -> Result<[u32; 3]> {
    let mut versions = Vec::new();
    for framework in ["QtCore", "QtGui"] {
        let info = app
            .join(format!(
                "Contents/Frameworks/{framework}.framework/Resources/Info.plist"
            ))
            .canonicalize()
            .wrap_err_with(|| format!("Krita {framework} metadata is missing"))?;
        ensure!(
            info.starts_with(app),
            "Krita {framework} metadata escapes the application"
        );
        bounded_file(&info, super::PLIST_LIMIT)?;
        let (status, stdout, _) = bounded_output(
            tool_command(Path::new("/usr/bin/plutil"))?
                .args(["-extract", "CFBundleVersion", "raw", "-o", "-"])
                .arg(&info),
            Duration::from_secs(15),
            64 * 1024,
        )?;
        ensure!(status.success(), "Krita {framework} has no complete Qt CFBundleVersion");
        versions.push(parse_qt_version(&stdout)?);
    }
    ensure!(versions[0] == versions[1], "Krita QtCore and QtGui versions differ");
    Ok(versions[0])
}

fn compatible_qt(version: [u32; 3]) -> Result<()> {
    ensure!(
        version >= MIN_QT,
        "Krita Next {}: BLOCKED_INCOMPATIBLE_QT — Qt {}.{}.{} is below 6.11.3; waiting for a compatible official Next build",
        source::CHANNEL,
        version[0],
        version[1],
        version[2]
    );
    Ok(())
}

fn current(app: &Path, installed: &Fingerprint, sha: &str) -> Result<()> {
    ensure!(
        source::valid_next_version(&installed.version),
        "Installed Krita version does not match the {} format",
        source::CHANNEL
    );
    let qt = qt_version(app)?;
    compatible_qt(qt)?;
    println!(
        "Krita Next {}: CURRENT — Git {sha}, Qt {}.{}.{}",
        source::CHANNEL,
        qt[0],
        qt[1],
        qt[2]
    );
    Ok(())
}

fn identity(app: &Path) -> Result<Option<String>> {
    let (status, stdout, stderr) = bounded_output(
        tool_command(Path::new("/usr/bin/codesign"))?
            .env("LC_ALL", "C")
            .args(["-dv", "--verbose=4"])
            .arg(app),
        Duration::from_secs(30),
        64 * 1024,
    )?;
    if !status.success() {
        ensure!(
            stderr
                .lines()
                .any(|line| line.ends_with(": code object is not signed at all")),
            "Krita code signing identity could not be inspected"
        );
        return Ok(None);
    }
    let identities: Vec<_> = stdout
        .lines()
        .chain(stderr.lines())
        .filter_map(|line| line.strip_prefix("TeamIdentifier="))
        .collect();
    match identities.as_slice() {
        ["not set"] => Ok(None),
        [team]
            if team.len() == 10
                && team
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit()) =>
        {
            Ok(Some((*team).into()))
        }
        _ => bail!("Krita has an invalid or ambiguous signing identity"),
    }
}

fn fingerprint(app: &Path) -> Result<Fingerprint> {
    ensure!(
        fs::symlink_metadata(app)?.is_dir() && app.canonicalize()? == app,
        "Unsafe Krita application path"
    );
    let info = app_info(app)?;
    let executable = field(&info, "CFBundleExecutable")?;
    ensure!(
        !executable.contains('/') && !matches!(executable.as_str(), "." | ".."),
        "Unsafe Krita executable name"
    );
    let version = field(&info, "CFBundleShortVersionString")?;
    let executable = bounded_file(&app.join("Contents/MacOS").join(executable), 256 * 1024 * 1024)?;
    let plist = bounded_file(&app.join("Contents/Info.plist"), super::PLIST_LIMIT)?;
    Ok(Fingerprint {
        inode: fs::symlink_metadata(app)?.ino(),
        version,
        executable_sha256: super::super::sha256(executable)?,
        info_plist_sha256: super::super::sha256(plist)?,
        team: identity(app)?,
    })
}

fn verify_publisher(
    app: &Path,
    before: &Fingerprint,
    candidate: &Fingerprint,
    allow_unsigned: bool,
) -> Result<&'static str> {
    match (&before.team, &candidate.team) {
        (Some(expected), Some(actual)) if expected == actual => {
            signature(app, None)?;
            tool_command(Path::new("/usr/sbin/spctl"))?
                .args(["--assess", "--type", "execute"])
                .arg(app)
                .status_checked()?;
            Ok("developer-id")
        }
        (None, None) if allow_unsigned => Ok("kde-unsigned-explicit-opt-in"),
        _ => bail!("Krita publisher identity changed or the existing receipt does not authorize an unsigned build"),
    }
}

fn mounted_app(mount: &Path) -> Result<PathBuf> {
    let mut candidates = Vec::new();
    let mut folders = vec![(mount.to_path_buf(), true)];
    while let Some((folder, top)) = folders.pop() {
        for entry in fs::read_dir(folder)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("app")) {
                let path = path.canonicalize()?;
                if path.starts_with(mount) && app_info(&path).is_ok() {
                    candidates.push(path);
                }
            } else if top {
                folders.push((path, false));
            }
        }
    }
    ensure!(
        candidates.len() == 1,
        "Official DMG must contain exactly one valid Krita bundle"
    );
    Ok(candidates.pop().expect("verified candidate"))
}

fn with_mounted<T>(image: &Path, action: impl FnOnce(&Path) -> Result<T>) -> Result<T> {
    let workspace = tempfile::Builder::new().prefix("preview-krita-mount-").tempdir()?;
    let root = workspace.path().canonicalize()?;
    let mount = root.join("mounted");
    fs::create_dir(&mount)?;
    // Do not let TempDir cleanup cross the mounted filesystem if detach fails.
    let _retained_mount = workspace.keep();
    let operation = (|| {
        tool_command(Path::new("/usr/bin/hdiutil"))?
            .args(["attach", "-readonly", "-nobrowse", "-mountpoint"])
            .arg(&mount)
            .arg(image)
            .status_checked()?;
        action(&mount)
    })();
    let detach = tool_command(Path::new("/usr/bin/hdiutil"))?
        .arg("detach")
        .arg(&mount)
        .status_checked();
    match (operation, detach) {
        (Ok(value), Ok(())) => {
            fs::remove_dir(&mount)?;
            fs::remove_dir(&root)?;
            Ok(value)
        }
        (Err(error), Ok(())) => {
            fs::remove_dir(&mount)?;
            fs::remove_dir(&root)?;
            Err(error)
        }
        (Ok(_), Err(error)) => Err(error.wrap_err(format!("Krita DMG mount retained at {}", mount.display()))),
        (Err(error), Err(detach)) => bail!(
            "Krita could not process the image ({error:#}); inspect retained mount {} ({detach:#})",
            mount.display()
        ),
    }
}

struct Image {
    path: PathBuf,
    _temporary: Option<tempfile::TempDir>,
}

fn prepare_image(home: &Path, release: &Release) -> Result<Image> {
    let cache = private_path(home, "Library/Caches/PreviewAppsUpdater")?;
    let staging = tempfile::Builder::new().prefix(".official-krita-").tempdir_in(&cache)?;
    let image = staging.path().join(&release.name);
    source::image(release, &image)?;
    bounded_file(&image, source::MAX_IMAGE)?.sync_all()?;
    tool_command(Path::new("/usr/bin/hdiutil"))?
        .arg("verify")
        .arg(&image)
        .status_checked()?;
    let published = cache.join(&release.name);
    if super::super::exists(&published)? {
        // Keep the prior cache and use this separately verified official copy.
        return Ok(Image {
            path: image,
            _temporary: Some(staging),
        });
    }
    super::move_exclusive(&image, &published)?;
    File::open(&cache)?.sync_all()?;
    Ok(Image {
        path: published,
        _temporary: None,
    })
}

fn base_version(version: &str) -> Result<Vec<u32>> {
    let base = version.split_once('-').map_or(version, |(base, _)| base);
    let base = base
        .split(|character: char| !character.is_ascii_digit() && character != '.')
        .next()
        .unwrap_or_default();
    let parts: Vec<u32> = base.split('.').map(str::parse).collect::<std::result::Result<_, _>>()?;
    ensure!((2..=4).contains(&parts.len()), "Invalid Krita base version");
    Ok(parts)
}

struct Activation<'a> {
    app: &'a Path,
    state_path: &'a Path,
    journal_path: &'a Path,
    original: &'a [u8],
    state: Value,
    before: &'a Fingerprint,
    prior: &'a Prior,
}

fn publish(activation: Activation<'_>, release: &Release, image: &Path) -> Result<()> {
    let Activation {
        app,
        state_path,
        journal_path,
        original,
        mut state,
        before,
        prior,
    } = activation;
    let applications = app.parent().context("Krita has no application parent")?;
    let staging = tempfile::Builder::new()
        .prefix(".krita.preview-")
        .tempdir_in(applications)?;
    let staged = staging.path().join("Krita.app");
    let (candidate, trust_mode, candidate_qt) = with_mounted(image, |mount| {
        let incoming = mounted_app(mount)?;
        let incoming_fp = fingerprint(&incoming)?;
        ensure!(
            base_version(&incoming_fp.version)? == base_version(&release.version)?,
            "Official Krita image version differs from the listing"
        );
        let trust = verify_publisher(&incoming, before, &incoming_fp, prior.allow_unsigned)?;
        let incoming_qt = qt_version(&incoming)?;
        compatible_qt(incoming_qt)?;
        tool_command(Path::new("/usr/bin/ditto"))?
            .arg(&incoming)
            .arg(&staged)
            .status_checked()?;
        let staged_fp = fingerprint(&staged)?;
        ensure!(
            staged_fp.executable_sha256 == incoming_fp.executable_sha256
                && staged_fp.info_plist_sha256 == incoming_fp.info_plist_sha256,
            "Krita copy changed the verified build fingerprint"
        );
        ensure!(
            verify_publisher(&staged, before, &staged_fp, prior.allow_unsigned)? == trust,
            "Krita copy changed publisher trust"
        );
        ensure!(
            qt_version(&staged)? == incoming_qt,
            "Krita copy changed Qt framework versions"
        );
        Ok((staged_fp, trust, incoming_qt))
    })?;
    ensure!(
        !running(app, "/krita")?
            && fingerprint(app)? == *before
            && read_private(state_path, MAX_STATE)?.as_deref() == Some(original),
        "Krita or its receipt changed after review; staged app was not activated"
    );
    state["krita"] = serde_json::json!({
        "sha": release.sha, "version": candidate.version, "inode": candidate.inode,
        "executable_sha256": candidate.executable_sha256, "team": candidate.team,
        "trust_mode": trust_mode, "allow_unsigned": prior.allow_unsigned,
        "info_plist_sha256": candidate.info_plist_sha256,
        "source_url": release.url, "modified_utc": release.modified_utc,
        "installed_utc": chrono::Utc::now().to_rfc3339(),
        "channel": source::CHANNEL,
        "qt_version": format!("{}.{}.{}", candidate_qt[0], candidate_qt[1], candidate_qt[2]),
    });
    let mut next_state = serde_json::to_vec_pretty(&state)?;
    next_state.push(b'\n');
    ensure!(next_state.len() as u64 <= MAX_STATE, "Krita receipt is too large");
    let _retained_stage = staging.keep();
    let journal = Journal {
        before: before.clone(),
        next: candidate.clone(),
        staged: staged.clone(),
        before_state: state_digest(original)?,
        next_state: state_digest(&next_state)?,
    };
    write_private(journal_path, &serde_json::to_vec(&journal)?, false)?;
    let result = (|| -> Result<()> {
        swap(app, &staged)?;
        ensure!(
            fingerprint(app)? == candidate,
            "Krita activation fingerprint readback failed"
        );
        verify_publisher(app, before, &candidate, prior.allow_unsigned)?;
        ensure!(
            read_private(state_path, MAX_STATE)?.as_deref() == Some(original),
            "Krita receipt changed during activation"
        );
        write_private(state_path, &next_state, true)?;
        ensure!(
            read_private(state_path, MAX_STATE)?.as_deref() == Some(next_state.as_slice()),
            "Krita receipt readback failed"
        );
        Ok(())
    })();
    let recovery = recover(app, state_path, journal_path);
    match (result, recovery) {
        (Ok(()), Ok(())) => {
            println!(
                "Krita Next: UPDATED — Git {}; previous app retained at {}",
                release.sha,
                staged.display()
            );
            Ok(())
        }
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(recovery)) => {
            bail!("Krita activation failed ({error:#}); recovery needs review ({recovery:#})")
        }
    }
}

pub(in crate::studio) fn krita_next() -> Result<()> {
    super::super::user_only()?;
    println!("Krita Next channel: KDE {} (Qt >= 6.11.3)", source::CHANNEL);
    let home = super::super::account_home()?;
    let _lock = super::lock(&home, "krita-preview.lock")?;
    let root = private_path(&home, "Library/Application Support/PreviewAppsUpdater")?;
    let app = Path::new(APPLICATIONS).join("krita.app");
    let state_path = root.join("state.json");
    let journal = root.join("krita-native-pending.json");
    recover(&app, &state_path, &journal)?;
    let original = read_private(&state_path, MAX_STATE)?
        .context("Krita Next: UNVERIFIED — establish the official build receipt before automatic updates")?;
    let state: Value = serde_json::from_slice(&original)?;
    let prior: Prior = serde_json::from_value(state["krita"].clone())
        .wrap_err("Krita Next: UNVERIFIED — missing or invalid official build receipt")?;
    let installed = fingerprint(&app)?;
    ensure!(
        prior.identifies(&installed),
        "Krita Next: UNVERIFIED — installed application does not match its trusted receipt"
    );
    if installed.team.is_some() {
        signature(&app, None)?;
    }
    let scratch = tempfile::tempdir()?;
    let release = source::release(scratch.path())?;
    println!(
        "Krita Next: installed {} (Git {}), official {}, published {}",
        installed.version,
        prior.sha,
        release.name,
        release.modified_utc.as_deref().unwrap_or("unverified")
    );
    let relation = if prior.sha.eq_ignore_ascii_case(&release.sha) {
        Relation::Same
    } else {
        source::relation(&prior.sha, &release.sha, scratch.path())?
    };
    match relation {
        Relation::Same => {
            if !rebuilt_since_install(&prior, &release)? {
                return current(&app, &installed, &release.sha);
            }
            println!(
                "Krita Next {}: official rebuild published after installation; verifying bundled Qt",
                source::CHANNEL
            );
        }
        Relation::Newer => {}
        Relation::Older => {
            compatible_qt(qt_version(&app)?)?;
            println!("Krita Next: DEFERRED_NOT_NEWER — official Git {} is older", release.sha);
            return Ok(());
        }
        Relation::Diverged => bail!("Krita Next: DEFERRED — official and installed Git histories diverged"),
    }
    if base_version(&installed.version)? > base_version(&release.version)? {
        compatible_qt(qt_version(&app)?)?;
        println!("Krita Next: DEFERRED_NOT_NEWER — official app version is older");
        return Ok(());
    }
    ensure!(
        !running(&app, "/krita")?,
        "Krita Next: DEFERRED — close Krita before installing"
    );
    let image = prepare_image(&home, &release)?;
    let activation = Activation {
        app: &app,
        state_path: &state_path,
        journal_path: &journal,
        original: &original,
        state,
        before: &installed,
        prior: &prior,
    };
    publish(activation, &release, &image.path)
}

#[cfg(test)]
#[test]
fn receipt_requires_build_bytes_and_never_assumes_the_same_version_is_the_same_build() -> Result<()> {
    let prior: Prior = serde_json::from_value(serde_json::json!({
        "sha": "2687e5d", "inode": 5, "version": "6.1.0", "executable_sha256": "original",
        "info_plist_sha256": "info", "team": null
    }))?;
    let mut current = Fingerprint {
        inode: 5,
        version: "6.1.0".into(),
        executable_sha256: "original".into(),
        info_plist_sha256: "info".into(),
        team: None,
    };
    assert!(prior.identifies(&current));
    current.executable_sha256 = "replacement".into();
    assert!(!prior.identifies(&current));
    assert_eq!(base_version("6.1.0-prealpha")?, [6, 1, 0]);
    Ok(())
}

#[cfg(test)]
#[test]
fn qt_admission_requires_a_complete_compatible_patch_version() -> Result<()> {
    for version in ["6.11.3", "6.11.4", "6.12.0"] {
        compatible_qt(parse_qt_version(version)?)?;
    }
    for version in ["6.11.0", "6.11.2", "6.8.3"] {
        assert!(compatible_qt(parse_qt_version(version)?).is_err());
    }
    for version in ["", "6.11", "6.11.3.0", "6.11.x", "6.11.+3", "6.11.3-dev"] {
        assert!(parse_qt_version(version).is_err());
    }
    Ok(())
}

#[cfg(test)]
#[test]
fn same_git_rebuilds_require_newer_official_publication_evidence() -> Result<()> {
    let mut prior: Prior = serde_json::from_value(serde_json::json!({
        "sha": "2687e5d", "inode": 1, "version": "6.1.0", "executable_sha256": "original",
        "team": null, "installed_utc": "2026-10-28T12:00:00+00:00"
    }))?;
    let mut release = Release {
        name: "krita-6.1.0-prealpha-2687e5d.dmg".into(),
        version: "6.1.0-prealpha".into(),
        sha: "2687e5d".into(),
        url: format!("{}krita-6.1.0-prealpha-2687e5d.dmg", source::CDN),
        modified_utc: Some("2026-10-24T02:22:46+00:00".into()),
    };
    assert!(!rebuilt_since_install(&prior, &release)?);
    release.modified_utc = Some("2026-11-06T02:22:46+00:00".into());
    assert!(rebuilt_since_install(&prior, &release)?);
    release.modified_utc = Some("invalid".into());
    assert!(rebuilt_since_install(&prior, &release).is_err());
    release.modified_utc = None;
    assert!(!rebuilt_since_install(&prior, &release)?);
    release.modified_utc = Some("2026-11-06T02:22:46+00:00".into());
    prior.installed_utc = Some("invalid".into());
    assert!(rebuilt_since_install(&prior, &release).is_err());
    prior.installed_utc = None;
    assert!(!rebuilt_since_install(&prior, &release)?);
    Ok(())
}

#[cfg(test)]
fn write_qt_fixture(app: &Path, framework: &str, version: &str) -> Result<PathBuf> {
    let root = app.join(format!("Contents/Frameworks/{framework}.framework"));
    let resources = root.join("Versions/A/Resources");
    fs::create_dir_all(&resources)?;
    let shortcut = root.join("Resources");
    if !shortcut.exists() {
        std::os::unix::fs::symlink("Versions/A/Resources", shortcut)?;
    }
    let plist = resources.join("Info.plist");
    fs::write(
        &plist,
        format!("<plist version=\"1.0\"><dict><key>CFBundleVersion</key><string>{version}</string></dict></plist>"),
    )?;
    Ok(plist)
}

#[cfg(test)]
#[test]
fn current_build_checks_qt_and_framework_metadata_cannot_escape_the_bundle() -> Result<()> {
    let workspace = tempfile::tempdir()?;
    let root = workspace.path().canonicalize()?;
    let app = root.join("Krita.app");
    write_qt_fixture(&app, "QtCore", "6.11.0")?;
    write_qt_fixture(&app, "QtGui", "6.11.0")?;
    let installed = Fingerprint {
        inode: 1,
        version: "6.1.0".into(),
        executable_sha256: "unchanged".into(),
        info_plist_sha256: "unchanged".into(),
        team: None,
    };
    let error = current(&app, &installed, "2687e5d").unwrap_err();
    assert!(format!("{error:#}").contains("BLOCKED_INCOMPATIBLE_QT"));
    write_qt_fixture(&app, "QtCore", "6.11.3")?;
    write_qt_fixture(&app, "QtGui", "6.11.3")?;
    current(&app, &installed, "2687e5d")?;
    write_qt_fixture(&app, "QtGui", "6.12.0")?;
    assert!(qt_version(&app).is_err());
    let core = write_qt_fixture(&app, "QtCore", "6.11.3")?;
    let outside = root.join("outside.plist");
    fs::rename(&core, &outside)?;
    std::os::unix::fs::symlink(outside, core)?;
    assert!(format!("{:#}", qt_version(&app).unwrap_err()).contains("escapes the application"));
    Ok(())
}
