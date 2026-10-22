//! Read-only local execution evidence. These records never authorize execution or recovery.
use crate::DeploymentPlan;
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs::{self, File, Metadata, OpenOptions},
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
};
use studio_core::Target;

const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
const MAX_FILES: usize = 256;
const MAX_EVENTS: usize = 2048;
const MAX_DETAIL_BYTES: usize = 2 * 1024 * 1024 + 4096;

/// One persisted run. It describes past recorded results, never current machine state.
#[derive(Debug, Clone)]
pub struct HistoryRecord {
    pub path: PathBuf,
    pub started_at: String,
    pub target: Target,
    pub catalog_revision: String,
    pub status: String,
    pub summary: String,
    pub completed: usize,
    pub failed: usize,
    pub cancelled: bool,
    pub restart_required: bool,
    pub tasks: Vec<HistoryTask>,
}
#[derive(Debug, Clone)]
pub struct HistoryTask {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub status: String,
    pub completed: bool,
    pub failed: bool,
    pub cancelled: bool,
    pub restart_required: bool,
    pub evidence: Vec<HistoryEvidence>,
}
/// Legacy histories retain stdout/stderr inside `detail` exactly as recorded.
#[derive(Debug, Clone)]
pub struct HistoryEvidence {
    pub event: String,
    pub state: Option<String>,
    pub detail: String,
}

/// Enumerate only this application's fixed history directory on any platform.
/// Missing history is empty; unreadable, corrupt, unsafe or oversized history is an error.
/// Data is user-local evidence, not authenticated proof or restore eligibility.
pub fn history() -> Result<Vec<HistoryRecord>> {
    history_in(&studio_platform::data_directory()?)
}

fn real_directory(path: &Path) -> Result<Option<PathBuf>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(
                !is_link(&metadata) && metadata.is_dir(),
                "History directory must be a real directory: {}",
                path.display()
            );
            Ok(Some(path.canonicalize()?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("Inspect history directory {}", path.display()))
        }
    }
}
fn is_link(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}
fn history_in(data: &Path) -> Result<Vec<HistoryRecord>> {
    let Some(data_root) = real_directory(data)? else {
        return Ok(Vec::new());
    };
    let directory = data_root.join("deployment-history");
    let Some(root) = real_directory(&directory)? else {
        return Ok(Vec::new());
    };
    let mut paths = Vec::new();
    for entry in fs::read_dir(&root).context("Read deployment history directory")? {
        let entry = entry.context("Read deployment history directory entry")?;
        if entry.path().extension().is_none_or(|ext| ext != "jsonl") {
            continue;
        }
        ensure!(
            paths.len() < MAX_FILES,
            "Deployment history exceeds {MAX_FILES} files"
        );
        let path = entry.path();
        let timestamp = filename_timestamp(&path)?;
        paths.push((timestamp, path));
    }
    paths.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let mut records = Vec::new();
    let mut total = 0;
    for (timestamp, path) in paths {
        let file = open_history(&path, &root)?;
        // Reader limits are authoritative even if a live writer grows the file.
        let mut reader = BufReader::new(file.take(MAX_FILE_BYTES + 1));
        let mut record = read_record(&mut reader, &path, timestamp, &mut total)
            .with_context(|| format!("Read deployment history {}", path.display()))?;
        ensure!(
            path.parent() == Some(root.as_path())
                && real_directory(&directory)?.as_ref() == Some(&root),
            "History directory changed while reading"
        );
        record.path = path;
        records.push(record);
    }
    ensure!(
        real_directory(data)?.as_ref() == Some(&data_root),
        "Application data directory changed while reading history"
    );
    Ok(records)
}
fn open_history(path: &Path, root: &Path) -> Result<File> {
    ensure!(
        path.parent() == Some(root),
        "History path is outside the history directory"
    );
    let before = fs::symlink_metadata(path)?;
    ensure!(
        !is_link(&before) && before.is_file(),
        "History entry must be a regular file: {}",
        path.display()
    );
    ensure!(
        before.len() <= MAX_FILE_BYTES,
        "History file exceeds 32 MiB: {}",
        path.display()
    );
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x100); // O_NOFOLLOW
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x20000); // O_NOFOLLOW
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let file = options
        .open(path)
        .with_context(|| format!("Open history {}", path.display()))?;
    let opened = file.metadata()?;
    ensure!(
        !is_link(&opened) && opened.is_file(),
        "Opened history is not a regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            before.dev() == opened.dev() && before.ino() == opened.ino(),
            "History file changed while opening"
        );
    }
    let after = fs::symlink_metadata(path)?;
    ensure!(
        !is_link(&after) && path.canonicalize()?.parent() == Some(root),
        "History path changed while opening"
    );
    Ok(file)
}

