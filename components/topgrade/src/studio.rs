//! The bounded macOS engine used by DevEnv Studio. It does not load Topgrade configuration.

use std::collections::BTreeMap;
use std::env;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Seek, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::os::unix::process::{CommandExt as _, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use color_eyre::eyre::{Context, Result, bail, ensure};
use nix::errno::Errno;
use nix::libc::{self, F_GETFD, F_SETFD, FD_CLOEXEC, O_NOFOLLOW, O_NONBLOCK};
use nix::sys::signal::{Signal, killpg};
use nix::unistd::{Pid, User, geteuid, getuid};
use serde::{Deserialize, Serialize};

use crate::command::CommandExt as _;
use crate::error::{MissingSudo, SkipStep};
use crate::execution_context::RunType;

mod diagnostics;
mod github_skills;
mod helm;
mod inventory;
mod terminal_profile;
#[cfg(test)]
mod tests;
mod xcode;

const SCHEMA: u32 = 1;
const STATE: &str = "Library/Application Support/LTSC Studio";
const MARKER: &str = "maintenance-active.json";
const RELEASES: &str = ".local/share/topgrade-rustup-head/releases";
const ACTIVATION_PENDING: &str = ".local/share/topgrade-rustup-head/studio-activation.pending.json";
const TOOLS_PATH: &str = "/opt/homebrew/bin:/opt/homebrew/sbin:/usr/bin:/bin:/usr/sbin:/sbin";
const REQUIRED_ASSETS: &[&str] = &[
    ".config/topgrade.toml",
    ".config/topgrade.d/90-brew-headsync.toml",
    ".config/topgrade.d/90-skills-coverage.toml",
    ".config/topgrade.d/90-xcode-beta-select.toml",
    ".config/topgrade.d/95-skills-unattended.toml",
];
const MAX_JSON: u64 = 256 * 1024;
const MAX_ASSET: u64 = 512 * 1024 * 1024;
const STEP_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const GRACE: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(100);
const WORKER_START: u8 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Step {
    BrewLinkageAudit,
    CliVersions,
    GithubCliExtensions,
    GithubCliSkills,
    Helm,
    Tldr,
}

impl Step {
    const ALL: [Self; 6] = [
        Self::BrewLinkageAudit,
        Self::CliVersions,
        Self::GithubCliExtensions,
        Self::GithubCliSkills,
        Self::Helm,
        Self::Tldr,
    ];

    fn id(self) -> &'static str {
        match self {
            Self::BrewLinkageAudit => "brew_linkage_audit",
            Self::CliVersions => "cli_versions",
            Self::GithubCliExtensions => "github_cli_extensions",
            Self::GithubCliSkills => "github_cli_skills",
            Self::Helm => "helm",
            Self::Tldr => "tldr",
        }
    }

    fn find(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|step| step.id() == id)
    }

    fn label(self) -> &'static str {
        match self {
            Self::BrewLinkageAudit => "Homebrew Linkage Audit",
            Self::CliVersions => "CLI Tool Versions",
            Self::GithubCliExtensions => "GitHub CLI Extensions",
            Self::GithubCliSkills => "GitHub CLI Skills",
            Self::Helm => "Helm Repositories",
            Self::Tldr => "tldr Cache",
        }
    }

    fn tools(self) -> &'static [&'static str] {
        match self {
            Self::BrewLinkageAudit => &["brew"],
            Self::CliVersions => &[],
            Self::GithubCliExtensions | Self::GithubCliSkills => &["gh"],
            Self::Helm => &["helm", "curl"],
            Self::Tldr => &["tldr"],
        }
    }

    fn run(self) -> Result<()> {
        match self {
            Self::BrewLinkageAudit => diagnostics::run_linkage(),
            Self::CliVersions => diagnostics::run_versions(),
            Self::GithubCliExtensions => tool_command(&resolve_tool("gh")?)?
                .args(["extension", "upgrade", "--all"])
                .status_checked(),
            Self::GithubCliSkills => github_skills::run(),
            Self::Helm => helm::run(),
            Self::Tldr => tool_command(&resolve_tool("tldr")?)?.arg("--update").status_checked(),
        }
    }
}

