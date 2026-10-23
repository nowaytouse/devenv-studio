//! Bounded capture of both pipes. No shell, downloaded script or inherited stdin.
use anyhow::{Context, Result, ensure};
use std::{
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
const OUTPUT_LIMIT: usize = 1024 * 1024;
#[cfg(windows)]
pub(crate) const TASK_TIMEOUT: Duration = Duration::from_secs(90 * 60);
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Invocation {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub vscode_node: bool,
    pub npm_node: bool,
}
#[derive(Debug, Clone)]
pub(crate) struct Output {
    pub code: u32,
    pub stdout: String,
    pub stderr: String,
    pub cancelled: bool,
    pub timed_out: bool,
}
#[cfg(any(windows, test))]
impl Output {
    pub fn diagnostic(&self) -> String {
        format!(
            "exit=0x{:08X}; cancelled={}; timed_out={}\nstdout:\n{}\nstderr:\n{}",
            self.code, self.cancelled, self.timed_out, self.stdout, self.stderr
        )
    }
}
pub(crate) trait Runner {
    fn run(&mut self, invocation: &Invocation, cancel: &Arc<AtomicBool>) -> Result<Output>;
}
fn configure_npm(command: &mut Command) {
    // npm CLI options pin behavior; remove environment overrides, credentials and Node preloads.
    for (name, _) in std::env::vars_os() {
        if npm_environment_override(&name.to_string_lossy()) {
            command.env_remove(name);
        }
    }
}
fn npm_environment_override(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    name.starts_with("NPM_CONFIG_") || matches!(name.as_str(), "NODE_OPTIONS" | "NODE_PATH")
}
#[cfg(any(windows, test))]
fn configure_pip(command: &mut Command) -> Result<()> {
    crate::pip::check_environment()?;
    for (name, _) in std::env::vars_os() {
        let upper = name.to_string_lossy().to_ascii_uppercase();
        if upper.starts_with("PIP_") || upper.starts_with("PYTHON") {
            command.env_remove(name);
        }
    }
    let null = if cfg!(windows) { "nul" } else { "/dev/null" };
    command.env("PIP_CONFIG_FILE", null).env("NETRC", null);
    Ok(())
}
#[cfg(target_os = "macos")]
fn configure_macos_inventory(command: &mut Command) {
    // Inventory must not inherit auto-update, runtime injection or alternate store overrides.
    for (name, _) in std::env::vars_os() {
        let upper = name.to_string_lossy().to_ascii_uppercase();
        if ["HOMEBREW_", "CARGO_", "RUSTUP_", "UV_", "DYLD_", "VSCODE_"]
            .iter()
            .any(|prefix| upper.starts_with(prefix))
            || matches!(
                upper.as_str(),
                "RUBYOPT"
                    | "RUBYLIB"
                    | "BASH_ENV"
                    | "ENV"
                    | "NODE_OPTIONS"
                    | "NODE_PATH"
                    | "XDG_CONFIG_HOME"
                    | "XDG_DATA_HOME"
            )
        {
            command.env_remove(name);
        }
    }
    command
        .env("HOMEBREW_NO_AUTO_UPDATE", "1")
        .env("HOMEBREW_NO_ANALYTICS", "1")
        .env("HOMEBREW_NO_INSTALL_CLEANUP", "1")
        .env("HOMEBREW_NO_ENV_HINTS", "1")
        .env("HOMEBREW_NO_INSTALL_FROM_API", "1")
        .env("UV_OFFLINE", "1")
        .env("RUSTUP_AUTO_INSTALL", "0")
        .env("LC_ALL", "C")
        .env("LANG", "C");
}
fn capture(mut reader: impl Read) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    let mut exceeded = false;
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error).context("Read command output pipe"),
        };
        if read == 0 {
            break;
        }
        let remaining = OUTPUT_LIMIT.saturating_sub(bytes.len());
        bytes.extend_from_slice(&buffer[..read.min(remaining)]);
        exceeded |= read > remaining;
    }
    ensure!(
        !exceeded,
        "Command output exceeded the 1 MiB per-stream limit; output is incomplete"
    );
    Ok(bytes)
}
fn decode(bytes: Vec<u8>) -> String {
    if bytes.starts_with(&[0xff, 0xfe]) || (bytes.len() > 3 && bytes[1] == 0 && bytes[3] == 0) {
        String::from_utf16_lossy(
            &bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .filter(|&n| n != 0xfeff)
                .collect::<Vec<_>>(),
        )
    } else {
        String::from_utf8_lossy(&bytes).into_owned()
    }
}
pub(crate) fn run_bounded(
    invocation: &Invocation,
    cancel: &Arc<AtomicBool>,
    timeout: Duration,
) -> Result<Output> {
    run_configured(invocation, cancel, timeout, |_| {})
}
#[cfg(any(windows, test))]
pub(crate) fn run_inventory(
    invocation: &Invocation,
    cancel: &Arc<AtomicBool>,
    timeout: Duration,
) -> Result<Output> {
    run_configured(invocation, cancel, timeout, configure_inventory)
}
#[cfg(any(windows, target_os = "macos", test))]
pub(crate) fn run_toolchain(
    invocation: &Invocation,
    cancel: &Arc<AtomicBool>,
    timeout: Duration,
) -> Result<Output> {
    run_configured(invocation, cancel, timeout, |command| {
        for (name, _) in std::env::vars_os() {
            let upper = name.to_string_lossy().to_ascii_uppercase();
            if upper.starts_with("RUSTUP_") || upper.starts_with("CARGO_") {
                command.env_remove(name);
            }
        }
        command.env("RUSTUP_AUTO_INSTALL", "0");
    })
}
#[cfg(any(windows, test))]
fn configure_inventory(command: &mut Command) {
    for (name, _) in std::env::vars_os() {
        let upper = name.to_string_lossy().to_ascii_uppercase();
        if ["CARGO_", "RUSTUP_", "UV_", "VSCODE_"]
            .iter()
            .any(|prefix| upper.starts_with(prefix))
            || matches!(upper.as_str(), "NODE_OPTIONS" | "NODE_PATH")
        {
            command.env_remove(name);
        }
    }
    command
        .env("RUSTUP_AUTO_INSTALL", "0")
        .env("UV_OFFLINE", "1")
        .env("UV_PYTHON_DOWNLOADS", "never");
}
fn run_configured(
    invocation: &Invocation,
    cancel: &Arc<AtomicBool>,
    timeout: Duration,
    configure: impl FnOnce(&mut Command),
) -> Result<Output> {
    ensure!(
        invocation.program.is_absolute(),
        "Command must use a trusted absolute executable path"
    );
    if cancel.load(Ordering::Acquire) {
        return Ok(Output {
            code: u32::MAX,
            stdout: String::new(),
            stderr: String::new(),
            cancelled: true,
            timed_out: false,
        });
    }
    let mut command = Command::new(&invocation.program);
    command
        .args(&invocation.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if invocation.vscode_node {
        ensure!(
            invocation.program.file_name().is_some_and(|name| {
                name.to_string_lossy().eq_ignore_ascii_case("Code.exe")
                    || (cfg!(target_os = "macos") && name == "Electron")
            }),
            "VS Code node environment is restricted to the supported Code executable"
        );
        command
            .env("ELECTRON_RUN_AS_NODE", "1")
            .env_remove("VSCODE_DEV")
            .env_remove("VSCODE_PORTABLE");
    }
    if invocation.npm_node {
        ensure!(
            !invocation.vscode_node
                && invocation.program.file_name().is_some_and(|name| {
                    name.to_string_lossy().eq_ignore_ascii_case("node.exe")
                        || (cfg!(target_os = "macos") && name == "node")
                }),
            "npm environment is restricted to the supported Node executable"
        );
        configure_npm(&mut command);
    }
    #[cfg(any(windows, test))]
    if crate::pip::is_invocation(invocation) {
        configure_pip(&mut command)?;
    }
    #[cfg(target_os = "macos")]
    configure_macos_inventory(&mut command);
    if let Some(parent) = invocation.program.parent() {
        command.current_dir(parent);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    configure(&mut command);
    let mut child = command
        .spawn()
        .with_context(|| format!("Spawn {}", invocation.program.display()))?;
    let stdout = child.stdout.take().context("Missing command stdout pipe")?;
    let stderr = child.stderr.take().context("Missing command stderr pipe")?;
    let (out_tx, out_rx) = mpsc::channel();
    let (err_tx, err_rx) = mpsc::channel();
    let out_thread = thread::spawn(move || {
        let _ = out_tx.send(capture(stdout));
    });
    let err_thread = thread::spawn(move || {
        let _ = err_tx.send(capture(stderr));
    });
    let started = Instant::now();
    let mut cancelled = false;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {}
            Err(error) => {
                let killed = child.kill();
                let waited = child.wait();
                break Err(anyhow::anyhow!(
                    "Wait for command failed: {error}; kill={killed:?}; reap={waited:?}"
                ));
            }
        }
        cancelled = cancel.load(Ordering::Acquire);
        timed_out = started.elapsed() >= timeout;
        if cancelled || timed_out {
            let killed = child.kill();
            let waited = child.wait();
            match waited {
                Ok(status) => {
                    if let Err(error) = killed
                        && status.success()
                    {
                        break Err(anyhow::anyhow!(
                            "Command completed while termination failed: {error}; completion is uncertain"
                        ));
                    }
                    break Ok(status);
                }
                Err(error) => {
                    break Err(anyhow::anyhow!(
                        "Unable to reap cancelled/timed out command: {error}; kill={killed:?}"
                    ));
                }
            }
        }
        thread::sleep(Duration::from_millis(50));
    }?;
    // A descendant can retain an inherited pipe after the direct child exits.
    // Bound that wait and report incomplete capture rather than blocking shutdown.
    let capture_deadline = Instant::now() + Duration::from_secs(2);
    let primary = format!(
        "Command exit={:?}; cancelled={cancelled}; timed_out={timed_out}. Output capture incomplete",
        status.code()
    );
    let stdout = out_rx
        .recv_timeout(capture_deadline.saturating_duration_since(Instant::now()))
        .with_context(|| {
            format!("{primary}: stdout did not close; a descendant may still be running")
        })?
        .with_context(|| primary.clone())?;
    let stderr = err_rx
        .recv_timeout(capture_deadline.saturating_duration_since(Instant::now()))
        .with_context(|| {
            format!("{primary}: stderr did not close; a descendant may still be running")
        })?
        .with_context(|| primary.clone())?;
    out_thread
        .join()
        .map_err(|_| anyhow::anyhow!("stdout reader panicked"))?;
    err_thread
        .join()
        .map_err(|_| anyhow::anyhow!("stderr reader panicked"))?;
    Ok(Output {
        code: status.code().map(|c| c as u32).unwrap_or(u32::MAX),
        stdout: decode(stdout),
        stderr: decode(stderr),
        cancelled,
        timed_out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Write};
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("injected pipe error"))
        }
    }
    #[test]
    fn capture_failures_and_limits_are_errors() {
        assert!(capture(Broken).is_err());
        assert!(capture(io::Cursor::new(vec![b'x'; OUTPUT_LIMIT + 1])).is_err());
    }
    #[test]
    fn child_fixture() {
        if let Ok(mode) = std::env::var("LTSC_DEPLOYMENT_CHILD") {
            if mode == "sleep" {
                thread::sleep(Duration::from_secs(60));
            } else {
                io::stdout().write_all(b"out-complete").unwrap();
                io::stderr().write_all(b"err-complete").unwrap();
            }
        }
    }
    #[test]
    fn standard_runner_captures_both_streams_and_spawn_failure() {
        // The child is this test binary, with one fixture selected. No shell or live system task.
        let exe = std::env::current_exe().unwrap();
        let inv = Invocation {
            vscode_node: false,
            npm_node: false,
            program: exe,
            args: vec![
                "--exact".into(),
                "process::tests::child_fixture".into(),
                "--nocapture".into(),
            ],
        };
        let out = run_configured(
            &inv,
            &Arc::new(AtomicBool::new(false)),
            Duration::from_secs(10),
            |c| {
                c.env("LTSC_DEPLOYMENT_CHILD", "output");
            },
        )
        .unwrap();
        assert_eq!(out.code, 0);
        assert!(!out.timed_out);
        assert!(out.stdout.contains("out-complete"));
        assert!(out.stderr.contains("err-complete"));
        let invalid = Invocation {
            vscode_node: false,
            npm_node: false,
            program: std::env::temp_dir().join("ltsc-does-not-exist.exe"),
            args: vec![],
        };
        assert!(
            run_bounded(
                &invalid,
                &Arc::new(AtomicBool::new(false)),
                Duration::from_millis(20)
            )
            .is_err()
        );
    }
    #[test]
    fn cancelled_before_spawn_and_relative_paths_fail() {
        let inv = Invocation {
            vscode_node: false,
            npm_node: false,
            program: std::env::current_exe().unwrap(),
            args: vec![],
        };
        assert!(
            run_bounded(
                &inv,
                &Arc::new(AtomicBool::new(true)),
                Duration::from_secs(1)
            )
            .unwrap()
            .cancelled
        );
        assert!(
            run_bounded(
                &Invocation {
                    vscode_node: false,
                    npm_node: false,
                    program: "dism.exe".into(),
                    args: vec![]
                },
                &Arc::new(AtomicBool::new(false)),
                Duration::from_secs(1)
            )
            .is_err()
        );
    }
    #[test]
    fn actual_child_timeout_and_cancellation_are_reaped() {
        let inv = Invocation {
            vscode_node: false,
            npm_node: false,
            program: std::env::current_exe().unwrap(),
            args: vec![
                "--exact".into(),
                "process::tests::child_fixture".into(),
                "--nocapture".into(),
            ],
        };
        let output = run_configured(
            &inv,
            &Arc::new(AtomicBool::new(false)),
            Duration::from_millis(150),
            |c| {
                c.env("LTSC_DEPLOYMENT_CHILD", "sleep");
            },
        )
        .unwrap();
        assert!(output.timed_out);
        assert!(!output.cancelled);
        let cancel = Arc::new(AtomicBool::new(false));
        let trigger = cancel.clone();
        let thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(150));
            trigger.store(true, Ordering::Release);
        });
        let output = run_configured(&inv, &cancel, Duration::from_secs(10), |c| {
            c.env("LTSC_DEPLOYMENT_CHILD", "sleep");
        })
        .unwrap();
        thread.join().unwrap();
        assert!(output.cancelled);
        assert!(!output.timed_out);
    }
    #[test]
    fn npm_configuration_removes_case_insensitive_overrides_and_node_preloads() {
        for name in [
            "npm_config_registry",
            "NPM_CONFIG_PREFIX",
            "npm_config_@scope:registry",
            "NODE_OPTIONS",
            "node_path",
        ] {
            assert!(npm_environment_override(name));
        }
        for name in ["PATH", "APPDATA", "USERPROFILE", "NODE_ENV"] {
            assert!(!npm_environment_override(name));
        }
    }
    #[test]
    fn pip_child_uses_exact_null_config_without_changing_parent_environment() {
        let parent = std::env::var_os("PIP_CONFIG_FILE");
        let mut command = Command::new(std::env::current_exe().unwrap());
        configure_pip(&mut command).unwrap();
        let variables: std::collections::BTreeMap<_, _> = command.get_envs().collect();
        let expected = if cfg!(windows) { "nul" } else { "/dev/null" };
        assert_eq!(
            variables[std::ffi::OsStr::new("PIP_CONFIG_FILE")],
            Some(std::ffi::OsStr::new(expected))
        );
        assert_eq!(
            variables[std::ffi::OsStr::new("NETRC")],
            Some(std::ffi::OsStr::new(expected))
        );
        assert_eq!(std::env::var_os("PIP_CONFIG_FILE"), parent);
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn mac_inventory_disables_bootstrap_network_and_updates_without_changing_parent() {
        let prior = std::env::var_os("RUSTUP_AUTO_INSTALL");
        let mut command = Command::new(std::env::current_exe().unwrap());
        configure_macos_inventory(&mut command);
        let variables: std::collections::BTreeMap<_, _> = command.get_envs().collect();
        for (name, value) in [
            ("HOMEBREW_NO_AUTO_UPDATE", "1"),
            ("HOMEBREW_NO_ANALYTICS", "1"),
            ("HOMEBREW_NO_INSTALL_CLEANUP", "1"),
            ("HOMEBREW_NO_INSTALL_FROM_API", "1"),
            ("RUSTUP_AUTO_INSTALL", "0"),
            ("UV_OFFLINE", "1"),
            ("LC_ALL", "C"),
        ] {
            assert_eq!(
                variables[std::ffi::OsStr::new(name)],
                Some(std::ffi::OsStr::new(value))
            );
        }
        assert_eq!(std::env::var_os("RUSTUP_AUTO_INSTALL"), prior);
    }
    #[test]
    fn inventory_disables_runtime_bootstrap_and_network_without_mutating_parent() {
        let mut command = Command::new("unused");
        configure_inventory(&mut command);
        let env: std::collections::BTreeMap<_, _> = command.get_envs().collect();
        assert_eq!(
            env.get(std::ffi::OsStr::new("RUSTUP_AUTO_INSTALL")),
            Some(&Some(std::ffi::OsStr::new("0")))
        );
        assert_eq!(
            env.get(std::ffi::OsStr::new("UV_OFFLINE")),
            Some(&Some(std::ffi::OsStr::new("1")))
        );
        assert_eq!(
            env.get(std::ffi::OsStr::new("UV_PYTHON_DOWNLOADS")),
            Some(&Some(std::ffi::OsStr::new("never")))
        );
        let invocation = Invocation {
            program: "relative.exe".into(),
            args: vec![],
            vscode_node: false,
            npm_node: false,
        };
        assert!(
            run_inventory(
                &invocation,
                &Arc::new(AtomicBool::new(false)),
                Duration::from_secs(1)
            )
            .is_err()
        );
    }
    #[test]
    fn utf16_dism_output_is_decoded() {
        assert_eq!(
            decode(
                "State : Enabled"
                    .encode_utf16()
                    .flat_map(u16::to_le_bytes)
                    .collect()
            ),
            "State : Enabled"
        );
    }
}
