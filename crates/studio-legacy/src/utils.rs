use chrono::Local;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::{Duration, Instant};

const MAX_CAPTURE_BYTES: usize = 512 * 1024;

pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    studio_core::write_atomic(path, bytes)
}

#[derive(Debug, Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn from_shared(cancel: Arc<AtomicBool>) -> Self {
        Self(cancel)
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandState {
    Exited,
    TimedOut,
    Cancelled,
    SpawnFailed,
    WaitFailed,
    OutputFailed,
}

#[derive(Debug, Clone)]
pub struct CommandResult {
    pub state: CommandState,
    pub exit_code: Option<i32>,
    pub output: String,
}

impl CommandResult {
    pub fn succeeded(&self) -> bool {
        self.state == CommandState::Exited && self.exit_code == Some(0)
    }

    pub fn succeeded_or_reboot_required(&self) -> bool {
        self.state == CommandState::Exited && matches!(self.exit_code, Some(0 | 3010))
    }

    pub fn cancelled(&self) -> bool {
        self.state == CommandState::Cancelled
    }

    pub fn diagnostic(&self) -> &str {
        self.output.trim().lines().last().unwrap_or("无诊断输出")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Ok,
    Warn,
    Error,
    Start,
    End,
}

#[derive(Debug, Clone)]
pub struct LogMessage {
    pub time: String,
    pub level: LogLevel,
    pub message: String,
}

impl LogMessage {
    pub fn new(level: LogLevel, message: impl Into<String>) -> Self {
        Self {
            time: Local::now().format("%H:%M:%S").to_string(),
            level,
            message: message.into(),
        }
    }
}

pub fn is_admin() -> bool {
    #[cfg(target_os = "windows")]
    {
        use winreg::RegKey;
        use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE};

        RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey_with_flags("SOFTWARE", KEY_READ | KEY_WRITE)
            .is_ok()
    }

    #[cfg(not(target_os = "windows"))]
    {
        true
    }
}

pub fn run_native_cmd(
    program: &str,
    args: &[&str],
    cancellation: &CancellationToken,
) -> CommandResult {
    run_native_cmd_timeout(program, args, 60, cancellation)
}

pub fn run_native_cmd_timeout(
    program: &str,
    args: &[&str],
    timeout_secs: u64,
    cancellation: &CancellationToken,
) -> CommandResult {
    if cancellation.is_cancelled() {
        return CommandResult {
            state: CommandState::Cancelled,
            exit_code: None,
            output: "操作已取消".to_string(),
        };
    }

    let mut command = match command_with_tool_paths(program) {
        Ok(command) => command,
        Err(error) => {
            return CommandResult {
                state: CommandState::SpawnFailed,
                exit_code: None,
                output: format!("无法构造 {program} 子进程环境：{error}"),
            };
        }
    };
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return CommandResult {
                state: CommandState::SpawnFailed,
                exit_code: None,
                output: format!("无法启动 {program}: {error}"),
            };
        }
    };

    let stdout = child.stdout.take().map(spawn_pipe_reader);
    let stderr = child.stderr.take().map(spawn_pipe_reader);
    let started = Instant::now();

    let (state, exit_code, wait_error) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (CommandState::Exited, status.code(), None),
            Ok(None) if cancellation.is_cancelled() => {
                terminate_process_tree(&mut child);
                break (
                    CommandState::Cancelled,
                    None,
                    Some("操作已取消".to_string()),
                );
            }
            Ok(None) if started.elapsed() >= Duration::from_secs(timeout_secs) => {
                terminate_process_tree(&mut child);
                break (
                    CommandState::TimedOut,
                    None,
                    Some(format!("执行超时（{} 秒），进程树已终止", timeout_secs)),
                );
            }
            Ok(None) => thread::sleep(Duration::from_millis(75)),
            Err(error) => {
                terminate_process_tree(&mut child);
                break (CommandState::WaitFailed, None, Some(error.to_string()));
            }
        }
    };

    // shortcut: inherited pipes can outlive this deadline; use cancellable I/O when supporting persistent descendants.
    let output_deadline = Instant::now() + Duration::from_secs(2);
    let stdout = receive_capture(stdout, output_deadline);
    let stderr = receive_capture(stderr, output_deadline);
    finish_command(state, exit_code, wait_error, stdout, stderr)
}

