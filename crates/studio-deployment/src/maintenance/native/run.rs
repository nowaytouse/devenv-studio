use super::{
    PATH, account_home, acquire_engine_activation_lock, data_dir, ensure_data, ensure_dir,
    open_regular, present, read_regular, real_metadata, release_tree, require_running,
    review_proof, rollback_candidate, same_file, valid_version,
};
use crate::{
    DeploymentEvent,
    maintenance::{
        EngineIdentity, Report, Review, RunStatus, StepReport,
        protocol::{
            EVENT_LIMIT, Request, Trace, parse_events, valid_hex, valid_run_id, validate_request,
        },
    },
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Sender},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const RECORD_LIMIT: u64 = 16 * 1024;
const LOG_LIMIT: usize = 8 * 1024 * 1024;
const MAX_HISTORY: usize = 256;
const ENGINE_SOURCE: &str = "studio_managed_build";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema_version: u8,
    run_id: String,
    status: RunStatus,
    steps: Vec<StepReport>,
    exit_code: Option<i32>,
    reason: Option<IncompleteReason>,
    engine_tree: Option<String>,
    elapsed_ms: Option<u64>,
    engine_source: Option<String>,
    engine: Option<EngineIdentity>,
    rollback: Option<EngineIdentity>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum IncompleteReason {
    Interrupted,
    Supervision,
    InvalidEvents,
    MissingFinal,
    ExitMismatch,
}

impl IncompleteReason {
    fn detail(&self) -> &'static str {
        match self {
            Self::Interrupted => "Studio did not persist a final result.",
            Self::Supervision => "The child process or complete capture could not be verified.",
            Self::InvalidEvents => "The engine event stream could not be validated.",
            Self::MissingFinal => "The engine exited without a final event.",
            Self::ExitMismatch => "The final event disagreed with the engine exit status.",
        }
    }
}

impl Record {
    fn started(
        request: &Request,
        engine: EngineIdentity,
        rollback: Option<EngineIdentity>,
    ) -> Self {
        Self {
            schema_version: 2,
            run_id: request.run_id.clone(),
            status: RunStatus::Incomplete,
            steps: Vec::new(),
            exit_code: None,
            reason: Some(IncompleteReason::Interrupted),
            engine_tree: Some(engine.component_tree.clone()),
            elapsed_ms: Some(0),
            engine_source: Some(ENGINE_SOURCE.into()),
            engine: Some(engine),
            rollback,
        }
    }

    fn report(self, path: PathBuf) -> Report {
        let detail = match self.status {
            RunStatus::Verified => {
                "Every selected step reported success and the engine exited successfully."
            }
            RunStatus::Failed => {
                "The engine reported a failure. Review the local log before retrying."
            }
            RunStatus::Cancelled => {
                "The engine acknowledged cancellation. Completed steps remain applied."
            }
            RunStatus::TimedOut => {
                "A maintenance step timed out. Review the local log and engine recovery state."
            }
            RunStatus::Incomplete => {
                "Completion could not be verified. Inspect local evidence and any maintenance recovery marker before retrying."
            }
        };
        Report {
            path,
            run_id: self.run_id,
            status: self.status,
            steps: self.steps,
            detail: match self.reason {
                Some(reason) => format!("{} {detail}", reason.detail()),
                None => detail.into(),
            },
            engine: self.engine,
            engine_tree: self.engine_tree,
            rollback: self.rollback,
        }
    }
}

fn directory(home: &Path) -> PathBuf {
    data_dir(home).join("maintenance/runs")
}

fn new_run(home: &Path) -> Result<(String, PathBuf)> {
    let data = ensure_data(home)?;
    ensure_dir(&data.join("maintenance"), true)?;
    let root = directory(home);
    ensure_dir(&root, true)?;
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    for _ in 0..32 {
        let seq = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let id = format!("{time}-{}-{seq}", std::process::id());
        let path = root.join(&id);
        match DirBuilder::new().mode(0o700).create(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            result => result.context("Create a private Topgrade run directory")?,
        }
        File::open(&root)?.sync_all()?;
        return Ok((id, path));
    }
    anyhow::bail!("Cannot allocate a Topgrade run directory")
}

