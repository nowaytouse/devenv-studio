//! Portable deployment drafts: candidate catalog and explicit selection, never execution approval.
use crate::{ExecutionTarget, NetworkMode, Options, SetupProfile};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fs::OpenOptions, io::Read, path::Path};

const MAX_BYTES: usize = studio_core::MAX_INPUT_BYTES as usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    WingetCore,
    WingetDev,
    Store,
    Scoop,
    Cargo,
    Npm,
    Pip,
    Uv,
    Extension,
    Model,
}

impl Group {
    pub const ALL: [Self; 10] = [
        Self::WingetCore,
        Self::WingetDev,
        Self::Store,
        Self::Scoop,
        Self::Cargo,
        Self::Npm,
        Self::Pip,
        Self::Uv,
        Self::Extension,
        Self::Model,
    ];

    fn key(self, id: &str) -> String {
        let group = match self {
            Self::WingetCore => "winget-core",
            Self::WingetDev => "winget-dev",
            Self::Store => "store",
            Self::Scoop => "scoop",
            Self::Cargo => "cargo",
            Self::Npm => "npm",
            Self::Pip => "pip",
            Self::Uv => "uv",
            Self::Extension => "extension",
            Self::Model => "model",
        };
        format!("{group}:{}", id.to_ascii_lowercase())
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub key: String,
    pub group: Group,
    pub id: String,
    pub name: String,
}

fn entries(profile: &SetupProfile) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut push = |group: Group, id: &str, name: &str| {
        entries.push(Entry {
            key: group.key(id),
            group,
            id: id.to_owned(),
            name: name.to_owned(),
        });
    };
    for (group, apps) in [
        (Group::WingetCore, &profile.packages.winget_core),
        (Group::WingetDev, &profile.packages.winget_dev),
        (Group::Store, &profile.packages.store_apps),
    ] {
        for app in apps {
            push(group, &app.id, &app.name);
        }
    }
    for (group, values) in [
        (Group::Scoop, &profile.packages.scoop_tools),
        (Group::Cargo, &profile.packages.cargo_packages),
        (Group::Npm, &profile.packages.npm_globals),
        (Group::Pip, &profile.packages.pip_packages),
        (Group::Uv, &profile.packages.uv_tools),
        (Group::Extension, &profile.vscode_config.extensions),
        (Group::Model, &profile.ollama_models),
    ] {
        for id in values {
            push(group, id, id);
        }
    }
    entries
}

fn parse_profile(profile_json: &str) -> Result<SetupProfile> {
    ensure!(
        profile_json.len() <= MAX_BYTES,
        "Deployment profile exceeds 1 MiB"
    );
    let profile: SetupProfile =
        serde_json::from_str(profile_json).context("Invalid deployment profile JSON")?;
    profile.validate()?;
    Ok(profile)
}

pub fn candidates(profile_json: &str) -> Result<Vec<Entry>> {
    Ok(entries(&parse_profile(profile_json)?))
}

fn validate_selection(profile: &SetupProfile, selected: &BTreeSet<String>) -> Result<()> {
    let available: BTreeSet<_> = entries(profile)
        .into_iter()
        .map(|entry| entry.key)
        .collect();
    for key in selected {
        ensure!(
            available.contains(key),
            "Unknown deployment selection: {key}"
        );
    }
    Ok(())
}

/// Filters every provider independently; selecting an item never selects another provider's item.
pub fn selected_profile(profile_json: &str, selected: &BTreeSet<String>) -> Result<String> {
    let mut profile = parse_profile(profile_json)?;
    validate_selection(&profile, selected)?;
    for (group, apps) in [
        (Group::WingetCore, &mut profile.packages.winget_core),
        (Group::WingetDev, &mut profile.packages.winget_dev),
        (Group::Store, &mut profile.packages.store_apps),
    ] {
        apps.retain(|app| selected.contains(&group.key(&app.id)));
    }
    for (group, values) in [
        (Group::Scoop, &mut profile.packages.scoop_tools),
        (Group::Cargo, &mut profile.packages.cargo_packages),
        (Group::Npm, &mut profile.packages.npm_globals),
        (Group::Pip, &mut profile.packages.pip_packages),
        (Group::Uv, &mut profile.packages.uv_tools),
        (Group::Extension, &mut profile.vscode_config.extensions),
        (Group::Model, &mut profile.ollama_models),
    ] {
        values.retain(|id| selected.contains(&group.key(id)));
    }
    profile.validate()?;
    Ok(serde_json::to_string_pretty(&profile)?)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub schema_version: u32,
    pub profile: SetupProfile,
    pub target: ExecutionTarget,
    pub network_mode: NetworkMode,
    pub options: Options,
    #[serde(deserialize_with = "unique_selection")]
    pub selection: BTreeSet<String>,
}

fn unique_selection<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeSet<String>, D::Error> {
    let values = Vec::<String>::deserialize(deserializer)?;
    let mut selected = BTreeSet::new();
    for key in values {
        if !selected.insert(key) {
            return Err(serde::de::Error::custom("Duplicate deployment selection"));
        }
    }
    Ok(selected)
}