pub(crate) fn run_native_profile(id: &str, run_type: RunType) -> Result<()> {
    let result = match (Step::find(id), run_type) {
        (None, _) => terminal_profile::run(id, run_type),
        (Some(step), RunType::Dry) => {
            println!("Dry running: {}", step.label());
            Ok(())
        }
        (Some(step), RunType::Wet) => step.run(),
        (Some(step), RunType::Damp) => {
            println!("Executing: {}", step.label());
            step.run()
        }
    };
    // Topgrade's unattended runner records the failure status but hides its reason by default.
    result.inspect_err(|error| {
        if error.downcast_ref::<SkipStep>().is_none() && error.downcast_ref::<MissingSudo>().is_none() {
            eprintln!("{id}: {error:#}");
        }
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    schema_version: u32,
    run_id: String,
    steps: Vec<Step>,
    review_digest: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema_version: u32,
    studio_commit: String,
    component_tree: String,
    binary_sha256: String,
    profile_sha256: String,
    assets: BTreeMap<String, String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Interruption {
    schema_version: u32,
    run_id: String,
    step: Step,
    child_pgid: Option<i32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    Success,
    Failed,
    Cancelled,
    Timeout,
}

struct Events<'a> {
    file: File,
    run_id: &'a str,
    seq: u32,
}

impl Events<'_> {
    fn write(&mut self, kind: &str, step: Option<Step>, status: Option<Outcome>) -> Result<()> {
        self.seq += 1;
        let event = serde_json::json!({
            "schema_version": SCHEMA,
            "run_id": self.run_id,
            "seq": self.seq,
            "kind": kind,
            "step": step,
            "status": status,
        });
        writeln!(self.file, "{event}")?;
        self.file.sync_data()?;
        Ok(())
    }
}

pub fn dispatch() -> Option<Result<bool>> {
    let mut args = env::args_os().skip(1);
    let first = args.next()?;
    if first == "--studio-inventory" {
        return Some(if args.next().is_none() {
            inventory::write().map(|()| true)
        } else {
            Err(color_eyre::eyre::eyre!("Usage: topgrade --studio-inventory"))
        });
    }
    if first == "--studio-run" {
        return Some(match (args.next(), args.next()) {
            (Some(dir), None) => run(Path::new(&dir)),
            _ => Err(color_eyre::eyre::eyre!(
                "Usage: topgrade --studio-run <absolute run directory>"
            )),
        });
    }
    if first == "--studio-worker" {
        return Some(match (args.next(), args.next(), args.next(), args.next()) {
            (Some(dir), Some(step), Some(fd), None) => worker(Path::new(&dir), &step, &fd).map(|()| true),
            _ => Err(color_eyre::eyre::eyre!("Invalid private Studio worker invocation")),
        });
    }
    if first == "--studio-acknowledge-interruption" {
        return Some(if args.next().is_none() {
            acknowledge().map(|()| true)
        } else {
            Err(color_eyre::eyre::eyre!(
                "Usage: topgrade --studio-acknowledge-interruption"
            ))
        });
    }
    None
}

fn account_home() -> Result<PathBuf> {
    let user = User::from_uid(getuid())?.ok_or_else(|| color_eyre::eyre::eyre!("Account home unavailable"))?;
    Ok(user.dir.canonicalize()?)
}

fn user_only() -> Result<()> {
    ensure!(
        !getuid().is_root() && getuid() == geteuid(),
        "Run Studio maintenance as the signed-in user"
    );
    Ok(())
}

fn check_dir(path: &Path, private: bool) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    let mask = if private { 0o077 } else { 0o022 };
    ensure!(
        metadata.is_dir() && metadata.uid() == getuid().as_raw() && metadata.mode() & mask == 0,
        "Unsafe directory: {}",
        path.display()
    );
    Ok(())
}

fn state_dir(home: &Path) -> Result<PathBuf> {
    let mut path = home.to_path_buf();
    for component in Path::new(STATE) {
        path.push(component);
        if let Err(error) = DirBuilder::new().mode(0o700).create(&path)
            && error.kind() != io::ErrorKind::AlreadyExists
        {
            return Err(error).wrap_err_with(|| format!("Could not create {}", path.display()));
        }
        check_dir(&path, false)?;
    }
    Ok(path)
}

