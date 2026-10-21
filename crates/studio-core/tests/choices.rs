use std::collections::BTreeSet;
use studio_core::*;

fn with_alternatives() -> Catalog {
    let mut c = load_catalog().expect("trusted current catalog");
    c.presets.clear();
    let mut first = c.actions[0].clone();
    first.choice_group = Some("test-choice".into());
    let mut other = first.clone();
    other.id = "test-opposite".into();
    other.changes[0].value ^= 1;
    c.actions = vec![first, other];
    c
}

#[test]
fn alternatives_share_an_address_but_cannot_be_selected_together() {
    let c = with_alternatives();
    validate_catalog(&c).expect("same explicit group permits alternatives");
    let ids = c.actions.iter().map(|a| a.id.clone()).collect();
    assert!(build_plan(&c, c.actions[0].targets[0], &ids).is_err());
    for action in &c.actions {
        assert!(build_plan(&c, action.targets[0], &BTreeSet::from([action.id.clone()])).is_ok());
    }
}

#[test]
fn overlapping_addresses_without_shared_group_are_rejected() {
    let mut c = with_alternatives();
    c.actions[1].choice_group = None;
    assert!(validate_catalog(&c).is_err());
    c.actions[1].choice_group = Some("another-group".into());
    assert!(validate_catalog(&c).is_err());
    let mut c = with_alternatives();
    let duplicate = c.actions[0].changes[0].clone();
    c.actions[0].changes.push(duplicate);
    assert!(validate_catalog(&c).is_err());
}

#[test]
fn presets_cannot_introduce_unknown_or_conflicting_actions() {
    let mut c = with_alternatives();
    let preset = Preset {
        id: "reviewed".into(),
        title: "Reviewed choice".into(),
        description: "Explicit selection only".into(),
        actions: BTreeSet::from([c.actions[0].id.clone()]),
    };
    c.presets = vec![preset];
    assert!(validate_catalog(&c).is_ok());
    c.presets[0].actions.insert(c.actions[1].id.clone());
    assert!(validate_catalog(&c).is_err());
    c.presets[0].actions = BTreeSet::from(["unknown".into()]);
    assert!(validate_catalog(&c).is_err());
}

#[test]
fn old_catalog_is_available_only_as_shipped_recovery_snapshot() {
    let legacy = load_catalog_for_revision("2026-10-21.1").expect("retained first delivery");
    assert_eq!(legacy.actions.len(), 12);
    assert!(legacy.presets.is_empty());
    assert!(
        build_plan(
            &legacy,
            Target::Win11Ltsc2024,
            &BTreeSet::from(["privacy-advertising-id".into()])
        )
        .is_ok()
    );
    assert!(load_catalog_for_revision("arbitrary-imported-catalog").is_err());
}

#[test]
fn empty_selection_stays_empty_even_when_presets_exist() {
    let mut c = with_alternatives();
    c.presets = vec![Preset {
        id: "optional".into(),
        title: "Optional".into(),
        description: "Never auto-selected".into(),
        actions: BTreeSet::from([c.actions[0].id.clone()]),
    }];
    assert!(
        build_plan(&c, c.actions[0].targets[0], &BTreeSet::new())
            .unwrap()
            .actions
            .is_empty()
    );
}
