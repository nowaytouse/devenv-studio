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
    #[serde(default, deserialize_with = "unique_deployment")]
    pub power: BTreeSet<String>,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspacePlan {
    pub schema_version: u32,
    pub settings: Plan,
    pub deployment: DeploymentPlan,
    pub power: studio_platform::power::PowerPlan,
}

fn unique_deployment<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<BTreeSet<String>, D::Error> {
    let values = Vec::<String>::deserialize(d)?;
    let mut ids = BTreeSet::new();
    for value in values {
        if !ids.insert(value) {
            return Err(serde::de::Error::custom("Duplicate workspace selection ID"));
        }
    }
    Ok(ids)
}

pub fn plan(
    catalog: &Catalog,
    target: Target,
    settings: &BTreeSet<String>,
    deployment: &BTreeSet<String>,
    power: &BTreeSet<String>,
) -> Result<WorkspacePlan> {
    Ok(WorkspacePlan {
        schema_version: 3,
        settings: studio_core::build_plan(catalog, target, settings)?,
        deployment: studio_deployment::build_plan(target, deployment)?,
        power: studio_platform::power::build_plan(target, power)?,
    })
}

pub fn save(
    catalog: &Catalog,
    target: Target,
    settings: &BTreeSet<String>,
    deployment: &BTreeSet<String>,
    power: &BTreeSet<String>,
    path: &Path,
) -> Result<()> {
    plan(catalog, target, settings, deployment, power)?;
    studio_core::export_document(
        &WorkspaceProfile {
            schema_version: 3,
            settings: Profile {
                schema_version: studio_core::SCHEMA_VERSION,
                target,
                selected: settings.clone(),
            },
            deployment: deployment.clone(),
            power: power.clone(),
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
                matches!(profile.schema_version, 2 | 3),
                "Unsupported workspace profile schema"
            );
            ensure!(
                profile.schema_version == 3 || profile.power.is_empty(),
                "Power choices require workspace profile schema 3"
            );
            profile
        }
        Document::Legacy(settings) => WorkspaceProfile {
            schema_version: 3,
            settings,
            deployment: BTreeSet::new(),
            power: BTreeSet::new(),
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
        &profile.power,
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
        assert!(loaded.power.is_empty());
        assert_eq!(loaded.settings, legacy);
    }

    #[test]
    fn profile_rejects_duplicate_unknown_and_wrong_schema() {
        let catalog = studio_core::load_catalog().unwrap();
        for document in [
            r#"{"schema_version":2,"settings":{"schema_version":1,"target":"win11_ltsc2024","selected":[]},"deployment":["same","same"]}"#,
            r#"{"schema_version":2,"settings":{"schema_version":1,"target":"win11_ltsc2024","selected":[]},"deployment":[],"surprise":true}"#,
            r#"{"schema_version":4,"settings":{"schema_version":1,"target":"win11_ltsc2024","selected":[]},"deployment":[]}"#,
        ] {
            assert!(parse(document.as_bytes(), &catalog).is_err());
        }
    }

    #[test]
    fn v2_profiles_never_gain_power_and_v3_rejects_untrusted_or_duplicate_choices() {
        let catalog = studio_core::load_catalog().unwrap();
        let legacy = include_bytes!("../../../examples/web-workspace.json");
        assert!(parse(legacy, &catalog).unwrap().power.is_empty());
        let choice = studio_platform::power::catalog().unwrap().remove(0).id;
        let mut value: serde_json::Value = serde_json::from_slice(legacy).unwrap();
        value["power"] = serde_json::json!([choice]);
        assert!(parse(&serde_json::to_vec(&value).unwrap(), &catalog).is_err());
        value["schema_version"] = 3.into();
        let loaded = parse(&serde_json::to_vec(&value).unwrap(), &catalog).unwrap();
        assert_eq!(loaded.power, BTreeSet::from([choice.clone()]));
        let bundle = plan(
            &catalog,
            loaded.settings.target,
            &loaded.settings.selected,
            &loaded.deployment,
            &loaded.power,
        )
        .unwrap();
        assert_eq!(bundle.power.choices.len(), 1);
        assert_eq!(bundle.power.choices[0].id, choice);
        value["power"] = serde_json::json!([choice, choice]);
        assert!(parse(&serde_json::to_vec(&value).unwrap(), &catalog).is_err());
        value["power"] = serde_json::json!(["unreviewed-power-choice"]);
        assert!(parse(&serde_json::to_vec(&value).unwrap(), &catalog).is_err());
    }

    #[test]
    fn bundle_contains_validated_settings_and_deployment() {
        let catalog = studio_core::load_catalog().unwrap();
        let bundle = plan(
            &catalog,
            Target::Win11Ltsc2024,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
        )
        .unwrap();
        let value = serde_json::to_value(bundle).unwrap();
        assert_eq!(value["schema_version"], 3);
        assert!(value["power"]["choices"].is_array());
        assert!(value["settings"]["actions"].is_array());
        assert!(value["deployment"]["tasks"].is_array());
        assert!(
            plan(
                &catalog,
                Target::Win11Ltsc2024,
                &BTreeSet::new(),
                &BTreeSet::from(["unknown-task".into()]),
                &BTreeSet::new(),
            )
            .is_err()
        );
    }
}