#[derive(Default)]
struct PipeCapture {
    bytes: Vec<u8>,
    truncated: bool,
    error: Option<String>,
}

fn receive_capture(reader: Option<Receiver<PipeCapture>>, deadline: Instant) -> PipeCapture {
    let result = match reader {
        Some(reader) => reader
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|error| match error {
                std::sync::mpsc::RecvTimeoutError::Timeout => "等待输出读取超时".to_string(),
                std::sync::mpsc::RecvTimeoutError::Disconnected => {
                    "输出读取线程异常退出".to_string()
                }
            }),
        None => Err("无法取得输出管道".to_string()),
    };
    result.unwrap_or_else(|error| PipeCapture {
        error: Some(error),
        ..PipeCapture::default()
    })
}

fn spawn_pipe_reader(mut pipe: impl Read + Send + 'static) -> Receiver<PipeCapture> {
    let (sender, receiver) = channel();
    thread::spawn(move || {
        let mut kept = VecDeque::with_capacity(MAX_CAPTURE_BYTES);
        let mut buffer = [0_u8; 8192];
        let mut truncated = false;
        let mut error = None;

        loop {
            let count = match pipe.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(read_error) => {
                    error = Some(read_error.to_string());
                    break;
                }
            };
            kept.extend(&buffer[..count]);
            if kept.len() > MAX_CAPTURE_BYTES {
                let excess = kept.len() - MAX_CAPTURE_BYTES;
                kept.drain(..excess);
                truncated = true;
            }
        }

        let _ = sender.send(PipeCapture {
            bytes: kept.into_iter().collect(),
            truncated,
            error,
        });
    });
    receiver
}

fn finish_command(
    state: CommandState,
    exit_code: Option<i32>,
    wait_error: Option<String>,
    stdout: PipeCapture,
    stderr: PipeCapture,
) -> CommandResult {
    let capture_failed = stdout.error.is_some() || stderr.error.is_some();
    let truncated = stdout.truncated || stderr.truncated;
    let mut bytes = stdout.bytes;
    if !bytes.is_empty() && !stderr.bytes.is_empty() {
        bytes.push(b'\n');
    }
    bytes.extend(stderr.bytes);

    let mut output = String::from_utf8_lossy(&bytes).trim().to_string();
    if truncated {
        let notice = "[较早的命令输出已截断]";
        output = if output.is_empty() {
            notice.to_string()
        } else {
            format!("{notice}\n{output}")
        };
    }
    for (stream, error) in [("stdout", stdout.error), ("stderr", stderr.error)] {
        if let Some(error) = error {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(&format!("{stream} 读取失败：{error}"));
        }
    }
    if let Some(error) = wait_error {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(&error);
    }
    CommandResult {
        state: if state == CommandState::Exited && capture_failed {
            CommandState::OutputFailed
        } else {
            state
        },
        exit_code,
        output,
    }
}

