use crate::{
    HostInfo, RecoveryPoint, RunReport, SettingState, data_directory,
    engine::{self, RawValue, Registry},
    journal::{self, FileJournal},
};
use anyhow::{Context, Result, ensure};
use std::{
    borrow::Cow,
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::{SystemTime, UNIX_EPOCH},
};
use studio_core::{Hive, Plan, RegistryChange, Target};
use winreg::{RegKey, RegValue, enums::*};

fn detected_target() -> Result<(Target, String)> {
    let key = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            KEY_READ | KEY_WOW64_64KEY,
        )
        .context("Read Windows host edition and build")?;
    let build: String = key.get_value("CurrentBuildNumber")?;
    let edition: String = key.get_value("EditionID")?;
    let installation: String = key.get_value("InstallationType")?;
    ensure!(
        installation == "Client",
        "Unsupported installation type: {installation}"
    );
    ensure!(
        edition == "EnterpriseS" || edition == "IoTEnterpriseS",
        "Unsupported edition {edition}; Enterprise LTSC is required"
    );
    let target = match build.as_str() {
        "19044" => Target::Win10Ltsc2021,
        "26100" => Target::Win11Ltsc2024,
        _ => anyhow::bail!("Unsupported Windows build {build}; expected LTSC 19044 or 26100"),
    };
    Ok((
        target,
        format!("{} · {edition} · build {build}", target.label()),
    ))
}
pub(super) fn host() -> HostInfo {
    match detected_target() {
        Ok((target,name))=>HostInfo{name,target:Some(target),can_apply:true,explanation:"Supported Windows LTSC host. Each selected registry value is backed up and verified. Machine-wide settings require existing write permission; this app does not elevate privileges.".into()},
        Err(e)=>HostInfo{name:"Windows · unsupported or unreadable host".into(),target:None,can_apply:false,explanation:format!("Registry changes are unavailable: {e:#}")},
    }
}
fn check_host(target: Target) -> Result<()> {
    let (actual, _) = detected_target()?;
    ensure!(
        actual == target,
        "Plan targets {}, but this host is {}",
        target.label(),
        actual.label()
    );
    Ok(())
}
struct NativeRegistry;
fn root(hive: Hive) -> RegKey {
    RegKey::predef(match hive {
        Hive::CurrentUser => HKEY_CURRENT_USER,
        Hive::LocalMachine => HKEY_LOCAL_MACHINE,
    })
}
fn permission(error: io::Error, change: &RegistryChange) -> anyhow::Error {
    if error.kind() == io::ErrorKind::PermissionDenied {
        anyhow::anyhow!(
            "Access denied for {:?}\\{}\\{}. Machine-wide settings require an account/process with existing registry write permission; no elevation was attempted",
            change.hive,
            change.path,
            change.name
        )
    } else {
        anyhow::anyhow!(error).context(format!(
            "Registry {:?}\\{}\\{}",
            change.hive, change.path, change.name
        ))
    }
}
fn kind(value: u32) -> Result<RegType> {
    Ok(match value {
        0 => REG_NONE,
        1 => REG_SZ,
        2 => REG_EXPAND_SZ,
        3 => REG_BINARY,
        4 => REG_DWORD,
        5 => REG_DWORD_BIG_ENDIAN,
        6 => REG_LINK,
        7 => REG_MULTI_SZ,
        8 => REG_RESOURCE_LIST,
        9 => REG_FULL_RESOURCE_DESCRIPTOR,
        10 => REG_RESOURCE_REQUIREMENTS_LIST,
        11 => REG_QWORD,
        _ => anyhow::bail!("Unsupported raw registry type {value}"),
    })
}
impl Registry for NativeRegistry {
    fn preflight(&self, change: &RegistryChange) -> Result<()> {
        let root = root(change.hive);
        let mut path = change.path.as_str();
        loop {
            match root.open_subkey_with_flags(
                path,
                KEY_QUERY_VALUE | KEY_SET_VALUE | KEY_CREATE_SUB_KEY | KEY_WOW64_64KEY,
            ) {
                Ok(_) => return Ok(()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    path = path
                        .rsplit_once('\\')
                        .map(|(parent, _)| parent)
                        .context("No writable parent registry key")?;
                }
                Err(e) => return Err(permission(e, change)),
            }
        }
    }
    fn read(&self, change: &RegistryChange) -> Result<Option<RawValue>> {
        let key = match root(change.hive)
            .open_subkey_with_flags(&change.path, KEY_QUERY_VALUE | KEY_WOW64_64KEY)
        {
            Ok(key) => key,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(permission(e, change)),
        };
        match key.get_raw_value(&change.name) {
            Ok(value) => Ok(Some(RawValue {
                kind: value.vtype as u32,
                bytes: value.bytes.into_owned(),
            })),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(permission(e, change)),
        }
    }
    fn write(&mut self, change: &RegistryChange, value: Option<&RawValue>) -> Result<()> {
        match value {
            Some(value) => {
                let (key, _) = root(change.hive)
                    .create_subkey_with_flags(
                        &change.path,
                        KEY_QUERY_VALUE | KEY_SET_VALUE | KEY_WOW64_64KEY,
                    )
                    .map_err(|e| permission(e, change))?;
                key.set_raw_value(
                    &change.name,
                    &RegValue {
                        bytes: Cow::Borrowed(&value.bytes),
                        vtype: kind(value.kind)?,
                    },
                )
                .map_err(|e| permission(e, change))?;
            }
            None => {
                let key = match root(change.hive)
                    .open_subkey_with_flags(&change.path, KEY_SET_VALUE | KEY_WOW64_64KEY)
                {
                    Ok(key) => key,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
                    Err(e) => return Err(permission(e, change)),
                };
                match key.delete_value(&change.name) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(permission(e, change)),
                }
            }
        }
        Ok(())
    }
}

