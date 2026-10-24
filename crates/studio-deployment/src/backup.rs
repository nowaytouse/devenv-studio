//! Local developer package inventory, never credentials, projects or a restore script.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::Read,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

#[cfg(any(windows, target_os = "macos", test))]
mod capture;
#[cfg(any(windows, test))]
mod windows_capture;

pub const PROVIDERS: [&str; 6] = ["brew_formula", "brew_cask", "cargo", "uv", "npm", "vscode"];
pub const WINDOWS_PROVIDERS: [&str; 5] = ["cargo", "uv", "npm", "pip", "vscode"];

pub fn provider_kinds(platform: &str) -> Result<&'static [&'static str]> {
    match platform {
        "macos" => Ok(&PROVIDERS),
        "windows" => Ok(&WINDOWS_PROVIDERS),
        _ => anyhow::bail!("Unsupported developer backup platform"),
    }
}
const INVENTORY: &str = "Inventory only: tool binaries, credentials, configuration, source code and virtual environments are not included.";
const LOCATIONS: &str =
    "Only supported standard installations are inspected; unavailable does not prove uninstalled.";
const PYTHON_SCOPE: &str = "Pip captures distributions visible to one selected registered standard CPython interpreter; other interpreters, virtual environments and editable projects are not backed up.";
const PARTIAL: &str =
    "Some providers were not completely captured; their entries are not a complete environment.";
const COMPLETE: &str = "Collected installed package identities and versions.";
const UNAVAILABLE: &str = "Tool not found in supported locations.";
const BREW_UNAVAILABLE: &str =
    "Homebrew or its required local runtime is unavailable in supported locations.";
const FAILED: &str = "Command failed; package list was not captured.";
const TIMED_OUT: &str = "Command timed out; package list was not captured.";
const INVALID: &str = "Command output was invalid; package list was not captured.";
const CAPTURE_FAILED: &str = "Command could not be started or fully captured.";
const OMITTED: &str =
    "Some entries were omitted because their identities or versions are unsupported.";
pub(crate) const MAX_PACKAGES: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub schema_version: u32,
    /// UTC Unix seconds, independent of locale and the current time zone.
    pub captured_at: String,
    pub platform: String,
    pub providers: Vec<ProviderSnapshot>,
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderSnapshot {
    pub kind: String,
    /// complete, unavailable or failed. An empty unavailable/failed list proves nothing.
    pub status: String,
    pub packages: Vec<Package>,
    pub detail: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub name: String,
    /// Homebrew can have multiple installed versions, joined with commas.
    pub version: String,
}

#[cfg(target_os = "macos")]
pub(crate) fn homebrew_program() -> Option<std::path::PathBuf> {
    ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"]
        .into_iter()
        .map(std::path::PathBuf::from)
        .find(|path| path.is_file() && capture::brew_runtime_ready(path))
}
#[cfg(target_os = "macos")]
pub(crate) fn uv_program() -> Result<std::path::PathBuf> {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .context("macOS home directory is unavailable")?;
    capture::uv_program(&home).context("uv was not found in supported standard locations")
}
#[cfg(windows)]
pub(crate) fn uv_program() -> Result<std::path::PathBuf> {
    Ok(crate::windows::developer_tool("uv")?.program)
}

#[cfg(any(windows, test))]
#[derive(Debug)]
pub(crate) struct MissingProvider;
#[cfg(any(windows, test))]
impl std::fmt::Display for MissingProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Tool not found in supported locations")
    }
}
#[cfg(any(windows, test))]
impl std::error::Error for MissingProvider {}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn vscode_cli_paths(
    home: &std::path::Path,
) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    capture::vscode_cli_paths(home)
}

pub fn collect(cancel: Arc<AtomicBool>) -> Result<Snapshot> {
    #[cfg(target_os = "macos")]
    {
        capture::collect(cancel)
    }
    #[cfg(windows)]
    {
        windows_capture::collect(cancel)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = cancel;
        anyhow::bail!("Developer environment collection requires macOS or Windows");
    }
}
#[cfg(any(windows, target_os = "macos", test))]
pub(crate) fn parse_uv_output(text: &str) -> Result<Vec<Package>> {
    let (packages, omitted) = capture::parse_packages("uv", text, None)?;
    ensure!(!omitted, "Incomplete uv tool inventory");
    Ok(packages)
}

#[cfg(any(windows, target_os = "macos", test))]
pub(crate) fn parse_cargo_output(text: &str) -> Result<Vec<Package>> {
    let (packages, omitted) = capture::parse_packages("cargo", text, None)?;
    ensure!(!omitted, "Incomplete Cargo tool inventory");
    Ok(packages)
}

