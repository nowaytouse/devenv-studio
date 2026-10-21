//! Versioned, validated workspace documents shared by the native workbench.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fs::File, io::Read, path::Path};
use studio_core::{Catalog, Plan, Profile, Target};
use studio_deployment::DeploymentPlan;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceProfile {
    pub schema_version: u32,
    pub settings: Profile,
    #[serde(deserialize_with = "unique_deployment")]
    pub deployment: BTreeSet<String>,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspacePlan {
    pub schema_version: u32,
    pub settings: Plan,
    pub deployment: DeploymentPlan,
}

fn unique_deployment<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<BTreeSet<String>, D::Error> {
    let values = Vec::<String>::deserialize(d)?;
    let mut ids = BTreeSet::new();
    for value in values {
        if !ids.insert(value) {
            return Err(serde::de::Error::custom("Duplicate deployment task ID"));
        }
    }
    Ok(ids)
}

pub fn plan(
    catalog: &Catalog,
    target: Target,
    settings: &BTreeSet<String>,
    deployment: &BTreeSet<String>,
) -> Result<WorkspacePlan> {
    Ok(WorkspacePlan {
        schema_version: 2,
        settings: studio_core::build_plan(catalog, target, settings)?,
        deployment: studio_deployment::build_plan(target, deployment)?,
    })
}

pub fn save(
    catalog: &Catalog,
    target: Target,
    settings: &BTreeSet<String>,
    deployment: &BTreeSet<String>,
    path: &Path,
) -> Result<()> {
    plan(catalog, target, settings, deployment)?;
    studio_core::export_document(
        &WorkspaceProfile {
            schema_version: 2,
            settings: Profile {
                schema_version: studio_core::SCHEMA_VERSION,
                target,
                selected: settings.clone(),
            },
            deployment: deployment.clone(),
        },
        path,
    )
}

pub fn load(path: &Path, catalog: &Catalog) -> Result<WorkspaceProfile> {
    let mut bytes = Vec::new();
    File::open(path)
        .with_context(|| format!("Open profile {}", path.display()))?
        .take(studio_core::MAX_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= studio_core::MAX_INPUT_BYTES,
        "Profile exceeds 1 MiB limit"
    );
    parse(&bytes, catalog)
}

fn parse(bytes: &[u8], catalog: &Catalog) -> Result<WorkspaceProfile> {
    // Parse a strict struct rather than a generic map so duplicate JSON fields
    // and unknown keys remain errors, including in legacy documents.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Document {
        Current(WorkspaceProfile),
        Legacy(Profile),
    }
    let profile = match serde_json::from_slice::<Document>(bytes)
        .context("Invalid workspace profile JSON")?
    {
        Document::Current(profile) => {
            ensure!(
                profile.schema_version == 2,
                "Unsupported workspace profile schema"
            );
            profile
        }
        Document::Legacy(settings) => WorkspaceProfile {
            schema_version: 2,
            settings,
            deployment: BTreeSet::new(),
        },
    };
    ensure!(
        profile.settings.schema_version == studio_core::SCHEMA_VERSION,
        "Unsupported settings profile schema"
    );
    plan(
        catalog,
        profile.settings.target,
        &profile.settings.selected,
        &profile.deployment,
    )?;
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_profile_loads_with_empty_deployment() {
        let catalog = studio_core::load_catalog().unwrap();
        let legacy = Profile {
            schema_version: studio_core::SCHEMA_VERSION,
            target: Target::Win11Ltsc2024,
            selected: BTreeSet::new(),
        };
        let loaded = parse(&serde_json::to_vec(&legacy).unwrap(), &catalog).unwrap();
        assert!(loaded.deployment.is_empty());
        assert_eq!(loaded.settings, legacy);
    }

    #[test]
    fn profile_rejects_duplicate_unknown_and_wrong_schema() {
        let catalog = studio_core::load_catalog().unwrap();
        for document in [
            r#"{"schema_version":2,"settings":{"schema_version":1,"target":"win11_ltsc2024","selected":[]},"deployment":["same","same"]}"#,
            r#"{"schema_version":2,"settings":{"schema_version":1,"target":"win11_ltsc2024","selected":[]},"deployment":[],"surprise":true}"#,
            r#"{"schema_version":3,"settings":{"schema_version":1,"target":"win11_ltsc2024","selected":[]},"deployment":[]}"#,
        ] {
            assert!(parse(document.as_bytes(), &catalog).is_err());
        }
    }

    #[test]
    fn bundle_contains_validated_settings_and_deployment() {
        let catalog = studio_core::load_catalog().unwrap();
        let bundle = plan(
            &catalog,
            Target::Win11Ltsc2024,
            &BTreeSet::new(),
            &BTreeSet::new(),
        )
        .unwrap();
        let value = serde_json::to_value(bundle).unwrap();
        assert_eq!(value["schema_version"], 2);
        assert!(value["settings"]["actions"].is_array());
        assert!(value["deployment"]["tasks"].is_array());
        assert!(
            plan(
                &catalog,
                Target::Win11Ltsc2024,
                &BTreeSet::new(),
                &BTreeSet::from(["unknown-task".into()])
            )
            .is_err()
        );
    }
}