/// Validates a draft, including all-off drafts; execution review remains a separate gate.
pub fn validate(request: &Request) -> Result<()> {
    ensure!(
        request.schema_version == 1,
        "Unsupported deployment request schema"
    );
    ensure!(
        !matches!(
            request.target,
            ExecutionTarget::AgentSkillsOnly | ExecutionTarget::RollbackLatest
        ),
        "Retired Agent payload and rollback targets are unavailable"
    );
    request.profile.validate()?;
    validate_selection(&request.profile, &request.selection)?;
    ensure!(
        serde_json::to_vec_pretty(request)?.len() < MAX_BYTES,
        "Deployment request exceeds 1 MiB"
    );
    Ok(())
}

/// Bounded regular-file read; final links/reparse points and linked ancestors are refused.
pub fn load(path: &Path) -> Result<Request> {
    crate::utils::reject_links(path)?;
    let before = std::fs::symlink_metadata(path)?;
    ensure!(
        before.is_file() && before.len() <= MAX_BYTES as u64,
        "Request must be a regular file of at most 1 MiB"
    );
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x100); // O_NOFOLLOW
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(linux_nofollow_flag(std::env::consts::ARCH)?);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let file = options
        .open(path)
        .with_context(|| format!("Open request {}", path.display()))?;
    let opened = file.metadata()?;
    ensure!(opened.is_file(), "Opened request is not a regular file");
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            opened.file_attributes() & 0x400 == 0,
            "Opened request is a reparse point"
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            before.dev() == opened.dev() && before.ino() == opened.ino(),
            "Request changed while opening"
        );
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= MAX_BYTES, "Deployment request exceeds 1 MiB");
    crate::utils::reject_links(path)?;
    let request: Request =
        serde_json::from_slice(&bytes).context("Invalid deployment request JSON")?;
    validate(&request)?;
    Ok(request)
}

#[cfg(any(target_os = "linux", target_os = "android", test))]
fn linux_nofollow_flag(architecture: &str) -> Result<i32> {
    // Linux/AOSP fcntl.h assigns different O_NOFOLLOW bits to ARM and x86.
    match architecture {
        "x86" | "x86_64" => Ok(0x20000),
        "arm" | "aarch64" => Ok(0x8000),
        _ => anyhow::bail!(
            "No verified O_NOFOLLOW flag for Linux/Android architecture {architecture}"
        ),
    }
}

