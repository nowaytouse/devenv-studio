//! Review-only Windows suggestions from a captured macOS development inventory.
use anyhow::Result;
use serde_json::json;
use std::collections::BTreeSet;
use studio_core::Target;
use studio_deployment::backup::Snapshot;

use crate::migration::{MigrationItem, MigrationReport};

/// Versions are review metadata; only trusted catalog IDs are selectable.
pub fn analyze(snapshot: &Snapshot, target: Target) -> Result<MigrationReport> {
    studio_deployment::backup::validate(snapshot)?;
    if snapshot.platform == "windows" {
        return analyze_windows(snapshot, target);
    }
    let mut inventory = json!({
        "schema_version": 1, "captured_at": "backup source metadata",
        "homebrew_formulae": [], "homebrew_casks": [], "homebrew_taps": [],
        "cargo_packages": [], "uv_tools": [], "npm_globals": [],
        "vscode_extensions": [], "cursor_extensions": [], "warnings": []
    });
    let mut report = MigrationReport {
        source: "studio_macos_backup".into(),
        settings: BTreeSet::new(),
        deployment: BTreeSet::new(),
        items: Vec::new(),
    };
    for provider in &snapshot.providers {
        if provider.status != "complete" {
            continue;
        }
        let origin = field(&provider.kind);
        // Reuse bounded legacy analysis without truncating a valid larger backup.
        for packages in provider.packages.chunks(1_024) {
            inventory[origin] = json!(
                packages
                    .iter()
                    .map(|package| &package.name)
                    .collect::<Vec<_>>()
            );
            let mut batch = crate::migration::analyze(&serde_json::to_vec(&inventory)?, target)?;
            batch.items.retain(|item| item.origin == origin);
            report.settings.extend(batch.settings);
            report.deployment.extend(batch.deployment);
            report.items.extend(batch.items);
        }
        inventory[origin] = json!([]);
    }
    for provider in &snapshot.providers {
        if provider.status == "complete" {
            for package in &provider.packages {
                if let Some(item) = report.items.iter_mut().find(|item| {
                    item.origin == field(&provider.kind) && item.requested == package.name
                }) {
                    item.detail.push_str(&format!(
                        " Captured macOS version: {}. Source metadata only; no Windows version pin, binary or configuration is imported.", package.version
                    ));
                }
            }
        }
        let detail = match provider.status.as_str() {
            "complete" => format!("The backup captured {} package identities for this provider. An empty successful list requests no installation or removal. Captured versions describe the source only; suggestions require separate review.", provider.packages.len()),
            "unavailable" => "This provider was unavailable during capture. Installed state is unknown; no empty or partial list is interpreted as uninstalled software or a removal request. Original diagnostic text is withheld.".into(),
            _ => "This provider failed during capture. Installed state is unknown; no partial package list contributes suggestions or removal requests. Original diagnostic text is withheld.".into(),
        };
        report.items.push(MigrationItem {
            origin: format!("providers.{}", provider.kind),
            requested: provider.status.clone(),
            status: "manual".into(),
            detail,
            settings: Vec::new(),
            deployment: Vec::new(),
        });
    }
    if !snapshot.warnings.is_empty() {
        report.items.push(MigrationItem {
            origin: "warnings".into(), requested: "[value redacted]".into(), status: "manual".into(),
            detail: "The backup contains source warnings. Potentially private text is withheld; inspect the original local backup for capture limitations before using suggestions.".into(),
            settings: Vec::new(), deployment: Vec::new(),
        });
    }
    Ok(report)
}

fn analyze_windows(snapshot: &Snapshot, target: Target) -> Result<MigrationReport> {
    let catalog = studio_deployment::catalog()?;
    let mut report = MigrationReport {
        source: "studio_windows_backup".into(),
        settings: BTreeSet::new(),
        deployment: BTreeSet::new(),
        items: vec![],
    };
    for provider in &snapshot.providers {
        if provider.status == "complete" {
            for package in &provider.packages {
                let matched = catalog.tasks.iter().find(|task| {
                    task.kind == provider.kind
                        && task.targets.contains(&target)
                        && task.package_id.as_deref().is_some_and(|id| {
                            if task.kind == "pip" {
                                studio_deployment::normalize_pip_id(id)
                                    == studio_deployment::normalize_pip_id(&package.name)
                            } else {
                                id.eq_ignore_ascii_case(&package.name)
                            }
                        })
                });
                let deployment: Vec<_> = matched
                    .map(|task| vec![task.id.clone()])
                    .unwrap_or_default();
                report.deployment.extend(deployment.iter().cloned());
                report.items.push(MigrationItem {
                    origin:format!("providers.{}",provider.kind), requested:package.name.clone(),
                    status:if matched.is_some() { "mapped" } else { "manual" }.into(),
                    detail:format!("Captured Windows version: {}. Only reviewed catalog identities can be selected. Versions are evidence, not installation pins; unknown tools require manual review.",package.version),
                    settings:vec![], deployment,
                });
            }
        } else {
            report.items.push(MigrationItem { origin:format!("providers.{}",provider.kind),
                requested:provider.status.clone(), status:"manual".into(),
                detail:"This provider was not completely captured. Installed state is unknown; no installation or removal is inferred.".into(), settings:vec![],deployment:vec![] });
        }
    }
    Ok(report)
}