fn check_file(file: &File, path: &Path, private: bool, limit: u64) -> Result<()> {
    let metadata = file.metadata()?;
    let mask = if private { 0o077 } else { 0o022 };
    ensure!(
        metadata.is_file()
            && metadata.uid() == getuid().as_raw()
            && metadata.mode() & mask == 0
            && metadata.nlink() == 1
            && metadata.len() <= limit,
        "Unsafe file: {}",
        path.display()
    );
    Ok(())
}

fn open_file(path: &Path, private: bool, limit: u64) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(path)
        .wrap_err_with(|| format!("Could not open {}", path.display()))?;
    check_file(&file, path, private, limit)?;
    ensure!(
        path.canonicalize()? == path,
        "Non-canonical file path: {}",
        path.display()
    );
    Ok(file)
}

fn create_file(path: &Path) -> Result<File> {
    Ok(OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?)
}

fn lock(state: &Path) -> Result<File> {
    let path = state.join("maintenance.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(&path)?;
    check_file(&file, &path, true, 0)?;
    file.try_lock()
        .wrap_err("Could not acquire Studio maintenance lock; another maintenance run may be active")?;
    Ok(file)
}

fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn check_admission(home: &Path, state: &Path) -> Result<()> {
    ensure!(
        !exists(&state.join(MARKER))?,
        "Previous Studio maintenance needs review. Inspect its logs and affected state, then run topgrade --studio-acknowledge-interruption."
    );
    let pending = home.join(ACTIVATION_PENDING);
    ensure!(
        !exists(&pending)?,
        "Pending Topgrade activation needs review before maintenance: {}",
        pending.display()
    );
    Ok(())
}

pub fn lock_cli_run() -> Result<File> {
    let home = account_home()?;
    let state = state_dir(&home)?;
    let lock = lock(&state)?;
    check_admission(&home, &state)?;
    Ok(lock)
}

fn read_json_bytes(file: &mut File) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(MAX_JSON + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= MAX_JSON, "Studio JSON exceeds its size limit");
    Ok(bytes)
}

fn is_hex(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid_run_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

fn request(dir: &Path) -> Result<Request> {
    ensure!(
        dir.is_absolute() && dir.canonicalize()? == dir,
        "Expected an absolute canonical run directory"
    );
    check_dir(dir, true)?;
    let request: Request = serde_json::from_slice(&read_json_bytes(&mut open_file(
        &dir.join("request.json"),
        true,
        MAX_JSON,
    )?)?)?;
    ensure!(
        request.schema_version == SCHEMA && valid_run_id(&request.run_id),
        "Invalid Studio request identity"
    );
    ensure!(is_hex(&request.review_digest, 64), "Invalid reviewed receipt digest");
    ensure!(
        (1..=Step::ALL.len()).contains(&request.steps.len())
            && request.steps.windows(2).all(|pair| pair[0].id() < pair[1].id()),
        "Select distinct supported Studio steps"
    );
    Ok(request)
}

#[expect(
    clippy::disallowed_methods,
    reason = "Studio uses fixed executables without a Topgrade configuration context"
)]
fn command(executable: &Path) -> Command {
    let mut command = Command::new(executable);
    command.env_clear().env("PATH", TOOLS_PATH);
    command
}

fn resolve_tool(name: &str) -> Result<PathBuf> {
    for root in TOOLS_PATH.split(':').map(Path::new) {
        let link = root.join(name);
        if !exists(&link)? {
            continue;
        }
        let path = link.canonicalize()?;
        let metadata = path.metadata()?;
        ensure!(
            ["/opt/homebrew", "/usr/bin", "/bin", "/usr/sbin", "/sbin"]
                .iter()
                .any(|root| path.starts_with(root))
                && metadata.is_file()
                && metadata.mode() & 0o111 != 0
                && metadata.mode() & 0o022 == 0,
            "Untrusted executable for {name}"
        );
        return Ok(path);
    }
    bail!("Required tool {name} is unavailable in the maintenance PATH")
}

fn tool_command(executable: &Path) -> Result<Command> {
    let mut command = command(executable);
    command
        .env("HOME", account_home()?)
        .env("HOMEBREW_NO_AUTO_UPDATE", "1")
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(Stdio::null());
    // Preserve explicit terminal transport and repository choices. The
    // supervisor starts Studio workers with a clean environment.
    for name in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
        "HELM_REPOSITORY_CONFIG",
        "HELM_REPOSITORY_CACHE",
        "HELM_CACHE_HOME",
        "HELM_CONFIG_HOME",
        "HELM_DATA_HOME",
    ] {
        if let Some(value) = env::var_os(name) {
            command.env(name, value);
        }
    }
    Ok(command)
}