fn private_file(path: &Path) -> Result<File> {
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .context("Create a private maintenance evidence file")
}

fn persist(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = private_file(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    File::open(path.parent().context("Evidence has no parent directory")?)?.sync_all()?;
    Ok(())
}

fn publish_record(path: &Path, record: &Record) -> Result<()> {
    let stage = path.join(".result.pending");
    persist(&stage, &serde_json::to_vec(record)?)?;
    fs::rename(stage, path.join("result.json"))?;
    File::open(path)?.sync_all()?;
    Ok(())
}

fn read_events(path: &Path) -> Result<Vec<u8>> {
    let path = path.join("events.jsonl");
    if !present(&path)? {
        return Ok(Vec::new());
    }
    // The engine appends while the parent reads. Identity, permissions and the cap still apply.
    let (file, before) = open_regular(&path, EVENT_LIMIT)?;
    ensure!(
        before.mode() & 0o077 == 0,
        "Maintenance event file is not private"
    );
    let mut bytes = Vec::new();
    file.take(EVENT_LIMIT + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= EVENT_LIMIT && same_file(&before, &real_metadata(&path)?),
        "Maintenance events changed identity or exceeded their bound"
    );
    Ok(bytes)
}

enum LogEvent {
    Fault,
    Closed,
}

fn drain(mut pipe: impl Read + Send + 'static, mut file: File, send: Sender<LogEvent>) {
    thread::spawn(move || {
        let mut total = 0;
        let mut buffer = [0; 8192];
        let mut faulted = false;
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    if !faulted {
                        let available = count.min(LOG_LIMIT - total);
                        if file.write_all(&buffer[..available]).is_err() || available != count {
                            faulted = true;
                            let _ = send.send(LogEvent::Fault);
                        }
                        total += available;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    faulted = true;
                    let _ = send.send(LogEvent::Fault);
                    break;
                }
            }
        }
        if !faulted && file.sync_all().is_err() {
            let _ = send.send(LogEvent::Fault);
        }
        let _ = send.send(LogEvent::Closed);
    });
}

struct Supervisor<'a> {
    path: &'a Path,
    request: &'a Request,
    cancel: &'a AtomicBool,
    events: Option<&'a Sender<DeploymentEvent>>,
}