fn filename_timestamp(path: &Path) -> Result<u128> {
    let stem = path
        .file_stem()
        .and_then(|v| v.to_str())
        .context("History filename is not UTF-8")?;
    ensure!(stem.len() <= 100, "History filename is too long");
    let parts: Vec<_> = stem.split('-').collect();
    ensure!(
        parts.len() == 3
            && parts
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())),
        "Unexpected deployment history filename"
    );
    let timestamp = parts[0]
        .parse::<u128>()
        .context("Invalid history timestamp")?;
    ensure!(
        timestamp / 1_000_000_000 <= 253_402_300_799,
        "History timestamp is outside supported UTC range"
    );
    parts[1]
        .parse::<u32>()
        .context("Invalid history process ID")?;
    parts[2]
        .parse::<u64>()
        .context("Invalid history sequence")?;
    Ok(timestamp)
}
fn format_timestamp(nanos: u128) -> String {
    let seconds = (nanos / 1_000_000_000) as u64;
    let mut days = seconds / 86_400;
    let mut year = 1970;
    loop {
        let count = if leap_year(year) { 366 } else { 365 };
        if days < count {
            break;
        }
        days -= count;
        year += 1;
    }
    let lengths = [
        31,
        if leap_year(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 0;
    while days >= lengths[month] {
        days -= lengths[month];
        month += 1;
    }
    let clock = seconds % 86_400;
    format!(
        "{year:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03} UTC",
        month + 1,
        days + 1,
        clock / 3600,
        (clock % 3600) / 60,
        clock % 60,
        (nanos % 1_000_000_000) / 1_000_000
    )
}
fn leap_year(year: u64) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}
fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    let text = value
        .get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("History event missing string {key}"))?;
    ensure!(
        text.len() <= MAX_DETAIL_BYTES && !text.contains('\0'),
        "History field {key} exceeds bounds or contains NUL"
    );
    Ok(text)
}
fn boolean(value: &Value, key: &str) -> Result<bool> {
    value
        .get(key)
        .and_then(Value::as_bool)
        .with_context(|| format!("History event missing boolean {key}"))
}
fn count(value: &Value, key: &str) -> Result<usize> {
    let count = value
        .get(key)
        .and_then(Value::as_u64)
        .with_context(|| format!("History event missing count {key}"))?;
    ensure!(count <= 128, "Invalid history count {key}");
    Ok(count as usize)
}

