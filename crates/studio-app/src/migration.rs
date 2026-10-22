//! Read-only legacy migration analysis. Imported data never supplies executable payloads.
//! Only IDs reconstructed from the current trusted catalogs can become suggestions.
use anyhow::{Context, Result, bail, ensure};
use serde::{
    Deserialize, Serialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value};
use std::{collections::BTreeSet, fs::File, io::Read, path::Path};
use studio_core::{Hive, RegistryChange, Target};

const MAX_BYTES: usize = 1_048_576;
const MAX_LIST: usize = 1_024;
const MAX_ITEMS: usize = 5_000;
const MAX_NODES: usize = 20_000;

#[derive(Debug, Clone, Serialize)]
pub struct MigrationReport {
    pub source: String,
    pub settings: BTreeSet<String>,
    pub deployment: BTreeSet<String>,
    pub items: Vec<MigrationItem>,
}
#[derive(Debug, Clone, Serialize)]
pub struct MigrationItem {
    pub origin: String,
    pub requested: String,
    pub status: String,
    pub detail: String,
    pub settings: Vec<String>,
    pub deployment: Vec<String>,
}

/// Read at most the input limit plus one byte. This never reads software inventories
/// automatically and never writes a profile, registry value, package or configuration.
pub fn load(path: &Path, target: Target) -> Result<MigrationReport> {
    let mut bytes = Vec::new();
    File::open(path)
        .context("Open migration input")?
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .context("Read migration input")?;
    analyze(&bytes, target)
}

pub fn analyze(bytes: &[u8], target: Target) -> Result<MigrationReport> {
    ensure!(bytes.len() <= MAX_BYTES, "Migration input exceeds 1 MiB");
    let root = serde_json::from_slice::<UniqueValue>(bytes)
        .map_err(|error| {
            anyhow::anyhow!(
                "Invalid or duplicate-key migration JSON at line {}, column {}",
                error.line(),
                error.column()
            )
        })?
        .0;
    let mut nodes = 0;
    check_shape(&root, 0, &mut nodes)?;
    let object = object(&root, "document")?;
    let source = match (
        object.contains_key("profile_version"),
        object.contains_key("schema_version"),
    ) {
        (true, false) => "setup_profile",
        (false, true) => "macos_inventory",
        _ => bail!("Migration document must identify exactly one supported source format"),
    };
    let settings_catalog = studio_core::load_catalog()?;
    let deployment_catalog = studio_deployment::catalog()?;
    let mut analyzer = Analyzer {
        target,
        settings_catalog,
        deployment_catalog,
        report: MigrationReport {
            source: source.into(),
            settings: BTreeSet::new(),
            deployment: BTreeSet::new(),
            items: Vec::new(),
        },
    };
    if source == "setup_profile" {
        analyzer.setup(object)?;
    } else {
        analyzer.inventory(object)?;
    }
    ensure!(
        analyzer.report.items.len() <= MAX_ITEMS,
        "Too many migration review items"
    );
    Ok(analyzer.report)
}

