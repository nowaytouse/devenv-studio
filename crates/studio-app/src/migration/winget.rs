//! WinGet exports supply identities for review, never executable import requests.
use super::*;
use std::collections::BTreeMap;

pub(super) const SCHEMA: &str = "https://aka.ms/winget-packages.schema.2.0.json";

const UNTRUSTED_SOURCE: &str = "The source is not the fixed Microsoft community source or contains unsupported metadata. Source values were not imported.";
const DUPLICATE: &str = "Multiple entries request this package identity. Repeated exports and version conflicts require manual review; no entry was selected automatically.";
const OPTIONS: &str = "This package contains unsupported channel, scope, installer arguments or additional properties. Their values were not imported.";
const VERSION_NOTICE: &str =
    "Version metadata does not pin installation. Only the current trusted task is suggested.";

struct Entry {
    origin: String,
    identity: String,
    version: Option<String>,
    trusted_source: bool,
    unsupported_options: bool,
}

impl Analyzer {
    pub(super) fn winget(&mut self, root: &Map<String, Value>) -> Result<()> {
        ensure!(
            root.keys().all(
                |key| ["$schema", "Sources", "WinGetVersion", "CreationDate"]
                    .contains(&key.as_str())
            ),
            "Unsupported top-level WinGet export properties"
        );
        ensure!(
            root.get("$schema").and_then(Value::as_str) == Some(SCHEMA),
            "Only the exact WinGet export schema 2.0 is supported"
        );
        for key in ["WinGetVersion", "CreationDate"] {
            optional_string(root, key)?;
        }
        let sources = array(required(root, "Sources")?, "Sources")?;
        if sources.is_empty() {
            self.empty("Sources");
            return Ok(());
        }
        let mut entries = Vec::new();
        for (source_index, value) in sources.iter().enumerate() {
            let source = object(value, "WinGet source")?;
            let details = object(required(source, "SourceDetails")?, "SourceDetails")?;
            let expected = [
                ("Name", "winget"),
                ("Identifier", "Microsoft.Winget.Source_8wekyb3d8bbwe"),
                ("Argument", "https://cdn.winget.microsoft.com/cache"),
                ("Type", "Microsoft.PreIndexed.Package"),
            ];
            let mut trusted_source = source
                .keys()
                .all(|key| ["SourceDetails", "Packages"].contains(&key.as_str()))
                && details
                    .keys()
                    .all(|key| expected.iter().any(|(name, _)| key == name));
            for (name, expected) in expected {
                let value = required(details, name)?
                    .as_str()
                    .with_context(|| format!("Expected source string at {name}"))?;
                trusted_source &= value == expected;
            }
            let packages = array(required(source, "Packages")?, "Packages")?;
            ensure!(
                entries.len() + packages.len() <= MAX_ITEMS,
                "Too many WinGet package entries"
            );
            if packages.is_empty() {
                if trusted_source {
                    self.empty(&format!("Sources[{source_index}].Packages"));
                } else {
                    self.item(
                        &format!("Sources[{source_index}]"),
                        "(unsupported source)",
                        "manual",
                        UNTRUSTED_SOURCE,
                        vec![],
                        vec![],
                    );
                }
            }
            for (package_index, value) in packages.iter().enumerate() {
                let package = object(value, "WinGet package")?;
                let identity =
                    identifier(required(package, "PackageIdentifier")?, "PackageIdentifier")?;
                ensure!(
                    identity.len() <= 100
                        && identity
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.+".contains(&byte)),
                    "WinGet package identity contains unsupported characters"
                );
                let identity = identity.to_owned();
                let version = optional_string(package, "Version")?
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned);
                let mut unsupported_options = package.keys().any(|key| {
                    ![
                        "PackageIdentifier",
                        "Version",
                        "Channel",
                        "Scope",
                        "InitialOverrideArguments",
                        "InitialCustomSwitches",
                    ]
                    .contains(&key.as_str())
                });
                for key in [
                    "Channel",
                    "Scope",
                    "InitialOverrideArguments",
                    "InitialCustomSwitches",
                ] {
                    unsupported_options |=
                        optional_string(package, key)?.is_some_and(|value| !value.is_empty());
                }
                entries.push(Entry {
                    origin: format!("Sources[{source_index}].Packages[{package_index}]"),
                    identity,
                    version,
                    trusted_source,
                    unsupported_options,
                });
            }
        }
        let mut identities = BTreeMap::new();
        for entry in &entries {
            *identities
                .entry(entry.identity.to_ascii_lowercase())
                .or_insert(0usize) += 1;
        }
        for entry in entries {
            let mut reasons = Vec::new();
            if !entry.trusted_source {
                reasons.push(UNTRUSTED_SOURCE);
            }
            if identities[&entry.identity.to_ascii_lowercase()] > 1 {
                reasons.push(DUPLICATE);
            }
            if entry.unsupported_options {
                reasons.push(OPTIONS);
            }
            let blocked = !reasons.is_empty();
            let mut detail = reasons.join(" ");
            if let Some(version) = &entry.version {
                if !detail.is_empty() {
                    detail.push(' ');
                }
                if safe_version(version) {
                    detail.push_str(&format!("Exported version: {version}. "));
                } else {
                    detail.push_str("Exported version metadata is present; its value is hidden. ");
                }
            } else if !detail.is_empty() {
                detail.push(' ');
            }
            detail.push_str(VERSION_NOTICE);
            if blocked {
                self.item(
                    &entry.origin,
                    &entry.identity,
                    "manual",
                    &detail,
                    vec![],
                    vec![],
                );
            } else {
                let row = self.report.items.len();
                self.package(
                    &entry.origin,
                    &entry.identity,
                    "package",
                    &entry.identity,
                    &detail,
                );
                let item = self
                    .report
                    .items
                    .get_mut(row)
                    .context("WinGet mapping did not produce a review row")?;
                if item.status != "mapped" {
                    item.detail.push(' ');
                    item.detail.push_str(&detail);
                }
            }
        }
        Ok(())
    }
}

