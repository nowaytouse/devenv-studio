//! App-scoped development networking. Native exports never modify a tool's global settings.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::{Ipv4Addr, Ipv6Addr},
    path::{Path, PathBuf},
    sync::Mutex,
};

const MAX_BYTES: u64 = 16_384;
const NPM_REGISTRY: &str = "https://registry.npmjs.org/";
const PYTHON_INDEX: &str = "https://pypi.org/simple";
const CARGO_INDEX: &str = "sparse+https://index.crates.io/";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    #[default]
    Inherit,
    Direct,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkProfile {
    pub schema_version: u32,
    pub mode: ProxyMode,
    pub proxy_url: String,
    pub npm_registry: String,
    pub python_index: String,
    pub cargo_index: String,
}
impl Default for NetworkProfile {
    fn default() -> Self {
        Self {
            schema_version: 1,
            mode: ProxyMode::Inherit,
            proxy_url: String::new(),
            npm_registry: NPM_REGISTRY.into(),
            python_index: PYTHON_INDEX.into(),
            cargo_index: CARGO_INDEX.into(),
        }
    }
}

pub fn validate(profile: &NetworkProfile) -> Result<()> {
    ensure!(
        profile.schema_version == 1,
        "Unsupported network profile schema"
    );
    if profile.mode == ProxyMode::Manual || !profile.proxy_url.is_empty() {
        parse_proxy_endpoint(&profile.proxy_url)?;
    }
    source_url(&profile.npm_registry)?;
    source_url(&profile.python_index)?;
    let cargo = profile
        .cargo_index
        .strip_prefix("sparse+")
        .context("Cargo index must use sparse+https://")?;
    source_url(cargo)?;
    ensure!(cargo.ends_with('/'), "Cargo sparse index must end with /");
    Ok(())
}

fn safe_url(text: &str) -> Result<()> {
    ensure!(
        !text.is_empty()
            && text.len() <= 2048
            && text.is_ascii()
            && text
                .bytes()
                .all(|c| !c.is_ascii_control() && !c.is_ascii_whitespace())
            && !text.bytes().any(|c| b"@?#\\%\"'$\x60".contains(&c)),
        "Network URLs must be ASCII and contain no credentials, escapes, query, fragment or shell expansion"
    );
    Ok(())
}

fn authority(text: &str, require_port: bool) -> Result<(String, Option<u16>)> {
    ensure!(!text.is_empty(), "Network URL host is required");
    let (host, port) = if let Some(ipv6) = text.strip_prefix('[') {
        let (address, tail) = ipv6
            .split_once(']')
            .context("Invalid bracketed IPv6 host")?;
        address.parse::<Ipv6Addr>().context("Invalid IPv6 host")?;
        let port = if tail.is_empty() {
            None
        } else {
            Some(tail.strip_prefix(':').context("Invalid IPv6 host suffix")?)
        };
        (format!("[{address}]"), port)
    } else {
        let (host, port) = match text.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (text, None),
        };
        ensure!(
            host.len() <= 253 && !host.is_empty(),
            "Invalid DNS host length"
        );
        if host.bytes().all(|c| c.is_ascii_digit() || c == b'.') {
            host.parse::<Ipv4Addr>().context("Invalid IPv4 host")?;
        } else {
            ensure!(
                host.split('.').all(|label| {
                    !label.is_empty()
                        && label.len() <= 63
                        && label.as_bytes()[0].is_ascii_alphanumeric()
                        && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                        && label
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
                }),
                "Invalid ASCII DNS host"
            );
        }
        (host.to_string(), port)
    };
    let port = port
        .map(|port| -> Result<u16> {
            ensure!(
                !port.is_empty() && port.bytes().all(|c| c.is_ascii_digit()),
                "Invalid port"
            );
            let value: u16 = port.parse().context("Port must be between 1 and 65535")?;
            ensure!(value != 0, "Port must be between 1 and 65535");
            Ok(value)
        })
        .transpose()?;
    ensure!(
        !require_port || port.is_some(),
        "Proxy must include an explicit port"
    );
    Ok((host, port))
}