struct Analyzer {
    target: Target,
    settings_catalog: studio_core::Catalog,
    deployment_catalog: studio_deployment::Catalog,
    report: MigrationReport,
}
impl Analyzer {
    fn item(
        &mut self,
        origin: &str,
        requested: &str,
        status: &str,
        detail: &str,
        settings: Vec<String>,
        deployment: Vec<String>,
    ) {
        self.report.settings.extend(settings.iter().cloned());
        self.report.deployment.extend(deployment.iter().cloned());
        self.report.items.push(MigrationItem {
            origin: origin.into(),
            requested: requested.into(),
            status: status.into(),
            detail: detail.into(),
            settings,
            deployment,
        });
    }
    fn redacted(&mut self, origin: &str, detail: &str) {
        self.item(
            origin,
            "[value redacted]",
            "manual",
            detail,
            Vec::new(),
            Vec::new(),
        );
    }
    fn empty(&mut self, origin: &str) {
        self.item(
            origin,
            "(empty list)",
            "manual",
            "The source list is empty. It requests no installation or removal.",
            Vec::new(),
            Vec::new(),
        );
    }
    fn unknown(&mut self, object: &Map<String, Value>, known: &[&str], prefix: &str) {
        for (index, _) in object
            .keys()
            .filter(|key| !known.contains(&key.as_str()))
            .enumerate()
        {
            self.redacted(&format!("{prefix}.[unrecognized field #{}]", index + 1), "Unrecognized custom field: no schema semantics or trusted migration mapping. Field name and value are withheld; no automatic changes.");
        }
    }
    fn package(&mut self, origin: &str, requested: &str, kind: &str, identity: &str, reason: &str) {
        let candidates: Vec<_> = self
            .deployment_catalog
            .tasks
            .iter()
            .filter(|task| task.kind == kind && task.package_id.as_deref() == Some(identity))
            .collect();
        if candidates.len() != 1 {
            self.item(origin, requested, "manual", "No unique exact provider/package ID exists in the current trusted catalog. No approximate match or provider substitution was selected.", Vec::new(), Vec::new());
            return;
        }
        let task = candidates[0];
        if !task.targets.contains(&self.target) {
            self.item(origin, requested, "unsupported", "The exact trusted task does not support the selected Windows target. Nothing was selected.", Vec::new(), Vec::new());
            return;
        }
        let id = task.id.clone();
        let prerequisite = match kind {
            "cargo" => {
                " Requires an existing working Windows Cargo/Rust toolchain and any crate-specific build dependencies; Mac binaries and build options are not copied."
            }
            "uv" => {
                " Requires an existing usable Python interpreter and uv on Windows; automatic Python downloads are disabled. The source environment is not copied."
            }
            "npm" => {
                " Requires official Node.js and bundled npm on Windows. Uses the fixed current-user npm prefix and official registry with lifecycle scripts disabled; no PATH, mirror or project configuration is imported."
            }
            "vscode" => {
                " Requires the Microsoft VS Code CLI on Windows. This does not import editor settings or install into Cursor."
            }
            _ => {
                " Requires the Windows provider and prerequisites shown by the trusted task; no installed-state or version equivalence is inferred."
            }
        };
        self.item(
            origin,
            requested,
            "mapped",
            &format!("{reason}{prerequisite}"),
            Vec::new(),
            vec![id],
        );
    }
    fn provider_list(&mut self, value: &Value, origin: &str, kind: Option<&str>) -> Result<usize> {
        let list = identifiers(value, origin)?;
        if list.is_empty() {
            self.empty(origin);
        }
        for identifier in &list {
            if let Some(kind) = kind {
                self.package(origin, identifier, kind, identifier, "Exact provider/package ID exists in the trusted catalog. This is a review suggestion only; installed state, version and configuration have not been migrated.");
            } else {
                self.item(origin, identifier, "manual", "This source provider is not executable in the current trusted catalog. Installing through a different provider would change semantics; no substitution was selected.", Vec::new(), Vec::new());
            }
        }
        Ok(list.len())
    }
    fn setup(&mut self, input: &Map<String, Value>) -> Result<()> {
        ensure!(
            input.get("profile_version").and_then(Value::as_str) == Some("2.2.0"),
            "Unsupported legacy SetupProfile version; supported version is 2.2.0"
        );
        self.item("profile_version", "2.2.0", "manual", "Supported legacy source schema. Analysis is read-only; importing does not execute the old deployment stages.", Vec::new(), Vec::new());
        for field in [
            "metadata",
            "git_config",
            "powershell_profile",
            "environment_mirrors",
        ] {
            let value = required(input, field)?;
            object(value, field)?;
            self.redacted(field, "Private/custom configuration is not migrated. Review it separately; no identity, alias, profile, mirror or environment value was copied or written.");
        }
        let packages = object(required(input, "packages")?, "packages")?;
        let mut winget_seen = BTreeSet::new();
        for field in ["winget_core", "winget_dev"] {
            let origin = format!("packages.{field}");
            let list = array(required(packages, field)?, &origin)?;
            if list.is_empty() {
                self.empty(&origin);
            }
            for entry in list {
                let entry = object(entry, &origin)?;
                let id = identifier(required(entry, "id")?, &origin)?;
                let name = required(entry, "name")?
                    .as_str()
                    .context("Package display name must be a string")?;
                ensure!(
                    name.len() <= 512 && !name.chars().any(char::is_control),
                    "Invalid legacy package display name"
                );
                ensure!(
                    winget_seen.insert(id.to_ascii_lowercase()),
                    "Duplicate legacy WinGet package ID"
                );
                self.package(&origin, id, "package", id, "Exact WinGet package ID exists in the current trusted catalog. Legacy display names and installer configuration are not used.");
                self.unknown(entry, &["id", "name"], &origin);
            }
        }
        for (field, kind) in [
            ("cargo_packages", Some("cargo")),
            ("uv_tools", Some("uv")),
            ("npm_globals", Some("npm")),
            ("pip_packages", None),
            ("scoop_tools", None),
        ] {
            self.provider_list(
                required(packages, field)?,
                &format!("packages.{field}"),
                kind,
            )?;
        }
        self.unknown(
            packages,
            &[
                "winget_core",
                "winget_dev",
                "cargo_packages",
                "uv_tools",
                "npm_globals",
                "pip_packages",
                "scoop_tools",
            ],
            "packages",
        );
        let vscode = object(required(input, "vscode_config")?, "vscode_config")?;
        required(vscode, "user_settings")?;
        self.redacted("vscode_config.user_settings","Editor settings may contain private paths, commands and credentials. They are not copied, merged or written by this importer.");
        self.provider_list(
            required(vscode, "extensions")?,
            "vscode_config.extensions",
            Some("vscode"),
        )?;
        self.unknown(vscode, &["extensions", "user_settings"], "vscode_config");
        let tweaks = object(required(input, "system_tweaks")?, "system_tweaks")?;
        ensure!(
            !tweaks.is_empty(),
            "Legacy system_tweaks cannot be an empty object"
        );
        for (key, value) in tweaks {
            let origin = format!(
                "system_tweaks.{}",
                safe_known_tweak(key).unwrap_or("[unrecognized field]")
            );
            if let Some(changes) = tweak_changes(key) {
                let enabled = value
                    .as_bool()
                    .context("Legacy boolean system tweak has an invalid type")?;
                if enabled {
                    self.registry(&origin, &changes);
                } else {
                    self.item(&origin,"false","manual","In the old tool, false skips this tweak; it does not restore or request the opposite value. No opposite setting was selected.",Vec::new(),Vec::new());
                }
            } else if key == "optimize_storage" {
                let enabled = value
                    .as_bool()
                    .context("Legacy boolean system tweak has an invalid type")?;
                if !enabled {
                    self.item(&origin, "false", "manual", "False means the old operation was skipped, not that an inverse operation should run. No changes were selected.", Vec::new(), Vec::new());
                } else if let Some(id) = self
                    .deployment_catalog
                    .tasks
                    .iter()
                    .find(|task| {
                        task.id == "storage-optimize-all"
                            && task.kind == "maintenance"
                            && task.category == "storage"
                            && task.targets.contains(&self.target)
                    })
                    .map(|task| task.id.clone())
                {
                    self.item(&origin, "true", "mapped", "Matches the legacy defrag /C /O /U operation: Windows chooses optimization for all volumes, potentially including removable volumes, not only fixed drives. This is a review suggestion; no analysis or TRIM-notification policy change is selected. Optimization has no rollback.", Vec::new(), vec![id]);
                } else {
                    self.item(&origin, "true", "manual", "No exact reviewed storage-optimization task supports this Windows target. No storage operation was selected.", Vec::new(), Vec::new());
                }
            } else if safe_known_tweak(key).is_some() {
                if matches!(
                    key.as_str(),
                    "system_responsiveness" | "network_throttling_index"
                ) {
                    let number = value
                        .as_u64()
                        .context("Legacy numeric tweak must be an unsigned integer")?;
                    ensure!(
                        if key == "system_responsiveness" {
                            number <= 100
                        } else {
                            number <= u32::MAX as u64
                        },
                        "Legacy numeric tweak is outside its allowed range"
                    );
                    self.redacted(&origin,"Legacy numeric network/performance policy has no exact reviewed mapping. No value is copied or written.");
                } else {
                    let enabled = value
                        .as_bool()
                        .context("Legacy boolean system tweak has an invalid type")?;
                    self.item(&origin,if enabled {"true"} else {"false"},"manual",if matches!(key.as_str(), "activate_ultimate_performance" | "extreme_ac_power_settings") && enabled {
                        "Legacy power setup creates or switches an Ultimate Performance scheme and may also change CPU and hibernation parameters. Current AC choices modify only the active scheme; no partial or approximate mapping was selected."
                    } else if key == "enable_trim" && enabled {
                        "TRIM-notification policy requires separate NTFS and ReFS state capture, exact parsing and verified restoration. No fsutil policy write or approximate optimization mapping was selected."
                    } else if key == "create_rollback_journal" {
                        "Legacy rollback-journal preference is not imported. New registry execution always uses its own durable journal and does not consume old rollback files."
                    } else if enabled {
                        "No exact reviewed equivalent exists in the current settings catalog. Power, storage, networking, security and broad performance changes are not approximated."
                    } else {
                        "False means the old operation was skipped, not that an inverse operation should run. No changes were selected."
                    },Vec::new(),Vec::new());
                }
            } else {
                self.redacted(
                    &origin,
                    "Unrecognized system tweak and value withheld. No automatic migration.",
                );
            }
        }
        let features = object(required(input, "windows_features")?, "windows_features")?;
        ensure!(
            !features.is_empty(),
            "Legacy windows_features cannot be an empty object"
        );
        for (key, value) in features {
            let feature_names: Option<&[&str]> = match key.as_str() {
                "enable_wsl2" => Some(&[
                    "Microsoft-Windows-Subsystem-Linux",
                    "VirtualMachinePlatform",
                ]),
                "enable_netfx3" => Some(&["NetFx3"]),
                "enable_windows_sandbox" => Some(&["Containers-DisposableClientVM"]),
                "enable_hyper_v" => Some(&["Microsoft-Hyper-V-All"]),
                _ => None,
            };
            if let Some(names) = feature_names {
                let enabled = value
                    .as_bool()
                    .context("Legacy Windows feature selection must be boolean")?;
                let origin = format!("windows_features.{key}");
                if enabled {
                    self.features(&origin, names);
                } else {
                    self.item(&origin,"false","manual","False skips enabling in the old tool. It does not request disabling an installed Windows feature.",Vec::new(),Vec::new());
                }
            } else {
                self.redacted("windows_features.[unrecognized field]","Unrecognized feature request; key and value withheld, and no component selected.");
            }
        }
        let models = identifiers(required(input, "ollama_models")?, "ollama_models")?;
        self.redacted("ollama_models",if models.is_empty() {"The source model list is empty; no model download or removal is requested."} else {"Model choices can be private and models have separate storage/license/runtime requirements. No model download, deletion or configuration is performed."});
        self.unknown(
            input,
            &[
                "profile_version",
                "metadata",
                "git_config",
                "powershell_profile",
                "environment_mirrors",
                "vscode_config",
                "packages",
                "system_tweaks",
                "windows_features",
                "ollama_models",
            ],
            "document",
        );
        Ok(())
    }
    fn registry(&mut self, origin: &str, changes: &[RegistryChange]) {
        let candidates: Vec<_> = self
            .settings_catalog
            .actions
            .iter()
            .filter(|action| {
                action.changes.len() == changes.len()
                    && changes.iter().all(|expected| {
                        action
                            .changes
                            .iter()
                            .any(|actual| same_change(expected, actual))
                    })
            })
            .collect();
        if candidates.len() != 1 {
            self.item(origin,"true","manual","No unique exact registry-equivalent action exists in the current trusted catalog. Similar labels or narrower/broader tweaks are not substituted.",Vec::new(),Vec::new());
        } else if !candidates[0].targets.contains(&self.target) {
            self.item(origin,"true","unsupported","The exact registry-equivalent setting does not support the selected Windows target.",Vec::new(),Vec::new());
        } else {
            let id = candidates[0].id.clone();
            self.item(origin,"true","mapped","Matched every hive, key, value name and DWORD value against one current trusted action. Original bytes are not copied; the trusted action is suggested for explicit review.",vec![id],Vec::new());
        }
    }
    fn features(&mut self, origin: &str, names: &[&str]) {
        let mut ids = Vec::new();
        for name in names {
            let candidates: Vec<_> = self
                .deployment_catalog
                .tasks
                .iter()
                .filter(|task| {
                    task.kind == "feature" && task.feature_name.as_deref() == Some(*name)
                })
                .collect();
            if candidates.len() != 1 || !candidates[0].targets.contains(&self.target) {
                self.item(origin,"true","unsupported","The complete feature request cannot be represented for this Windows target. No partial feature selection was added.",Vec::new(),Vec::new());
                return;
            }
            ids.push(candidates[0].id.clone());
        }
        self.item(origin,"true","mapped",if names.len()==2 {
            "WSL2 requires BOTH Windows Subsystem for Linux and Virtual Machine Platform; both trusted tasks are suggested together. This does not install a distribution, kernel update, or configure firmware virtualization; a restart may be needed."
        } else {"Exact Windows optional-feature identity exists in the trusted catalog. Source files, administrator rights and a restart may be required."},Vec::new(),ids);
    }
    fn inventory(&mut self, input: &Map<String, Value>) -> Result<()> {
        ensure!(
            input.get("schema_version").and_then(Value::as_u64) == Some(1),
            "Unsupported MacosInventory schema; supported schema is 1"
        );
        required(input, "captured_at")?
            .as_str()
            .context("Inventory captured_at must be a string")?;
        self.redacted("captured_at","Imported inventory timestamp is source metadata, not a live inspection or proof of installed Windows software.");
        let mappings: MacMappings =
            serde_json::from_slice(include_bytes!("../../../data/mac-mappings.json"))
                .context("Invalid embedded Mac mappings")?;
        mappings.validate()?;
        let mut count = 0;
        for (field, kind) in [("homebrew_formulae", "formula"), ("homebrew_casks", "cask")] {
            let values = identifiers(required(input, field)?, field)?;
            count += values.len();
            if values.is_empty() {
                self.empty(field);
            }
            for value in values {
                ensure!(
                    !value.contains('/'),
                    "Legacy Homebrew formula/cask names must not contain tap paths"
                );
                if let Some(rule) = mappings
                    .mappings
                    .iter()
                    .find(|rule| rule.source_kind == kind && rule.source == value)
                {
                    self.package(
                        field,
                        &value,
                        &rule.kind,
                        &rule.package_id,
                        &format!("{} Evidence: {}", rule.detail, rule.evidence.join(" ")),
                    );
                } else {
                    self.item(field,&value,"manual","No explicit reviewed macOS-to-Windows mapping exists. No same-name search, guessed package, alternative app or macOS binary was selected.",Vec::new(),Vec::new());
                }
            }
        }
        for (field, kind) in [
            ("cargo_packages", Some("cargo")),
            ("uv_tools", Some("uv")),
            ("npm_globals", Some("npm")),
            ("vscode_extensions", Some("vscode")),
            ("cursor_extensions", None),
        ] {
            count += self.provider_list(required(input, field)?, field, kind)?;
        }
        ensure!(count > 0, "Mac inventory contains no software items");
        let taps = identifiers(required(input, "homebrew_taps")?, "homebrew_taps")?;
        self.redacted("homebrew_taps",if taps.is_empty() {"No Homebrew taps were supplied; no Windows package sources are changed."} else {"Homebrew repository sources are not Windows package sources. Names/URLs are withheld and no package-manager source is added."});
        let warnings = array(required(input, "warnings")?, "warnings")?;
        for warning in warnings {
            warning
                .as_str()
                .context("Inventory warning must be a string")?;
        }
        self.redacted("warnings",if warnings.is_empty() {"The inventory reports no source warnings. This is not a live completeness guarantee."} else {"The imported inventory contains source collection warnings. Their potentially private text is withheld; the software list may be incomplete. Review the original file before using suggestions."});
        self.unknown(
            input,
            &[
                "schema_version",
                "captured_at",
                "homebrew_formulae",
                "homebrew_casks",
                "homebrew_taps",
                "cargo_packages",
                "npm_globals",
                "uv_tools",
                "vscode_extensions",
                "cursor_extensions",
                "warnings",
            ],
            "document",
        );
        Ok(())
    }
}