fn field(kind: &str) -> &str {
    match kind {
        "brew_formula" => "homebrew_formulae",
        "brew_cask" => "homebrew_casks",
        "cargo" => "cargo_packages",
        "uv" => "uv_tools",
        "npm" => "npm_globals",
        "vscode" => "vscode_extensions",
        _ => unreachable!("backup provider validation precedes mapping"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn windows_backup_uses_same_provider_identity_and_only_reviewed_target_tasks() {
        let mut snapshot: Snapshot = serde_json::from_value(source()).unwrap();
        snapshot.platform = "windows".into();
        snapshot.providers.retain(|p| !p.kind.starts_with("brew_"));
        snapshot
            .providers
            .push(studio_deployment::backup::ProviderSnapshot {
                kind: "pip".into(),
                status: "complete".into(),
                packages: vec![
                    studio_deployment::backup::Package {
                        name: "numpy".into(),
                        version: "2.3.4".into(),
                    },
                    studio_deployment::backup::Package {
                        name: "unknown-distribution".into(),
                        version: "1.0".into(),
                    },
                ],
                detail: "Collected installed package identities and versions.".into(),
            });
        let report = analyze(&snapshot, Target::Win11Ltsc2024).unwrap();
        assert_eq!(report.source, "studio_windows_backup");
        assert!(report.deployment.contains("developer-pip-numpy"));
        assert!(
            report
                .items
                .iter()
                .any(|i| i.requested == "unknown-distribution"
                    && i.status == "manual"
                    && i.deployment.is_empty())
        );
        assert!(
            report
                .items
                .iter()
                .any(|i| i.requested == "numpy" && i.detail.contains("2.3.4"))
        );
        assert!(report.settings.is_empty());
        snapshot.providers.last_mut().unwrap().status = "failed".into();
        snapshot.providers.last_mut().unwrap().detail =
            "Some entries were omitted because their identities or versions are unsupported."
                .into();
        assert!(
            !analyze(&snapshot, Target::Win11Ltsc2024)
                .unwrap()
                .deployment
                .contains("developer-pip-numpy")
        );
    }

    fn source() -> Value {
        json!({
            "schema_version": 1, "captured_at": "1792700000", "platform": "macos",
            "providers": [
                {"kind":"brew_formula","status":"complete","packages":[{"name":"git","version":"2.51.0"}],"detail":"Collected installed package identities and versions."},
                {"kind":"brew_cask","status":"complete","packages":[{"name":"firefox","version":"144.0"}],"detail":"Collected installed package identities and versions."},
                {"kind":"cargo","status":"complete","packages":[{"name":"ripgrep","version":"14.1.1"}],"detail":"Collected installed package identities and versions."},
                {"kind":"uv","status":"complete","packages":[{"name":"ruff","version":"0.13.0"}],"detail":"Collected installed package identities and versions."},
                {"kind":"npm","status":"complete","packages":[{"name":"typescript","version":"5.9.2"}],"detail":"Collected installed package identities and versions."},
                {"kind":"vscode","status":"complete","packages":[{"name":"rust-lang.rust-analyzer","version":"0.3.2657"}],"detail":"Collected installed package identities and versions."}
            ], "warnings": ["Inventory only: tool binaries, credentials, configuration, source code and virtual environments are not included."]
        })
    }
    fn review(value: Value) -> Result<MigrationReport> {
        analyze(&serde_json::from_value(value)?, Target::Win11Ltsc2024)
    }

    #[test]
    fn complete_providers_reuse_trusted_mappings_and_retain_source_versions_only() {
        let report = review(source()).unwrap();
        assert_eq!(report.source, "studio_macos_backup");
        assert!(report.settings.is_empty());
        assert_eq!(report.deployment.len(), 6);
        for id in [
            "developer-cargo-ripgrep",
            "developer-uv-ruff",
            "developer-npm-typescript",
            "developer-vscode-rust-lang-rust-analyzer",
        ] {
            assert!(report.deployment.contains(id));
        }
        let git = report
            .items
            .iter()
            .find(|item| item.origin == "homebrew_formulae" && item.requested == "git")
            .unwrap();
        assert_eq!(git.status, "mapped");
        assert!(git.detail.contains("2.51.0"));
        assert!(git.detail.contains("no Windows version pin"));
        let serialized = serde_json::to_string(&report).unwrap();
        assert!(!serialized.contains("PRIVATE_SOURCE_DETAIL"));
        assert!(!serialized.contains("PRIVATE_SOURCE_WARNING"));
        assert!(report.deployment.iter().all(|id| !id.contains("2.51.0")));
    }

    #[test]
    fn unavailable_capture_is_unknown_not_uninstalled_or_a_guess() {
        let mut value = source();
        for provider in value["providers"].as_array_mut().unwrap() {
            provider["status"] = json!("unavailable");
            provider["packages"] = json!([]);
            provider["detail"] = json!("Tool not found in supported locations.");
        }
        let report = review(value).unwrap();
        assert!(report.deployment.is_empty() && report.settings.is_empty());
        assert_eq!(report.items.len(), 7);
        assert!(
            report
                .items
                .iter()
                .filter(|item| item.origin.starts_with("providers."))
                .all(|item| item.detail.contains("Installed state is unknown")
                    && item.status == "manual")
        );
    }

    #[test]
    fn empty_success_remains_valid_without_inventing_software() {
        let mut value = source();
        for provider in value["providers"].as_array_mut().unwrap() {
            provider["packages"] = json!([]);
        }
        let report = review(value).unwrap();
        assert!(report.deployment.is_empty());
        assert_eq!(
            report
                .items
                .iter()
                .filter(|item| item.origin.starts_with("providers."))
                .count(),
            6
        );
    }

    #[test]
    fn failed_provider_contributes_no_mapping() {
        let mut value = source();
        value["providers"][0]["status"] = json!("failed");
        value["providers"][0]["detail"] = json!(
            "Some entries were omitted because their identities or versions are unsupported."
        );
        let report = review(value).unwrap();
        assert!(!report.items.iter().any(|item| item.requested == "git"));
        assert!(
            report
                .items
                .iter()
                .any(|item| item.origin == "providers.brew_formula"
                    && item.requested == "failed"
                    && item.detail.contains("no partial package list"))
        );
        assert_eq!(report.deployment.len(), 5);
    }

    #[test]
    fn invalid_snapshot_and_version_injection_are_rejected() {
        let mut value = source();
        value["schema_version"] = json!(2);
        assert!(review(value).is_err());
        let mut value = source();
        value["providers"][0]["packages"][0]["version"] = json!("1;$(PRIVATE_COMMAND)");
        assert!(review(value).is_err());
        let mut value = source();
        value["providers"][0]["kind"] = json!("arbitrary_command");
        assert!(review(value).is_err());
    }

    #[test]
    fn larger_valid_inventory_is_mapped_in_full_without_legacy_list_truncation() {
        let mut value = source();
        value["providers"][0]["packages"] = json!(
            (0..1_025)
                .map(|index| json!({"name": format!("unmapped-{index}"), "version": "1.0"}))
                .collect::<Vec<_>>()
        );
        let report = review(value).unwrap();
        let items: Vec<_> = report
            .items
            .iter()
            .filter(|item| item.origin == "homebrew_formulae")
            .collect();
        assert_eq!(items.len(), 1_025);
        assert!(
            items.iter().all(|item| item.status == "manual"
                && item.detail.contains("Captured macOS version: 1.0"))
        );
        assert!(items.iter().any(|item| item.requested == "unmapped-1024"));
    }

    #[test]
    fn imported_diagnostics_cannot_expose_private_data_through_review() {
        for field in ["detail", "warning"] {
            let mut value = source();
            if field == "detail" {
                value["providers"][0]["detail"] = json!("PRIVATE_SOURCE_DETAIL");
            } else {
                value["warnings"] = json!(["PRIVATE_SOURCE_WARNING"]);
            }
            let error = review(value).unwrap_err().to_string();
            assert!(!error.contains("PRIVATE_SOURCE"));
        }
    }

    #[test]
    fn existing_migration_entry_accepts_only_valid_native_backup_source() {
        let report = crate::migration::analyze(
            &serde_json::to_vec(&source()).unwrap(),
            Target::Win10Ltsc2021,
        )
        .unwrap();
        assert_eq!(report.source, "studio_macos_backup");
        assert_eq!(report.deployment.len(), 6);
        let mut invalid = source();
        invalid["platform"] = json!("windows");
        assert!(
            crate::migration::analyze(
                &serde_json::to_vec(&invalid).unwrap(),
                Target::Win10Ltsc2021
            )
            .is_err()
        );
        let duplicate = br#"{"schema_version":1,"platform":"macos","providers":[],"providers":[]}"#;
        assert!(crate::migration::analyze(duplicate, Target::Win10Ltsc2021).is_err());
    }
}