/// HTTP CONNECT proxy endpoint, with no authentication, PAC, path or implicit port.
pub fn parse_proxy_endpoint(text: &str) -> Result<(String, u16)> {
    safe_url(text)?;
    let endpoint = text
        .strip_prefix("http://")
        .context("Proxy must start with http://")?;
    ensure!(
        !endpoint.contains('/'),
        "Proxy must contain only a host and port"
    );
    let (host, port) = authority(endpoint, true)?;
    Ok((host, port.context("Proxy port is required")?))
}

fn source_url(text: &str) -> Result<()> {
    safe_url(text)?;
    let endpoint = text
        .strip_prefix("https://")
        .context("Package sources must use https://")?;
    let (host, path) = endpoint.split_once('/').unwrap_or((endpoint, ""));
    authority(host, false)?;
    ensure!(
        path.split('/').all(|segment| {
            segment != "."
                && segment != ".."
                && segment
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.~".contains(&c))
        }),
        "Package source path contains an unsupported character or traversal"
    );
    Ok(())
}

thread_local! {
    static ACTIVE: RefCell<Option<NetworkProfile>> = const { RefCell::new(None) };
}
struct Scope(Option<NetworkProfile>);
impl Drop for Scope {
    fn drop(&mut self) {
        ACTIVE.with(|active| *active.borrow_mut() = self.0.take());
    }
}
/// Keep one immutable policy for the entire worker, including nested invocations.
pub fn with_profile<T>(
    profile: &NetworkProfile,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    validate(profile)?;
    let _scope = Scope(ACTIVE.with(|active| active.replace(Some(profile.clone()))));
    operation()
}
pub(crate) fn active_profile() -> Option<NetworkProfile> {
    ACTIVE.with(|active| active.borrow().clone())
}
/// Publish the immutable policy used by the current worker as execution evidence.
pub fn record_current(path: &Path) -> Result<()> {
    match active_profile() {
        Some(profile) => save(&profile, path),
        None => studio_core::export_document(
            &serde_json::json!({
                "schema_version": 1, "app_network_profile_applied": false,
                "scope": "No app network profile was applied; native provider behavior and inherited environment remain in effect."
            }),
            path,
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Git,
    Cargo,
    Npm,
    Pip,
    Uv,
}

pub fn native_config(profile: &NetworkProfile, format: Format) -> Result<String> {
    validate(profile)?;
    let proxy = match profile.mode {
        ProxyMode::Inherit => None,
        ProxyMode::Direct => Some(""),
        ProxyMode::Manual => Some(profile.proxy_url.as_str()),
    };
    let mut text = match format {
        Format::Git => proxy.map_or_else(
            || "# Proxy is inherited from the existing Git configuration and environment.\n".into(),
            |value| format!("[http]\n\tproxy = {value}\n"),
        ),
        Format::Cargo => {
            let mut text = proxy.map_or_else(String::new, |value| {
                format!("[http]\nproxy = \"{value}\"\n")
            });
            if profile.cargo_index != CARGO_INDEX {
                text.push_str(&format!(
                    "[source.crates-io]\nreplace-with = \"ltsc-studio\"\n[source.ltsc-studio]\nregistry = \"{}\"\n",
                    profile.cargo_index
                ));
            }
            if text.is_empty() {
                text.push_str("# Official crates.io source; proxy inherited.\n");
            }
            text
        }
        Format::Npm => {
            let mut text = format!("registry={}\n", profile.npm_registry);
            if let Some(value) = proxy {
                let value = if value.is_empty() { "null" } else { value };
                text.push_str(&format!("proxy={value}\nhttps-proxy={value}\nnoproxy=\n"));
            }
            text
        }
        Format::Pip => {
            let mut text = format!("[global]\nindex-url = {}\n", profile.python_index);
            if let Some(value) = proxy {
                text.push_str(&format!("proxy = {value}\n"));
            }
            text
        }
        Format::Uv => format!(
            "# HTTP proxy is configured through the process environment, not uv.toml.\n[[index]]\nurl = \"{}\"\ndefault = true\n",
            profile.python_index
        ),
    };
    if profile.mode == ProxyMode::Direct {
        text.insert_str(0, "# Direct mode also requires clearing proxy environment variables in the launching process.\n");
    }
    Ok(text)
}
pub fn save(profile: &NetworkProfile, path: &Path) -> Result<()> {
    validate(profile)?;
    studio_core::export_document(profile, path)
}
pub fn export(profile: &NetworkProfile, format: Format, path: &Path) -> Result<()> {
    studio_core::export_bytes(native_config(profile, format)?.as_bytes(), path)
}
pub fn load(path: &Path) -> Result<NetworkProfile> {
    decode(&read_bytes(path)?)
}
fn decode(bytes: &[u8]) -> Result<NetworkProfile> {
    let profile = serde_json::from_slice(bytes).context("Invalid network profile JSON")?;
    validate(&profile)?;
    Ok(profile)
}
fn read_bytes(path: &Path) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).context("Inspect network profile")?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Network profile must be a regular file"
    );
    ensure!(
        metadata.len() <= MAX_BYTES,
        "Network profile exceeds 16 KiB"
    );
    let file = File::open(path).context("Open network profile")?;
    ensure!(
        file.metadata()?.is_file(),
        "Opened network profile must be a regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened = file.metadata()?;
        ensure!(
            opened.dev() == metadata.dev() && opened.ino() == metadata.ino(),
            "Network profile changed while opening"
        );
    }
    ensure!(
        fs::symlink_metadata(path)?.is_file(),
        "Network profile changed while opening"
    );
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("Read network profile")?;
    ensure!(
        bytes.len() as u64 <= MAX_BYTES,
        "Network profile exceeds 16 KiB"
    );
    Ok(bytes)
}

