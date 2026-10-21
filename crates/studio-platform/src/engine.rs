//! Portable registry transaction protocol. Every mutation has a durable intent.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicBool, Ordering},
};
use studio_core::{Plan, RegistryChange, Target, build_plan, load_catalog};

pub(crate) const MAX_RAW_BYTES: usize = 65_536;
pub(crate) const MAX_RECORDS: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawValue {
    pub kind: u32,
    pub bytes: Vec<u8>,
}
impl RawValue {
    pub fn desired(change: &RegistryChange) -> Self {
        Self {
            kind: 4,
            bytes: change.value.to_le_bytes().to_vec(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    pub change: RegistryChange,
    pub original: Option<RawValue>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Header {
    pub schema_version: u32,
    pub id: String,
    pub started_at: String,
    pub target: Target,
    pub catalog_revision: String,
    pub actions: Vec<String>,
    pub records: Vec<Record>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Event {
    Header(Header),
    ApplyIntent { index: usize },
    Applied { index: usize },
    ApplyFinished { cancelled: bool },
    RestoreIntent { index: usize },
    Restored { index: usize },
    RestoreFinished { cancelled: bool },
}
pub(crate) trait Registry {
    fn preflight(&self, change: &RegistryChange) -> Result<()>;
    fn read(&self, change: &RegistryChange) -> Result<Option<RawValue>>;
    fn write(&mut self, change: &RegistryChange, value: Option<&RawValue>) -> Result<()>;
}
pub(crate) trait Journal {
    fn append(&mut self, event: &Event) -> Result<()>;
}
#[derive(Debug, Default)]
pub(crate) struct Outcome {
    pub changed: usize,
    pub already_set: usize,
    pub cancelled: bool,
}

pub(crate) fn validate_header(header: &Header) -> Result<()> {
    ensure!(header.schema_version == 1, "Unsupported recovery schema");
    ensure!(
        !header.id.is_empty()
            && header.id.len() <= 100
            && header
                .id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || b == b'-'),
        "Invalid recovery ID"
    );
    ensure!(
        !header.started_at.is_empty()
            && header.started_at.len() <= 64
            && header
                .started_at
                .bytes()
                .all(|b| b.is_ascii_digit() || b".-Z:T ".contains(&b)),
        "Invalid recovery timestamp"
    );
    let ids: BTreeSet<_> = header.actions.iter().cloned().collect();
    ensure!(
        ids.len() == header.actions.len(),
        "Duplicate recovery action"
    );
    let plan = build_plan(&load_catalog()?, header.target, &ids)?;
    ensure!(
        plan.catalog_revision == header.catalog_revision,
        "Recovery catalog revision differs; use the originating application version"
    );
    ensure!(
        header.records.len() <= MAX_RECORDS,
        "Recovery has too many records"
    );
    let mut seen = BTreeSet::new();
    for record in &header.records {
        ensure!(
            plan.actions
                .iter()
                .flat_map(|a| &a.changes)
                .any(|c| c == &record.change),
            "Recovery address or desired value is outside trusted catalog"
        );
        ensure!(
            seen.insert((
                record.change.hive,
                record.change.path.to_lowercase(),
                record.change.name.to_lowercase()
            )),
            "Duplicate recovery address"
        );
        if let Some(value) = &record.original {
            ensure!(
                value.bytes.len() <= MAX_RAW_BYTES && value.kind <= 11,
                "Invalid original registry value"
            );
        }
    }
    Ok(())
}

pub(crate) fn apply_backend(
    plan: &Plan,
    registry: &mut impl Registry,
    journal: &mut impl Journal,
    id: String,
    started_at: String,
    cancel: &AtomicBool,
) -> Result<Outcome> {
    let mut outcome = Outcome::default();
    let mut records = Vec::new();
    for change in plan.actions.iter().flat_map(|a| &a.changes) {
        if cancel.load(Ordering::Acquire) {
            outcome.cancelled = true;
            break;
        }
        registry
            .preflight(change)
            .with_context(|| format!("Check write access: {}\\{}", change.path, change.name))?;
        let original = registry
            .read(change)
            .context("Read original registry value")?;
        if let Some(value) = &original {
            ensure!(
                value.bytes.len() <= MAX_RAW_BYTES && value.kind <= 11,
                "Original registry value exceeds recovery limits"
            );
        }
        if original.as_ref() == Some(&RawValue::desired(change)) {
            outcome.already_set += 1;
        } else {
            records.push(Record {
                change: change.clone(),
                original,
            });
        }
    }
    ensure!(
        records.len() <= MAX_RECORDS,
        "Plan exceeds recovery record limit"
    );
    let header = Header {
        schema_version: 1,
        id,
        started_at,
        target: plan.target,
        catalog_revision: plan.catalog_revision.clone(),
        actions: plan.actions.iter().map(|a| a.id.clone()).collect(),
        records,
    };
    journal
        .append(&Event::Header(header.clone()))
        .context("Persist recovery originals before applying")?;
    for (index, record) in header.records.iter().enumerate() {
        if outcome.cancelled || cancel.load(Ordering::Acquire) {
            outcome.cancelled = true;
            break;
        }
        ensure!(
            registry.read(&record.change)? == record.original,
            "Registry changed after preview; stopped before overwriting {}\\{}",
            record.change.path,
            record.change.name
        );
        journal
            .append(&Event::ApplyIntent { index })
            .context("Persist write intent; no further mutation is safe")?;
        // Cancellation after the durable intent still leaves original bytes available.
        if cancel.load(Ordering::Acquire) {
            outcome.cancelled = true;
            break;
        }
        ensure!(
            registry.read(&record.change)? == record.original,
            "Registry changed while persisting write intent; stopped before overwriting {}\\{}",
            record.change.path,
            record.change.name
        );
        let desired = RawValue::desired(&record.change);
        registry
            .write(&record.change, Some(&desired))
            .with_context(|| {
                format!(
                    "Registry write failed after {} verified changes; recovery may be required",
                    outcome.changed
                )
            })?;
        ensure!(
            registry
                .read(&record.change)
                .context("Read back applied value; recovery may be required")?
                .as_ref()
                == Some(&desired),
            "Applied registry value failed verification after {} changes; recovery may be required",
            outcome.changed
        );
        outcome.changed += 1;
        journal
            .append(&Event::Applied { index })
            .context("Record verification failed; stop and inspect recovery")?;
    }
    journal
        .append(&Event::ApplyFinished {
            cancelled: outcome.cancelled,
        })
        .context("Persist final apply state; recovery journal remains available")?;
    Ok(outcome)
}

pub(crate) fn restore_backend(
    header: &Header,
    intended: &BTreeSet<usize>,
    registry: &mut impl Registry,
    journal: &mut impl Journal,
    cancel: &AtomicBool,
) -> Result<Outcome> {
    validate_header(header)?;
    let mut outcome = Outcome::default();
    for &index in intended.iter().rev() {
        let record = header
            .records
            .get(index)
            .context("Recovery index out of bounds")?;
        if cancel.load(Ordering::Acquire) {
            outcome.cancelled = true;
            break;
        }
        registry.preflight(&record.change)?;
        let current = registry.read(&record.change)?;
        if current == record.original {
            outcome.already_set += 1;
            continue;
        }
        if current.as_ref() != Some(&RawValue::desired(&record.change)) {
            bail!(
                "Recovery conflict at {}\\{}: current value differs from both original and applied value; {} values restored",
                record.change.path,
                record.change.name,
                outcome.changed
            );
        }
        journal
            .append(&Event::RestoreIntent { index })
            .context("Persist restore intent")?;
        if cancel.load(Ordering::Acquire) {
            outcome.cancelled = true;
            break;
        }
        // Recheck after durable I/O to reduce the interval in which external changes could race.
        ensure!(
            registry.read(&record.change)? == current,
            "Recovery conflict after intent; current registry value changed"
        );
        registry
            .write(&record.change, record.original.as_ref())
            .with_context(|| {
                format!("Restore failed after {} verified restores", outcome.changed)
            })?;
        ensure!(
            registry.read(&record.change)? == record.original,
            "Restored registry value failed verification; recovery is incomplete"
        );
        outcome.changed += 1;
        journal.append(&Event::Restored { index })?;
    }
    journal.append(&Event::RestoreFinished {
        cancelled: outcome.cancelled,
    })?;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, collections::BTreeMap, sync::Arc};
    #[derive(Default)]
    struct Fake {
        values: BTreeMap<String, RawValue>,
        writes: usize,
        fail_write: Option<usize>,
        corrupt: bool,
        cancel_after: Option<Arc<AtomicBool>>,
        external_during_intent: Option<Arc<AtomicBool>>,
        reads: Cell<usize>,
    }
    impl Registry for Fake {
        fn preflight(&self, _: &RegistryChange) -> Result<()> {
            Ok(())
        }
        fn read(&self, c: &RegistryChange) -> Result<Option<RawValue>> {
            self.reads.set(self.reads.get() + 1);
            if self
                .external_during_intent
                .as_ref()
                .is_some_and(|v| v.load(Ordering::Acquire))
            {
                return Ok(Some(RawValue {
                    kind: 4,
                    bytes: 42_u32.to_le_bytes().to_vec(),
                }));
            }
            Ok(self.values.get(&c.name).cloned())
        }
        fn write(&mut self, c: &RegistryChange, v: Option<&RawValue>) -> Result<()> {
            self.writes += 1;
            if self.fail_write == Some(self.writes) {
                bail!("injected write failure");
            }
            if let Some(v) = v {
                let mut v = v.clone();
                if self.corrupt {
                    v.bytes = vec![99];
                }
                self.values.insert(c.name.clone(), v);
            } else {
                self.values.remove(&c.name);
            }
            if let Some(cancel) = &self.cancel_after {
                cancel.store(true, Ordering::Release);
            }
            Ok(())
        }
    }
    #[derive(Default)]
    struct Log {
        events: Vec<Event>,
        fail_at: Option<usize>,
        external_during_intent: Option<Arc<AtomicBool>>,
    }
    impl Journal for Log {
        fn append(&mut self, e: &Event) -> Result<()> {
            if self.fail_at == Some(self.events.len()) {
                bail!("injected journal failure");
            }
            self.events.push(e.clone());
            if matches!(e, Event::ApplyIntent { .. })
                && let Some(flag) = &self.external_during_intent
            {
                flag.store(true, Ordering::Release);
            }
            Ok(())
        }
    }
    fn plan() -> Plan {
        let c = load_catalog().unwrap();
        let ids = c.actions.iter().take(2).map(|a| a.id.clone()).collect();
        build_plan(&c, Target::Win10Ltsc2021, &ids).unwrap()
    }
    fn apply(p: &Plan, r: &mut Fake, l: &mut Log, c: &AtomicBool) -> Result<Outcome> {
        apply_backend(p, r, l, "ab-12".into(), "2026-10-21T00:00:00Z".into(), c)
    }
    fn header(l: &Log) -> Header {
        match &l.events[0] {
            Event::Header(h) => h.clone(),
            _ => unreachable!(),
        }
    }
    fn intents(l: &Log) -> BTreeSet<usize> {
        l.events
            .iter()
            .filter_map(|e| match e {
                Event::ApplyIntent { index } => Some(*index),
                _ => None,
            })
            .collect()
    }
    #[test]
    fn journal_failure_prevents_any_write() {
        for fail_at in [0, 1] {
            let mut r = Fake::default();
            let mut l = Log {
                fail_at: Some(fail_at),
                ..Default::default()
            };
            assert!(apply(&plan(), &mut r, &mut l, &AtomicBool::new(false)).is_err());
            assert_eq!(r.writes, 0);
        }
    }
    #[test]
    fn failed_readback_stops_and_leaves_intent() {
        let mut r = Fake {
            corrupt: true,
            ..Default::default()
        };
        let mut l = Log::default();
        assert!(apply(&plan(), &mut r, &mut l, &AtomicBool::new(false)).is_err());
        assert_eq!(r.writes, 1);
        assert_eq!(intents(&l).len(), 1);
        assert!(!l.events.iter().any(|e| matches!(e, Event::Applied { .. })));
    }
    #[test]
    fn partial_write_failure_is_reported_and_recoverable() {
        let mut r = Fake {
            fail_write: Some(2),
            ..Default::default()
        };
        let mut l = Log::default();
        assert!(apply(&plan(), &mut r, &mut l, &AtomicBool::new(false)).is_err());
        r.fail_write = None;
        let o = restore_backend(
            &header(&l),
            &intents(&l),
            &mut r,
            &mut l,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(o.changed, 1);
        assert!(r.values.is_empty());
    }
    #[test]
    fn cancellation_stops_before_next_write() {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut r = Fake {
            cancel_after: Some(cancel.clone()),
            ..Default::default()
        };
        let mut l = Log::default();
        let o = apply(&plan(), &mut r, &mut l, &cancel).unwrap();
        assert_eq!(o.changed, 1);
        assert!(o.cancelled);
        assert_eq!(r.writes, 1);
    }
    #[test]
    fn early_cancellation_has_a_real_cancelled_journal() {
        let mut r = Fake::default();
        let mut l = Log::default();
        let outcome = apply(&plan(), &mut r, &mut l, &AtomicBool::new(true)).unwrap();
        assert!(outcome.cancelled);
        assert_eq!(r.writes, 0);
        assert!(header(&l).records.is_empty());
        assert!(matches!(
            l.events.last(),
            Some(Event::ApplyFinished { cancelled: true })
        ));
    }
    #[test]
    fn external_edit_during_journal_flush_prevents_the_write() {
        let flag = Arc::new(AtomicBool::new(false));
        let mut r = Fake {
            external_during_intent: Some(flag.clone()),
            ..Default::default()
        };
        let mut l = Log {
            external_during_intent: Some(flag),
            ..Default::default()
        };
        let error = apply(&plan(), &mut r, &mut l, &AtomicBool::new(false)).unwrap_err();
        assert!(error.to_string().contains("persisting write intent"));
        assert_eq!(r.writes, 0);
        assert_eq!(intents(&l).len(), 1);
        assert!(!l.events.iter().any(|e| matches!(e, Event::Applied { .. })));
    }
    #[test]
    fn absent_and_raw_values_restore_idempotently() {
        let p = plan();
        let c = &p.actions[0].changes[0];
        let raw = RawValue {
            kind: 3,
            bytes: vec![0, 255, 0, 123],
        };
        let mut r = Fake::default();
        r.values.insert(c.name.clone(), raw.clone());
        let initial = r.values.clone();
        let mut l = Log::default();
        apply(&p, &mut r, &mut l, &AtomicBool::new(false)).unwrap();
        let h = header(&l);
        let i = intents(&l);
        restore_backend(&h, &i, &mut r, &mut l, &AtomicBool::new(false)).unwrap();
        assert_eq!(r.values, initial);
        let writes = r.writes;
        let o = restore_backend(&h, &i, &mut r, &mut l, &AtomicBool::new(false)).unwrap();
        assert_eq!(o.changed, 0);
        assert_eq!(r.writes, writes);
    }
    #[test]
    fn external_change_is_not_clobbered() {
        let p = plan();
        let mut r = Fake::default();
        let mut l = Log::default();
        apply(&p, &mut r, &mut l, &AtomicBool::new(false)).unwrap();
        let h = header(&l);
        let i = intents(&l);
        let index = *i.last().unwrap();
        let name = h.records[index].change.name.clone();
        let external = RawValue {
            kind: 4,
            bytes: 42_u32.to_le_bytes().to_vec(),
        };
        r.values.insert(name.clone(), external.clone());
        assert!(restore_backend(&h, &i, &mut r, &mut l, &AtomicBool::new(false)).is_err());
        assert_eq!(r.values[&name], external);
    }
    #[test]
    fn tampered_address_and_duplicate_records_fail() {
        let p = plan();
        let mut r = Fake::default();
        let mut l = Log::default();
        apply(&p, &mut r, &mut l, &AtomicBool::new(false)).unwrap();
        let mut h = header(&l);
        h.records[0].change.path = "Software\\Unexpected".into();
        assert!(validate_header(&h).is_err());
        let mut h = header(&l);
        h.records.push(h.records[0].clone());
        assert!(validate_header(&h).is_err());
    }
    #[test]
    fn already_set_creates_no_registry_write() {
        let p = plan();
        let mut r = Fake::default();
        for c in p.actions.iter().flat_map(|a| &a.changes) {
            r.values.insert(c.name.clone(), RawValue::desired(c));
        }
        let mut l = Log::default();
        let o = apply(&p, &mut r, &mut l, &AtomicBool::new(false)).unwrap();
        assert_eq!(o.changed, 0);
        assert_eq!(r.writes, 0);
        assert!(o.already_set > 0);
    }
}
