//! Native registry integration with an honest preview on other operating systems.
use anyhow::{Context, Result};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};
use studio_core::{Plan, Target};

pub mod power;

#[cfg(any(windows, test))]
mod engine;
#[cfg(any(windows, test))]
mod journal;
#[cfg(windows)]
mod windows;

#[derive(Debug, Clone)]
pub struct HostInfo {
    pub name: String,
    pub target: Option<Target>,
    pub can_apply: bool,
    pub explanation: String,
}
#[derive(Debug, Clone)]
pub struct RunReport {
    pub summary: String,
    pub journal: PathBuf,
    pub changed: usize,
    pub already_set: usize,
    pub cancelled: bool,
}
#[derive(Debug, Clone)]
pub struct RecoveryPoint {
    pub path: PathBuf,
    pub id: String,
    pub status: String,
    pub started_at: String,
    pub actions: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingState {
    pub id: String,
    pub state: String,
    pub detail: String,
}

/// Read only; a preview host does not impersonate a Windows registry inspection.
pub fn inspect(plan: Plan, cancel: Arc<AtomicBool>) -> Result<Vec<SettingState>> {
    let plan = studio_core::validate_plan(&plan)?;
    #[cfg(windows)]
    {
        windows::inspect(plan, cancel)
    }
    #[cfg(not(windows))]
    {
        let mut states = Vec::new();
        for action in plan.actions {
            if cancel.load(std::sync::atomic::Ordering::Acquire) {
                anyhow::bail!("Configuration inspection cancelled");
            }
            states.push(SettingState {
                id: action.id,
                state: "preview".into(),
                detail: "预览模式；未读取 Windows 当前设置。".into(),
            });
        }
        Ok(states)
    }
}

pub fn host() -> HostInfo {
    #[cfg(windows)]
    {
        windows::host()
    }
    #[cfg(not(windows))]
    {
        HostInfo {name:format!("{} preview",std::env::consts::OS),target:None,can_apply:false,explanation:"Preview and export Windows configuration plans here. Applying and restoring require a supported Windows LTSC host; this computer's registry has not been inspected.".into()}
    }
}
pub fn data_directory() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        Ok(
            PathBuf::from(std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is unavailable")?)
                .join("LTSC Studio"),
        )
    }
    #[cfg(target_os = "macos")]
    {
        Ok(
            PathBuf::from(std::env::var_os("HOME").context("Home directory is unavailable")?)
                .join("Library/Application Support/LTSC Studio"),
        )
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        let base = match std::env::var_os("XDG_DATA_HOME") {
            Some(v) => PathBuf::from(v),
            None => {
                PathBuf::from(std::env::var_os("HOME").context("Home directory is unavailable")?)
                    .join(".local/share")
            }
        };
        Ok(base.join("ltsc-studio"))
    }
}
pub fn apply(plan: Plan, cancel: Arc<AtomicBool>) -> Result<RunReport> {
    #[cfg(windows)]
    {
        windows::apply(plan, cancel)
    }
    #[cfg(not(windows))]
    {
        let _ = (plan, cancel);
        anyhow::bail!(
            "Applying registry changes is unavailable on this operating system; export a plan for a supported Windows LTSC host"
        )
    }
}
pub fn recovery_points() -> Result<Vec<RecoveryPoint>> {
    #[cfg(windows)]
    {
        windows::recovery_points()
    }
    #[cfg(not(windows))]
    {
        Ok(Vec::new())
    }
}
pub fn restore(path: PathBuf, cancel: Arc<AtomicBool>) -> Result<RunReport> {
    #[cfg(windows)]
    {
        windows::restore(path, cancel)
    }
    #[cfg(not(windows))]
    {
        let _ = (path, cancel);
        anyhow::bail!("Restoring Windows registry changes is unavailable on this operating system")
    }
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;
    #[test]
    fn preview_cannot_mutate() {
        #[cfg(not(windows))]
        {
            assert!(!host().can_apply);
            assert!(host().target.is_none());
            let p = studio_core::build_plan(
                &studio_core::load_catalog().unwrap(),
                Target::Win10Ltsc2021,
                &Default::default(),
            )
            .unwrap();
            assert!(apply(p, Arc::new(AtomicBool::new(false))).is_err());
            assert!(restore(PathBuf::new(), Arc::new(AtomicBool::new(false))).is_err());
            let p = studio_core::build_plan(
                &studio_core::load_catalog().unwrap(),
                Target::Win11Ltsc2024,
                &std::collections::BTreeSet::from(["privacy-advertising-id".into()]),
            )
            .unwrap();
            let states = inspect(p.clone(), Arc::new(AtomicBool::new(false))).unwrap();
            assert_eq!(states.len(), 1);
            assert_eq!(states[0].state, "preview");
            assert!(inspect(p, Arc::new(AtomicBool::new(true))).is_err());
        }
    }
}