fn sha256(file: File) -> Result<String> {
    let output = command(Path::new("/usr/bin/shasum"))
        .args(["-a", "256"])
        .stdin(Stdio::from(file))
        .output_checked_utf8()?;
    let digest = output.stdout.split_whitespace().next().unwrap_or_default();
    ensure!(is_hex(digest, 64), "Invalid SHA-256 output");
    Ok(digest.to_owned())
}

fn verify_file(path: &Path, digest: &str) -> Result<()> {
    ensure!(
        is_hex(digest, 64) && sha256(open_file(path, false, MAX_ASSET)?)? == digest,
        "Managed file changed: {}",
        path.display()
    );
    Ok(())
}

fn verify_assets(home: &Path, receipt: &Receipt) -> Result<()> {
    ensure!(
        receipt.assets.len() == REQUIRED_ASSETS.len(),
        "Engine receipt contains an unexpected managed asset set"
    );
    for &required in REQUIRED_ASSETS {
        let digest = receipt
            .assets
            .get(required)
            .ok_or_else(|| color_eyre::eyre::eyre!("Engine receipt omits required asset {required}"))?;
        verify_file(&home.join(required), digest)?;
    }
    Ok(())
}

fn verify_release(home: &Path, executable: &Path, reviewed_digest: &str) -> Result<Receipt> {
    let bin = executable
        .parent()
        .ok_or_else(|| color_eyre::eyre::eyre!("Missing engine bin directory"))?;
    let release = bin
        .parent()
        .ok_or_else(|| color_eyre::eyre::eyre!("Missing engine release directory"))?;
    ensure!(
        release.parent() == Some(home.join(RELEASES).as_path()) && executable.ends_with("bin/topgrade"),
        "Engine is outside its reviewed managed release"
    );
    let mut file = open_file(&release.join("studio-engine.json"), false, MAX_JSON)?;
    let bytes = read_json_bytes(&mut file)?;
    file.rewind()?;
    ensure!(
        sha256(file.try_clone()?)? == reviewed_digest,
        "Reviewed engine receipt has changed"
    );
    file.rewind()?;
    ensure!(
        bytes == read_json_bytes(&mut file)?,
        "Engine receipt changed while reading"
    );
    let receipt: Receipt = serde_json::from_slice(&bytes)?;
    ensure!(
        receipt.schema_version == SCHEMA && is_hex(&receipt.studio_commit, 40) && is_hex(&receipt.component_tree, 40),
        "Invalid engine receipt provenance"
    );
    ensure!(
        executable == home.join(RELEASES).join(&receipt.component_tree).join("bin/topgrade"),
        "Engine is outside its reviewed managed release"
    );
    verify_file(executable, &receipt.binary_sha256)?;
    verify_file(&release.join("profile-manifest.json"), &receipt.profile_sha256)?;
    verify_assets(home, &receipt)?;
    Ok(receipt)
}

fn persist_marker(state: &Path, marker: &Interruption) -> Result<()> {
    let path = state.join(MARKER);
    if marker.child_pgid.is_none() {
        let mut file = create_file(&path)?;
        serde_json::to_writer(&mut file, marker)?;
        file.sync_all()?;
    } else {
        let mut file = tempfile::NamedTempFile::new_in(state)?;
        serde_json::to_writer(file.as_file_mut(), marker)?;
        file.as_file().sync_all()?;
        file.persist(&path)?;
    }
    File::open(state)?.sync_all()?;
    Ok(())
}

fn clear_marker(state: &Path) -> Result<()> {
    fs::remove_file(state.join(MARKER))?;
    File::open(state)?.sync_all()?;
    Ok(())
}

fn signal_group(pgid: i32, signal: Option<Signal>) -> Result<bool> {
    ensure!(pgid > 1, "Invalid Studio helper process group");
    match killpg(Pid::from_raw(pgid), signal) {
        Ok(()) => Ok(true),
        Err(Errno::ESRCH) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn wait_until(child: &mut Child, deadline: Instant) -> Result<Option<ExitStatus>> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(POLL);
    }
}