pub(crate) fn valid_name(kind: &str, value: &str) -> bool {
    if matches!(kind, "brew_formula" | "brew_cask") {
        !value.is_empty()
            && value.len() <= 128
            && value.as_bytes()[0].is_ascii_alphanumeric()
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.@+".contains(&b))
    } else {
        crate::developer_ids::valid(kind, value)
    }
}
fn identity_key(kind: &str, name: &str) -> String {
    if matches!(kind, "uv" | "pip") {
        crate::normalize_pip_id(name).unwrap_or_else(|| name.to_ascii_lowercase())
    } else {
        name.to_ascii_lowercase()
    }
}
pub(crate) fn valid_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".,_+~-".contains(&b))
        && (value.bytes().any(|b| b.is_ascii_digit())
            || value.strip_prefix("HEAD-").is_some_and(|commit| {
                !commit.is_empty() && commit.bytes().all(|b| b.is_ascii_hexdigit())
            }))
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn npm_cli_paths() -> Option<(std::path::PathBuf, std::path::PathBuf, std::path::PathBuf)>
{
    capture::npm_cli_paths()
}

#[cfg(any(windows, target_os = "macos", test))]
pub(crate) fn parse_npm_inventory(
    text: &str,
    root: &std::path::Path,
) -> anyhow::Result<Vec<Package>> {
    let (packages, omitted) = capture::parse_packages("npm", text, Some(root))?;
    ensure!(!omitted, "Incomplete npm package listing");
    Ok(packages)
}
pub fn validate(snapshot: &Snapshot) -> Result<()> {
    ensure!(
        snapshot.schema_version == 1,
        "Unsupported developer backup schema"
    );
    let expected_providers = provider_kinds(&snapshot.platform)?;
    ensure!(
        !snapshot.captured_at.is_empty()
            && snapshot.captured_at.len() <= 20
            && snapshot.captured_at.bytes().all(|b| b.is_ascii_digit())
            && snapshot.captured_at.parse::<u64>().is_ok(),
        "Invalid UTC Unix timestamp"
    );
    ensure!(
        snapshot.providers.len() == expected_providers.len(),
        "Backup must describe every provider for its platform"
    );
    let mut providers = BTreeSet::new();
    let mut count = 0usize;
    for provider in &snapshot.providers {
        ensure!(
            expected_providers.contains(&provider.kind.as_str())
                && providers.insert(&provider.kind),
            "Unknown or duplicate backup provider"
        );
        let details: &[&str] = match provider.status.as_str() {
            "complete" => &[COMPLETE],
            "unavailable" => &[UNAVAILABLE, BREW_UNAVAILABLE],
            "failed" => &[FAILED, TIMED_OUT, INVALID, CAPTURE_FAILED, OMITTED],
            _ => anyhow::bail!("Invalid backup provider status"),
        };
        ensure!(
            details.contains(&provider.detail.as_str()),
            "Invalid backup detail"
        );
        ensure!(
            provider.status == "complete"
                || provider.detail == OMITTED
                || provider.packages.is_empty(),
            "Incomplete provider cannot claim a complete package list"
        );
        count = count
            .checked_add(provider.packages.len())
            .context("Package count overflow")?;
        ensure!(count <= MAX_PACKAGES, "Backup exceeds package count limit");
        let mut packages = BTreeSet::new();
        for package in &provider.packages {
            ensure!(
                valid_name(&provider.kind, &package.name) && valid_version(&package.version),
                "Invalid backup package identity or version"
            );
            ensure!(
                packages.insert(identity_key(&provider.kind, &package.name)),
                "Duplicate backup package identity"
            );
        }
    }
    ensure!(snapshot.warnings.len() <= 4, "Too many backup warnings");
    let mut warnings = BTreeSet::new();
    for warning in &snapshot.warnings {
        ensure!(
            ([INVENTORY, LOCATIONS, PARTIAL].contains(&warning.as_str())
                || (snapshot.platform == "windows" && warning == PYTHON_SCOPE))
                && warnings.insert(warning),
            "Unknown or duplicate backup warning"
        );
    }
    ensure!(
        serde_json::to_vec(snapshot)?.len() as u64 <= studio_core::MAX_INPUT_BYTES,
        "Backup exceeds 1 MiB limit"
    );
    Ok(())
}
pub fn save(snapshot: &Snapshot, path: &Path) -> Result<()> {
    validate(snapshot)?;
    studio_core::export_document(snapshot, path)
}
pub fn load(path: &Path) -> Result<Snapshot> {
    let metadata = fs::symlink_metadata(path).context("Read developer backup metadata")?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Backup must be a regular file, not a symlink"
    );
    let file = File::open(path).context("Open developer backup")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened = file.metadata()?;
        ensure!(
            opened.dev() == metadata.dev() && opened.ino() == metadata.ino(),
            "Backup changed while opening"
        );
    }
    let mut bytes = Vec::new();
    file.take(studio_core::MAX_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    parse(&bytes)
}
fn parse(bytes: &[u8]) -> Result<Snapshot> {
    ensure!(
        bytes.len() as u64 <= studio_core::MAX_INPUT_BYTES,
        "Backup exceeds 1 MiB limit"
    );
    // Deriving strict structs rejects duplicate known JSON fields at every document level.
    let snapshot: Snapshot =
        serde_json::from_slice(bytes).context("Invalid developer backup JSON")?;
    validate(&snapshot)?;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn sample() -> Snapshot {
        Snapshot {
            schema_version: 1,
            captured_at: "1792720000".into(),
            platform: "macos".into(),
            providers: PROVIDERS
                .iter()
                .map(|kind| ProviderSnapshot {
                    kind: (*kind).into(),
                    status: "unavailable".into(),
                    packages: vec![],
                    detail: UNAVAILABLE.into(),
                })
                .collect(),
            warnings: vec![INVENTORY.into(), LOCATIONS.into(), PARTIAL.into()],
        }
    }
    #[test]
    fn strict_document_rejects_duplicates_unknown_schema_and_private_fields() {
        let sample = sample();
        let text = serde_json::to_string(&sample).unwrap();
        assert_eq!(parse(text.as_bytes()).unwrap(), sample);
        assert!(
            parse(
                text.replacen(
                    "\"schema_version\":1",
                    "\"schema_version\":1,\"schema_version\":1",
                    1
                )
                .as_bytes()
            )
            .is_err()
        );
        assert!(
            parse(
                text.replacen("\"schema_version\":1", "\"schema_version\":2", 1)
                    .as_bytes()
            )
            .is_err()
        );
        assert!(
            parse(
                text.replacen(
                    "\"platform\":\"macos\"",
                    "\"platform\":\"macos\",\"home\":\"/private\"",
                    1
                )
                .as_bytes()
            )
            .is_err()
        );
        let mut invalid = sample.clone();
        invalid.providers[0].status = "complete".into();
        invalid.providers[0].detail = COMPLETE.into();
        invalid.providers[0].packages = vec![
            Package {
                name: "rust".into(),
                version: "1.91.0".into(),
            },
            Package {
                name: "rust".into(),
                version: "1.90.0".into(),
            },
        ];
        assert!(validate(&invalid).is_err());
        invalid.providers[0].packages.pop();
        invalid.providers[0].detail = "/private/user/token".into();
        assert!(validate(&invalid).is_err());
        assert!(!valid_name("brew_formula", "private/tap/tool"));
        assert!(!valid_version("1.0 (/private/home)"));
        assert!(valid_version("HEAD-deadbeef"));
        assert!(!valid_version("HEAD-"));
    }
    #[test]
    fn platform_provider_contracts_preserve_macos_and_reject_cross_platform_lists() {
        let mut windows = sample();
        windows.platform = "windows".into();
        windows.providers = WINDOWS_PROVIDERS
            .iter()
            .map(|kind| ProviderSnapshot {
                kind: (*kind).into(),
                status: "unavailable".into(),
                packages: vec![],
                detail: UNAVAILABLE.into(),
            })
            .collect();
        assert_eq!(provider_kinds("windows").unwrap(), WINDOWS_PROVIDERS);
        assert_eq!(provider_kinds("macos").unwrap(), PROVIDERS);
        assert!(provider_kinds("linux").is_err());
        assert_eq!(
            parse(&serde_json::to_vec(&windows).unwrap()).unwrap(),
            windows
        );
        windows.providers[0].kind = "brew_formula".into();
        assert!(validate(&windows).is_err());
        let mut mac = sample();
        mac.providers[0].kind = "pip".into();
        assert!(validate(&mac).is_err());
        assert!(validate(&sample()).is_ok());
    }
    #[test]
    fn atomic_save_load_never_overwrites_or_follows_destination_links() {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ltsc-backup-test-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("backup.json");
        let sample = sample();
        save(&sample, &path).unwrap();
        assert_eq!(load(&path).unwrap(), sample);
        assert!(save(&sample, &path).is_err());
        let before = fs::read(&path).unwrap();
        #[cfg(unix)]
        {
            let link = dir.join("linked.json");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert!(load(&link).is_err());
            assert!(save(&sample, &link).is_err());
            assert_eq!(fs::read(&path).unwrap(), before);
        }
        #[cfg(not(unix))]
        assert!(!before.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }
}