fn read_record(
    reader: &mut impl BufRead,
    path: &Path,
    timestamp: u128,
    total: &mut u64,
) -> Result<HistoryRecord> {
    let mut bytes = 0;
    let mut events = 0;
    let mut record: Option<HistoryRecord> = None;
    let mut finished = false;
    let mut next_task = 0;
    let mut stopped = false;
    loop {
        let mut line = Vec::new();
        let read = reader
            .read_until(b'\n', &mut line)
            .context("Read history event")?;
        if read == 0 {
            break;
        }
        bytes += read as u64;
        *total += read as u64;
        ensure!(
            bytes <= MAX_FILE_BYTES && *total <= MAX_TOTAL_BYTES,
            "History exceeds file or total input bounds"
        );
        ensure!(
            line.last() == Some(&b'\n'),
            "History has an incomplete trailing event; completion cannot be established"
        );
        ensure!(events < MAX_EVENTS, "History has too many events");
        let value: Value = serde_json::from_slice(&line).context("Malformed history JSON event")?;
        let event = string(&value, "event")?;
        ensure!(!finished, "History has events after its final result");
        if events == 0 {
            ensure!(
                event == "plan"
                    && value.get("schema_version").and_then(Value::as_u64) == Some(1)
                    && value.get("rollback").and_then(Value::as_bool) == Some(false),
                "Unsupported history header or rollback claim"
            );
            // Historical plans are deliberately not compared with today's catalog.
            // Their old titles/revisions remain readable but can never be executed here.
            let plan: DeploymentPlan = serde_json::from_value(
                value
                    .get("plan")
                    .context("Missing historical plan")?
                    .clone(),
            )
            .context("Invalid historical plan")?;
            ensure!(
                plan.schema_version == 1
                    && !plan.tasks.is_empty()
                    && plan.tasks.len() <= 128
                    && !plan.catalog_revision.is_empty()
                    && plan.catalog_revision.len() <= 128,
                "Invalid historical plan bounds"
            );
            let mut ids = BTreeSet::new();
            for task in &plan.tasks {
                ensure!(
                    crate::safe_id(&task.id)
                        && ids.insert(&task.id)
                        && !task.title.is_empty()
                        && task.title.len() <= 8192
                        && !task.title.contains('\0')
                        && matches!(
                            task.kind.as_str(),
                            "package" | "feature" | "maintenance" | "cargo" | "uv" | "vscode"
                        ),
                    "Invalid historical task identity"
                );
            }
            record = Some(HistoryRecord {
                path: path.to_path_buf(),
                started_at: format_timestamp(timestamp),
                target: plan.target,
                catalog_revision: plan.catalog_revision,
                status: "interrupted".into(),
                summary: String::new(),
                completed: 0,
                failed: 0,
                cancelled: false,
                restart_required: false,
                tasks: plan
                    .tasks
                    .into_iter()
                    .map(|task| HistoryTask {
                        id: task.id,
                        title: task.title,
                        kind: task.kind,
                        status: "not_started".into(),
                        completed: false,
                        failed: false,
                        cancelled: false,
                        restart_required: false,
                        evidence: Vec::new(),
                    })
                    .collect(),
            });
        } else {
            let record = record.as_mut().context("History has no plan")?;
            if event == "finished" {
                let completed = count(&value, "completed")?;
                let failed = count(&value, "failed")?;
                let cancelled = boolean(&value, "cancelled")?;
                let restart = boolean(&value, "restart_required")?;
                ensure!(
                    completed == record.completed
                        && failed == record.failed
                        && (cancelled || !record.cancelled),
                    "History final counts or cancellation contradict task evidence"
                );
                ensure!(
                    cancelled || failed > 0 || completed == record.tasks.len(),
                    "Incomplete deployment marked successful"
                );
                ensure!(
                    !record.restart_required || restart,
                    "History final result dropped a recorded restart requirement"
                );
                record.cancelled = cancelled;
                record.restart_required = restart;
                record.status = if cancelled {
                    "cancelled"
                } else if failed > 0 {
                    "failed"
                } else {
                    "completed"
                }
                .into();
                finished = true;
            } else {
                ensure!(
                    matches!(
                        event,
                        "inspection"
                            | "intent"
                            | "command_result"
                            | "verification"
                            | "completed"
                            | "failed"
                            | "cancelled"
                    ),
                    "Unknown history event {event}"
                );
                ensure!(
                    !stopped,
                    "History continued after a failed or cancelled task"
                );
                let id = string(&value, "task_id")?;
                let task = record
                    .tasks
                    .get_mut(next_task)
                    .context("History task is outside the plan")?;
                ensure!(
                    id == task.id,
                    "History task order or identity differs from recorded plan"
                );
                let has = |name: &str| task.evidence.iter().any(|item| item.event == name);
                ensure!(!has(event), "Duplicate history event {event}");
                let state = if matches!(event, "inspection" | "verification") {
                    Some(string(&value, "state")?.to_owned())
                } else {
                    None
                };
                let detail = if event == "intent" {
                    ensure!(has("inspection"), "History intent precedes inspection");
                    let program = string(&value, "program")?;
                    let args = value
                        .get("args")
                        .and_then(Value::as_array)
                        .context("History intent missing argv")?;
                    ensure!(
                        args.len() <= 64
                            && args.iter().all(|arg| arg
                                .as_str()
                                .is_some_and(|s| s.len() <= 8192 && !s.contains('\0'))),
                        "Invalid historical argv"
                    );
                    let environment = if value.get("environment").is_some() {
                        format!("\nenvironment: {}", string(&value, "environment")?)
                    } else {
                        String::new()
                    };
                    format!(
                        "{program}\nargv: {}{environment}\n{}",
                        serde_json::to_string(args)?,
                        string(&value, "warning")?
                    )
                } else {
                    string(&value, "detail")?.to_owned()
                };
                match event {
                    "inspection" => ensure!(
                        task.evidence.is_empty(),
                        "Inspection is not the first task event"
                    ),
                    "command_result" => ensure!(
                        has("intent") && !has("verification"),
                        "Command result has no preceding intent"
                    ),
                    "verification" => {
                        ensure!(has("command_result"), "Verification has no command result")
                    }
                    "completed" => {
                        ensure!(
                            has("inspection"),
                            "Completed task has no inspection evidence"
                        );
                        let already_set = task.evidence.iter().any(|item| {
                            item.event == "inspection"
                                && item.state.as_deref().is_some_and(|state| {
                                    matches!(state, "installed" | "enabled" | "enable_pending")
                                })
                        });
                        ensure!(
                            already_set
                                || (has("command_result")
                                    && (task.kind == "maintenance"
                                        || task.evidence.iter().any(|item| item.event
                                            == "verification"
                                            && item.state.as_deref().is_some_and(
                                                |state| matches!(
                                                    state,
                                                    "installed" | "enabled" | "enable_pending"
                                                )
                                            )))),
                            "Completed task lacks its result or verification evidence"
                        );
                        task.restart_required |= boolean(&value, "restart_required")?;
                        task.completed = true;
                        task.status = "completed".into();
                        record.completed += 1;
                        next_task += 1;
                    }
                    "failed" => {
                        task.failed = true;
                        task.status = "failed".into();
                        record.failed += 1;
                        stopped = true;
                    }
                    "cancelled" => {
                        task.cancelled = true;
                        task.status = "cancelled".into();
                        record.cancelled = true;
                        stopped = true;
                    }
                    _ => {}
                }
                if state.as_deref() == Some("enable_pending") {
                    task.restart_required = true;
                }
                if event == "command_result" {
                    // Legacy output keeps numeric exit information in the first diagnostic field.
                    if let Some(code) = detail
                        .strip_prefix("exit=0x")
                        .and_then(|s| s.split_once(';'))
                        .and_then(|(s, _)| u32::from_str_radix(s, 16).ok())
                    {
                        task.restart_required |= code == 3010
                            || (task.kind == "package" && matches!(code, 0x8a150109..=0x8a15010b));
                    }
                }
                record.restart_required |= task.restart_required;
                if !task.completed && !task.failed && !task.cancelled {
                    task.status = "interrupted".into();
                }
                task.evidence.push(HistoryEvidence {
                    event: event.into(),
                    state,
                    detail,
                });
            }
        }
        events += 1;
    }
    let mut record = record.context("Empty history file has no recorded plan")?;
    record.summary = format!(
        "{}：记录完成 {} / {} 项、失败 {} 项{}。{}执行历史仅为已记录的结果，不表示当前主机状态，也不提供恢复资格。",
        match record.status.as_str() {
            "completed" => "部署完成",
            "failed" => "部署失败",
            "cancelled" => "部署已取消",
            _ => "部署记录未结束",
        },
        record.completed,
        record.tasks.len(),
        record.failed,
        if record.cancelled { "，已取消" } else { "" },
        if record.restart_required {
            "记录中存在重启要求。"
        } else {
            ""
        }
    );
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        sync::atomic::{AtomicU64, Ordering},
    };
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "ltsc-history-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn write(&self, events: Vec<Value>) -> PathBuf {
            let dir = self.0.join("deployment-history");
            fs::create_dir_all(&dir).unwrap();
            let path = dir.join("1792540800000000000-12-0.jsonl");
            let mut file = File::create(&path).unwrap();
            for event in events {
                serde_json::to_writer(&mut file, &event).unwrap();
                file.write_all(b"\n").unwrap();
            }
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn header() -> Value {
        let c = crate::catalog().unwrap();
        let mut plan = crate::build_plan(
            Target::Win10Ltsc2021,
            &BTreeSet::from([c.tasks[0].id.clone()]),
        )
        .unwrap();
        plan.catalog_revision = "historical-revision".into();
        plan.tasks[0].title = "Historical package title".into();
        serde_json::json!({"event":"plan","schema_version":1,"plan":plan,"rollback":false})
    }
    fn id() -> String {
        crate::catalog().unwrap().tasks[0].id.clone()
    }
    fn inspected() -> Value {
        serde_json::json!({"event":"inspection","task_id":id(),"state":"installed","detail":"exit=0x00000000; cancelled=false; timed_out=false\nstdout:\n7zip.7zip\nstderr:\n"})
    }
    fn intent() -> Value {
        serde_json::json!({"event":"intent","task_id":id(),"program":"C:\\WindowsApps\\winget.exe","args":["install","--id","7zip.7zip"],"warning":"No rollback"})
    }
    #[test]
    fn completed_legacy_history_retains_actual_evidence_and_old_catalog() {
        let f = Fixture::new();
        let path=f.write(vec![header(),inspected(),serde_json::json!({"event":"completed","task_id":id(),"restart_required":false,"detail":"already installed"}),serde_json::json!({"event":"finished","completed":1,"failed":0,"cancelled":false,"restart_required":false})]);
        let records = history_in(&f.0).unwrap();
        let record = &records[0];
        assert_eq!(record.path, path.canonicalize().unwrap());
        assert_eq!(record.status, "completed");
        assert_eq!(record.catalog_revision, "historical-revision");
        assert_eq!(record.tasks[0].title, "Historical package title");
        assert!(record.tasks[0].completed);
        assert!(
            record.tasks[0].evidence[0]
                .detail
                .contains("stdout:\n7zip.7zip")
        );
        assert!(record.started_at.ends_with("UTC"));
    }
    #[test]
    fn failed_and_cancelled_history_preserve_result() {
        for cancelled in [false, true] {
            let f = Fixture::new();
            f.write(vec![header(),inspected(),intent(),serde_json::json!({"event":if cancelled {"cancelled"}else{"failed"},"task_id":id(),"detail":"actual failure or cancellation","partial_state_possible":true}),serde_json::json!({"event":"finished","completed":0,"failed":usize::from(!cancelled),"cancelled":cancelled,"restart_required":false})]);
            let r = history_in(&f.0).unwrap().remove(0);
            assert_eq!(r.status, if cancelled { "cancelled" } else { "failed" });
            assert_eq!(r.tasks[0].cancelled, cancelled);
            assert_eq!(r.tasks[0].failed, !cancelled);
        }
    }
    #[test]
    fn intent_only_run_is_interrupted_not_completed() {
        let f = Fixture::new();
        f.write(vec![header(), inspected(), intent()]);
        let r = history_in(&f.0).unwrap().remove(0);
        assert_eq!(r.status, "interrupted");
        assert_eq!(r.completed, 0);
        assert_eq!(r.tasks[0].status, "interrupted");
        assert_eq!(r.tasks[0].evidence.last().unwrap().event, "intent");
    }
    #[test]
    fn restart_requirement_survives_unfinished_history() {
        let f = Fixture::new();
        f.write(vec![header(),inspected(),intent(),serde_json::json!({"event":"command_result","task_id":id(),"detail":"exit=0x8A150109; cancelled=false; timed_out=false\nstdout:\nreboot required\nstderr:\n"})]);
        let r = history_in(&f.0).unwrap().remove(0);
        assert!(r.restart_required);
        assert!(r.tasks[0].restart_required);
        assert_eq!(r.status, "interrupted");
    }
    #[test]
    fn missing_directory_is_empty_and_no_directory_is_created() {
        let f = Fixture::new();
        assert!(history_in(&f.0).unwrap().is_empty());
        assert!(!f.0.join("deployment-history").exists());
        assert!(history_in(&f.0.join("missing")).unwrap().is_empty());
    }
    #[test]
    fn malformed_incomplete_and_false_success_are_errors() {
        let f = Fixture::new();
        let path = f.write(vec![header()]);
        for bytes in [
            b"{bad}\n".as_slice(),
            b"{\"event\":\"plan\"".as_slice(),
            b"".as_slice(),
        ] {
            fs::write(&path, bytes).unwrap();
            assert!(history_in(&f.0).is_err());
        }
        f.write(vec![header(),serde_json::json!({"event":"finished","completed":0,"failed":0,"cancelled":false,"restart_required":false})]);
        assert!(history_in(&f.0).is_err());
    }
    #[test]
    fn oversized_file_and_unsafe_filename_are_errors() {
        let f = Fixture::new();
        let path = f.write(vec![header()]);
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();
        assert!(history_in(&f.0).is_err());
        fs::remove_file(&path).unwrap();
        let bad = path.parent().unwrap().join("arbitrary.jsonl");
        fs::write(bad, b"{}\n").unwrap();
        assert!(history_in(&f.0).is_err());
    }
    #[test]
    fn unknown_task_reference_and_traversal_never_select_a_file() {
        let f = Fixture::new();
        f.write(vec![
            header(),
            serde_json::json!({"event":"failed","task_id":"../outside","detail":"bad"}),
        ]);
        assert!(history_in(&f.0).is_err());
        assert!(open_history(&f.0.join("outside.jsonl"), &f.0.join("deployment-history")).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn symlink_file_and_directory_are_rejected() {
        use std::os::unix::fs::symlink;
        let f = Fixture::new();
        let outside = Fixture::new();
        let path = f.write(vec![header()]);
        let external = outside.0.join("external");
        fs::write(&external, b"sensitive external data").unwrap();
        fs::remove_file(&path).unwrap();
        symlink(&external, &path).unwrap();
        assert!(history_in(&f.0).is_err());
        fs::remove_file(&path).unwrap();
        fs::remove_dir(path.parent().unwrap()).unwrap();
        symlink(&outside.0, f.0.join("deployment-history")).unwrap();
        assert!(history_in(&f.0).is_err());
    }
    #[test]
    fn timestamps_have_correct_utc_calendar() {
        assert_eq!(format_timestamp(0), "1970-01-01 00:00:00.000 UTC");
        assert_eq!(
            format_timestamp(951_782_400_123_000_000),
            "2000-02-29 00:00:00.123 UTC"
        );
        assert!(
            filename_timestamp(Path::new("999999999999999999999999999999999-1-0.jsonl")).is_err()
        );
    }
    #[test]
    fn developer_kind_history_and_legacy_schema_are_both_readable() {
        for kind in ["cargo", "uv", "vscode"] {
            let f = Fixture::new();
            let mut plan = header();
            plan["plan"]["tasks"][0]["kind"] = Value::String(kind.into());
            plan["plan"]["tasks"][0]["category"] = Value::String("developer".into());
            f.write(vec![plan,inspected(),serde_json::json!({"event":"completed","task_id":id(),"restart_required":false,"detail":"provider verified installed"}),serde_json::json!({"event":"finished","completed":1,"failed":0,"cancelled":false,"restart_required":false})]);
            let records = history_in(&f.0).unwrap();
            assert_eq!(records[0].tasks[0].kind, kind);
            assert_eq!(records[0].status, "completed");
        }
    }
    #[test]
    fn io_failures_and_total_bounds_do_not_become_empty_history() {
        let f = Fixture::new();
        fs::write(f.0.join("deployment-history"), b"not a directory").unwrap();
        assert!(history_in(&f.0).is_err());
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("injected history read failure"))
            }
        }
        impl BufRead for Broken {
            fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
                Err(std::io::Error::other("injected history read failure"))
            }
            fn consume(&mut self, _: usize) {}
        }
        assert!(read_record(&mut Broken, Path::new("1-1-0.jsonl"), 1, &mut 0).is_err());
        let mut bytes = serde_json::to_vec(&header()).unwrap();
        bytes.push(b'\n');
        let mut total = MAX_TOTAL_BYTES;
        assert!(
            read_record(
                &mut bytes.as_slice(),
                Path::new("1-1-0.jsonl"),
                1,
                &mut total
            )
            .is_err()
        );
    }
}