impl Supervisor<'_> {
    fn publish(&self, trace: &Trace) {
        if let Some(events) = self.events {
            let id = trace
                .running
                .as_deref()
                .or_else(|| trace.results.last().map(|result| result.id.as_str()));
            if let Some(id) = id {
                let message = if trace.running.is_some() {
                    format!("Started {id}.")
                } else {
                    format!("{id}: {:?}.", trace.results.last().unwrap().status)
                };
                let _ = events.send(DeploymentEvent {
                    task_id: id.into(),
                    message,
                    completed: trace.results.len(),
                    total: self.request.steps.len(),
                });
            }
        }
    }

    fn observe(&self, mut command: Command) -> Result<ExitStatus> {
        let stdout = private_file(&self.path.join("stdout.log"))?;
        let stderr = private_file(&self.path.join("stderr.log"))?;
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Start the reviewed Topgrade engine")?;
        let (sender, receiver) = mpsc::channel();
        // Both pipes were explicitly requested. A missing pipe is handled by the same reap path.
        let result = (|| -> Result<ExitStatus> {
            drain(
                child.stdout.take().context("Missing Topgrade stdout")?,
                stdout,
                sender.clone(),
            );
            drain(
                child.stderr.take().context("Missing Topgrade stderr")?,
                stderr,
                sender,
            );
            let start = Instant::now();
            // Match the engine watchdog for each reviewed operation.
            let step_budget = Duration::from_secs(
                self.request
                    .steps
                    .iter()
                    .map(|step| {
                        if step == "llvm_head" {
                            4 * 60 * 60
                        } else {
                            15 * 60
                        }
                    })
                    .sum(),
            );
            let soft_deadline = step_budget + Duration::from_secs(60);
            let hard_deadline = step_budget + Duration::from_secs(3 * 60);
            let mut previous = Vec::new();
            let mut announced = (0, None);
            let (mut faulted, mut cancel_sent, mut closed) = (false, false, 0);
            let status = loop {
                for event in receiver.try_iter() {
                    match event {
                        LogEvent::Fault => faulted = true,
                        LogEvent::Closed => closed += 1,
                    }
                }
                if let Some(status) = child.try_wait().context("Wait for the Topgrade engine")? {
                    break status;
                }
                if !faulted {
                    match self.read_live(&previous) {
                        Ok((bytes, trace)) => {
                            let now = (trace.results.len(), trace.running.clone());
                            if now != announced {
                                self.publish(&trace);
                                announced = now;
                            }
                            previous = bytes;
                        }
                        Err(_) => faulted = true,
                    }
                }
                if !cancel_sent
                    && (faulted
                        || self.cancel.load(Ordering::Acquire)
                        || start.elapsed() >= soft_deadline)
                {
                    if persist(&self.path.join("cancel"), b"").is_err() {
                        faulted = true;
                    }
                    cancel_sent = true;
                }
                ensure!(
                    start.elapsed() < hard_deadline,
                    "Topgrade engine did not stop before its outer deadline"
                );
                thread::sleep(Duration::from_millis(100));
            };
            let deadline = Instant::now() + Duration::from_secs(2);
            while closed < 2 {
                match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(LogEvent::Fault) => faulted = true,
                    Ok(LogEvent::Closed) => closed += 1,
                    Err(_) => anyhow::bail!("Topgrade log capture did not close"),
                }
            }
            ensure!(!faulted, "Topgrade log or event capture was incomplete");
            let (bytes, trace) = self.read_live(&previous)?;
            parse_events(&bytes, self.request, true)?;
            self.publish(&trace);
            Ok(status)
        })();
        if result.is_err() && !matches!(child.try_wait(), Ok(Some(_))) {
            // The engine's durable guard is left in place if it was interrupted mid-mutation.
            let killed = child.kill();
            let reaped = child.wait();
            killed.context("Terminate the unresponsive Topgrade engine")?;
            reaped.context("Reap the terminated Topgrade engine")?;
        }
        result
    }

    fn read_live(&self, previous: &[u8]) -> Result<(Vec<u8>, Trace)> {
        let bytes = read_events(self.path)?;
        ensure!(
            bytes.starts_with(previous),
            "Maintenance events were changed in place"
        );
        let trace = parse_events(&bytes, self.request, false)?;
        Ok((bytes, trace))
    }
}

fn matched_exit(status: RunStatus, exit: Option<i32>) -> bool {
    match status {
        RunStatus::Verified => exit == Some(0),
        RunStatus::Failed | RunStatus::Cancelled | RunStatus::TimedOut => {
            exit.is_some_and(|code| code != 0)
        }
        RunStatus::Incomplete => true,
    }
}