fn terminate_process_tree(child: &mut Child) {
    #[cfg(target_os = "windows")]
    {
        let pid = child.id().to_string();
        if let Ok(mut killer) = Command::new("taskkill.exe")
            .args(["/PID", &pid, "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            let started = Instant::now();
            loop {
                match killer.try_wait() {
                    Ok(Some(_)) | Err(_) => break,
                    Ok(None) if started.elapsed() >= Duration::from_secs(5) => {
                        let _ = killer.kill();
                        let _ = killer.wait();
                        break;
                    }
                    Ok(None) => thread::sleep(Duration::from_millis(50)),
                }
            }
        }
    }

    let _ = child.kill();
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(5) {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) => thread::sleep(Duration::from_millis(50)),
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn command_with_tool_paths(program: &str) -> anyhow::Result<Command> {
    Ok(Command::new(program))
}

#[cfg(target_os = "windows")]
fn command_with_tool_paths(program: &str) -> anyhow::Result<Command> {
    let lower = program.to_ascii_lowercase();
    let mut command = if lower.ends_with(".cmd") || lower.ends_with(".bat") {
        let mut command = Command::new("cmd.exe");
        command.args(["/D", "/S", "/C", program]);
        command
    } else {
        Command::new(program)
    };
    let mut paths: Vec<_> = std::env::var_os("PATH")
        .map(|value| std::env::split_paths(&value).collect())
        .unwrap_or_default();

    let mut add = |base: Option<std::ffi::OsString>, suffix: &[&str]| {
        if let Some(base) = base {
            let path = suffix
                .iter()
                .fold(std::path::PathBuf::from(base), |path, part| path.join(part));
            if !paths.iter().any(|existing| {
                existing
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&path.to_string_lossy())
            }) {
                paths.push(path);
            }
        }
    };

    add(
        std::env::var_os("LOCALAPPDATA"),
        &["Microsoft", "WindowsApps"],
    );
    add(
        std::env::var_os("LOCALAPPDATA"),
        &["Programs", "Microsoft VS Code", "bin"],
    );
    add(
        std::env::var_os("LOCALAPPDATA"),
        &["Programs", "cursor", "resources", "app", "bin"],
    );
    let scoop_root = crate::platform::scoop_root()?;
    let shims = scoop_shims_path(Some(scoop_root.as_os_str()), None)
        .ok_or_else(|| anyhow::anyhow!("Scoop shims path is unavailable"))?;
    add(Some(shims.into_os_string()), &[]);
    command.env("SCOOP", &scoop_root);
    add(std::env::var_os("USERPROFILE"), &[".cargo", "bin"]);
    add(std::env::var_os("USERPROFILE"), &[".local", "bin"]);
    add(std::env::var_os("APPDATA"), &["npm"]);
    add(std::env::var_os("PROGRAMFILES"), &["Git", "cmd"]);
    add(std::env::var_os("PROGRAMFILES"), &["PowerShell", "7"]);
    add(std::env::var_os("PROGRAMFILES"), &["nodejs"]);
    add(
        std::env::var_os("PROGRAMFILES"),
        &["Microsoft VS Code", "bin"],
    );

    command.env("PATH", std::env::join_paths(paths)?);
    Ok(command)
}

#[cfg(any(target_os = "windows", test))]
fn scoop_shims_path(
    scoop_root: Option<&std::ffi::OsStr>,
    user_profile: Option<&std::ffi::OsStr>,
) -> Option<std::path::PathBuf> {
    scoop_root
        .map(std::path::PathBuf::from)
        .or_else(|| user_profile.map(|profile| std::path::PathBuf::from(profile).join("scoop")))
        .map(|root| root.join("shims"))
}

pub(crate) fn reject_links(path: &Path) -> std::io::Result<()> {
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                #[cfg(windows)]
                let is_reparse = {
                    use std::os::windows::fs::MetadataExt;
                    metadata.file_attributes() & 0x400 != 0
                };
                #[cfg(not(windows))]
                let is_reparse = false;
                if metadata.file_type().is_symlink() || is_reparse {
                    return Err(std::io::Error::other(format!(
                        "Configuration path contains a link: {}",
                        ancestor.display()
                    )));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub(crate) fn read_bytes_bounded(path: &Path) -> std::io::Result<Vec<u8>> {
    reject_links(path)?;
    if !std::fs::symlink_metadata(path)?.is_file() {
        return Err(std::io::Error::other("Configuration is not a regular file"));
    }
    let file = std::fs::File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other(
            "Configuration handle is not a regular file",
        ));
    }
    let mut bytes = Vec::new();
    file.take(4_194_305).read_to_end(&mut bytes)?;
    if bytes.len() > 4_194_304 {
        return Err(std::io::Error::other("Configuration exceeds 4 MiB"));
    }
    Ok(bytes)
}

pub(crate) fn read_config(path: &Path) -> std::io::Result<String> {
    String::from_utf8(read_bytes_bounded(path)?)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

pub(crate) fn write_preserving(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    reject_links(path)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match read_bytes_bounded(path) {
        Ok(original) => {
            if original == bytes {
                return Ok(());
            }
            static BACKUP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let mut saved = false;
            for _ in 0..32 {
                let mut name = path
                    .file_name()
                    .ok_or_else(|| std::io::Error::other("Missing file name"))?
                    .to_os_string();
                name.push(format!(
                    ".devenv-original-{}-{}-{}",
                    chrono::Utc::now().format("%Y%m%dT%H%M%S%fZ"),
                    std::process::id(),
                    BACKUP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
                ));
                let backup = path.with_file_name(name);
                let mut options = std::fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                match options.open(&backup) {
                    Ok(mut file) => {
                        file.write_all(&original)?;
                        file.sync_all()?;
                        saved = true;
                        break;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            }
            if !saved {
                return Err(std::io::Error::other(
                    "Unable to reserve original-file backup",
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    reject_links(path)?;
    write_atomic(path, bytes)
}

pub fn update_managed_block(
    path: &Path,
    start_marker: &str,
    end_marker: &str,
    body: &str,
) -> std::io::Result<bool> {
    let existing = match read_config(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let block = format!(
        "{}\n{}\n{}",
        start_marker,
        body.trim_matches(['\r', '\n']),
        end_marker
    );

    let updated = match (existing.find(start_marker), existing.find(end_marker)) {
        (Some(start), Some(end)) if end >= start => {
            let suffix_start = end + end_marker.len();
            format!(
                "{}{}{}",
                &existing[..start],
                block,
                &existing[suffix_start..]
            )
        }
        _ if existing.trim().is_empty() => format!("{block}\n"),
        _ => format!("{}\n\n{block}\n", existing.trim_end()),
    };

    if updated == existing {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_preserving(path, updated.as_bytes())?;
    Ok(true)
}

pub fn upsert_ini_values(
    path: &Path,
    section: &str,
    values: &[(&str, &str)],
) -> std::io::Result<bool> {
    let existing = match read_config(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();
    let header = format!("[{section}]");
    let section_start = lines
        .iter()
        .position(|line| line.trim().eq_ignore_ascii_case(&header));

    let (start, mut end) = if let Some(start) = section_start {
        let end = lines
            .iter()
            .enumerate()
            .skip(start + 1)
            .find(|(_, line)| {
                let line = line.trim();
                line.starts_with('[') && line.ends_with(']')
            })
            .map(|(index, _)| index)
            .unwrap_or(lines.len());
        (start + 1, end)
    } else {
        if !lines.is_empty() && !lines.last().is_some_and(String::is_empty) {
            lines.push(String::new());
        }
        lines.push(header);
        (lines.len(), lines.len())
    };

    for (key, value) in values {
        let existing_key = lines[start..end].iter().position(|line| {
            line.split_once('=')
                .map(|(candidate, _)| candidate.trim().eq_ignore_ascii_case(key))
                .unwrap_or(false)
        });
        let updated = format!("{key} = {value}");
        if let Some(relative_index) = existing_key {
            lines[start + relative_index] = updated;
        } else {
            lines.insert(end, updated);
            end += 1;
        }
    }

    let updated = if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    };
    if updated == existing {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_preserving(path, updated.as_bytes())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::{
        CancellationToken, CommandState, PipeCapture, finish_command, receive_capture,
        run_native_cmd_timeout, scoop_shims_path, spawn_pipe_reader, update_managed_block,
        upsert_ini_values, write_atomic,
    };
    use std::time::{Duration, SystemTime};

    fn test_file(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("ltsc-tools-{name}-{}-{unique}", std::process::id()))
    }

    #[test]
    fn pipe_read_failure_preserves_partial_output_and_reports_error() {
        struct FailingReader;
        impl std::io::Read for FailingReader {
            fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("injected read failure"))
            }
        }
        use std::io::Read;
        let reader = std::io::Cursor::new(b"partial output").chain(FailingReader);
        let capture = spawn_pipe_reader(reader)
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        let result = finish_command(
            CommandState::Exited,
            Some(0),
            None,
            capture,
            PipeCapture::default(),
        );
        assert!(result.output.contains("partial output"));
        assert!(
            result
                .output
                .contains("stdout 读取失败：injected read failure")
        );
        assert_eq!(result.state, CommandState::OutputFailed);
        assert!(!result.succeeded());
        assert!(!result.succeeded_or_reboot_required());
    }

    #[test]
    fn interrupted_pipe_read_is_retried() {
        struct InterruptedOnce(bool);
        impl std::io::Read for InterruptedOnce {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                if !self.0 {
                    self.0 = true;
                    Err(std::io::ErrorKind::Interrupted.into())
                } else {
                    std::io::Read::read(&mut &b"complete"[..], buffer)
                }
            }
        }
        let reader = InterruptedOnce(false);
        use std::io::Read;
        let capture = spawn_pipe_reader(reader.take(8))
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        let result = finish_command(
            CommandState::Exited,
            Some(0),
            None,
            capture,
            PipeCapture::default(),
        );
        assert!(result.succeeded(), "{}", result.output);
        assert_eq!(result.output, "complete");
    }

    #[test]
    fn incomplete_output_cannot_be_success_even_with_a_success_exit_code() {
        for code in [0, 3010] {
            let (_sender, receiver) = std::sync::mpsc::channel();
            let capture = receive_capture(Some(receiver), std::time::Instant::now());
            let result = finish_command(
                CommandState::Exited,
                Some(code),
                None,
                PipeCapture::default(),
                capture,
            );
            assert_eq!(result.state, CommandState::OutputFailed);
            assert!(!result.succeeded_or_reboot_required());
            assert!(result.output.contains("stderr 读取失败：等待输出读取超时"));
        }
    }

    #[test]
    fn disconnected_output_preserves_timeout_and_cancellation_states() {
        for state in [
            CommandState::Cancelled,
            CommandState::TimedOut,
            CommandState::WaitFailed,
        ] {
            let (sender, receiver) = std::sync::mpsc::channel();
            drop(sender);
            let capture = receive_capture(Some(receiver), std::time::Instant::now());
            let result = finish_command(
                state.clone(),
                None,
                Some("primary failure".into()),
                capture,
                PipeCapture::default(),
            );
            assert_eq!(result.state, state);
            assert!(result.output.contains("输出读取线程异常退出"));
            assert_eq!(result.diagnostic(), "primary failure");
        }
    }

    #[test]
    fn empty_output_at_eof_is_still_a_success() {
        let capture = spawn_pipe_reader(std::io::Cursor::new(Vec::<u8>::new()))
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        let result = finish_command(
            CommandState::Exited,
            Some(0),
            None,
            capture,
            PipeCapture::default(),
        );
        assert!(result.succeeded(), "{}", result.output);
        assert!(result.output.is_empty());
    }

    #[test]
    fn atomic_write_replaces_whole_files_and_cleans_up_failed_staging() {
        let root = test_file("atomic");
        std::fs::create_dir_all(&root).unwrap();
        let destination = root.join("state.json");
        std::fs::write(&destination, b"old").unwrap();
        write_atomic(&destination, b"new state").unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"new state");

        let directory = root.join("directory");
        std::fs::create_dir(&directory).unwrap();
        assert!(write_atomic(&directory, b"must not replace a directory").is_err());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
        assert_eq!(std::fs::read(&destination).unwrap(), b"new state");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn drains_large_stdout_and_stderr_without_timing_out() {
        let executable = std::env::current_exe().unwrap();
        let executable = executable.to_str().unwrap();
        let args = [
            "--exact",
            "utils::tests::emit_large_output",
            "--ignored",
            "--nocapture",
        ];

        let result = run_native_cmd_timeout(executable, &args, 10, &CancellationToken::default());

        assert!(result.succeeded(), "child failed: {}", result.output);
        assert!(result.output.contains("large-output-complete"));
    }

    #[test]
    fn cancellation_terminates_running_command() {
        let executable = std::env::current_exe().unwrap();
        let executable = executable.to_str().unwrap().to_string();
        let cancellation = CancellationToken::default();
        let child_cancellation = cancellation.clone();
        let worker = std::thread::spawn(move || {
            run_native_cmd_timeout(
                &executable,
                &[
                    "--exact",
                    "utils::tests::wait_until_cancelled",
                    "--ignored",
                    "--nocapture",
                ],
                30,
                &child_cancellation,
            )
        });

        std::thread::sleep(Duration::from_millis(150));
        cancellation.cancel();
        let result = worker.join().unwrap();

        assert_eq!(result.state, CommandState::Cancelled);
    }

    #[cfg(windows)]
    #[test]
    fn tool_paths_are_injected_only_into_the_child_environment() {
        let before_path = std::env::var_os("PATH");
        let before_scoop = std::env::var_os("SCOOP");
        let command = super::command_with_tool_paths("scoop.cmd").unwrap();
        assert_eq!(std::env::var_os("PATH"), before_path);
        assert_eq!(std::env::var_os("SCOOP"), before_scoop);
        let environment: std::collections::BTreeMap<_, _> = command.get_envs().collect();
        let root = crate::platform::scoop_root().unwrap();
        assert_eq!(
            environment
                .get(std::ffi::OsStr::new("SCOOP"))
                .copied()
                .flatten(),
            Some(root.as_os_str())
        );
        let path = environment
            .get(std::ffi::OsStr::new("PATH"))
            .copied()
            .flatten()
            .unwrap();
        assert!(std::env::split_paths(path).any(|entry| entry == root.join("shims")));
    }

    #[test]
    fn scoop_shims_path_uses_custom_root_and_default_user_root() {
        use std::ffi::OsStr;
        use std::path::Path;

        assert_eq!(
            scoop_shims_path(
                Some(OsStr::new("/portable/scoop")),
                Some(OsStr::new("/home/user"))
            ),
            Some(Path::new("/portable/scoop").join("shims"))
        );
        assert_eq!(
            scoop_shims_path(None, Some(OsStr::new("/home/user"))),
            Some(Path::new("/home/user").join("scoop").join("shims"))
        );
        assert_eq!(
            scoop_shims_path(Some(OsStr::new("")), Some(OsStr::new("/home/user"))),
            Some(Path::new("shims").to_path_buf())
        );
        assert_eq!(scoop_shims_path(None, None), None);
    }

    #[test]
    fn preserving_write_saves_original_and_bounds_reads() {
        let directory = test_file("preserving-directory");
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("settings.json");
        std::fs::write(&path, b"original").unwrap();
        super::write_preserving(&path, b"updated").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"updated");
        let backups: Vec<_> = std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|entry| entry != &path)
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read(&backups[0]).unwrap(), b"original");
        let oversized = directory.join("large");
        std::fs::File::create(&oversized)
            .unwrap()
            .set_len(4_194_305)
            .unwrap();
        assert!(super::read_bytes_bounded(&oversized).is_err());
        assert!(super::write_preserving(&oversized, b"replacement").is_err());
        assert_eq!(std::fs::metadata(&oversized).unwrap().len(), 4_194_305);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn preserving_write_rejects_dangling_and_parent_symlinks() {
        let directory = test_file("link-directory");
        std::fs::create_dir(&directory).unwrap();
        let target = directory.join("target");
        std::fs::create_dir(&target).unwrap();
        let parent_link = directory.join("linked-parent");
        std::os::unix::fs::symlink(&target, &parent_link).unwrap();
        assert!(super::write_preserving(&parent_link.join("config"), b"new").is_err());
        assert!(!target.join("config").exists());
        let dangling = directory.join("dangling");
        std::os::unix::fs::symlink(directory.join("absent"), &dangling).unwrap();
        assert!(super::write_preserving(&dangling, b"new").is_err());
        assert!(
            std::fs::symlink_metadata(&dangling)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn managed_block_preserves_unmanaged_content_and_updates_in_place() {
        let path = test_file("managed");
        std::fs::write(&path, "user setting\n").unwrap();

        assert!(update_managed_block(&path, "# start", "# end", "first").unwrap());
        assert!(update_managed_block(&path, "# start", "# end", "second").unwrap());
        assert!(!update_managed_block(&path, "# start", "# end", "second").unwrap());

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.starts_with("user setting"));
        assert!(!content.contains("first"));
        assert_eq!(content.matches("# start").count(), 1);
        assert!(content.contains("second"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn ini_upsert_reuses_existing_section() {
        let path = test_file("ini");
        std::fs::write(&path, "[global]\ntimeout = 30\n\n[install]\nquiet = true\n").unwrap();

        assert!(
            upsert_ini_values(
                &path,
                "global",
                &[
                    ("index-url", "https://example.test/simple"),
                    ("timeout", "60")
                ],
            )
            .unwrap()
        );
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content.matches("[global]").count(), 1);
        assert!(content.contains("timeout = 60"));
        assert!(content.contains("[install]\nquiet = true"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    #[ignore]
    fn emit_large_output() {
        for _ in 0..12_000 {
            println!("stdout-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx");
            eprintln!("stderr-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx");
        }
        println!("large-output-complete");
    }

    #[test]
    #[ignore]
    fn wait_until_cancelled() {
        std::thread::sleep(Duration::from_secs(20));
    }
}
