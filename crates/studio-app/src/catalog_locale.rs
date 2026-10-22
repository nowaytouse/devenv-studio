//! Presentation-only translations. Trusted executable catalogs remain unchanged.
use crate::prefs::Language;
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{collections::BTreeMap, sync::OnceLock};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Overlay {
    actions: BTreeMap<String, BTreeMap<String, String>>,
    presets: BTreeMap<String, BTreeMap<String, String>>,
    tasks: BTreeMap<String, BTreeMap<String, String>>,
}
static ENGLISH: OnceLock<Result<Overlay, String>> = OnceLock::new();
fn overlay() -> &'static Result<Overlay, String> {
    ENGLISH.get_or_init(|| {
        let result = (|| -> Result<Overlay> {
            let overlay: Overlay =
                serde_json::from_str(include_str!("../../../data/locales/en-US.json"))
                    .context("Invalid English catalog translations")?;
            let settings = studio_core::load_catalog()?;
            let deployment = studio_deployment::catalog()?;
            validate_section(
                &overlay.actions,
                settings.actions.iter().map(|a| a.id.as_str()),
                &["title", "description", "category", "restart", "risk"],
            )?;
            validate_section(
                &overlay.presets,
                settings.presets.iter().map(|a| a.id.as_str()),
                &["title", "description"],
            )?;
            validate_section(
                &overlay.tasks,
                deployment.tasks.iter().map(|a| a.id.as_str()),
                &["title", "description", "category", "restart", "limitations"],
            )?;
            Ok(overlay)
        })();
        result.map_err(|error| format!("{error:#}"))
    })
}
fn validate_section<'a>(
    section: &BTreeMap<String, BTreeMap<String, String>>,
    ids: impl Iterator<Item = &'a str>,
    fields: &[&str],
) -> Result<()> {
    let ids: Vec<_> = ids.collect();
    ensure!(
        section.len() == ids.len(),
        "Translation catalog ID count mismatch"
    );
    for id in ids {
        let values = section
            .get(id)
            .with_context(|| format!("Missing translation for {id}"))?;
        ensure!(
            values.len() == fields.len(),
            "Unexpected translation fields for {id}"
        );
        for field in fields {
            let value = values
                .get(*field)
                .with_context(|| format!("Missing translation {id}.{field}"))?;
            ensure!(
                !value.trim().is_empty() && !value.contains('\0') && value.len() < 8192,
                "Invalid translation {id}.{field}"
            );
        }
    }
    Ok(())
}
pub fn validate() -> Result<()> {
    overlay()
        .as_ref()
        .map(|_| ())
        .map_err(|error| anyhow::anyhow!("{error}"))
}
pub fn text(language: Language, kind: &str, id: &str, field: &str, original: &str) -> String {
    if language == Language::EnUs
        && let Ok(overlay) = overlay()
    {
        let section = match kind {
            "actions" => Some(&overlay.actions),
            "presets" => Some(&overlay.presets),
            "tasks" => Some(&overlay.tasks),
            _ => None,
        };
        if let Some(value) = section.and_then(|s| s.get(id)).and_then(|s| s.get(field)) {
            return value.clone();
        }
    }
    original.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn english_is_complete_for_current_catalogs() {
        validate().unwrap();
    }
    #[test]
    fn presentation_does_not_modify_canonical_actions_or_plan() {
        let catalog = studio_core::load_catalog().unwrap();
        let before = serde_json::to_value(&catalog).unwrap();
        let selected = ["explorer-extensions".to_owned()].into_iter().collect();
        let plan = studio_core::build_plan(&catalog, studio_core::Target::Win11Ltsc2024, &selected)
            .unwrap();
        for action in &catalog.actions {
            assert!(
                !text(
                    Language::EnUs,
                    "actions",
                    &action.id,
                    "title",
                    &action.title
                )
                .is_empty()
            );
            assert_eq!(
                text(
                    Language::ZhCn,
                    "actions",
                    &action.id,
                    "title",
                    &action.title
                ),
                action.title
            );
        }
        assert_eq!(serde_json::to_value(&catalog).unwrap(), before);
        assert_eq!(studio_core::validate_plan(&plan).unwrap(), plan);
    }
    #[test]
    fn incomplete_or_unknown_fields_are_rejected() {
        let mut section = BTreeMap::from([(
            "known".into(),
            BTreeMap::from([("title".into(), "Title".into())]),
        )]);
        assert!(
            validate_section(&section, ["known"].into_iter(), &["title", "description"]).is_err()
        );
        section
            .get_mut("known")
            .unwrap()
            .insert("description".into(), "Description".into());
        assert!(
            validate_section(&section, ["known"].into_iter(), &["title", "description"]).is_ok()
        );
        assert!(
            validate_section(
                &section,
                ["different"].into_iter(),
                &["title", "description"]
            )
            .is_err()
        );
        section
            .get_mut("known")
            .unwrap()
            .insert("extra".into(), "Value".into());
        assert!(
            validate_section(&section, ["known"].into_iter(), &["title", "description"]).is_err()
        );
    }
}