fn ensure_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .with_context(|| format!("Create data directory {}", path.display()))?;
    ensure!(
        fs::symlink_metadata(path)?.is_dir(),
        "Data path must be a real directory"
    );
    Ok(())
}
pub(super) fn mutation_lock(directory: &Path) -> Result<File> {
    ensure_directory(directory)?;
    let path = directory.join("mutation.lock");
    if path.try_exists()? {
        ensure!(
            !fs::symlink_metadata(&path)?.file_type().is_symlink(),
            "Mutation lock cannot be a symbolic link"
        );
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    file.try_lock()
        .context("Another DevEnv Studio apply or restore is running; mutation is serialized")?;
    Ok(file)
}
fn unique_id() -> Result<String> {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|e| anyhow::anyhow!("Generate recovery ID: {e}"))?;
    Ok(random.iter().map(|b| format!("{b:02x}")).collect())
}
fn timestamp() -> Result<String> {
    Ok(format!(
        "{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("System clock predates UNIX epoch")?
            .as_secs()
    ))
}
fn report(operation: &str, outcome: engine::Outcome, journal: PathBuf) -> RunReport {
    let summary = if outcome.cancelled {
        format!(
            "{operation} cancelled after {} verified changes; {} already at the required value. Recovery information is saved.",
            outcome.changed, outcome.already_set
        )
    } else {
        format!(
            "{operation} complete: {} verified changes; {} already at the required value.",
            outcome.changed, outcome.already_set
        )
    };
    RunReport {
        summary,
        journal,
        changed: outcome.changed,
        already_set: outcome.already_set,
        cancelled: outcome.cancelled,
    }
}
pub(super) fn inspect(plan: Plan, cancel: Arc<AtomicBool>) -> Result<Vec<SettingState>> {
    check_host(plan.target)?;
    let registry = NativeRegistry;
    let mut states = Vec::new();
    for action in plan.actions {
        if cancel.load(std::sync::atomic::Ordering::Acquire) {
            anyhow::bail!("Configuration inspection cancelled");
        }
        let mut aligned = 0;
        let mut details = Vec::new();
        let mut unavailable = false;
        for change in &action.changes {
            match registry.read(change) {
                Ok(Some(value)) if value == RawValue::desired(change) => aligned += 1,
                Ok(Some(value)) => {
                    let current = if value.kind == 4 && value.bytes.len() == 4 {
                        format!(
                            "REG_DWORD {}",
                            u32::from_le_bytes(value.bytes.as_slice().try_into()?)
                        )
                    } else {
                        format!("注册表类型 {}，{} 字节", value.kind, value.bytes.len())
                    };
                    details.push(format!(
                        "{} 当前为 {}，方案值为 {}",
                        change.name, current, change.value
                    ));
                }
                Ok(None) => {
                    details.push(format!("{} 尚未设置；将写入 {}", change.name, change.value))
                }
                Err(error) => {
                    unavailable = true;
                    details.push(format!("{} 读取失败：{error:#}", change.name));
                }
            }
        }
        let state = if unavailable {
            "unavailable"
        } else if aligned == action.changes.len() {
            "aligned"
        } else {
            "different"
        };
        if state == "aligned" {
            details.push("所有记录值均与选择一致；组织策略仍可能影响实际效果。".into());
        }
        states.push(SettingState {
            id: action.id,
            state: state.into(),
            detail: details.join("\n"),
        });
    }
    Ok(states)
}