fn outcome(
    mut record: Record,
    elapsed_ms: u64,
    exit: Result<ExitStatus>,
    trace: Result<Trace>,
) -> Record {
    let code = exit.as_ref().ok().and_then(ExitStatus::code);
    let (status, steps, reason) = match (exit, trace) {
        (_, Err(_)) => (
            RunStatus::Incomplete,
            Vec::new(),
            Some(IncompleteReason::InvalidEvents),
        ),
        (Err(_), Ok(trace)) => (
            RunStatus::Incomplete,
            trace.results,
            Some(IncompleteReason::Supervision),
        ),
        (Ok(_), Ok(trace)) => match trace.final_status {
            Some(status) if matched_exit(status, code) => (status, trace.results, None),
            Some(_) => (
                RunStatus::Incomplete,
                trace.results,
                Some(IncompleteReason::ExitMismatch),
            ),
            None => (
                RunStatus::Incomplete,
                trace.results,
                Some(IncompleteReason::MissingFinal),
            ),
        },
    };
    record.status = status;
    record.steps = steps;
    record.exit_code = code;
    record.reason = reason;
    record.elapsed_ms = Some(elapsed_ms);
    record
}

pub(in crate::maintenance) fn run(
    review: Review,
    cancel: &AtomicBool,
    events: Option<&Sender<DeploymentEvent>>,
) -> Result<Report> {
    require_running(cancel)?;
    // The child takes the maintenance lock. This lock prevents an activation before it does.
    let _activation = acquire_engine_activation_lock(&review.proof.home)?;
    let proof = review_proof(&review.selected, cancel)?;
    ensure!(
        proof == review.proof,
        "Topgrade engine, profile, tools or LLVM candidate changed after review; review again"
    );
    // A prior release is optional; a damaged rollback candidate cannot change this run's scope.
    let rollback = rollback_candidate(&proof.home, &proof.engine, cancel)
        .ok()
        .flatten();
    require_running(cancel)?;
    let (run_id, path) = new_run(&proof.home)?;
    let request = Request {
        schema_version: 1,
        run_id: run_id.clone(),
        steps: review.selected,
        review_digest: proof.digest,
        llvm_plan_digest: proof.llvm.map(|plan| plan.digest),
    };
    validate_request(&request, &run_id)?;
    persist(&path.join("request.json"), &serde_json::to_vec(&request)?)
        .context("Persist the reviewed request before starting Topgrade")?;
    let mut command = Command::new(&proof.engine.binary);
    command
        .arg("--studio-run")
        .arg(&path)
        .current_dir(&proof.home)
        .env_clear()
        .env("HOME", &proof.home)
        .env("PATH", PATH)
        .env("HOMEBREW_NO_AUTO_UPDATE", "1");
    let record = Record::started(&request, proof.engine, rollback);
    publish_record(&path, &record).context("Persist engine provenance before starting Topgrade")?;
    let started = Instant::now();
    let exit = Supervisor {
        path: &path,
        request: &request,
        cancel,
        events,
    }
    .observe(command);
    let trace = read_events(&path).and_then(|bytes| parse_events(&bytes, &request, true));
    let elapsed_ms =
        u64::try_from(started.elapsed().as_millis()).context("Invalid run duration")?;
    let record = outcome(record, elapsed_ms, exit, trace);
    publish_record(&path, &record).context("Publish terminal maintenance evidence")?;
    Ok(record.report(path))
}

fn valid_identity(home: &Path, engine: &EngineIdentity) -> bool {
    release_tree(home, &engine.binary).is_ok_and(|tree| tree == engine.component_tree)
        && valid_hex(&engine.studio_commit, 40)
        && valid_hex(&engine.binary_sha256, 64)
        && valid_hex(&engine.profile_sha256, 64)
        && valid_version(&engine.upstream_version)
        && engine.profile_version == 1
}