/// Publishes a complete draft without replacing any existing file, including dangling links.
pub fn save(request: &Request, path: &Path) -> Result<()> {
    validate(request)?;
    crate::utils::reject_links(path)?;
    studio_core::export_document(request, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft() -> Request {
        Request {
            schema_version: 1,
            profile: SetupProfile::load_default(),
            target: ExecutionTarget::DevToolsOnly,
            network_mode: NetworkMode::Basic,
            options: Options::default(),
            selection: BTreeSet::new(),
        }
    }

    fn directory() -> std::path::PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        loop {
            let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
                "studio-request-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return path,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create temporary request directory: {error}"),
            }
        }
    }

    #[test]
    fn exact_keys_keep_providers_separate_and_select_no_store_implicitly() {
        let mut profile = draft().profile;
        profile.packages.winget_core = vec![crate::config::WingetApp {
            id: "Same.Id".into(),
            name: "Core".into(),
        }];
        profile.packages.winget_dev = vec![crate::config::WingetApp {
            id: "Same.Id".into(),
            name: "Dev".into(),
        }];
        profile.packages.cargo_packages = vec!["same".into()];
        profile.packages.pip_packages = vec!["same".into()];
        let json = serde_json::to_string(&profile).unwrap();
        let entries = candidates(&json).unwrap();
        assert!(
            entries
                .iter()
                .any(|entry| entry.key == "winget-core:same.id")
        );
        assert!(
            entries
                .iter()
                .any(|entry| entry.key == "winget-dev:same.id")
        );
        let selected = BTreeSet::from(["winget-dev:same.id".into(), "cargo:same".into()]);
        let filtered: SetupProfile =
            serde_json::from_str(&selected_profile(&json, &selected).unwrap()).unwrap();
        assert!(filtered.packages.winget_core.is_empty());
        assert_eq!(filtered.packages.winget_dev.len(), 1);
        assert_eq!(filtered.packages.cargo_packages, ["same"]);
        assert!(filtered.packages.pip_packages.is_empty());
        assert!(filtered.packages.store_apps.is_empty());
        assert!(filtered.vscode_config.extensions.is_empty());
        assert!(filtered.ollama_models.is_empty());
        assert!(selected_profile(&json, &BTreeSet::from(["unknown:same".into()])).is_err());
        let mut invalid = profile;
        invalid.packages.pip_packages.push("--inject".into());
        assert!(
            selected_profile(&serde_json::to_string(&invalid).unwrap(), &BTreeSet::new()).is_err()
        );
    }

    #[test]
    fn old_profiles_have_no_store_requests_and_store_ids_are_strict() {
        let mut json = serde_json::to_value(draft().profile).unwrap();
        json["packages"]
            .as_object_mut()
            .unwrap()
            .remove("store_apps");
        let old: SetupProfile = serde_json::from_value(json).unwrap();
        old.validate().unwrap();
        assert!(old.packages.store_apps.is_empty());
        let mut profile = draft().profile;
        for id in [
            "--injectxxxx",
            "9WZDNCRFHVN/",
            "9WZDNCRFHVN",
            "９WZDNCRFHVN5",
        ] {
            profile.packages.store_apps[0].id = id.into();
            assert!(profile.validate().is_err(), "{id}");
        }
        let mut profile = draft().profile;
        let mut duplicate = profile.packages.store_apps[0].clone();
        duplicate.id.make_ascii_lowercase();
        profile.packages.store_apps.push(duplicate);
        assert!(profile.validate().is_err());
    }

    #[test]
    fn request_roundtrip_preserves_exact_draft_and_never_overwrites() {
        let directory = directory();
        let path = directory.join("request.json");
        let mut request = draft();
        request.selection.insert("cargo:cargo-edit".into());
        validate(&request).unwrap();
        save(&request, &path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(
            serde_json::to_value(&loaded).unwrap(),
            serde_json::to_value(&request).unwrap()
        );
        assert!(save(&request, &path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert!(!String::from_utf8(bytes).unwrap().contains("confirmed"));
        let mut boundary = draft();
        boundary.profile.metadata = serde_json::json!("");
        let overhead = serde_json::to_vec_pretty(&boundary).unwrap().len();
        boundary.profile.metadata = serde_json::json!("x".repeat(MAX_BYTES - 1 - overhead));
        let boundary_path = directory.join("boundary.json");
        save(&boundary, &boundary_path).unwrap();
        assert_eq!(
            std::fs::metadata(&boundary_path).unwrap().len(),
            MAX_BYTES as u64
        );
        load(&boundary_path).unwrap();
        boundary.profile.metadata = serde_json::json!("x".repeat(MAX_BYTES - overhead));
        assert!(save(&boundary, &directory.join("over-boundary.json")).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn malformed_unknown_and_oversized_requests_leave_draft_and_file_untouched() {
        let directory = directory();
        let path = directory.join("request.json");
        let request = draft();
        for content in [
            "{".to_owned(),
            {
                let mut value = serde_json::to_value(&request).unwrap();
                value["confirmed"] = true.into();
                value.to_string()
            },
            {
                let mut value = serde_json::to_value(&request).unwrap();
                value["options"]["unknown"] = true.into();
                value.to_string()
            },
            {
                let mut value = serde_json::to_value(&request).unwrap();
                value["schema_version"] = 2.into();
                value.to_string()
            },
            {
                let mut value = serde_json::to_value(&request).unwrap();
                value["selection"] = serde_json::json!(["cargo:cargo-edit", "cargo:cargo-edit"]);
                value.to_string()
            },
            " ".repeat(MAX_BYTES + 1),
        ] {
            std::fs::write(&path, &content).unwrap();
            assert!(load(&path).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), content.as_bytes());
        }
        let mut oversized = request.clone();
        oversized.profile.metadata = serde_json::json!("x".repeat(MAX_BYTES));
        let absent = directory.join("absent.json");
        assert!(save(&oversized, &absent).is_err());
        assert!(!absent.exists());
        for target in [
            ExecutionTarget::AgentSkillsOnly,
            ExecutionTarget::RollbackLatest,
        ] {
            let mut forbidden = request.clone();
            forbidden.target = target;
            assert!(validate(&forbidden).is_err());
        }
        assert_eq!(request.options, Options::default());
        assert!(request.selection.is_empty());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn request_files_reject_final_dangling_and_parent_links_and_directories() {
        use std::os::unix::fs::symlink;
        let directory = directory();
        let real = directory.join("real");
        std::fs::create_dir(&real).unwrap();
        let request = draft();
        let file = real.join("request.json");
        save(&request, &file).unwrap();
        let link = directory.join("link.json");
        symlink(&file, &link).unwrap();
        assert!(load(&link).is_err());
        assert!(save(&request, &link).is_err());
        let dangling = directory.join("dangling.json");
        symlink(directory.join("missing"), &dangling).unwrap();
        assert!(load(&dangling).is_err());
        assert!(save(&request, &dangling).is_err());
        let parent = directory.join("parent");
        symlink(&real, &parent).unwrap();
        assert!(load(&parent.join("request.json")).is_err());
        assert!(save(&request, &parent.join("new.json")).is_err());
        assert!(load(&real).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn linux_nofollow_uses_verified_architecture_values() {
        for (architecture, expected) in [
            ("x86", 0x20000),
            ("x86_64", 0x20000),
            ("arm", 0x8000),
            ("aarch64", 0x8000),
        ] {
            assert_eq!(
                linux_nofollow_flag(architecture).unwrap(),
                expected,
                "{architecture}"
            );
        }
        for architecture in ["", "unknown", "sparc64"] {
            assert!(linux_nofollow_flag(architecture).is_err(), "{architecture}");
        }
    }
}
