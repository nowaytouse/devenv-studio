//! Reviewed macOS Topgrade maintenance. Local history is execution evidence.
use crate::DeploymentEvent;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::File,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool, mpsc::Sender},
};

#[cfg(target_os = "macos")]
mod native;
#[cfg(any(target_os = "macos", test))]
mod protocol;

pub const TERMINAL_COMMAND: &str = "\"$HOME/.local/bin/topgrade\"";

#[derive(Debug, Clone)]
pub struct Step {
    pub id: String,
    pub label: String,
    pub detail: String,
    pub terminal: bool,
    pub available: bool,
}

#[derive(Debug, Clone)]
pub struct Check {
    pub available: bool,
    pub detail: String,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone)]
pub struct Review {
    pub steps: Vec<Step>,
    #[cfg(target_os = "macos")]
    proof: native::Proof,
    #[cfg(target_os = "macos")]
    selected: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Verified,
    Failed,
    Cancelled,
    TimedOut,
    Incomplete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Success,
    Failed,
    Skipped,
    TimedOut,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepReport {
    pub id: String,
    pub status: StepStatus,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub path: PathBuf,
    pub run_id: String,
    pub status: RunStatus,
    pub steps: Vec<StepReport>,
    pub detail: String,
}

pub fn check(cancel: Arc<AtomicBool>) -> Result<Check> {
    #[cfg(target_os = "macos")]
    return native::check(&cancel);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = cancel;
        Ok(Check {
            available: false,
            detail: "Topgrade maintenance is available on macOS only.".into(),
            steps: Vec::new(),
        })
    }
}

pub fn review(selected: &BTreeSet<String>, cancel: Arc<AtomicBool>) -> Result<Review> {
    #[cfg(target_os = "macos")]
    return native::review(selected, &cancel);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (selected, cancel);
        anyhow::bail!("Topgrade maintenance is available on macOS only")
    }
}

pub fn run(
    review: Review,
    cancel: Arc<AtomicBool>,
    events: Option<Sender<DeploymentEvent>>,
) -> Result<Report> {
    #[cfg(target_os = "macos")]
    return native::run(review, &cancel, events.as_ref());
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (review, cancel, events);
        anyhow::bail!("Topgrade maintenance is available on macOS only")
    }
}

pub fn history() -> Result<Vec<Report>> {
    #[cfg(target_os = "macos")]
    return native::history();
    #[cfg(not(target_os = "macos"))]
    Ok(Vec::new())
}

/// Resolve the logged-in macOS user's account home and reject a conflicting HOME.
pub fn account_home() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    return native::account_home();
    #[cfg(not(target_os = "macos"))]
    anyhow::bail!("The macOS account home is unavailable on this operating system")
}

/// Hash a bounded, regular macOS engine or profile asset.
pub fn sha256_file(path: &std::path::Path) -> Result<String> {
    #[cfg(target_os = "macos")]
    return native::sha256_file(path);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        anyhow::bail!("The macOS engine hash is unavailable on this operating system")
    }
}

/// Hold through the entire non-maintenance mutation. The engine acquires this lock itself.
pub fn acquire_mutation_lock() -> Result<File> {
    #[cfg(target_os = "macos")]
    return native::acquire_mutation_lock();
    #[cfg(not(target_os = "macos"))]
    anyhow::bail!("The macOS maintenance lock is unavailable on this operating system")
}