fn read_run(home: &Path, path: PathBuf, run_id: &str) -> Result<Report> {
    let metadata = real_metadata(&path)?;
    ensure!(
        metadata.is_dir() && metadata.mode() & 0o077 == 0,
        "Invalid maintenance run directory"
    );
    let request_path = path.join("request.json");
    let request = if present(&request_path)? {
        let bytes = read_regular(&request_path, RECORD_LIMIT)?;
        let request: Request = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Invalid maintenance history request"))?;
        validate_request(&request, run_id)?;
        Some(request)
    } else {
        None
    };
    let terminal = path.join("result.json");
    if !present(&terminal)? {
        return Ok(Record {
            schema_version: 1,
            run_id: run_id.into(),
            status: RunStatus::Incomplete,
            steps: Vec::new(),
            exit_code: None,
            reason: Some(IncompleteReason::Interrupted),
            engine_tree: None,
            elapsed_ms: None,
            engine_source: None,
            engine: None,
            rollback: None,
        }
        .report(path));
    }
    let request = request.context("Maintenance result has no recorded request")?;
    let bytes = read_regular(&terminal, RECORD_LIMIT)?;
    let record: Record = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("Invalid maintenance history result"))?;
    ensure!(
        record.run_id == run_id
            && record
                .engine_tree
                .as_deref()
                .is_some_and(|tree| valid_hex(tree, 40))
            && record.elapsed_ms.is_some()
            && matched_exit(record.status, record.exit_code)
            && (record.status == RunStatus::Incomplete) == record.reason.is_some(),
        "Invalid maintenance result identity or exit status"
    );
    ensure!(
        match record.schema_version {
            1 =>
                record.engine_source.is_none()
                    && record.engine.is_none()
                    && record.rollback.is_none(),
            2 =>
                record.engine_source.as_deref() == Some(ENGINE_SOURCE)
                    && record.engine.as_ref().is_some_and(|engine| {
                        valid_identity(home, engine)
                            && record.engine_tree.as_deref() == Some(engine.component_tree.as_str())
                            && record.rollback.as_ref().is_none_or(|previous| {
                                valid_identity(home, previous)
                                    && previous.component_tree != engine.component_tree
                            })
                    }),
            _ => false,
        },
        "Invalid maintenance engine provenance"
    );
    if record.status != RunStatus::Incomplete {
        let trace = parse_events(&read_events(&path)?, &request, true)?;
        ensure!(
            trace.final_status == Some(record.status) && trace.results == record.steps,
            "Maintenance history disagrees with the recorded engine events"
        );
    } else {
        ensure!(
            record
                .steps
                .iter()
                .zip(&request.steps)
                .all(|(result, id)| &result.id == id)
                && record.steps.len() <= request.steps.len(),
            "Invalid incomplete maintenance result"
        );
    }
    Ok(record.report(path))
}

pub(in crate::maintenance) fn output_excerpt(report: &Report) -> Result<String> {
    const EXCERPT: usize = 16 * 1024;
    ensure!(
        valid_run_id(&report.run_id),
        "Invalid maintenance output identifier"
    );
    let path = directory(&account_home()?).join(&report.run_id);
    ensure!(
        path == report.path,
        "Maintenance output is outside its run directory"
    );
    let metadata = real_metadata(&path)?;
    ensure!(
        metadata.is_dir() && metadata.mode() & 0o077 == 0,
        "Invalid maintenance output directory"
    );
    let mut excerpt = String::new();
    for filename in ["stdout.log", "stderr.log"] {
        let file = path.join(filename);
        if !present(&file)? {
            continue;
        }
        let (input, metadata) = super::open_regular(&file, LOG_LIMIT as u64)?;
        ensure!(
            metadata.mode() & 0o077 == 0,
            "Maintenance output is not private"
        );
        let mut bytes = Vec::new();
        input.take((EXCERPT + 1) as u64).read_to_end(&mut bytes)?;
        if bytes.is_empty() {
            continue;
        }
        excerpt.push_str(filename);
        excerpt.push('\n');
        excerpt.extend(
            String::from_utf8_lossy(&bytes[..bytes.len().min(EXCERPT)])
                .chars()
                .filter(|c| matches!(c, '\n' | '\t') || !c.is_control()),
        );
        if bytes.len() > EXCERPT {
            excerpt.push_str("\n[Excerpt truncated. Full output remains in the run directory.]");
        }
        excerpt.push_str("\n\n");
    }
    Ok(excerpt)
}