fn same_change(a: &RegistryChange, b: &RegistryChange) -> bool {
    a.hive == b.hive
        && a.path.eq_ignore_ascii_case(&b.path)
        && a.name.eq_ignore_ascii_case(&b.name)
        && a.value == b.value
}
fn safe_known_tweak(key: &str) -> Option<&str> {
    [
        "create_rollback_journal",
        "activate_ultimate_performance",
        "extreme_ac_power_settings",
        "disable_hibernation",
        "enable_trim",
        "optimize_storage",
        "disable_telemetry",
        "disable_bing_search",
        "disable_consumer_features",
        "disable_advertising_id",
        "disable_activity_history",
        "disable_feedback_prompts",
        "explorer_open_to_this_pc",
        "explorer_show_file_extensions",
        "explorer_show_hidden_files",
        "enable_ntfs_long_paths",
        "enable_developer_mode",
        "system_responsiveness",
        "network_throttling_index",
    ]
    .contains(&key)
    .then_some(key)
}
fn tweak_changes(key: &str) -> Option<Vec<RegistryChange>> {
    let (hive, path, pairs): (Hive, &str, Vec<(&str, u32)>) = match key {
        "disable_telemetry" => (
            Hive::LocalMachine,
            r"SOFTWARE\Policies\Microsoft\Windows\DataCollection",
            vec![("AllowTelemetry", 0)],
        ),
        "disable_bing_search" => (
            Hive::CurrentUser,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Search",
            vec![("BingSearchEnabled", 0), ("DisableSearchBoxSuggestions", 1)],
        ),
        "disable_consumer_features" => (
            Hive::LocalMachine,
            r"SOFTWARE\Policies\Microsoft\Windows\CloudContent",
            vec![("DisableWindowsConsumerFeatures", 1)],
        ),
        "disable_advertising_id" => (
            Hive::CurrentUser,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\AdvertisingInfo",
            vec![("Enabled", 0)],
        ),
        "disable_activity_history" => (
            Hive::LocalMachine,
            r"SOFTWARE\Policies\Microsoft\Windows\System",
            vec![
                ("EnableActivityFeed", 0),
                ("PublishUserActivities", 0),
                ("UploadUserActivities", 0),
            ],
        ),
        "disable_feedback_prompts" => (
            Hive::CurrentUser,
            r"SOFTWARE\Microsoft\Siuf\Rules",
            vec![("NumberOfSIUFInPeriod", 0)],
        ),
        "explorer_open_to_this_pc" => (
            Hive::CurrentUser,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\Advanced",
            vec![("LaunchTo", 1)],
        ),
        "explorer_show_file_extensions" => (
            Hive::CurrentUser,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\Advanced",
            vec![("HideFileExt", 0)],
        ),
        "explorer_show_hidden_files" => (
            Hive::CurrentUser,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\Advanced",
            vec![("Hidden", 1)],
        ),
        "enable_ntfs_long_paths" => (
            Hive::LocalMachine,
            r"SYSTEM\CurrentControlSet\Control\FileSystem",
            vec![("LongPathsEnabled", 1)],
        ),
        "enable_developer_mode" => (
            Hive::LocalMachine,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock",
            vec![("AllowDevelopmentWithoutDevLicense", 1)],
        ),
        _ => return None,
    };
    Some(
        pairs
            .into_iter()
            .map(|(name, value)| RegistryChange {
                hive,
                path: path.into(),
                name: name.into(),
                value,
            })
            .collect(),
    )
}
fn required<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a Value> {
    object
        .get(key)
        .with_context(|| format!("Missing required source field: {key}"))
}
fn object<'a>(value: &'a Value, label: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .with_context(|| format!("Expected object at {label}"))
}
fn array<'a>(value: &'a Value, label: &str) -> Result<&'a Vec<Value>> {
    let values = value
        .as_array()
        .with_context(|| format!("Expected list at {label}"))?;
    ensure!(values.len() <= MAX_LIST, "Too many entries at {label}");
    Ok(values)
}
fn identifier<'a>(value: &'a Value, label: &str) -> Result<&'a str> {
    let text = value
        .as_str()
        .with_context(|| format!("Expected identifier string at {label}"))?;
    ensure!(
        !text.is_empty()
            && text.len() <= 256
            && text.trim() == text
            && !text.chars().any(|c| c.is_control() || c.is_whitespace()),
        "Invalid identifier at {label}"
    );
    Ok(text)
}
fn identifiers(value: &Value, label: &str) -> Result<Vec<String>> {
    let mut seen = BTreeSet::new();
    array(value, label)?
        .iter()
        .map(|value| {
            let id = identifier(value, label)?;
            ensure!(
                seen.insert(id.to_ascii_lowercase()),
                "Duplicate identifier at {label}"
            );
            Ok(id.into())
        })
        .collect()
}
fn check_shape(value: &Value, depth: usize, nodes: &mut usize) -> Result<()> {
    *nodes += 1;
    ensure!(
        depth <= 32 && *nodes <= MAX_NODES,
        "Migration document exceeds structural limits"
    );
    match value {
        Value::String(text) => ensure!(
            text.len() <= 16_384,
            "Migration string exceeds length limit"
        ),
        Value::Array(values) => {
            ensure!(
                values.len() <= MAX_LIST,
                "Migration list exceeds count limit"
            );
            for value in values {
                check_shape(value, depth + 1, nodes)?;
            }
        }
        Value::Object(values) => {
            ensure!(
                values.len() <= MAX_LIST,
                "Migration object exceeds count limit"
            );
            for (key, value) in values {
                ensure!(
                    !key.is_empty() && key.len() <= 256 && !key.chars().any(char::is_control),
                    "Invalid migration object key"
                );
                check_shape(value, depth + 1, nodes)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MacMappings {
    schema_version: u32,
    description: String,
    mappings: Vec<MacMapping>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MacMapping {
    source_kind: String,
    source: String,
    kind: String,
    package_id: String,
    detail: String,
    evidence: Vec<String>,
}
impl MacMappings {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == 1 && !self.description.is_empty() && self.mappings.len() <= 256,
            "Invalid embedded Mac mapping schema"
        );
        let mut seen = BTreeSet::new();
        for mapping in &self.mappings {
            ensure!(
                matches!(mapping.source_kind.as_str(), "formula" | "cask")
                    && matches!(mapping.kind.as_str(), "package" | "cargo" | "uv" | "vscode"),
                "Invalid embedded Mac mapping provider"
            );
            ensure!(
                seen.insert((&mapping.source_kind, &mapping.source))
                    && !mapping.source.is_empty()
                    && !mapping.package_id.is_empty()
                    && !mapping.detail.is_empty()
                    && !mapping.evidence.is_empty(),
                "Invalid or duplicate embedded Mac mapping"
            );
            ensure!(mapping.evidence.iter().all(|url|url.starts_with("https://") && !url.chars().any(char::is_whitespace)),"Invalid embedded Mac mapping evidence");
        }
        Ok(())
    }
}

/// serde_json::Value normally retains the last duplicate object key. This visitor
/// rejects duplicates at every depth before any migration decision is made.
struct UniqueValue(Value);
impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: de::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct UniqueVisitor;
        impl<'de> Visitor<'de> for UniqueVisitor {
            type Value = UniqueValue;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("bounded JSON")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Self::Value, E> {
                Ok(UniqueValue(Value::Bool(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Self::Value, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| UniqueValue(Value::Number(n)))
                    .ok_or_else(|| E::custom("invalid number"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Self::Value, E> {
                Ok(UniqueValue(Value::String(v.into())))
            }
            fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Self::Value, E> {
                Ok(UniqueValue(Value::String(v)))
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_none<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element::<UniqueValue>()? {
                    if values.len() >= MAX_LIST {
                        return Err(de::Error::custom("list limit"));
                    }
                    values.push(value.0);
                }
                Ok(UniqueValue(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut values = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if values.len() >= MAX_LIST || values.contains_key(&key) {
                        return Err(de::Error::custom("duplicate key or object limit"));
                    }
                    values.insert(key, map.next_value::<UniqueValue>()?.0);
                }
                Ok(UniqueValue(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(UniqueVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn profile() -> Value {
        serde_json::from_slice(include_bytes!("../../../examples/migration-profile.json")).unwrap()
    }
    fn analyze_value(value: &Value) -> Result<MigrationReport> {
        analyze(&serde_json::to_vec(value)?, Target::Win11Ltsc2024)
    }
    #[test]
    fn profile_accounts_for_every_provider_and_preserves_false_semantics() {
        let report = analyze_value(&profile()).unwrap();
        for origin in [
            "packages.winget_core",
            "packages.winget_dev",
            "packages.cargo_packages",
            "packages.uv_tools",
            "packages.npm_globals",
            "packages.pip_packages",
            "packages.scoop_tools",
            "vscode_config.extensions",
            "system_tweaks.disable_telemetry",
        ] {
            assert!(
                report.items.iter().any(|item| item.origin == origin),
                "{origin}"
            );
        }
        let false_item = report
            .items
            .iter()
            .find(|item| item.origin == "system_tweaks.disable_telemetry")
            .unwrap();
        assert_eq!(false_item.status, "manual");
        assert!(false_item.settings.is_empty());
        assert!(report.settings.contains("privacy-advertising-id"));
        assert!(report.settings.contains("explorer-extensions"));
    }
    #[test]
    fn storage_optimization_maps_exact_scope_and_preserves_false_and_trim() {
        for target in [Target::Win10Ltsc2021, Target::Win11Ltsc2024] {
            for enabled in [true, false] {
                let mut value = profile();
                value["system_tweaks"] =
                    serde_json::json!({"optimize_storage": enabled, "enable_trim": true});
                let report = analyze(&serde_json::to_vec(&value).unwrap(), target).unwrap();
                let optimization = report
                    .items
                    .iter()
                    .find(|item| item.origin == "system_tweaks.optimize_storage")
                    .unwrap();
                if enabled {
                    assert_eq!(optimization.status, "mapped");
                    assert_eq!(optimization.deployment, ["storage-optimize-all"]);
                    assert!(optimization.detail.contains("/C /O /U"));
                    assert!(optimization.detail.contains("including removable volumes"));
                } else {
                    assert_eq!(optimization.status, "manual");
                    assert!(optimization.deployment.is_empty());
                }
                let storage: Vec<_> = report
                    .deployment
                    .iter()
                    .filter(|id| id.starts_with("storage-"))
                    .map(String::as_str)
                    .collect();
                assert_eq!(
                    storage,
                    if enabled {
                        vec!["storage-optimize-all"]
                    } else {
                        vec![]
                    }
                );
                let trim = report
                    .items
                    .iter()
                    .find(|item| item.origin == "system_tweaks.enable_trim")
                    .unwrap();
                assert_eq!(trim.status, "manual");
                assert!(trim.settings.is_empty() && trim.deployment.is_empty());
                assert!(trim.detail.contains("NTFS and ReFS"));
                assert!(report.settings.is_empty());
            }
        }
        let mut value = profile();
        value["system_tweaks"]["optimize_storage"] = "true".into();
        assert!(analyze_value(&value).is_err());
    }

    #[test]
    fn wsl2_maps_both_components_atomically_not_a_distribution() {
        let report = analyze_value(&profile()).unwrap();
        let item = report
            .items
            .iter()
            .find(|item| item.origin == "windows_features.enable_wsl2")
            .unwrap();
        assert_eq!(item.status, "mapped");
        assert_eq!(item.deployment.len(), 2);
        assert!(item.deployment.contains(&"feature-wsl".into()));
        assert!(
            item.deployment
                .contains(&"feature-virtual-machine-platform".into())
        );
        assert!(item.detail.contains("does not install a distribution"));
    }
    #[test]
    fn private_and_unknown_values_never_appear_in_report() {
        let mut value = profile();
        value["git_config"]["user_email"] = "PRIVATE_TOKEN_123".into();
        value["custom-secret-key"] = serde_json::json!({"password":"PRIVATE_TOKEN_123"});
        value["system_tweaks"]["secret_value"] = "PRIVATE_TOKEN_123".into();
        let report = serde_json::to_string(&analyze_value(&value).unwrap()).unwrap();
        assert!(!report.contains("PRIVATE_TOKEN_123"));
        assert!(!report.contains("custom-secret-key"));
        assert!(report.contains("unrecognized field"));
    }
    #[test]
    fn rejects_duplicate_keys_lists_future_versions_and_malformed_types() {
        assert!(
            analyze(
                br#"{"profile_version":"2.2.0","profile_version":"2.2.0"}"#,
                Target::Win11Ltsc2024
            )
            .is_err()
        );
        let mut value = profile();
        value["packages"]["cargo_packages"] = serde_json::json!(["ripgrep", "RIPGREP"]);
        assert!(analyze_value(&value).is_err());
        let mut value = profile();
        value["profile_version"] = "99.0.0".into();
        assert!(analyze_value(&value).is_err());
        let mut value = profile();
        value["packages"]["uv_tools"] = "not-a-list".into();
        assert!(analyze_value(&value).is_err());
        let mut value = profile();
        value["system_tweaks"]["disable_advertising_id"] = "false".into();
        assert!(analyze_value(&value).is_err());
        assert!(analyze(&vec![b' '; MAX_BYTES + 1], Target::Win11Ltsc2024).is_err());
    }
    #[test]
    fn mac_inventory_reports_unmapped_provider_and_source_warnings() {
        let value = serde_json::json!({"schema_version":1,"captured_at":"synthetic","homebrew_formulae":["git","unmapped-formula"],"homebrew_casks":["firefox"],"homebrew_taps":["example/tools"],"cargo_packages":[],"npm_globals":["unmapped-npm"],"uv_tools":[],"vscode_extensions":[],"cursor_extensions":["example.extension"],"warnings":["PRIVATE_SOURCE_WARNING"]});
        let report = analyze_value(&value).unwrap();
        assert_eq!(report.source, "macos_inventory");
        assert!(report.items.iter().any(|item| item.requested == "Git.Git"
            || (item.requested == "git" && item.status == "mapped")));
        for requested in ["unmapped-formula", "unmapped-npm", "example.extension"] {
            assert!(
                report
                    .items
                    .iter()
                    .any(|item| item.requested == requested && item.status == "manual")
            );
        }
        assert!(
            !serde_json::to_string(&report)
                .unwrap()
                .contains("PRIVATE_SOURCE_WARNING")
        );
    }
    #[test]
    fn legacy_device_policies_map_exactly_and_false_never_requests_the_inverse() {
        let mut value = profile();
        value["system_tweaks"]["disable_consumer_features"] = true.into();
        value["system_tweaks"]["disable_telemetry"] = true.into();
        let report = analyze_value(&value).unwrap();
        let item = report
            .items
            .iter()
            .find(|item| item.origin == "system_tweaks.disable_consumer_features")
            .unwrap();
        assert_eq!(item.status, "mapped");
        assert_eq!(item.settings, vec!["experience-consumer-features-off"]);
        assert!(report.settings.contains("privacy-diagnostic-data-off"));
        for field in ["disable_consumer_features", "disable_telemetry"] {
            value["system_tweaks"][field] = false.into();
        }
        let skipped = analyze_value(&value).unwrap();
        assert!(!skipped.settings.contains("privacy-diagnostic-data-off"));
        assert!(
            !skipped
                .settings
                .contains("experience-consumer-features-off")
        );
    }

    #[test]
    fn npm_import_maps_only_trusted_exact_identities_for_profile_and_inventory() {
        let mut value = profile();
        value["packages"]["npm_globals"] = serde_json::json!([
            "typescript",
            "unknown-npm",
            "typescript@latest",
            "@unreviewed/tool"
        ]);
        let report = analyze_value(&value).unwrap();
        assert!(report.deployment.contains("developer-npm-typescript"));
        for requested in ["unknown-npm", "typescript@latest", "@unreviewed/tool"] {
            assert!(report.items.iter().any(|item| item.requested == requested
                && item.status == "manual"
                && item.deployment.is_empty()));
        }
        let inventory = serde_json::json!({"schema_version":1,"captured_at":"synthetic","homebrew_formulae":[],"homebrew_casks":[],"homebrew_taps":[],"cargo_packages":[],"uv_tools":[],"npm_globals":["prettier"],"vscode_extensions":[],"cursor_extensions":[],"warnings":[]});
        let inventory = analyze_value(&inventory).unwrap();
        assert_eq!(
            inventory.deployment,
            BTreeSet::from(["developer-npm-prettier".to_owned()])
        );
        assert!(inventory.settings.is_empty());
    }

    #[test]
    fn legacy_ultimate_setup_is_not_silently_replaced_by_active_scheme_choices() {
        let mut value = profile();
        for field in ["activate_ultimate_performance", "extreme_ac_power_settings"] {
            value["system_tweaks"][field] = true.into();
        }
        let report = analyze_value(&value).unwrap();
        for field in ["activate_ultimate_performance", "extreme_ac_power_settings"] {
            let origin = format!("system_tweaks.{field}");
            let item = report
                .items
                .iter()
                .find(|item| item.origin == origin)
                .unwrap();
            assert_eq!(item.status, "manual");
            assert!(item.settings.is_empty() && item.deployment.is_empty());
            assert!(item.detail.contains("active scheme"));
        }
    }

    #[test]
    fn nested_duplicate_keys_are_not_last_value_wins() {
        let bytes = br#"{"profile_version":"2.2.0","packages":{"cargo_packages":[],"cargo_packages":["ripgrep"]}}"#;
        let error = analyze(bytes, Target::Win11Ltsc2024)
            .unwrap_err()
            .to_string();
        assert!(error.contains("duplicate-key"));
        assert!(!error.contains("ripgrep"));
    }

    #[test]
    fn bounds_and_wrong_source_identity_are_errors_not_empty_reports() {
        let mut value = profile();
        value["packages"]["cargo_packages"] = serde_json::json!(["x".repeat(257)]);
        assert!(analyze_value(&value).is_err());
        let mut value = profile();
        value["packages"]["cargo_packages"] = serde_json::json!(vec!["x"; MAX_LIST + 1]);
        assert!(analyze_value(&value).is_err());
        let mut value = profile();
        value["schema_version"] = 1.into();
        assert!(analyze_value(&value).is_err());
        assert!(analyze(b"[]", Target::Win11Ltsc2024).is_err());
        assert!(analyze(b"not json", Target::Win11Ltsc2024).is_err());
    }
}