fn terminate_group(child: &mut Child, pgid: i32) -> Result<()> {
    for signal in [Signal::SIGTERM, Signal::SIGKILL] {
        signal_group(pgid, Some(signal))?;
        let deadline = Instant::now() + GRACE;
        loop {
            if child.try_wait()?.is_some() && !signal_group(pgid, None)? {
                return Ok(());
            }
            if Instant::now() >= deadline {
                break;
            }
            thread::sleep(POLL);
        }
    }
    bail!("Studio helper group {pgid} is still present after termination");
}

fn set_close_on_exec(fd: RawFd, enabled: bool) -> io::Result<()> {
    // fcntl also checks whether the descriptor is valid before worker adoption.
    let flags = unsafe { libc::fcntl(fd, F_GETFD) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    let flags = if enabled {
        flags | FD_CLOEXEC
    } else {
        flags & !FD_CLOEXEC
    };
    if unsafe { libc::fcntl(fd, F_SETFD, flags) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn worker_command(home: &Path, executable: &Path, lock: &File, dir: &Path, step: Step) -> Command {
    let fd = lock.as_raw_fd();
    let mut command = command(executable);
    command
        .arg("--studio-worker")
        .arg(dir)
        .arg(step.id())
        .arg(fd.to_string())
        .env("HOME", home)
        .env("HOMEBREW_NO_AUTO_UPDATE", "1")
        .env("GH_PROMPT_DISABLED", "1")
        .current_dir(dir);
    // After fork, change the descriptor's close-on-exec bit using fcntl only.
    // The worker restores it before launching tools, so they cannot retain the lock.
    unsafe {
        command.pre_exec(move || set_close_on_exec(fd, false));
    }
    command
}

fn worker(dir: &Path, id: &std::ffi::OsStr, raw: &std::ffi::OsStr) -> Result<()> {
    user_only()?;
    let step = id
        .to_str()
        .and_then(Step::find)
        .ok_or_else(|| color_eyre::eyre::eyre!("Invalid Studio worker step"))?;
    let fd: RawFd = raw
        .to_str()
        .and_then(|value| value.parse().ok())
        .filter(|fd| *fd >= 3)
        .ok_or_else(|| color_eyre::eyre::eyre!("Missing inherited Studio maintenance lock"))?;
    set_close_on_exec(fd, true).wrap_err("Missing inherited Studio maintenance lock")?;
    // This private worker takes ownership of the validated inherited descriptor.
    let lock = unsafe { File::from_raw_fd(fd) };
    let home = account_home()?;
    let state = home.join(STATE);
    let path = state.join("maintenance.lock");
    check_file(&lock, &path, true, 0)?;
    let opened = lock.metadata()?;
    let expected = fs::symlink_metadata(&path)?;
    ensure!(
        expected.is_file() && opened.dev() == expected.dev() && opened.ino() == expected.ino(),
        "Studio worker did not inherit the maintenance lock"
    );
    lock.try_lock()
        .wrap_err("Studio worker did not inherit the active maintenance lock")?;
    let mut start = [0];
    io::stdin()
        .read_exact(&mut start)
        .wrap_err("Studio supervisor did not authorize worker startup")?;
    ensure!(start == [WORKER_START], "Invalid Studio worker startup signal");
    let request = request(dir)?;
    let mut file = open_file(&state.join(MARKER), true, MAX_JSON)?;
    let marker: Interruption = serde_json::from_slice(&read_json_bytes(&mut file)?)?;
    let pid = i32::try_from(std::process::id())?;
    ensure!(
        marker.schema_version == SCHEMA
            && marker.run_id == request.run_id
            && marker.step == step
            && marker.child_pgid == Some(pid)
            && unsafe { libc::getpgrp() } == pid
            && request.steps.contains(&step),
        "Studio worker did not match the authorized run and process group"
    );
    step.run()
}

fn run_step(state: &Path, run_id: &str, step: Step, mut command: Command) -> Result<Outcome> {
    let mut marker = Interruption {
        schema_version: SCHEMA,
        run_id: run_id.to_owned(),
        step,
        child_pgid: None,
    };
    persist_marker(state, &marker)?;
    let mut child = match command
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .process_group(0)
        .spawn_checked()
    {
        Ok(child) => child,
        Err(error) => {
            clear_marker(state)?;
            return Err(error);
        }
    };
    let pgid = i32::try_from(child.id())?;
    marker.child_pgid = Some(pgid);
    let start: Result<()> = (|| {
        persist_marker(state, &marker)?;
        child
            .stdin
            .take()
            .ok_or_else(|| color_eyre::eyre::eyre!("Missing Studio worker startup pipe"))?
            .write_all(&[WORKER_START])?;
        Ok(())
    })();
    if let Err(error) = start {
        if let Err(cleanup) = terminate_group(&mut child, pgid) {
            eprintln!("Studio helper cleanup: {cleanup:#}");
        }
        return Err(error);
    }
    let Some(status) = wait_until(&mut child, Instant::now() + STEP_TIMEOUT)? else {
        if let Err(error) = terminate_group(&mut child, pgid) {
            eprintln!("Studio helper cleanup: {error:#}");
        }
        eprintln!("Studio step {step:?} timed out; review its state before acknowledging interruption");
        return Ok(Outcome::Timeout);
    };
    ensure!(
        status.signal().is_none() && !signal_group(pgid, None)?,
        "Studio step {step:?} did not finish cleanly ({status}); review the retained interruption marker"
    );
    clear_marker(state)?;
    if status.success() {
        Ok(Outcome::Success)
    } else {
        eprintln!("Studio step {step:?} exited with {status}");
        Ok(Outcome::Failed)
    }
}

fn execute(dir: &Path, request: &Request, events: &mut Events<'_>) -> Result<Outcome> {
    let home = account_home()?;
    let state = state_dir(&home)?;
    let lock = lock(&state)?;
    check_admission(&home, &state)?;
    let executable = env::current_exe()?.canonicalize()?;
    let receipt = verify_release(&home, &executable, &request.review_digest)?;
    for (index, &step) in request.steps.iter().enumerate() {
        if exists(&dir.join("cancel"))? {
            return Ok(Outcome::Cancelled);
        }
        // A preceding helper must not invalidate the reviewed next entry point or its dependencies.
        if index > 0 {
            verify_assets(&home, &receipt)?;
        }
        events.write("started", Some(step), None)?;
        let command = worker_command(&home, &executable, &lock, dir, step);
        let outcome = run_step(&state, &request.run_id, step, command).unwrap_or_else(|error| {
            eprintln!("Topgrade Studio: {error:#}");
            Outcome::Failed
        });
        events.write("result", Some(step), Some(outcome))?;
        if outcome != Outcome::Success {
            return Ok(outcome);
        }
    }
    Ok(Outcome::Success)
}

fn run(dir: &Path) -> Result<bool> {
    user_only()?;
    let request = request(dir)?;
    let mut events = Events {
        file: create_file(&dir.join("events.jsonl"))?,
        run_id: &request.run_id,
        seq: 0,
    };
    let outcome = execute(dir, &request, &mut events).unwrap_or_else(|error| {
        eprintln!("Topgrade Studio: {error:#}");
        Outcome::Failed
    });
    events.write("finished", None, Some(outcome))?;
    Ok(outcome == Outcome::Success)
}

fn acknowledge() -> Result<()> {
    user_only()?;
    let state = state_dir(&account_home()?)?;
    let _lock = lock(&state)?;
    let mut file = open_file(&state.join(MARKER), true, MAX_JSON)?;
    let marker: Interruption = serde_json::from_slice(&read_json_bytes(&mut file)?)?;
    ensure!(
        marker.schema_version == SCHEMA && valid_run_id(&marker.run_id),
        "Invalid interruption marker; preserve it for manual review"
    );
    let Some(pgid) = marker.child_pgid else {
        bail!(
            "Helper group was never recorded. Review the run logs and affected Skills or Helm state, confirm there are no retained helpers or descendants, then archive {} before retrying.",
            state.join(MARKER).display()
        );
    };
    ensure!(
        !signal_group(pgid, None)?,
        "Studio helper group {pgid} is still present"
    );
    clear_marker(&state)?;
    println!(
        "Acknowledged Studio interruption for run {} ({:?})",
        marker.run_id, marker.step
    );
    Ok(())
}