fn optional_string<'a>(object: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>> {
    object
        .get(key)
        .map(|value| {
            value
                .as_str()
                .with_context(|| format!("Expected string at {key}"))
        })
        .transpose()
}

fn safe_version(version: &str) -> bool {
    version.len() <= 128
        && version.as_bytes().first().is_some_and(u8::is_ascii_digit)
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn document(packages: Value) -> Value {
        json!({
            "$schema": SCHEMA,
            "WinGetVersion": "1.12.350",
            "CreationDate": "2026-11-01T00:00:00Z",
            "Sources": [{
                "SourceDetails": {
                    "Name": "winget",
                    "Identifier": "Microsoft.Winget.Source_8wekyb3d8bbwe",
                    "Argument": "https://cdn.winget.microsoft.com/cache",
                    "Type": "Microsoft.PreIndexed.Package"
                },
                "Packages": packages
            }]
        })
    }

    fn review(value: &Value, target: Target) -> Result<MigrationReport> {
        analyze(&serde_json::to_vec(value)?, target)
    }

    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Result<Self> {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "studio-winget-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).context("Create isolated WinGet test directory")?;
            Ok(Self(path))
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn native_exports_roundtrip_only_trusted_package_identities_for_both_ltsc_targets() -> Result<()>
    {
        let directory = Directory::new()?;
        let catalog = studio_deployment::catalog()?;
        let selected = catalog
            .tasks
            .iter()
            .filter(|task| {
                task.kind == "package"
                    && [Target::Win10Ltsc2021, Target::Win11Ltsc2024]
                        .iter()
                        .all(|target| task.targets.contains(target))
            })
            .take(3)
            .map(|task| task.id.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(selected.len(), 3);
        for (index, target) in [Target::Win10Ltsc2021, Target::Win11Ltsc2024]
            .into_iter()
            .enumerate()
        {
            let plan = studio_deployment::build_plan(target, &selected)?;
            let path = directory.0.join(format!("export-{index}.json"));
            studio_deployment::environment::export_winget(&plan, &path)?;
            let report = load(&path, target)?;
            assert_eq!(report.source, "winget_export");
            assert_eq!(report.deployment, selected);
            assert!(report.settings.is_empty());
            assert_eq!(report.items.len(), 3);
            assert!(report.items.iter().all(|item| item.status == "mapped"
                && item.detail.contains(VERSION_NOTICE)
                && item.settings.is_empty()));
        }
        Ok(())
    }

    #[test]
    fn synthetic_example_maps_only_plain_official_catalog_identities() -> Result<()> {
        let report = analyze(
            include_bytes!("../../../../examples/winget-export.json"),
            Target::Win11Ltsc2024,
        )?;
        assert_eq!(report.items.len(), 6);
        assert_eq!(
            report
                .items
                .iter()
                .filter(|item| item.status == "mapped")
                .count(),
            3
        );
        assert_eq!(
            report
                .items
                .iter()
                .filter(|item| item.status == "manual")
                .count(),
            3
        );
        assert_eq!(report.deployment.len(), 3);
        assert!(!serde_json::to_string(&report)?.contains("packages.example.invalid"));
        Ok(())
    }

    #[test]
    fn mixed_sources_and_unknown_packages_remain_visible_without_private_source_values()
    -> Result<()> {
        let mut value = document(json!([
            {"PackageIdentifier":"Git.Git"},
            {"PackageIdentifier":"Unreviewed.Package","Version":"7.0"}
        ]));
        let mut custom = value["Sources"][0].clone();
        custom["SourceDetails"]["Name"] = json!("PRIVATE_SOURCE_NAME");
        custom["SourceDetails"]["Argument"] =
            json!("https://private.example/?token=PRIVATE_SOURCE_TOKEN");
        custom["Packages"] = json!([{"PackageIdentifier":"Microsoft.VisualStudioCode"}]);
        let mut store = custom.clone();
        store["SourceDetails"]["Name"] = json!("msstore");
        store["SourceDetails"]["Identifier"] = json!("StoreEdgeFD");
        store["Packages"] = json!([{"PackageIdentifier":"7zip.7zip"}]);
        value["Sources"]
            .as_array_mut()
            .unwrap()
            .extend([custom, store]);
        let report = review(&value, Target::Win11Ltsc2024)?;
        assert_eq!(report.items.len(), 4);
        assert_eq!(
            report
                .items
                .iter()
                .filter(|item| item.status == "mapped")
                .count(),
            1
        );
        assert_eq!(
            report
                .items
                .iter()
                .filter(|item| item.status == "manual")
                .count(),
            3
        );
        assert_eq!(report.deployment.len(), 1);
        let serialized = serde_json::to_string(&report)?;
        assert!(!serialized.contains("PRIVATE_SOURCE") && !serialized.contains("private.example"));
        assert!(
            report
                .items
                .iter()
                .any(|item| item.requested == "Unreviewed.Package" && item.deployment.is_empty())
        );
        assert!(report.items[1].detail.contains("Exported version: 7.0."));
        assert!(report.items[1].detail.contains(VERSION_NOTICE));
        value["Sources"][0]["Packages"][1]["Version"] =
            json!("https://private.example/PRIVATE_UNKNOWN_VERSION");
        let report = review(&value, Target::Win11Ltsc2024)?;
        assert!(report.items[1].detail.contains("its value is hidden"));
        assert!(report.items[1].detail.contains(VERSION_NOTICE));
        assert!(!serde_json::to_string(&report)?.contains("PRIVATE_UNKNOWN_VERSION"));
        Ok(())
    }

    #[test]
    fn unsupported_package_options_block_only_their_row_and_empty_native_options_are_harmless()
    -> Result<()> {
        for key in [
            "Channel",
            "Scope",
            "InitialOverrideArguments",
            "InitialCustomSwitches",
            "UnknownProperty",
        ] {
            let mut value = document(json!([
                {"PackageIdentifier":"Git.Git"},
                {"PackageIdentifier":"Microsoft.VisualStudioCode"}
            ]));
            value["Sources"][0]["Packages"][0][key] = json!("PRIVATE_OPTION_VALUE");
            let report = review(&value, Target::Win10Ltsc2021)?;
            assert_eq!(report.items[0].status, "manual", "{key}");
            assert!(report.items[0].detail.contains(OPTIONS), "{key}");
            assert!(report.items[0].deployment.is_empty());
            assert_eq!(report.items[1].status, "mapped");
            assert!(!serde_json::to_string(&report)?.contains("PRIVATE_OPTION_VALUE"));
            value["Sources"][0]["Packages"][0][key] = json!("");
            let report = review(&value, Target::Win10Ltsc2021)?;
            assert_eq!(
                report.items[0].status,
                if key == "UnknownProperty" {
                    "manual"
                } else {
                    "mapped"
                }
            );
        }
        Ok(())
    }

    #[test]
    fn repeated_export_versions_and_conflicting_sources_are_manual_not_invalid_json() -> Result<()>
    {
        let mut value = document(json!([
            {"PackageIdentifier":"Git.Git","Version":"2.0"},
            {"PackageIdentifier":"Git.Git","Version":"3.0"},
            {"PackageIdentifier":"Microsoft.VisualStudioCode","Version":"https://private.example/PRIVATE_VERSION"}
        ]));
        let mut another_source = value["Sources"][0].clone();
        another_source["SourceDetails"]["Argument"] =
            json!("https://private.example/PRIVATE_SOURCE");
        another_source["Packages"] = json!([{"PackageIdentifier":"git.git","Version":"4.0"}]);
        value["Sources"]
            .as_array_mut()
            .unwrap()
            .push(another_source);
        let report = review(&value, Target::Win11Ltsc2024)?;
        assert_eq!(report.items.len(), 4);
        for index in [0, 1, 3] {
            assert_eq!(report.items[index].status, "manual");
            assert!(report.items[index].detail.contains(DUPLICATE));
            assert!(report.items[index].deployment.is_empty());
        }
        assert_eq!(report.items[2].status, "mapped");
        assert!(report.items[2].detail.contains("its value is hidden"));
        assert!(report.items[0].detail.contains("Exported version: 2.0."));
        assert_eq!(report.deployment.len(), 1);
        assert!(!serde_json::to_string(&report)?.contains("PRIVATE_"));
        Ok(())
    }

    #[test]
    fn malformed_mixed_or_oversized_exports_fail_without_last_key_wins() {
        let valid = document(json!([{"PackageIdentifier":"Git.Git"}]));
        for schema in [
            Value::Null,
            json!("https://aka.ms/winget-packages.schema.1.0.json"),
            json!("https://untrusted.example/schema"),
        ] {
            let mut value = valid.clone();
            value["$schema"] = schema;
            assert!(review(&value, Target::Win11Ltsc2024).is_err());
        }
        for marker in ["profile_version", "schema_version", "platform", "providers"] {
            let mut value = valid.clone();
            value[marker] = json!(1);
            assert!(review(&value, Target::Win11Ltsc2024).is_err(), "{marker}");
        }
        for identity in [
            "https://user:PRIVATE_IDENTITY@private.example/path",
            "/Users/PRIVATE_IDENTITY/tool",
            "Git.Git --PRIVATE_IDENTITY",
        ] {
            let mut value = valid.clone();
            value["Sources"][0]["Packages"][0]["PackageIdentifier"] = json!(identity);
            let error = review(&value, Target::Win11Ltsc2024)
                .unwrap_err()
                .to_string();
            assert!(!error.contains("PRIVATE_IDENTITY"));
        }
        let mut unknown_top = valid.clone();
        unknown_top["PRIVATE_UNKNOWN_TOP"] = json!("PRIVATE_VALUE");
        let error = review(&unknown_top, Target::Win11Ltsc2024)
            .unwrap_err()
            .to_string();
        assert!(!error.contains("PRIVATE_"));
        for field in [
            "PackageIdentifier",
            "Version",
            "Channel",
            "Scope",
            "InitialOverrideArguments",
            "InitialCustomSwitches",
        ] {
            let mut value = valid.clone();
            value["Sources"][0]["Packages"][0][field] = json!(42);
            assert!(review(&value, Target::Win11Ltsc2024).is_err(), "{field}");
        }
        let text = serde_json::to_string(&valid).unwrap().replace(
            "\"PackageIdentifier\":\"Git.Git\"",
            "\"PackageIdentifier\":\"Git.Git\",\"PackageIdentifier\":\"PRIVATE_DUPLICATE\"",
        );
        let error = analyze(text.as_bytes(), Target::Win11Ltsc2024)
            .unwrap_err()
            .to_string();
        assert!(error.contains("duplicate-key") && !error.contains("PRIVATE_DUPLICATE"));

        assert!(analyze(&vec![b' '; MAX_BYTES + 1], Target::Win11Ltsc2024).is_err());
        let mut too_many_sources = valid.clone();
        too_many_sources["Sources"] = Value::Array(vec![valid["Sources"][0].clone(); MAX_LIST + 1]);
        assert!(review(&too_many_sources, Target::Win11Ltsc2024).is_err());
        let too_many_packages = document(Value::Array(vec![
            json!({"PackageIdentifier":"Git.Git"});
            MAX_LIST + 1
        ]));
        assert!(review(&too_many_packages, Target::Win11Ltsc2024).is_err());
        let mut too_many_total = document(Value::Array(vec![
            json!({"PackageIdentifier":"Git.Git"});
            1000
        ]));
        too_many_total["Sources"] = Value::Array(vec![too_many_total["Sources"][0].clone(); 6]);
        let error = review(&too_many_total, Target::Win11Ltsc2024)
            .unwrap_err()
            .to_string();
        assert!(error.contains("Too many WinGet package entries"));
    }

    #[test]
    fn source_extensions_and_empty_lists_are_explicit_review_notes() -> Result<()> {
        for location in ["source", "details"] {
            let mut value = document(json!([{"PackageIdentifier":"Git.Git"}]));
            if location == "source" {
                value["Sources"][0]["PRIVATE_FIELD"] = json!("PRIVATE_VALUE");
            } else {
                value["Sources"][0]["SourceDetails"]["PRIVATE_FIELD"] = json!("PRIVATE_VALUE");
            }
            let report = review(&value, Target::Win11Ltsc2024)?;
            assert_eq!(report.items[0].status, "manual");
            assert!(report.deployment.is_empty());
            assert!(!serde_json::to_string(&report)?.contains("PRIVATE_"));
        }
        let mut empty = document(json!([]));
        let report = review(&empty, Target::Win11Ltsc2024)?;
        assert!(!report.items.is_empty() && report.deployment.is_empty());
        empty["Sources"] = json!([]);
        let report = review(&empty, Target::Win11Ltsc2024)?;
        assert!(!report.items.is_empty() && report.deployment.is_empty());
        Ok(())
    }

    #[test]
    fn ambiguous_catalog_identity_and_wrong_target_never_become_suggestions() -> Result<()> {
        let value = document(json!([{"PackageIdentifier":"Git.Git","Version":"2.0"}]));
        let mut analyzer = Analyzer {
            target: Target::Win11Ltsc2024,
            settings_catalog: studio_core::load_catalog()?,
            deployment_catalog: studio_deployment::catalog()?,
            report: MigrationReport {
                source: "winget_export".into(),
                settings: BTreeSet::new(),
                deployment: BTreeSet::new(),
                items: Vec::new(),
            },
        };
        let task = analyzer
            .deployment_catalog
            .tasks
            .iter_mut()
            .find(|task| task.kind == "package" && task.package_id.as_deref() == Some("Git.Git"))
            .unwrap();
        task.targets = vec![Target::Win10Ltsc2021];
        let duplicate = task.clone();
        analyzer.winget(value.as_object().unwrap())?;
        assert_eq!(analyzer.report.items[0].status, "unsupported");
        assert!(
            analyzer.report.items[0]
                .detail
                .contains("Exported version: 2.0.")
        );
        assert!(analyzer.report.items[0].detail.contains(VERSION_NOTICE));
        assert!(analyzer.report.deployment.is_empty());
        analyzer.report.items.clear();
        analyzer.deployment_catalog.tasks.push(duplicate);
        analyzer.winget(value.as_object().unwrap())?;
        assert_eq!(analyzer.report.items[0].status, "manual");
        assert!(
            analyzer.report.items[0]
                .detail
                .contains("Exported version: 2.0.")
        );
        assert!(analyzer.report.items[0].detail.contains(VERSION_NOTICE));
        assert!(analyzer.report.deployment.is_empty());
        Ok(())
    }

    #[test]
    fn file_loader_rejects_links_and_non_regular_files_before_reading() -> Result<()> {
        let directory = Directory::new()?;
        let path = directory.0.join("export.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&document(json!([{"PackageIdentifier":"Git.Git"}])))?,
        )
        .context("Write WinGet loader test input")?;
        assert!(load(&path, Target::Win11Ltsc2024).is_ok());
        assert!(load(&directory.0, Target::Win11Ltsc2024).is_err());
        let oversized = directory.0.join("oversized.json");
        std::fs::File::create(&oversized)
            .context("Create oversized WinGet test input")?
            .set_len(MAX_BYTES as u64 + 1)
            .context("Set oversized WinGet test length")?;
        assert!(load(&oversized, Target::Win11Ltsc2024).is_err());
        #[cfg(unix)]
        {
            let link = directory.0.join("linked.json");
            std::os::unix::fs::symlink(&path, &link).context("Create WinGet test symlink")?;
            assert!(load(&link, Target::Win11Ltsc2024).is_err());
            let socket = directory.0.join("socket");
            let _listener = std::os::unix::net::UnixListener::bind(&socket)
                .context("Bind WinGet test socket")?;
            assert!(load(&socket, Target::Win11Ltsc2024).is_err());
            assert!(load(Path::new("/dev/null"), Target::Win11Ltsc2024).is_err());
        }
        Ok(())
    }
}
