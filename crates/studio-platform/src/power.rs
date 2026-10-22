//! AC settings in the existing active Windows power scheme, with authenticated recovery.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};
use studio_core::Target;

#[cfg(any(windows, test))]
mod engine;
#[cfg(any(windows, test))]
mod store;
#[cfg(windows)]
mod windows;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PowerChoice {
    pub id: String,
    pub title: [String; 2],
    pub description: [String; 2],
    pub group: String,
    pub subgroup: String,
    pub setting: String,
    pub value: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PowerPlan {
    pub schema_version: u32,
    pub target: Target,
    pub choices: Vec<PowerChoice>,
}

/// This embedded catalog is the only authority for power addresses and desired values.
pub fn catalog() -> Result<Vec<PowerChoice>> {
    let choices: Vec<PowerChoice> = serde_json::from_str(include_str!("../../../data/power.json"))?;
    ensure!(choices.len() == 16, "Invalid power catalog size");
    let mut ids = BTreeSet::new();
    for c in &choices {
        ensure!(ids.insert(&c.id), "Duplicate power choice");
        ensure!(
            valid_guid(&c.subgroup) && valid_guid(&c.setting),
            "Invalid power setting GUID"
        );
        ensure!(
            c.title.iter().all(|v| !v.is_empty()) && c.description.iter().all(|v| !v.is_empty()),
            "Missing power label"
        );
    }
    Ok(choices)
}
pub fn build_plan(target: Target, selected: &BTreeSet<String>) -> Result<PowerPlan> {
    ensure!(
        selected.len() <= 5,
        "Choose at most one value in each power group"
    );
    let choices: Vec<_> = catalog()?
        .into_iter()
        .filter(|c| selected.contains(&c.id))
        .collect();
    ensure!(choices.len() == selected.len(), "Unknown power choice");
    let mut groups = BTreeSet::new();
    let mut addresses = BTreeSet::new();
    for c in &choices {
        ensure!(
            groups.insert(&c.group) && addresses.insert((&c.subgroup, &c.setting)),
            "Power choices in the same group are mutually exclusive"
        );
    }
    Ok(PowerPlan {
        schema_version: 1,
        target,
        choices,
    })
}
pub fn validate_plan(plan: &PowerPlan) -> Result<PowerPlan> {
    ensure!(plan.schema_version == 1, "Unsupported power plan schema");
    let selected: BTreeSet<_> = plan.choices.iter().map(|c| c.id.clone()).collect();
    ensure!(
        selected.len() == plan.choices.len(),
        "Duplicate power choice"
    );
    let canonical = build_plan(plan.target, &selected)?;
    ensure!(
        &canonical == plan,
        "Power plan differs from the trusted catalog or order"
    );
    Ok(canonical)
}
fn valid_guid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
            }
        })
}
pub fn inspect(plan: PowerPlan, cancel: Arc<AtomicBool>) -> Result<Vec<crate::SettingState>> {
    let plan = validate_plan(&plan)?;
    #[cfg(windows)]
    {
        windows::inspect(plan, cancel)
    }
    #[cfg(not(windows))]
    {
        plan.choices
            .into_iter()
            .map(|choice| {
                ensure!(
                    !cancel.load(std::sync::atomic::Ordering::Acquire),
                    "Power inspection cancelled"
                );
                Ok(crate::SettingState {
                    id: choice.id,
                    state: "preview".into(),
                    detail: "预览模式；未读取 Windows 当前电源方案。".into(),
                })
            })
            .collect()
    }
}
pub fn apply(plan: PowerPlan, cancel: Arc<AtomicBool>) -> Result<crate::RunReport> {
    let plan = validate_plan(&plan)?;
    #[cfg(windows)]
    {
        windows::apply(plan, cancel)
    }
    #[cfg(not(windows))]
    {
        let _ = (plan, cancel);
        anyhow::bail!(
            "Power changes require a supported Windows LTSC host; this platform is preview only"
        )
    }
}
pub fn restore(path: PathBuf, cancel: Arc<AtomicBool>) -> Result<crate::RunReport> {
    #[cfg(windows)]
    {
        windows::restore(path, cancel)
    }
    #[cfg(not(windows))]
    {
        let _ = (path, cancel);
        anyhow::bail!(
            "Power recovery requires a supported Windows LTSC host; this platform is preview only"
        )
    }
}
pub fn recovery_points() -> Result<Vec<crate::RecoveryPoint>> {
    #[cfg(windows)]
    {
        windows::recovery_points()
    }
    #[cfg(not(windows))]
    {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_and_plans_are_bounded_and_canonical() {
        let choices = catalog().unwrap();
        assert_eq!(choices.len(), 16);
        let mut selected = BTreeSet::from([choices[0].id.clone()]);
        let mut p = build_plan(Target::Win11Ltsc2024, &selected).unwrap();
        assert_eq!(validate_plan(&p).unwrap(), p);
        p.choices[0].value += 1;
        assert!(validate_plan(&p).is_err());
        selected.insert(choices[1].id.clone());
        assert!(build_plan(Target::Win11Ltsc2024, &selected).is_err());
        assert!(!valid_guid("0012EE47-9041-4b5d-9b77-535fba8b1442"));
    }
    #[test]
    #[cfg(not(windows))]
    fn preview_never_mutates_or_claims_inspection() {
        let p = build_plan(
            Target::Win10Ltsc2021,
            &BTreeSet::from(["power-display-15".into()]),
        )
        .unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        assert_eq!(
            inspect(p.clone(), cancel.clone()).unwrap()[0].state,
            "preview"
        );
        assert!(apply(p.clone(), cancel.clone()).is_err());
        assert!(restore(PathBuf::new(), cancel).is_err());
        assert!(inspect(p, Arc::new(AtomicBool::new(true))).is_err());
        assert!(recovery_points().unwrap().is_empty());
    }
}
