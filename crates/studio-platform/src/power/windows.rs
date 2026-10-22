//! https://learn.microsoft.com/windows/win32/api/powrprof/
use super::{
    PowerChoice, PowerPlan,
    engine::{self, Backend, Outcome},
    store::{self, FileJournal},
    valid_guid,
};
use anyhow::{Context, Result, ensure};
use std::{
    ffi::c_void,
    path::PathBuf,
    ptr,
    sync::{Arc, atomic::AtomicBool},
    time::{SystemTime, UNIX_EPOCH},
};
use studio_core::Target;

#[repr(C)]
#[derive(Clone, Copy)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}
impl Guid {
    fn parse(text: &str) -> Result<Self> {
        ensure!(valid_guid(text), "Invalid power GUID");
        let tail: String = text[19..].chars().filter(|c| *c != '-').collect();
        let mut data4 = [0; 8];
        for (i, byte) in data4.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&tail[i * 2..i * 2 + 2], 16)?;
        }
        Ok(Self {
            data1: u32::from_str_radix(&text[..8], 16)?,
            data2: u16::from_str_radix(&text[9..13], 16)?,
            data3: u16::from_str_radix(&text[14..18], 16)?,
            data4,
        })
    }
    fn text(self) -> String {
        let d = self.data4;
        format!(
            "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            self.data1, self.data2, self.data3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
        )
    }
}
#[link(name = "powrprof")]
unsafe extern "system" {
    fn PowerGetActiveScheme(root: *const c_void, scheme: *mut *mut Guid) -> u32;
    fn PowerReadACValueIndex(
        root: *const c_void,
        scheme: *const Guid,
        subgroup: *const Guid,
        setting: *const Guid,
        value: *mut u32,
    ) -> u32;
    fn PowerWriteACValueIndex(
        root: *const c_void,
        scheme: *const Guid,
        subgroup: *const Guid,
        setting: *const Guid,
        value: u32,
    ) -> u32;
    fn PowerSetActiveScheme(root: *const c_void, scheme: *const Guid) -> u32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
}
fn status(code: u32, operation: &str) -> Result<()> {
    ensure!(code == 0, "{operation} failed (Windows error {code})");
    Ok(())
}
struct NativePower;
impl Backend for NativePower {
    fn active(&self) -> Result<String> {
        let mut allocated = ptr::null_mut();
        // The API allocates one GUID with LocalAlloc; copy it before releasing ownership.
        status(
            unsafe { PowerGetActiveScheme(ptr::null(), &mut allocated) },
            "Read active power scheme",
        )?;
        ensure!(
            !allocated.is_null(),
            "Windows returned no active power scheme"
        );
        let guid = unsafe { *allocated };
        ensure!(
            unsafe { LocalFree(allocated.cast()) }.is_null(),
            "Free active power scheme allocation failed"
        );
        Ok(guid.text())
    }
    fn read(&self, scheme: &str, choice: &PowerChoice) -> Result<u32> {
        let (scheme, subgroup, setting) = (
            Guid::parse(scheme)?,
            Guid::parse(&choice.subgroup)?,
            Guid::parse(&choice.setting)?,
        );
        let mut value = 0;
        status(
            unsafe { PowerReadACValueIndex(ptr::null(), &scheme, &subgroup, &setting, &mut value) },
            "Read AC power value",
        )?;
        Ok(value)
    }
    fn write(&mut self, scheme: &str, choice: &PowerChoice, value: u32) -> Result<()> {
        let (scheme, subgroup, setting) = (
            Guid::parse(scheme)?,
            Guid::parse(&choice.subgroup)?,
            Guid::parse(&choice.setting)?,
        );
        status(
            unsafe { PowerWriteACValueIndex(ptr::null(), &scheme, &subgroup, &setting, value) },
            "Write AC power value",
        )
    }
    fn activate(&mut self, scheme: &str) -> Result<()> {
        let scheme = Guid::parse(scheme)?;
        status(
            unsafe { PowerSetActiveScheme(ptr::null(), &scheme) },
            "Activate AC changes in the existing power scheme",
        )
    }
}
fn check_host(target: Target) -> Result<()> {
    let host = crate::host();
    ensure!(
        host.can_apply && host.target == Some(target),
        "Power changes require the matching supported LTSC host: {}",
        host.explanation
    );
    Ok(())
}
fn report(operation: &str, outcome: Outcome, journal: PathBuf) -> crate::RunReport {
    crate::RunReport {
        summary: format!(
            "{operation} {}: {} verified AC values; {} already set. Battery settings and other schemes were preserved.",
            if outcome.cancelled {
                "cancelled"
            } else {
                "complete"
            },
            outcome.changed,
            outcome.already_set
        ),
        journal,
        changed: outcome.changed,
        already_set: outcome.already_set,
        cancelled: outcome.cancelled,
    }
}
pub(super) fn inspect(
    plan: PowerPlan,
    cancel: Arc<AtomicBool>,
) -> Result<Vec<crate::SettingState>> {
    check_host(plan.target)?;
    engine::inspect_backend(&plan, &NativePower, &cancel)
}
pub(super) fn apply(plan: PowerPlan, cancel: Arc<AtomicBool>) -> Result<crate::RunReport> {
    check_host(plan.target)?;
    ensure!(!plan.choices.is_empty(), "Select at least one power choice");
    let directory = crate::data_directory()?;
    let _lock = store::mutation_lock(&directory)?;
    let recovery = directory.join("power-recovery");
    store::ensure_directory(&recovery, true)?;
    ensure!(
        store::paths(&recovery)?.len() < 256,
        "Power recovery journal limit reached (256)"
    );
    store::validate_key_path(&directory)?;
    let key = crate::journal::key(&directory, true)?;
    let mut random = [0; 16];
    getrandom::fill(&mut random).map_err(|e| anyhow::anyhow!("Generate power recovery ID: {e}"))?;
    let id: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let started_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("Clock predates UNIX epoch")?
        .as_secs()
        .to_string();
    let path = recovery.join(format!("{id}.jsonl"));
    let mut journal = FileJournal::create(path.clone(), key)?;
    let outcome = engine::apply_backend(
        &plan,
        &mut NativePower,
        &mut journal,
        id,
        started_at,
        &cancel,
    )
    .with_context(|| {
        if journal.has_intent {
            format!(
                "Power apply stopped; partial changes may exist. Recovery journal: {}",
                path.display()
            )
        } else {
            "Power apply stopped before any AC mutation".into()
        }
    })?;
    Ok(report("Power apply", outcome, path))
}
pub(super) fn restore(path: PathBuf, cancel: Arc<AtomicBool>) -> Result<crate::RunReport> {
    let directory = crate::data_directory()?;
    let _lock = store::mutation_lock(&directory)?;
    let path = store::journal_path(&path, &directory.join("power-recovery"))?;
    store::validate_key_path(&directory)?;
    let key = crate::journal::key(&directory, false)?;
    let loaded = store::load(&path, &key)?;
    ensure!(
        path.file_stem()
            .is_some_and(|stem| stem == loaded.header.id.as_str()),
        "Power recovery filename differs from signed ID"
    );
    check_host(loaded.header.plan.target)?;
    let mut journal = FileJournal::resume(path.clone(), key, &loaded)?;
    let outcome = engine::restore_backend(
        &loaded.header,
        &loaded.intended,
        &mut NativePower,
        &mut journal,
        &cancel,
    )
    .with_context(|| {
        format!(
            "Power restore stopped; recovery may be partial. Journal: {}",
            path.display()
        )
    })?;
    Ok(report("Power restore", outcome, path))
}
pub(super) fn recovery_points() -> Result<Vec<crate::RecoveryPoint>> {
    let directory = crate::data_directory()?;
    let recovery = directory.join("power-recovery");
    if !recovery.try_exists()? {
        return Ok(Vec::new());
    }
    store::ensure_directory(&directory, false)?;
    store::validate_key_path(&directory)?;
    let key = crate::journal::key(&directory, false)?;
    let mut points = Vec::new();
    for path in store::paths(&recovery)? {
        let loaded = store::load(&path, &key)
            .with_context(|| format!("Inspect power recovery {}", path.display()))?;
        ensure!(
            path.file_stem()
                .is_some_and(|stem| stem == loaded.header.id.as_str()),
            "Power recovery filename differs from signed ID"
        );
        points.push(crate::RecoveryPoint {
            path,
            id: loaded.header.id,
            status: loaded.status,
            started_at: loaded.header.started_at,
            actions: loaded.header.plan.choices.len(),
        });
    }
    points.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    Ok(points)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guid_abi_roundtrips_and_checks_direct_status_codes() {
        let text = "381b4222-f694-41f0-9685-ff5bb260df2e";
        assert_eq!(Guid::parse(text).unwrap().text(), text);
        assert_eq!(std::mem::size_of::<Guid>(), 16);
        assert!(status(5, "fake").is_err());
        assert!(status(0, "fake").is_ok());
    }
}
