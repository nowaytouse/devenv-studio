use super::super::dotnetup::signature::verify_publisher;
use super::{FRONTIER, LOG, desktop, env_path, field, python::optional, read_json, sha};
use color_eyre::eyre::{Context, Result, ensure, eyre};
use regex::Regex;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::Duration,
};
use windows_registry::{CURRENT_USER, LOCAL_MACHINE};

use crate::{command::CommandExt, execution_context::RunType};
mod win32;
const KDE: &str = "K Desktop Environment e.V.";
const MICROSOFT: &str = "Microsoft Corporation";
const INDEX: &str = "https://cdn.kde.org/ci-builds/graphics/krita/master/windows/";
const RELEASES: &str = "https://github.com/microsoft/PowerToys/releases";

fn page(url: &str) -> Result<String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("Topgrade Windows preview updater")
        .https_only(true)
        .timeout(Duration::from_secs(40))
        .build()?;
    Ok(client.get(url).send()?.error_for_status()?.text()?)
}
fn download(mode: RunType, url: &str, dest: &Path) -> Result<()> {
    // The system curl supports HTTPS-only redirects and bounded retries for immutable release assets.
    mode.execute(env_path("SystemRoot")?.join("System32/curl.exe"))
        .args([
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--location",
            "--fail",
            "--silent",
            "--show-error",
            "--retry",
            "2",
            "--connect-timeout",
            "20",
            "--max-time",
            "900",
            "--output",
        ])
        .arg(dest)
        .arg(url)
        .status_checked()
        .wrap_err("Release download failed")?;
    Ok(())
}
fn build_matches(version: &str, build: &str) -> bool {
    Regex::new(r"\bgit ([a-f0-9]{7,40})\b")
        .unwrap()
        .captures(version)
        .is_some_and(|c| build.starts_with(&c[1]))
}
fn krita_closed() -> Result<()> {
    let mut system = sysinfo::System::new()?;
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        sysinfo::ProcessRefreshKind::nothing(),
    );
    ensure!(
        !system
            .processes()
            .values()
            .any(|p| p.name().eq_ignore_ascii_case("krita.exe")),
        "Save and close Krita before updating"
    );
    Ok(())
}
fn plain(path: &Path) -> Result<()> {
    ensure!(
        fs::symlink_metadata(path)?.file_attributes() & 0x400 == 0,
        "Redirected installer staging path"
    );
    Ok(())
}
fn staging() -> Result<PathBuf> {
    let base = if Path::new(r"D:\").is_dir() {
        PathBuf::from(r"D:\Maintenance-Staging")
    } else {
        env_path("LOCALAPPDATA")?.join("Topgrade").join("Maintenance-Staging")
    };
    for path in [&base, &base.join("KritaNext-Installers")] {
        fs::create_dir_all(path)?;
        plain(path)?;
    }
    Ok(base.join("KritaNext-Installers"))
}
pub(super) fn krita(mode: RunType, check: bool) -> Result<Value> {
    let root = env_path("ProgramFiles")?.join("Krita (x64)");
    let installed = LOCAL_MACHINE
        .options()
        .read()
        .wow64_64()
        .open(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Krita_x64")?
        .get_string("InstallLocation")?;
    ensure!(
        fs::canonicalize(installed)? == fs::canonicalize(&root)?,
        "Unexpected machine-wide Krita installation"
    );
    let exe = root.join("bin/krita.exe");
    verify_publisher(&exe, KDE)?;
    let before = win32::version(&exe, "ProductVersion")?;
    let re = Regex::new(r#"href="(krita-x64-[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.]+)?-([a-f0-9]{7,40})-setup\.exe)""#)?;
    let html = page(INDEX)?;
    let files: BTreeMap<_, _> = re
        .captures_iter(&html)
        .map(|c| (c[1].to_owned(), c[2].to_owned()))
        .collect();
    ensure!(
        files.len() == 1,
        "KDE CI did not provide one unambiguous Windows x64 installer"
    );
    let (file, build) = files.into_iter().next().unwrap();
    let url = format!("{INDEX}{file}");
    let mut report =
        json!({"before":before,"selected_build":build,"official_url":url,"status":"Current","changed":false});
    if build_matches(&before, &build) {
        return Ok(report);
    }
    if check {
        report["status"] = json!("UpdateAvailable");
        return Ok(report);
    }
    krita_closed()?;
    let stage = staging()?;
    let setup = stage.join(file);
    if !setup.try_exists()? {
        let tmp = tempfile::Builder::new()
            .suffix(".exe")
            .tempfile_in(&stage)?
            .into_temp_path();
        download(mode, &url, &tmp)?;
        verify_publisher(&tmp, KDE)?;
        tmp.persist_noclobber(&setup).map_err(|e| e.error)?;
    }
    plain(&setup)?;
    // Pin the verified installer until the elevated child exits.
    let _installer = fs::OpenOptions::new().read(true).share_mode(1).open(&setup)?;
    verify_publisher(&setup, KDE)?;
    report["installer_sha256"] = json!(sha(&setup)?);
    let logs = desktop()?.join(LOG);
    fs::create_dir_all(&logs)?;
    let backup = tempfile::Builder::new()
        .prefix("KritaNext-before-")
        .tempdir_in(logs)?
        .keep();
    let mut prefs = Vec::new();
    for path in [
        env_path("LOCALAPPDATA")?.join("kritarc"),
        env_path("LOCALAPPDATA")?.join("kritadisplayrc"),
        env_path("APPDATA")?.join("krita/KRITA_RESOURCE_VERSION"),
    ] {
        if path.try_exists()? {
            let hash = sha(&path)?;
            let copy = backup.join(path.file_name().unwrap());
            fs::copy(&path, &copy)?;
            ensure!(sha(&copy)? == hash, "Preference backup changed");
            prefs.push((path, hash));
        }
    }
    krita_closed()?;
    win32::elevated_krita(&setup, &root)?;
    verify_publisher(&exe, KDE)?;
    let after = win32::version(&exe, "ProductVersion")?;
    ensure!(
        build_matches(&after, &build),
        "Installed Krita differs from selected KDE build"
    );
    for (path, hash) in prefs {
        ensure!(
            sha(&path)? == hash,
            "Krita preferences changed; backup at {}",
            backup.display()
        );
    }
    report["after"] = json!(after);
    report["installer_retained"] = json!(setup);
    report["changed"] = json!(true);
    report["status"] = json!("Updated");
    Ok(report)
}

fn numbers(s: &str) -> Result<[u32; 4]> {
    let parts = s.split('.').map(str::parse).collect::<Result<Vec<u32>, _>>()?;
    ensure!((3..=4).contains(&parts.len()), "Invalid release version {s}");
    let mut result = [0; 4];
    result[..parts.len()].copy_from_slice(&parts);
    Ok(result)
}
struct Release {
    version: [u32; 4],
    name: String,
    url: String,
    hash: String,
}
fn release(tag: &str, name: &str, checksum_text: &str) -> Result<Release> {
    let ver = Regex::new(r"^PowerToysUserSetup-(\d+\.\d+\.\d+\.\d+)-x64\.exe$")?;
    let caps = ver
        .captures(name)
        .ok_or_else(|| eyre!("Unexpected per-user x64 filename"))?;
    numbers(tag.strip_prefix('v').ok_or_else(|| eyre!("Unexpected release tag"))?)?;
    let hashes = Regex::new(r"(?i)\b[0-9a-f]{64}\b")?
        .find_iter(checksum_text)
        .map(|h| h.as_str().to_ascii_lowercase())
        .collect::<std::collections::BTreeSet<_>>();
    ensure!(hashes.len() == 1, "Missing or ambiguous PowerToys installer checksum");
    Ok(Release {
        version: numbers(&caps[1])?,
        name: name.into(),
        url: format!("{RELEASES}/download/{tag}/{name}"),
        hash: hashes.into_iter().next().unwrap(),
    })
}
fn api_release() -> Result<Release> {
    let data: Vec<Value> = serde_json::from_str(&page(
        "https://api.github.com/repos/microsoft/PowerToys/releases?per_page=20",
    )?)?;
    let filename = Regex::new(r"^PowerToysUserSetup-(\d+\.\d+\.\d+\.\d+)-x64\.exe$")?;
    let mut candidates = Vec::new();
    for r in &data {
        if r["draft"] != false {
            continue;
        }
        for asset in r["assets"].as_array().ok_or_else(|| eyre!("Invalid release assets"))? {
            let name = field(asset, "name")?;
            if let Some(c) = filename.captures(name) {
                candidates.push((numbers(&c[1])?, r, asset, name));
            }
        }
    }
    let (_, r, asset, name) = candidates
        .into_iter()
        .max_by_key(|c| c.0)
        .ok_or_else(|| eyre!("Missing official Windows release"))?;
    let rows = field(r, "body")?
        .lines()
        .filter(|line| line.contains(name))
        .collect::<Vec<_>>()
        .join("\n");
    let selected = release(field(r, "tag_name")?, name, &rows)?;
    ensure!(
        field(asset, "browser_download_url")? == selected.url,
        "Unexpected PowerToys asset URL"
    );
    Ok(selected)
}
fn html_release() -> Result<Release> {
    let html = page(RELEASES)?;
    let tags = Regex::new(r#"/microsoft/PowerToys/releases/tag/(v\d+\.\d+\.\d+(?:\.\d+)?)["'/?#]"#)?;
    let mut versions = BTreeMap::new();
    for c in tags.captures_iter(&html) {
        versions.insert(numbers(&c[1][1..])?, c[1].to_owned());
    }
    let (_, tag) = versions
        .last_key_value()
        .ok_or_else(|| eyre!("No numeric PowerToys release tag"))?;
    let html = page(&format!("{RELEASES}/tag/{tag}"))?;
    let names = Regex::new(r"PowerToysUserSetup-\d+\.\d+\.\d+\.\d+-x64\.exe")?;
    let rows = Regex::new(r"(?is)<tr\b[^>]*>.*?</tr>")?
        .find_iter(&html)
        .filter(|r| names.is_match(r.as_str()))
        .collect::<Vec<_>>();
    ensure!(rows.len() == 1, "Ambiguous PowerToys checksum table");
    let row = rows[0].as_str();
    release(tag, names.find(row).unwrap().as_str(), row)
}
pub(super) fn powertoys(mode: RunType, check: bool) -> Result<Value> {
    let local = env_path("LOCALAPPDATA")?;
    let exe = local.join("PowerToys/PowerToys.exe");
    let settings = local.join("Microsoft/PowerToys/settings.json");
    verify_publisher(&exe, MICROSOFT)?;
    let before = read_json(&settings)?;
    ensure!(
        before["include_prerelease_updates"] == true,
        "PowerToys preview updates are disabled"
    );
    for hive in [CURRENT_USER, LOCAL_MACHINE] {
        if let Some(key) = optional(hive.open(r"SOFTWARE\Policies\PowerToys"))? {
            ensure!(
                optional(key.get_u32("DisablePreviewUpdates"))? != Some(1),
                "PowerToys preview updates are disabled by policy"
            );
        }
    }
    let current = win32::version(&exe, "FileVersion")?;
    let (latest, metadata) = match api_release() {
        Ok(r) => (r, "GitHub API"),
        Err(err) => (
            html_release().wrap_err_with(|| format!("API failed ({err:#}); official HTML fallback failed"))?,
            "GitHub HTML",
        ),
    };
    let pending = latest.version > numbers(&current)?;
    let mut report = json!({"status":if pending{"UpdateAvailable"}else{"Current"},"before":current,"release":latest.name,"metadata":metadata,"source":latest.url,"changed":false});
    if !pending || check {
        return Ok(report);
    }
    let logs = desktop()?.join(FRONTIER).join("PowerToys");
    fs::create_dir_all(&logs)?;
    fs::copy(&settings, logs.join("最近更新前-settings.json"))?;
    let tmp = tempfile::tempdir()?;
    let setup = tmp.path().join(&latest.name);
    download(mode, &latest.url, &setup)?;
    let _installer = fs::OpenOptions::new().read(true).share_mode(1).open(&setup)?;
    ensure!(sha(&setup)? == latest.hash, "PowerToys download checksum mismatch");
    verify_publisher(&setup, MICROSOFT)?;
    let rc = mode
        .execute(setup)
        .args(["/install", "/quiet", "/norestart", "/log"])
        .arg(logs.join("最近PowerToys正常更新.log"))
        .output_checked_with(|o| {
            if matches!(o.status.code(), Some(0 | 3010)) {
                Ok(())
            } else {
                Err(())
            }
        })?
        .status;
    verify_publisher(&exe, MICROSOFT)?;
    let after = read_json(&settings)?;
    for key in [
        "startup",
        "run_elevated",
        "include_prerelease_updates",
        "download_updates_automatically",
        "enable_experimentation",
    ] {
        ensure!(before.get(key) == after.get(key), "PowerToys preference changed: {key}");
    }
    for (key, value) in before["enabled"]
        .as_object()
        .ok_or_else(|| eyre!("Missing module settings"))?
    {
        ensure!(
            after["enabled"].get(key) == Some(value),
            "PowerToys module preference changed: {key}"
        );
    }
    let installed = win32::version(&exe, "FileVersion")?;
    ensure!(
        numbers(&installed)? == latest.version,
        "PowerToys installed version differs from the release"
    );
    report["status"] = json!("Updated");
    report["after"] = json!(installed);
    report["changed"] = json!(true);
    report["reboot_required"] = json!(rc.code() == Some(3010));
    Ok(report)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_ambiguous_checksum_or_non_release_tags() {
        let name = "PowerToysUserSetup-1.2.3.4-x64.exe";
        assert!(release("v1.2.3", name, &"a".repeat(64)).is_ok());
        assert!(release("v1.2.3", name, &format!("{} {}", "a".repeat(64), "b".repeat(64))).is_err());
        assert!(release("v1.2.3/other", name, &"a".repeat(64)).is_err());
    }
}