pub(super) fn apply(plan: Plan, cancel: Arc<AtomicBool>) -> Result<RunReport> {
    let plan = studio_core::validate_plan(&plan)?;
    check_host(plan.target)?;
    ensure!(
        !plan.actions.is_empty(),
        "Select at least one action before applying"
    );
    let directory = data_directory()?;
    let _lock = mutation_lock(&directory)?;
    let journals = directory.join("recovery");
    ensure_directory(&journals)?;
    let mut count = 0;
    for entry in fs::read_dir(&journals)? {
        let entry = entry.context("Inspect existing recovery directory entry")?;
        if entry.path().extension().is_some_and(|s| s == "jsonl") {
            count += 1;
        }
        ensure!(
            count < 256,
            "Recovery journal limit reached (256); archive reviewed journals before applying again"
        );
    }
    let key = journal::key(&directory, true)?;
    let id = unique_id()?;
    let path = journals.join(format!("{id}.jsonl"));
    let mut store = FileJournal::create(path.clone(), key)?;
    let outcome = engine::apply_backend(
        &plan,
        &mut NativeRegistry,
        &mut store,
        id,
        timestamp()?,
        &cancel,
    )
    .with_context(|| {
        if store.may_have_mutations() {
            format!(
                "Apply stopped; partial changes may exist. Recovery journal: {}",
                path.display()
            )
        } else {
            "Apply stopped before any registry mutation. No registry recovery is required.".into()
        }
    })?;
    Ok(report("Apply", outcome, path))
}
fn journal_path(path: &Path, directory: &Path) -> Result<PathBuf> {
    ensure!(
        !fs::symlink_metadata(path)?.file_type().is_symlink(),
        "Recovery journal cannot be a symbolic link"
    );
    let canonical = path.canonicalize()?;
    ensure!(
        canonical.parent() == Some(directory.canonicalize()?.as_path())
            && canonical.extension().is_some_and(|s| s == "jsonl"),
        "Recovery must use a journal from this user's recovery directory"
    );
    Ok(canonical)
}
pub(super) fn restore(path: PathBuf, cancel: Arc<AtomicBool>) -> Result<RunReport> {
    let directory = data_directory()?;
    let _lock = mutation_lock(&directory)?;
    let path = journal_path(&path, &directory.join("recovery"))?;
    let key = journal::key(&directory, false)?;
    let loaded = journal::load(&path, &key)?;
    check_host(loaded.header.target)?;
    ensure!(
        path.file_stem()
            .is_some_and(|s| s == loaded.header.id.as_str()),
        "Recovery journal ID does not match its filename"
    );
    let mut store = FileJournal::resume(path.clone(), key, &loaded)?;
    let outcome = engine::restore_backend(
        &loaded.header,
        &loaded.intended,
        &mut NativeRegistry,
        &mut store,
        &cancel,
    )
    .with_context(|| {
        format!(
            "Restore stopped; recovery may be partial. Journal: {}",
            path.display()
        )
    })?;
    Ok(report("Restore", outcome, path))
}
pub(super) fn recovery_points() -> Result<Vec<RecoveryPoint>> {
    let directory = data_directory()?;
    let journals = directory.join("recovery");
    if !journals.try_exists()? {
        return Ok(Vec::new());
    }
    let key = journal::key(&directory, false)?;
    let mut points = Vec::new();
    for entry in fs::read_dir(&journals)? {
        let entry = entry?;
        if entry.path().extension().is_none_or(|s| s != "jsonl") {
            continue;
        }
        ensure!(points.len() < 256, "Too many recovery journals");
        let path = journal_path(&entry.path(), &journals)?;
        let loaded = journal::load(&path, &key)
            .with_context(|| format!("Inspect recovery {}", path.display()))?;
        ensure!(
            path.file_stem()
                .is_some_and(|s| s == loaded.header.id.as_str()),
            "Recovery filename does not match signed ID"
        );
        points.push(RecoveryPoint {
            path,
            id: loaded.header.id,
            status: loaded.status,
            started_at: loaded.header.started_at,
            actions: loaded.header.actions.len(),
        });
    }
    points.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    Ok(points)
}