pub(in crate::maintenance) fn history() -> Result<Vec<Report>> {
    let home = account_home()?;
    let root = directory(&home);
    if !present(&root)? {
        return Ok(Vec::new());
    }
    let metadata = real_metadata(&root)?;
    ensure!(
        metadata.is_dir() && metadata.mode() & 0o077 == 0,
        "Invalid maintenance history directory"
    );
    let mut entries = Vec::new();
    for entry in fs::read_dir(&root)? {
        ensure!(
            entries.len() < MAX_HISTORY,
            "Maintenance history exceeds its record bound"
        );
        let entry = entry?;
        let id = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("Invalid maintenance history name"))?;
        ensure!(valid_run_id(&id), "Invalid maintenance history identifier");
        entries.push((id, entry.path()));
    }
    entries.sort_by(|left, right| right.0.cmp(&left.0));
    entries
        .into_iter()
        .map(|(id, path)| read_run(&home, path, &id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let time = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
                "studio-maintenance-test-{time}-{}-{sequence}",
                std::process::id()
            ));
            DirBuilder::new().mode(0o700).create(&root).unwrap();
            Self { root }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).unwrap();
        }
    }

    fn request() -> Request {
        Request {
            schema_version: 1,
            run_id: "fixture".into(),
            steps: vec!["helm".into()],
            review_digest: "a".repeat(64),
            llvm_plan_digest: None,
        }
    }

    fn identity(home: &Path, byte: &str) -> EngineIdentity {
        let tree = byte.repeat(40);
        EngineIdentity {
            binary: home
                .join(".local/share/topgrade-rustup-head/releases")
                .join(&tree)
                .join("bin/topgrade"),
            studio_commit: byte.repeat(40),
            component_tree: tree,
            binary_sha256: byte.repeat(64),
            profile_sha256: "e".repeat(64),
            upstream_version: "17.15.0".into(),
            profile_version: 1,
        }
    }

    fn release(home: &Path, byte: &str) -> EngineIdentity {
        use super::super::{ASSETS, sha256_file, verified_engine_release};
        let fixture = identity(home, byte);
        let dir = fixture.binary.parent().unwrap().parent().unwrap();
        fs::create_dir_all(fixture.binary.parent().unwrap()).unwrap();
        fs::write(
            &fixture.binary,
            format!("#!/bin/sh\nprintf '%s\\n' 'topgrade 17.15.0'\n# {byte}\n"),
        )
        .unwrap();
        fs::set_permissions(&fixture.binary, fs::Permissions::from_mode(0o755)).unwrap();
        let manifest = dir.join("profile-manifest.json");
        let assets = ASSETS.map(|name| (name, "e".repeat(64)));
        fs::write(
            &manifest,
            serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "files": assets.iter().map(|(destination, sha256)| {
                    serde_json::json!({"destination": destination, "sha256": sha256})
                }).collect::<Vec<_>>(),
            }))
            .unwrap(),
        )
        .unwrap();
        fs::create_dir(dir.join("profile")).unwrap();
        fs::copy(&manifest, dir.join("profile/manifest.json")).unwrap();
        fs::write(
            dir.join("studio-engine.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "studio_commit": fixture.studio_commit,
                "component_tree": fixture.component_tree,
                "binary_sha256": sha256_file(&fixture.binary).unwrap(),
                "profile_sha256": sha256_file(&manifest).unwrap(),
                "assets": assets.into_iter().collect::<std::collections::BTreeMap<_, _>>(),
            }))
            .unwrap(),
        )
        .unwrap();
        verified_engine_release(home, &fixture.binary).unwrap()
    }

    fn events() -> String {
        [
            (1, "started", Some("helm"), None),
            (2, "result", Some("helm"), Some("success")),
            (3, "finished", None, Some("success")),
        ]
        .into_iter()
        .map(|(seq, kind, step, status)| {
            serde_json::json!({"schema_version":1,"run_id":"fixture","seq":seq,"kind":kind,"step":step,"status":status}).to_string() + "\n"
        })
        .collect()
    }

    #[test]
    fn real_child_requires_both_valid_protocol_and_zero_exit() {
        for (script, expected) in [
            (
                "printf 'stdout'; printf 'stderr' >&2; exit 0",
                RunStatus::Incomplete,
            ),
            (
                "umask 077; printf '%s' \"$1\" > events.jsonl; exit 1",
                RunStatus::Incomplete,
            ),
            (
                "umask 077; printf '%s' \"$1\" > events.jsonl; printf 'stdout'; printf 'stderr' >&2",
                RunStatus::Verified,
            ),
        ] {
            let fixture = Fixture::new();
            let request = request();
            let cancel = AtomicBool::new(false);
            let mut command = Command::new("/bin/sh");
            command
                .args(["-c", script, "fixture", &events()])
                .current_dir(&fixture.root);
            let result = Supervisor {
                path: &fixture.root,
                request: &request,
                cancel: &cancel,
                events: None,
            }
            .observe(command);
            let trace =
                read_events(&fixture.root).and_then(|bytes| parse_events(&bytes, &request, true));
            let current = identity(&fixture.root, "b");
            let previous = identity(&fixture.root, "c");
            let record = outcome(
                Record::started(&request, current.clone(), Some(previous.clone())),
                42,
                result,
                trace,
            );
            assert_eq!(record.status, expected);
            if expected == RunStatus::Verified {
                assert_eq!(
                    fs::read(fixture.root.join("stdout.log")).unwrap(),
                    b"stdout"
                );
                assert_eq!(
                    fs::read(fixture.root.join("stderr.log")).unwrap(),
                    b"stderr"
                );
                assert_eq!(record.steps.len(), 1);
                let evidence = serde_json::to_value(&record).unwrap();
                assert_eq!(evidence["engine_tree"], "b".repeat(40));
                assert_eq!(evidence["elapsed_ms"], 42);
                assert!(evidence.get("stdout").is_none() && evidence.get("stderr").is_none());
                persist(
                    &fixture.root.join("request.json"),
                    &serde_json::to_vec(&request).unwrap(),
                )
                .unwrap();
                persist(
                    &fixture.root.join("result.json"),
                    &serde_json::to_vec(&record).unwrap(),
                )
                .unwrap();
                let report =
                    read_run(&fixture.root, fixture.root.clone(), &request.run_id).unwrap();
                assert_eq!(report.status, RunStatus::Verified);
                assert_eq!(report.engine, Some(current));
                assert_eq!(report.rollback, Some(previous));
            }
        }
    }

    #[test]
    fn reads_old_history_and_preserves_provenance_on_interruption() {
        let fixture = Fixture::new();
        let request = request();
        persist(
            &fixture.root.join("request.json"),
            &serde_json::to_vec(&request).unwrap(),
        )
        .unwrap();
        let current = identity(&fixture.root, "b");
        let initial = Record::started(&request, current.clone(), None);
        let mut legacy = serde_json::to_value(&initial).unwrap();
        legacy["schema_version"] = 1.into();
        for key in ["engine_source", "engine", "rollback"] {
            legacy.as_object_mut().unwrap().remove(key);
        }
        persist(
            &fixture.root.join("result.json"),
            &serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        let read = || read_run(&fixture.root, fixture.root.clone(), &request.run_id).unwrap();
        let old = read();
        assert_eq!(old.status, RunStatus::Incomplete);
        assert_eq!(old.engine_tree, Some(current.component_tree.clone()));
        assert!(old.engine.is_none());
        publish_record(&fixture.root, &initial).unwrap();
        let interrupted = read();
        assert_eq!(interrupted.status, RunStatus::Incomplete);
        assert_eq!(interrupted.engine, Some(current));
    }

    #[test]
    fn same_version_releases_are_distinct_and_tampered_rollback_is_rejected() {
        let fixture = Fixture::new();
        let current = release(&fixture.root, "a");
        let previous = release(&fixture.root, "b");
        assert_eq!(current.upstream_version, previous.upstream_version);
        assert_ne!(current.binary_sha256, previous.binary_sha256);
        let journal = fixture
            .root
            .join(".local/share/topgrade-rustup-head")
            .join(format!("studio-activation-{}.json", current.component_tree));
        fs::write(
            journal,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "target": current.binary,
                "previous": previous.binary,
                "profile_transaction": null,
            }))
            .unwrap(),
        )
        .unwrap();
        let cancel = AtomicBool::new(false);
        assert_eq!(
            rollback_candidate(&fixture.root, &current, &cancel).unwrap(),
            Some(previous.clone())
        );
        fs::write(&previous.binary, "changed").unwrap();
        assert!(rollback_candidate(&fixture.root, &current, &cancel).is_err());
    }

    #[test]
    fn engine_activation_lock_conflicts_with_managed_and_legacy_switches() {
        let fixture = Fixture::new();
        let base = fixture.root.join(".local/share/topgrade-rustup-head");
        fs::create_dir_all(&base).unwrap();
        let path = base.join("lock");
        let legacy_attempt = || {
            Command::new("/bin/sh")
                .args([
                    "-c",
                    "exec 9>>\"$1\" && /usr/bin/lockf -s -t 0 9",
                    "fixture",
                ])
                .arg(&path)
                .status()
                .unwrap()
                .success()
        };
        let running = acquire_engine_activation_lock(&fixture.root).unwrap();
        assert!(acquire_engine_activation_lock(&fixture.root).is_err());
        assert!(!legacy_attempt());
        drop(running);
        assert!(legacy_attempt());
        assert!(acquire_engine_activation_lock(&fixture.root).is_ok());

        fs::remove_file(&path).unwrap();
        let elsewhere = fixture.root.join("unrelated-lock");
        fs::write(&elsewhere, b"keep").unwrap();
        std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
        assert!(acquire_engine_activation_lock(&fixture.root).is_err());
        assert_eq!(fs::read(elsewhere).unwrap(), b"keep");
    }

    #[test]
    fn interrupted_engine_activation_blocks_maintenance() {
        let fixture = Fixture::new();
        let base = fixture.root.join(".local/share/topgrade-rustup-head");
        fs::create_dir_all(&base).unwrap();
        super::super::no_pending(&fixture.root).unwrap();
        persist(&base.join("studio-activation.pending.json"), b"{}").unwrap();
        let error = super::super::no_pending(&fixture.root).unwrap_err();
        assert!(error.to_string().contains("activation is unresolved"));
    }

    #[test]
    fn cancellation_is_acknowledged_only_by_engine() {
        let fixture = Fixture::new();
        let request = request();
        let cancel = AtomicBool::new(true);
        let mut command = Command::new("/bin/sh");
        command.args(["-c", r#"i=0
while [ ! -f cancel ] && [ "$i" -lt 100 ]; do
    i=$((i+1))
    /bin/sleep 0.01
done
[ -f cancel ] || exit 8
umask 077
printf '%s\n' '{"schema_version":1,"run_id":"fixture","seq":1,"kind":"finished","step":null,"status":"cancelled"}' > events.jsonl
exit 1"#]).current_dir(&fixture.root);
        let result = Supervisor {
            path: &fixture.root,
            request: &request,
            cancel: &cancel,
            events: None,
        }
        .observe(command);
        let trace =
            read_events(&fixture.root).and_then(|bytes| parse_events(&bytes, &request, true));
        assert_eq!(
            outcome(
                Record::started(&request, identity(&fixture.root, "b"), None),
                42,
                result,
                trace,
            )
            .status,
            RunStatus::Cancelled
        );
    }
}