pub fn load_current() -> Result<NetworkProfile> {
    load_current_at(&studio_platform::data_directory()?.join("network-profile.json"))
}
fn load_current_at(path: &Path) -> Result<NetworkProfile> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(NetworkProfile::default()),
        Err(error) => Err(error).context("Inspect current network profile"),
        Ok(_) => load(path),
    }
}
static APPLY_LOCK: Mutex<()> = Mutex::new(());
/// Replaces only DevEnv Studio's profile, after preserving the previous exact bytes.
pub fn apply_current(profile: &NetworkProfile) -> Result<PathBuf> {
    let _lock = APPLY_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("Network profile writer is unavailable"))?;
    apply_at(
        &studio_platform::data_directory()?.join("network-profile.json"),
        profile,
    )
}
fn apply_at(path: &Path, profile: &NetworkProfile) -> Result<PathBuf> {
    validate(profile)?;
    let directory = path
        .parent()
        .context("Network profile has no parent directory")?;
    let _writer = lock_directory(directory)?;
    let previous = match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).context("Inspect previous network profile"),
        Ok(_) => {
            let bytes = read_bytes(path)?;
            decode(&bytes)?;
            Some(bytes)
        }
    };
    let history = crate::environment::create_history(directory)?;
    let before = history.join("before.json");
    match &previous {
        Some(bytes) => {
            write_private(&history.join("before.raw"), bytes)?;
            save(&decode(bytes)?, &before)?;
        }
        None => save(&NetworkProfile::default(), &before)?,
    }
    studio_core::export_document(
        &serde_json::json!({"existed":previous.is_some()}),
        &history.join("beforemeta.json"),
    )?;
    save(profile, &history.join("request.json"))?;
    let mut bytes = serde_json::to_vec_pretty(profile)?;
    bytes.push(b'\n');
    let staged = history.join("profile.tmp");
    write_private(&staged, &bytes)?;
    let result = (|| -> Result<()> {
        ensure!(
            fs::symlink_metadata(directory)?.is_dir(),
            "Application data must remain a real directory"
        );
        match &previous {
            Some(old) => ensure!(
                read_bytes(path)? == *old,
                "Current network profile changed; reload before applying"
            ),
            None => ensure!(
                matches!(fs::symlink_metadata(path), Err(error) if error.kind() == std::io::ErrorKind::NotFound),
                "Current network profile appeared; reload before applying"
            ),
        }
        // Other app instances share the advisory lock; recheck bytes to catch external edits before replacement.
        fs::rename(&staged, path).context("Publish current network profile")?;
        sync_directory(directory)?;
        verify_written(path, &bytes, profile)
    })();
    let status = if result.is_ok() { "verified" } else { "failed" };
    studio_core::export_document(
        &serde_json::json!({"schema_version":1,"kind":"network-profile","status":status}),
        &history.join("result.json"),
    )
    .with_context(|| format!("Persist network profile result in {}", history.display()))?;
    result.with_context(|| {
        format!(
            "Network profile was not verified; previous bytes are in {}",
            history.display()
        )
    })?;
    Ok(history)
}
fn lock_directory(directory: &Path) -> Result<File> {
    ensure!(
        directory.is_absolute(),
        "Application data directory must be absolute"
    );
    fs::create_dir_all(directory).context("Create application data directory")?;
    ensure!(
        fs::symlink_metadata(directory)?.is_dir(),
        "Application data must be a real directory"
    );
    let path = directory.join("network-profile.lock");
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = match options.open(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            ensure!(
                fs::symlink_metadata(&path)?.is_file(),
                "Network profile lock must be a regular file"
            );
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .context("Open network profile writer lock")?
        }
        result => result.context("Create network profile writer lock")?,
    };
    let metadata = fs::symlink_metadata(&path)?;
    let opened = file.metadata()?;
    ensure!(
        metadata.is_file() && opened.is_file(),
        "Network profile lock must remain a regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            metadata.dev() == opened.dev() && metadata.ino() == opened.ino(),
            "Network profile lock changed while opening"
        );
    }
    file.try_lock()
        .map_err(|error| anyhow::anyhow!("Cannot acquire network profile writer lock: {error}"))?;
    Ok(file)
}
fn verify_written(path: &Path, bytes: &[u8], profile: &NetworkProfile) -> Result<()> {
    ensure!(
        read_bytes(path)? == bytes,
        "Published network profile failed exact readback"
    );
    ensure!(
        load(path)? == *profile,
        "Published network profile failed validation"
    );
    Ok(())
}
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .context("Create private network profile")?;
    file.write_all(bytes)?;
    file.sync_all().context("Persist private network profile")?;
    sync_directory(
        path.parent()
            .context("Private network file has no parent")?,
    )
}
fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?
        .sync_all()
        .context("Persist network profile directory")?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "ltsc-network-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn file(&self) -> PathBuf {
            self.0.join("network-profile.json")
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    #[test]
    fn proxy_and_source_validation_rejects_ambiguous_or_executable_inputs() {
        for text in [
            "http://localhost:7890",
            "http://127.0.0.1:80",
            "http://[::1]:65535",
        ] {
            assert!(parse_proxy_endpoint(text).is_ok(), "{text}");
        }
        for text in [
            "https://localhost:80",
            "http://localhost",
            "http://localhost:0",
            "http://localhost:65536",
            "http://a:1/",
            "http://a:1?q",
            "http://a:1#x",
            "http://user:pass@a:1",
            "http://a:1\n",
            "http://a%2eb:1",
            "http://[::x]:1",
            "http://01.2.3.4:1",
            "http://a_b:1",
            "http://-a:1",
            "http://a:1\x60",
            "http://${HOST}:1",
        ] {
            assert!(parse_proxy_endpoint(text).is_err(), "{text}");
        }
        for url in [
            "https://example.com/simple",
            "https://[::1]:443/a-b",
            "https://localhost/",
        ] {
            assert!(source_url(url).is_ok(), "{url}");
        }
        for url in [
            "http://example.com/",
            "https://user@example.com/",
            "https://example.com/../x",
            "https://example.com/%2e",
            "https://example.com/a?q=x",
            "https://例子.test/",
            "https://example.com/a\\b",
            "https://example.com/a\n",
            "https://example.com/a'",
        ] {
            assert!(source_url(url).is_err(), "{url}");
        }
        let mut profile = NetworkProfile {
            mode: ProxyMode::Manual,
            ..NetworkProfile::default()
        };
        assert!(validate(&profile).is_err());
        profile.proxy_url = "http://127.0.0.1:7890".into();
        validate(&profile).unwrap();
        profile.cargo_index = "sparse+https://index.crates.io".into();
        assert!(validate(&profile).is_err());
        profile.cargo_index = CARGO_INDEX.into();
        profile.schema_version = 2;
        assert!(validate(&profile).is_err());
    }
    #[test]
    fn unscoped_execution_evidence_never_claims_default_sources_were_applied() {
        let directory = Directory::new();
        let path = directory.0.join("network.json");
        assert!(active_profile().is_none());
        record_current(&path).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["app_network_profile_applied"], false);
        assert!(value.get("npm_registry").is_none());
        assert!(
            load(&path).is_err(),
            "Unapplied evidence must not masquerade as an active profile"
        );
    }

    #[test]
    fn strict_json_and_no_clobber_roundtrip() {
        let directory = Directory::new();
        let profile = NetworkProfile::default();
        save(&profile, &directory.file()).unwrap();
        assert_eq!(load(&directory.file()).unwrap(), profile);
        assert!(save(&profile, &directory.file()).is_err());
        let json = serde_json::to_string(&profile).unwrap();
        for text in [
            json.replacen('{', "{\"extra\":1,", 1),
            json.replacen('{', "{\"schema_version\":1,", 1),
            "{}".into(),
        ] {
            assert!(decode(text.as_bytes()).is_err());
        }
        export(&profile, Format::Npm, &directory.0.join("npmrc")).unwrap();
        assert!(export(&profile, Format::Npm, &directory.0.join("npmrc")).is_err());
    }
    #[test]
    fn native_exports_contain_only_supported_fields_and_literal_values() {
        let mut profile = NetworkProfile::default();
        assert!(
            native_config(&profile, Format::Git)
                .unwrap()
                .contains("inherited")
        );
        profile.mode = ProxyMode::Direct;
        assert!(
            native_config(&profile, Format::Git)
                .unwrap()
                .ends_with("[http]\n\tproxy = \n")
        );
        assert!(
            native_config(&profile, Format::Cargo)
                .unwrap()
                .contains("proxy = \"\"")
        );
        assert!(
            native_config(&profile, Format::Npm)
                .unwrap()
                .contains("https-proxy=null")
        );
        profile.mode = ProxyMode::Manual;
        profile.proxy_url = "http://[::1]:7890".into();
        profile.cargo_index = "sparse+https://mirror.example/crates/".into();
        assert!(
            native_config(&profile, Format::Cargo)
                .unwrap()
                .contains("[source.crates-io]")
        );
        assert!(
            native_config(&profile, Format::Pip)
                .unwrap()
                .contains("proxy = http://[::1]:7890")
        );
        assert!(
            native_config(&profile, Format::Uv)
                .unwrap()
                .contains("default = true")
        );
    }
    #[test]
    fn scope_is_immutable_nested_and_restored_after_error_or_panic() {
        let outer = NetworkProfile::default();
        let mut inner = outer.clone();
        inner.mode = ProxyMode::Direct;
        assert!(active_profile().is_none());
        with_profile(&outer, || {
            assert_eq!(active_profile(), Some(outer.clone()));
            with_profile(&inner, || {
                assert_eq!(active_profile(), Some(inner.clone()));
                Ok(())
            })?;
            assert_eq!(active_profile(), Some(outer.clone()));
            assert!(
                std::panic::catch_unwind(|| with_profile(&inner, || -> Result<()> {
                    panic!("test scope");
                }))
                .is_err()
            );
            assert_eq!(active_profile(), Some(outer.clone()));
            Ok(())
        })
        .unwrap();
        assert!(active_profile().is_none());
        assert!(
            with_profile(&outer, || -> Result<()> {
                anyhow::bail!("test error");
            })
            .is_err()
        );
        assert!(active_profile().is_none());
    }
    #[test]
    fn apply_preserves_exact_before_bytes_and_verifies_current_and_records() {
        let directory = Directory::new();
        assert_eq!(
            load_current_at(&directory.file()).unwrap(),
            NetworkProfile::default()
        );
        let first = NetworkProfile::default();
        let history = apply_at(&directory.file(), &first).unwrap();
        assert_eq!(
            load(&history.join("before.json")).unwrap(),
            NetworkProfile::default()
        );
        assert!(
            fs::read_to_string(history.join("beforemeta.json"))
                .unwrap()
                .contains("false")
        );
        let before = fs::read(directory.file()).unwrap();
        let mut second = first.clone();
        second.mode = ProxyMode::Direct;
        let history = apply_at(&directory.file(), &second).unwrap();
        assert_eq!(fs::read(history.join("before.raw")).unwrap(), before);
        assert_eq!(load(&history.join("before.json")).unwrap(), first);
        assert_eq!(load(&history.join("request.json")).unwrap(), second);
        assert_eq!(load_current_at(&directory.file()).unwrap(), second);
        assert!(
            fs::read_to_string(history.join("result.json"))
                .unwrap()
                .contains("verified")
        );
        with_profile(&second, || record_current(&history.join("network.json"))).unwrap();
        assert_eq!(load(&history.join("network.json")).unwrap(), second);
        fs::write(directory.file(), b"{broken").unwrap();
        assert!(load_current_at(&directory.file()).is_err());
        assert!(apply_at(&directory.file(), &first).is_err());
        assert_eq!(fs::read(directory.file()).unwrap(), b"{broken");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(history.join("before.raw"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn another_writer_or_linked_lock_stops_before_replacing_profile() {
        let directory = Directory::new();
        let _writer = lock_directory(&directory.0).unwrap();
        assert!(apply_at(&directory.file(), &NetworkProfile::default()).is_err());
        assert!(!directory.file().exists());
        drop(_writer);
        apply_at(&directory.file(), &NetworkProfile::default()).unwrap();
        #[cfg(unix)]
        {
            let lock = directory.0.join("network-profile.lock");
            fs::remove_file(&lock).unwrap();
            std::os::unix::fs::symlink(directory.file(), &lock).unwrap();
            assert!(apply_at(&directory.file(), &NetworkProfile::default()).is_err());
        }
    }
    #[test]
    fn readback_rejects_changed_bytes_and_invalid_schema() {
        let directory = Directory::new();
        let profile = NetworkProfile::default();
        save(&profile, &directory.file()).unwrap();
        let bytes = fs::read(directory.file()).unwrap();
        verify_written(&directory.file(), &bytes, &profile).unwrap();
        fs::write(directory.file(), b"{broken").unwrap();
        assert!(verify_written(&directory.file(), &bytes, &profile).is_err());
        assert!(verify_written(&directory.file(), b"{broken", &profile).is_err());
    }
    #[test]
    fn nonregular_or_linked_profile_is_never_followed_or_replaced() {
        let directory = Directory::new();
        fs::create_dir(directory.file()).unwrap();
        assert!(load_current_at(&directory.file()).is_err());
        assert!(apply_at(&directory.file(), &NetworkProfile::default()).is_err());
        #[cfg(unix)]
        {
            fs::remove_dir(directory.file()).unwrap();
            let target = directory.0.join("untouched");
            fs::write(&target, b"sentinel").unwrap();
            std::os::unix::fs::symlink(&target, directory.file()).unwrap();
            assert!(load_current_at(&directory.file()).is_err());
            assert!(apply_at(&directory.file(), &NetworkProfile::default()).is_err());
            assert_eq!(fs::read(&target).unwrap(), b"sentinel");
            fs::remove_file(directory.file()).unwrap();
            std::os::unix::fs::symlink(directory.0.join("missing"), directory.file()).unwrap();
            assert!(apply_at(&directory.file(), &NetworkProfile::default()).is_err());
        }
    }
}
