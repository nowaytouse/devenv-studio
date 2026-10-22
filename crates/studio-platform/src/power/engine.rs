use super::{PowerChoice, PowerPlan, valid_guid, validate_plan};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Record {
    pub choice: PowerChoice,
    pub original: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Header {
    pub schema_version: u32,
    pub id: String,
    pub started_at: String,
    pub plan: PowerPlan,
    pub scheme: String,
    pub records: Vec<Record>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Event {
    Header(Header),
    ApplyIntent { index: usize },
    Applied { index: usize },
    ApplyFinished { cancelled: bool },
    RestoreIntent { index: usize },
    Restored { index: usize },
    RestoreFinished { cancelled: bool },
}
pub(super) trait Backend {
    fn active(&self) -> Result<String>;
    fn read(&self, scheme: &str, choice: &PowerChoice) -> Result<u32>;
    fn write(&mut self, scheme: &str, choice: &PowerChoice, value: u32) -> Result<()>;
    fn activate(&mut self, scheme: &str) -> Result<()>;
}
pub(super) trait Journal {
    fn append(&mut self, event: &Event) -> Result<()>;
}
#[derive(Debug, Default)]
pub(super) struct Outcome {
    pub changed: usize,
    pub already_set: usize,
    pub cancelled: bool,
}
pub(super) fn validate_header(header: &Header) -> Result<()> {
    ensure!(
        header.schema_version == 1,
        "Unsupported power recovery schema"
    );
    ensure!(
        header.id.len() == 32 && header.id.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid power recovery ID"
    );
    ensure!(
        !header.started_at.is_empty()
            && header.started_at.len() <= 20
            && header.started_at.bytes().all(|b| b.is_ascii_digit()),
        "Invalid power recovery timestamp"
    );
    ensure!(
        valid_guid(&header.scheme),
        "Invalid active power scheme GUID"
    );
    let plan = validate_plan(&header.plan)?;
    ensure!(
        !plan.choices.is_empty() && header.records.len() == plan.choices.len(),
        "Incomplete power recovery originals"
    );
    ensure!(
        header
            .records
            .iter()
            .zip(&plan.choices)
            .all(|(r, c)| &r.choice == c),
        "Power recovery address or value is outside the trusted catalog"
    );
    Ok(())
}
fn unchanged(backend: &impl Backend, scheme: &str) -> Result<()> {
    ensure!(
        backend.active()? == scheme,
        "Active power scheme changed; stopped without switching schemes"
    );
    Ok(())
}
fn checked_read(backend: &impl Backend, scheme: &str, choice: &PowerChoice) -> Result<u32> {
    unchanged(backend, scheme)?;
    let value = backend.read(scheme, choice)?;
    unchanged(backend, scheme)?;
    Ok(value)
}
pub(super) fn inspect_backend(
    plan: &PowerPlan,
    backend: &impl Backend,
    cancel: &AtomicBool,
) -> Result<Vec<crate::SettingState>> {
    let plan = validate_plan(plan)?;
    if plan.choices.is_empty() {
        return Ok(Vec::new());
    }
    let scheme = backend.active()?;
    ensure!(valid_guid(&scheme), "Invalid active power scheme GUID");
    let mut states = Vec::new();
    for choice in &plan.choices {
        ensure!(
            !cancel.load(Ordering::Acquire),
            "Power inspection cancelled"
        );
        let value = checked_read(backend, &scheme, choice)?;
        states.push(crate::SettingState {
            id: choice.id.clone(),
            state: if value == choice.value {
                "aligned"
            } else {
                "different"
            }
            .into(),
            detail: format!(
                "AC · scheme {scheme} · current {value} → desired {}",
                choice.value
            ),
        });
    }
    Ok(states)
}
pub(super) fn apply_backend(
    plan: &PowerPlan,
    backend: &mut impl Backend,
    journal: &mut impl Journal,
    id: String,
    started_at: String,
    cancel: &AtomicBool,
) -> Result<Outcome> {
    let plan = validate_plan(plan)?;
    ensure!(!plan.choices.is_empty(), "Select at least one power choice");
    let scheme = backend.active()?;
    ensure!(valid_guid(&scheme), "Invalid active power scheme GUID");
    let mut records = Vec::new();
    for choice in &plan.choices {
        ensure!(
            !cancel.load(Ordering::Acquire),
            "Power apply cancelled before any mutation"
        );
        records.push(Record {
            choice: choice.clone(),
            original: checked_read(backend, &scheme, choice)
                .context("Read original AC power value")?,
        });
    }
    let header = Header {
        schema_version: 1,
        id,
        started_at,
        plan,
        scheme,
        records,
    };
    validate_header(&header)?;
    journal
        .append(&Event::Header(header.clone()))
        .context("Persist original AC values before applying")?;
    let mut outcome = Outcome::default();
    for (index, record) in header.records.iter().enumerate() {
        if cancel.load(Ordering::Acquire) {
            outcome.cancelled = true;
            break;
        }
        ensure!(
            checked_read(backend, &header.scheme, &record.choice)? == record.original,
            "Power value changed after snapshot; refusing to overwrite"
        );
        if record.original == record.choice.value {
            outcome.already_set += 1;
            continue;
        }
        journal
            .append(&Event::ApplyIntent { index })
            .context("Persist power write intent")?;
        if cancel.load(Ordering::Acquire) {
            outcome.cancelled = true;
            break;
        }
        ensure!(
            checked_read(backend, &header.scheme, &record.choice)? == record.original,
            "Power value changed while persisting intent"
        );
        backend
            .write(&header.scheme, &record.choice, record.choice.value)
            .context("Write AC power value; recovery may be required")?;
        unchanged(backend, &header.scheme)?;
        backend.activate(&header.scheme).context(
            "Activate changed AC values in the same power scheme; recovery may be required",
        )?;
        ensure!(
            checked_read(backend, &header.scheme, &record.choice)? == record.choice.value,
            "AC power readback failed; recovery may be required"
        );
        outcome.changed += 1;
        journal
            .append(&Event::Applied { index })
            .context("Persist power verification; recovery may be required")?;
    }
    journal
        .append(&Event::ApplyFinished {
            cancelled: outcome.cancelled,
        })
        .context("Persist power completion")?;
    Ok(outcome)
}
pub(super) fn restore_backend(
    header: &Header,
    intended: &BTreeSet<usize>,
    backend: &mut impl Backend,
    journal: &mut impl Journal,
    cancel: &AtomicBool,
) -> Result<Outcome> {
    validate_header(header)?;
    ensure!(
        intended.iter().all(|i| header
            .records
            .get(*i)
            .is_some_and(|r| r.original != r.choice.value)),
        "Invalid power recovery intent"
    );
    unchanged(backend, &header.scheme)?;
    let mut outcome = Outcome::default();
    for &index in intended.iter().rev() {
        if cancel.load(Ordering::Acquire) {
            outcome.cancelled = true;
            break;
        }
        let record = &header.records[index];
        let current = checked_read(backend, &header.scheme, &record.choice)?;
        ensure!(
            current == record.original || current == record.choice.value,
            "AC power value changed externally; refusing to overwrite"
        );
        journal
            .append(&Event::RestoreIntent { index })
            .context("Persist power restore intent")?;
        if cancel.load(Ordering::Acquire) {
            outcome.cancelled = true;
            break;
        }
        ensure!(
            checked_read(backend, &header.scheme, &record.choice)? == current,
            "Power value changed while persisting restore intent"
        );
        if current != record.original {
            backend
                .write(&header.scheme, &record.choice, record.original)
                .context("Restore original AC power value")?;
            outcome.changed += 1;
        } else {
            outcome.already_set += 1;
        }
        unchanged(backend, &header.scheme)?;
        // A previous interrupted restore may have written the original but not activated it.
        backend
            .activate(&header.scheme)
            .context("Activate restored AC values")?;
        ensure!(
            checked_read(backend, &header.scheme, &record.choice)? == record.original,
            "Restored AC power value failed readback"
        );
        journal
            .append(&Event::Restored { index })
            .context("Persist power restore verification")?;
    }
    journal
        .append(&Event::RestoreFinished {
            cancelled: outcome.cancelled,
        })
        .context("Persist power restore completion")?;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, collections::BTreeMap, rc::Rc, sync::Arc};
    const SCHEME: &str = "381b4222-f694-41f0-9685-ff5bb260df2e";
    struct Fake {
        values: BTreeMap<String, u32>,
        writes: usize,
        activations: usize,
        fail_read: bool,
        fail_write: bool,
        fail_activate: bool,
        ignore_write: bool,
        changed_scheme: Rc<Cell<bool>>,
    }
    impl Fake {
        fn new(plan: &PowerPlan) -> Self {
            Self {
                values: plan.choices.iter().map(|c| (c.id.clone(), 42)).collect(),
                writes: 0,
                activations: 0,
                fail_read: false,
                fail_write: false,
                fail_activate: false,
                ignore_write: false,
                changed_scheme: Rc::new(Cell::new(false)),
            }
        }
    }
    impl Backend for Fake {
        fn active(&self) -> Result<String> {
            ensure!(!self.fail_read, "active scheme unavailable");
            Ok(if self.changed_scheme.get() {
                "11111111-1111-1111-1111-111111111111"
            } else {
                SCHEME
            }
            .into())
        }
        fn read(&self, _: &str, c: &PowerChoice) -> Result<u32> {
            ensure!(!self.fail_read, "read failure");
            Ok(self.values[&c.id])
        }
        fn write(&mut self, _: &str, c: &PowerChoice, value: u32) -> Result<()> {
            ensure!(!self.fail_write, "write failure");
            self.writes += 1;
            if !self.ignore_write {
                self.values.insert(c.id.clone(), value);
            }
            Ok(())
        }
        fn activate(&mut self, _: &str) -> Result<()> {
            ensure!(!self.fail_activate, "activation failure");
            self.activations += 1;
            Ok(())
        }
    }
    #[derive(Default)]
    struct Log {
        events: Vec<Event>,
        fail_at: Option<usize>,
        cancel_at: Option<usize>,
        cancel: Option<Arc<AtomicBool>>,
        change_scheme: Option<(usize, Rc<Cell<bool>>)>,
    }
    impl Journal for Log {
        fn append(&mut self, e: &Event) -> Result<()> {
            ensure!(self.fail_at != Some(self.events.len()), "journal failure");
            self.events.push(e.clone());
            if let Some((at, signal)) = &self.change_scheme
                && *at == self.events.len()
            {
                signal.set(true);
            }
            if self.cancel_at == Some(self.events.len()) {
                self.cancel.as_ref().unwrap().store(true, Ordering::Release);
            }
            Ok(())
        }
    }
    fn plan() -> PowerPlan {
        super::super::build_plan(
            studio_core::Target::Win11Ltsc2024,
            &BTreeSet::from(["power-display-15".into(), "power-sleep-30".into()]),
        )
        .unwrap()
    }
    fn run(p: &PowerPlan, b: &mut Fake, j: &mut Log, c: &AtomicBool) -> Result<Outcome> {
        apply_backend(p, b, j, "a".repeat(32), "1792700000".into(), c)
    }
    fn header(j: &Log) -> Header {
        match &j.events[0] {
            Event::Header(h) => h.clone(),
            _ => panic!(),
        }
    }
    #[test]
    fn apply_and_restore_verify_exact_originals_and_same_scheme() {
        let p = plan();
        let mut b = Fake::new(&p);
        let mut j = Log::default();
        let c = AtomicBool::new(false);
        assert_eq!(inspect_backend(&p, &b, &c).unwrap()[0].state, "different");
        assert_eq!(run(&p, &mut b, &mut j, &c).unwrap().changed, 2);
        let h = header(&j);
        let intended = BTreeSet::from([0, 1]);
        let outcome = restore_backend(&h, &intended, &mut b, &mut j, &c).unwrap();
        assert_eq!(outcome.changed, 2);
        assert!(b.values.values().all(|&v| v == 42));
        assert_eq!(b.activations, 4);
        assert_eq!(
            restore_backend(&h, &intended, &mut b, &mut j, &c)
                .unwrap()
                .already_set,
            2
        );
        assert_eq!(b.activations, 6);
    }
    #[test]
    fn read_write_activation_and_readback_failures_are_errors() {
        for failure in 0..4 {
            let p = plan();
            let mut b = Fake::new(&p);
            let mut j = Log::default();
            match failure {
                0 => b.fail_read = true,
                1 => b.fail_write = true,
                2 => b.fail_activate = true,
                _ => b.ignore_write = true,
            }
            assert!(run(&p, &mut b, &mut j, &AtomicBool::new(false)).is_err());
            assert!(
                !j.events
                    .iter()
                    .any(|e| matches!(e, Event::ApplyFinished { cancelled: false }))
            );
        }
    }
    #[test]
    fn journal_failures_cannot_be_reported_as_success() {
        for fail_at in 0..6 {
            let p = plan();
            let mut b = Fake::new(&p);
            let mut j = Log {
                fail_at: Some(fail_at),
                ..Default::default()
            };
            assert!(run(&p, &mut b, &mut j, &AtomicBool::new(false)).is_err());
            if fail_at < 2 {
                assert_eq!(b.writes, 0);
            }
        }
    }
    #[test]
    fn cancelled_after_durable_intent_is_recoverable_without_write() {
        let p = plan();
        let mut b = Fake::new(&p);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut j = Log {
            cancel_at: Some(2),
            cancel: Some(cancel.clone()),
            ..Default::default()
        };
        assert!(run(&p, &mut b, &mut j, &cancel).unwrap().cancelled);
        assert_eq!(b.writes, 0);
        cancel.store(false, Ordering::Release);
        assert_eq!(
            restore_backend(&header(&j), &BTreeSet::from([0]), &mut b, &mut j, &cancel)
                .unwrap()
                .already_set,
            1
        );
    }
    #[test]
    fn restore_refuses_changed_scheme_external_values_or_untrusted_payloads() {
        let p = plan();
        let mut b = Fake::new(&p);
        let mut j = Log::default();
        let c = AtomicBool::new(false);
        run(&p, &mut b, &mut j, &c).unwrap();
        let mut h = header(&j);
        b.changed_scheme.set(true);
        assert!(restore_backend(&h, &BTreeSet::from([0]), &mut b, &mut j, &c).is_err());
        b.changed_scheme.set(false);
        b.values.insert(p.choices[0].id.clone(), 99);
        assert!(restore_backend(&h, &BTreeSet::from([0]), &mut b, &mut j, &c).is_err());
        assert_eq!(b.writes, 2);
        h.records[0].choice.setting = "00000000-0000-0000-0000-000000000000".into();
        assert!(restore_backend(&h, &BTreeSet::from([0]), &mut b, &mut j, &c).is_err());
    }
    #[test]
    fn external_scheme_switch_after_durable_intent_stops_before_write_or_activation() {
        let p = plan();
        let mut b = Fake::new(&p);
        let mut j = Log {
            change_scheme: Some((2, Rc::clone(&b.changed_scheme))),
            ..Default::default()
        };
        assert!(run(&p, &mut b, &mut j, &AtomicBool::new(false)).is_err());
        assert_eq!(b.writes, 0);
        assert_eq!(b.activations, 0);
        assert!(
            j.events
                .iter()
                .any(|event| matches!(event, Event::ApplyIntent { index: 0 }))
        );
        assert!(
            !j.events
                .iter()
                .any(|event| matches!(event, Event::ApplyFinished { cancelled: false }))
        );
    }
    #[test]
    fn activation_failure_retains_original_and_partial_intent_for_recovery() {
        let p = plan();
        let mut b = Fake::new(&p);
        b.fail_activate = true;
        let mut j = Log::default();
        let cancel = AtomicBool::new(false);
        assert!(run(&p, &mut b, &mut j, &cancel).is_err());
        assert_eq!(b.values[&p.choices[0].id], p.choices[0].value);
        b.fail_activate = false;
        let result =
            restore_backend(&header(&j), &BTreeSet::from([0]), &mut b, &mut j, &cancel).unwrap();
        assert_eq!(result.changed, 1);
        assert!(b.values.values().all(|&v| v == 42));
        assert_eq!(b.activations, 1);
    }
    #[test]
    fn cancellation_during_restore_is_durable_and_retry_recovers_remaining_values() {
        let p = plan();
        let mut b = Fake::new(&p);
        let mut j = Log::default();
        let cancel = Arc::new(AtomicBool::new(false));
        run(&p, &mut b, &mut j, &cancel).unwrap();
        let h = header(&j);
        let intended = BTreeSet::from([0, 1]);
        j.cancel = Some(Arc::clone(&cancel));
        j.cancel_at = Some(j.events.len() + 1);
        assert!(
            restore_backend(&h, &intended, &mut b, &mut j, &cancel)
                .unwrap()
                .cancelled
        );
        assert!(b.values.values().all(|&v| v != 42));
        assert!(matches!(
            j.events.last(),
            Some(Event::RestoreFinished { cancelled: true })
        ));
        cancel.store(false, Ordering::Release);
        j.cancel_at = None;
        assert_eq!(
            restore_backend(&h, &intended, &mut b, &mut j, &cancel)
                .unwrap()
                .changed,
            2
        );
        assert!(b.values.values().all(|&v| v == 42));
    }
    #[test]
    fn empty_power_inspection_does_not_query_a_native_scheme() {
        let p =
            super::super::build_plan(studio_core::Target::Win11Ltsc2024, &BTreeSet::new()).unwrap();
        let mut b = Fake::new(&p);
        b.fail_read = true;
        assert!(
            inspect_backend(&p, &b, &AtomicBool::new(false))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn already_values_never_receive_apply_intents() {
        let p = plan();
        let mut b = Fake::new(&p);
        let mut j = Log::default();
        for c in &p.choices {
            b.values.insert(c.id.clone(), c.value);
        }
        assert_eq!(
            run(&p, &mut b, &mut j, &AtomicBool::new(false))
                .unwrap()
                .already_set,
            2
        );
        assert_eq!(b.writes, 0);
        assert_eq!(j.events.len(), 2);
    }
}
