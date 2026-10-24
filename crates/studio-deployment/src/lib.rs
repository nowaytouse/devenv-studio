//! Opt-in native deployment tasks. Execution history is evidence, never rollback.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool, mpsc::Sender},
};
use studio_core::Target;

pub mod backup;
pub mod cargo_tools;
mod developer_ids;
#[cfg(any(windows, test))]
mod engine;
pub mod environment;
mod history;
pub mod mise_tools;
pub mod network;
pub mod npm_tools;
#[cfg(any(windows, test))]
mod pip;
#[cfg(any(windows, target_os = "macos", test))]
mod process;
pub mod python_tools;
pub mod scoop_tools;
mod storage;
pub mod toolchains;
pub mod vscode_tools;
#[cfg(windows)]
mod windows;

pub use developer_ids::normalize_pip_id;
pub use history::{HistoryEvidence, HistoryRecord, HistoryTask, history};
pub use storage::{NotificationState, StorageStatus};

pub const MAX_SESSION_TASKS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub revision: String,
    pub tasks: Vec<Task>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub description: String,
    pub category: String,
    pub kind: String,
    pub targets: Vec<Target>,
    pub requires_admin: bool,
    pub restart: String,
    pub limitations: String,
    pub evidence: Vec<String>,
    pub package_id: Option<String>,
    pub feature_name: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentPlan {
    pub schema_version: u32,
    pub catalog_revision: String,
    pub target: Target,
    pub tasks: Vec<Task>,
}
#[derive(Debug, Clone)]
pub struct Capability {
    pub available: bool,
    pub detail: String,
}
#[derive(Debug, Clone)]
pub struct ProviderCapability {
    pub provider: String,
    pub available: bool,
    pub detail: String,
}
#[derive(Debug, Clone)]
pub struct Detection {
    pub id: String,
    pub state: String,
    pub detail: String,
}
#[derive(Debug, Clone)]
pub struct DeploymentReport {
    pub summary: String,
    pub history: PathBuf,
    pub completed: usize,
    pub failed: usize,
    pub cancelled: bool,
    pub restart_required: bool,
}
#[derive(Debug, Clone)]
pub struct DeploymentEvent {
    pub task_id: String,
    pub message: String,
    pub completed: usize,
    pub total: usize,
}

fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}
fn safe_package_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.+".contains(&b))
}
pub fn catalog() -> Result<Catalog> {
    let catalog: Catalog = serde_json::from_str(include_str!("../../../data/deployment.json"))
        .context("Invalid embedded deployment catalog")?;
    ensure!(
        !catalog.revision.is_empty() && catalog.tasks.len() <= 2048 && !catalog.tasks.is_empty(),
        "Invalid deployment catalog bounds"
    );
    let mut ids = BTreeSet::new();
    let mut packages = BTreeSet::new();
    let mut developer_packages = BTreeSet::new();
    let mut features = BTreeSet::new();
    for task in &catalog.tasks {
        ensure!(
            safe_id(&task.id) && ids.insert(&task.id),
            "Duplicate or invalid deployment task ID"
        );
        ensure!(
            !task.targets.is_empty()
                && task.targets.iter().collect::<BTreeSet<_>>().len() == task.targets.len(),
            "Invalid deployment targets"
        );
        for text in [
            &task.title,
            &task.description,
            &task.restart,
            &task.limitations,
        ] {
            ensure!(
                !text.trim().is_empty() && text.len() < 8192 && !text.contains('\0'),
                "Incomplete task {}",
                task.id
            );
        }
        ensure!(
            !task.evidence.is_empty() && task.evidence.len() <= 8,
            "Missing deployment provenance"
        );
        for url in &task.evidence {
            let value = url
                .strip_prefix("https://")
                .context("Deployment evidence must use HTTPS")?;
            let authority = value.split('/').next().unwrap_or_default();
            ensure!(
                authority.contains('.')
                    && authority
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
                    && !url
                        .chars()
                        .any(|c| c.is_control() || c.is_whitespace() || c == '\\'),
                "Invalid evidence URL"
            );
        }
        match task.kind.as_str() {
            "package" => {
                let id = task.package_id.as_deref().context("Missing package ID")?;
                ensure!(
                    task.category == "software"
                        && safe_package_id(id)
                        && id.contains('.')
                        && packages.insert(id)
                        && task.feature_name.is_none(),
                    "Invalid package task"
                );
            }
            "feature" => {
                let name = task
                    .feature_name
                    .as_deref()
                    .context("Missing feature name")?;
                ensure!(
                    task.category == "features"
                        && task.requires_admin
                        && safe_id(name)
                        && features.insert(name)
                        && task.package_id.is_none(),
                    "Invalid feature task"
                );
            }
            "cargo" | "uv" | "vscode" | "npm" | "pip" => {
                let id = task
                    .package_id
                    .as_deref()
                    .context("Missing developer package ID")?;
                ensure!(
                    task.category == "developer"
                        && !task.requires_admin
                        && task.feature_name.is_none()
                        && developer_ids::valid(&task.kind, id),
                    "Invalid developer task {}",
                    task.id
                );
                ensure!(
                    developer_packages.insert((task.kind.as_str(), id)),
                    "Duplicate package identifier in developer catalog"
                );
            }
            "maintenance" => {
                ensure!(
                    ((task.category == "maintenance"
                        && matches!(
                            task.id.as_str(),
                            "maintenance-check-health"
                                | "maintenance-scan-health"
                                | "maintenance-restore-health"
                                | "maintenance-sfc-verify"
                        ))
                        || (task.category == "storage" && storage::is_task(&task.id)))
                        && task.requires_admin
                        && task.package_id.is_none()
                        && task.feature_name.is_none(),
                    "Unsupported maintenance task"
                );
            }
            _ => anyhow::bail!("Unsupported deployment kind {}", task.kind),
        }
    }
    Ok(catalog)
}
pub fn build_plan(target: Target, selected: &BTreeSet<String>) -> Result<DeploymentPlan> {
    let catalog = catalog()?;
    ensure!(
        selected.len() <= catalog.tasks.len(),
        "Too many deployment selections"
    );
    for id in selected {
        let task = catalog
            .tasks
            .iter()
            .find(|t| &t.id == id)
            .with_context(|| format!("Unknown deployment task {id}"))?;
        ensure!(
            task.targets.contains(&target),
            "Task {id} is unavailable for {}",
            target.label()
        );
    }
    let mut tasks: Vec<_> = catalog
        .tasks
        .into_iter()
        .filter(|t| selected.contains(&t.id))
        .collect();
    tasks.sort_by_key(|task| task_order(&task.kind));
    Ok(DeploymentPlan {
        schema_version: 1,
        catalog_revision: catalog.revision,
        target,
        tasks,
    })
}
fn task_order(kind: &str) -> u8 {
    match kind {
        "package" => 0,
        "cargo" | "uv" | "vscode" | "npm" | "pip" => 1,
        "feature" => 2,
        _ => 3,
    }
}
fn validate_plan(plan: &DeploymentPlan) -> Result<()> {
    let selected: BTreeSet<_> = plan.tasks.iter().map(|t| t.id.clone()).collect();
    ensure!(
        selected.len() == plan.tasks.len(),
        "Duplicate deployment task"
    );
    ensure!(
        *plan == build_plan(plan.target, &selected)?,
        "Deployment plan differs from trusted built-in catalog; rebuild it"
    );
    Ok(())
}
pub fn winget_capability() -> Capability {
    #[cfg(windows)]
    {
        windows::winget_capability()
    }
    #[cfg(not(windows))]
    {
        Capability {available:false,detail:"WinGet is inspected only on Windows. Preview does not detect installed Windows applications. See https://learn.microsoft.com/windows/package-manager/winget/ for App Installer prerequisites.".into()}
    }
}
/// Cheap fixed-location provider discovery. Never launches a process or installs a prerequisite.
pub fn provider_capabilities() -> Vec<ProviderCapability> {
    #[cfg(windows)]
    {
        windows::provider_capabilities()
    }
    #[cfg(not(windows))]
    {
        ["winget","cargo","uv","vscode","npm","pip"].into_iter().map(|provider|ProviderCapability{provider:provider.into(),available:false,detail:"Windows deployment provider discovery is unavailable in preview. No local developer tools or Windows installations were inspected.".into()}).collect()
    }
}
/// Reads filesystem notification policy only; never scans, optimizes or changes a volume.
pub fn storage_status(cancel: Arc<AtomicBool>) -> Result<StorageStatus> {
    ensure!(
        !cancel.load(std::sync::atomic::Ordering::Acquire),
        "Storage inspection cancelled"
    );
    #[cfg(windows)]
    {
        windows::storage_status(cancel)
    }
    #[cfg(not(windows))]
    {
        Ok(StorageStatus {
            preview: true,
            ntfs: NotificationState::Unknown,
            refs: NotificationState::Unknown,
            detail: "Preview only. No Windows filesystem, device TRIM support or optimization state was inspected.".into(),
        })
    }
}
pub fn inspect(
    target: Target,
    selected: &BTreeSet<String>,
    cancel: Arc<AtomicBool>,
) -> Result<Vec<Detection>> {
    let plan = build_plan(target, selected)?;
    ensure!(
        plan.tasks.len() <= MAX_SESSION_TASKS,
        "An inspection session supports at most 128 tasks; split the selection into separate runs"
    );
    #[cfg(windows)]
    {
        windows::inspect(plan, cancel)
    }
    #[cfg(not(windows))]
    {
        ensure!(
            !cancel.load(std::sync::atomic::Ordering::Acquire),
            "Inspection cancelled"
        );
        Ok(plan
            .tasks
            .into_iter()
            .map(|t| Detection {
                id: t.id,
                state: "preview".into(),
                detail: "未检查 Windows 主机；软件与系统组件状态仅在受支持的 Windows LTSC 上读取。"
                    .into(),
            })
            .collect())
    }
}
pub fn execute(
    plan: DeploymentPlan,
    cancel: Arc<AtomicBool>,
    events: Option<Sender<DeploymentEvent>>,
) -> Result<DeploymentReport> {
    validate_plan(&plan)?;
    ensure!(
        plan.tasks.len() <= MAX_SESSION_TASKS,
        "A deployment session supports at most 128 tasks; split the selection into separate reviewed runs"
    );
    ensure!(
        !plan.tasks.is_empty(),
        "Select at least one deployment task"
    );
    #[cfg(windows)]
    {
        windows::execute(plan, cancel, events)
    }
    #[cfg(not(windows))]
    {
        let _ = (cancel, events);
        anyhow::bail!(
            "Deployment is unavailable on this operating system. Use a supported Windows LTSC host; no software or system component was changed."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn storage_preview_does_not_invent_a_windows_policy() {
        #[cfg(not(windows))]
        {
            let status = storage_status(Arc::new(AtomicBool::new(false))).unwrap();
            assert!(status.preview);
            assert_eq!(
                (status.ntfs, status.refs),
                (NotificationState::Unknown, NotificationState::Unknown)
            );
            assert!(storage_status(Arc::new(AtomicBool::new(true))).is_err());
        }
    }
    #[test]
    fn catalog_tasks_rebuild_exactly_without_default_selection() {
        let c = catalog().unwrap();
        assert!(
            build_plan(Target::Win10Ltsc2021, &BTreeSet::new())
                .unwrap()
                .tasks
                .is_empty()
        );
        let mut seen = BTreeSet::new();
        for task in &c.tasks {
            assert!(seen.insert(&task.id));
            for target in &task.targets {
                let plan = build_plan(*target, &BTreeSet::from([task.id.clone()])).unwrap();
                assert_eq!(plan.tasks, vec![task.clone()]);
                validate_plan(&plan).unwrap();
            }
            assert!(matches!(
                task.kind.as_str(),
                "package" | "feature" | "maintenance" | "cargo" | "uv" | "vscode" | "npm" | "pip"
            ));
        }
    }
    #[test]
    fn imported_payload_and_unknown_tasks_rejected() {
        let c = catalog().unwrap();
        assert!(build_plan(Target::Win10Ltsc2021, &BTreeSet::from(["unknown".into()])).is_err());
        let mut p = build_plan(
            Target::Win10Ltsc2021,
            &BTreeSet::from([c.tasks[0].id.clone()]),
        )
        .unwrap();
        p.tasks[0].package_id = Some("Unexpected.Installer".into());
        assert!(validate_plan(&p).is_err());
    }
    #[test]
    fn dependency_order_and_session_bounds_are_explicit() {
        let catalog = catalog().unwrap();
        let selected: BTreeSet<_> = catalog
            .tasks
            .iter()
            .filter(|task| task.targets.contains(&Target::Win11Ltsc2024))
            .map(|task| task.id.clone())
            .collect();
        let plan = build_plan(Target::Win11Ltsc2024, &selected).unwrap();
        assert!(
            plan.tasks
                .windows(2)
                .all(|tasks| task_order(&tasks[0].kind) <= task_order(&tasks[1].kind))
        );
        assert_eq!(plan.tasks.len(), selected.len());
        if plan.tasks.len() > 128 {
            let error = execute(plan, Arc::new(AtomicBool::new(false)), None).unwrap_err();
            assert!(error.to_string().contains("at most 128"));
        }
        let original = catalog
            .tasks
            .iter()
            .find(|task| {
                matches!(
                    task.kind.as_str(),
                    "cargo" | "uv" | "vscode" | "npm" | "pip"
                )
            })
            .unwrap();
        let mut plan =
            build_plan(original.targets[0], &BTreeSet::from([original.id.clone()])).unwrap();
        plan.tasks[0].package_id = Some("--untrusted-option".into());
        assert!(validate_plan(&plan).is_err());
    }
    #[test]
    fn preview_never_mutates_or_claims_detection() {
        #[cfg(not(windows))]
        {
            let c = catalog().unwrap();
            let ids = BTreeSet::from([c.tasks[0].id.clone()]);
            let p = build_plan(Target::Win10Ltsc2021, &ids).unwrap();
            let cancel = Arc::new(AtomicBool::new(false));
            assert!(execute(p, cancel.clone(), None).is_err());
            assert_eq!(
                inspect(Target::Win10Ltsc2021, &ids, cancel).unwrap()[0].state,
                "preview"
            );
        }
    }
}
