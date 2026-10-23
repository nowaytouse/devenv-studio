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

fn network_provider(invocation: &Invocation) -> Option<&'static str> {
    if invocation.npm_node {
        return Some("npm");
    }
    #[cfg(any(windows, test))]
    if crate::pip::is_invocation(invocation) {
        return Some("pip");
    }
    let name = invocation
        .program
        .file_name()?
        .to_str()?
        .trim_end_matches(".exe");
    match name {
        "brew" => Some("brew"),
        "rustup" => Some("rustup"),
        "cargo" => Some("cargo"),
        "uv" => Some("uv"),
        _ => None,
    }
}
fn network_arguments(
    invocation: &Invocation,
    profile: &crate::network::NetworkProfile,
) -> Result<Vec<String>> {
    use crate::network::ProxyMode;
    crate::network::validate(profile)?;
    let mut args = invocation.args.clone();
    let Some(provider) = network_provider(invocation) else {
        return Ok(args);
    };
    let mut options = Vec::new();
    match provider {
        "npm" => {
            for arg in &mut args {
                if arg.starts_with("--registry=") {
                    *arg = format!("--registry={}", profile.npm_registry);
                }
            }
            options.push("--strict-ssl=true".into());
            match profile.mode {
                ProxyMode::Manual => options.extend([
                    format!("--proxy={}", profile.proxy_url),
                    format!("--https-proxy={}", profile.proxy_url),
                    "--noproxy=".into(),
                ]),
                ProxyMode::Direct => options.extend([
                    "--proxy=null".into(),
                    "--https-proxy=null".into(),
                    "--noproxy=*".into(),
                ]),
                ProxyMode::Inherit => {}
            }
        }
        "pip" => {
            for arg in &mut args {
                if arg.starts_with("--index-url=") {
                    *arg = format!("--index-url={}", profile.python_index);
                }
            }
            if profile.mode == ProxyMode::Manual {
                options.push(format!("--proxy={}", profile.proxy_url));
            }
        }
        "uv" if args.first().is_some_and(|a| a == "tool")
            && args
                .get(1)
                .is_some_and(|a| matches!(a.as_str(), "install" | "upgrade")) =>
        {
            options.push(format!("--default-index={}", profile.python_index));
        }
        "cargo" if profile.cargo_index != crate::network::NetworkProfile::default().cargo_index => {
            // CLI source replacement preserves Cargo's checksum validation and existing --locked policy.
            options.extend([
                "--config".into(),
                "source.crates-io.replace-with=\"ltsc-studio\"".into(),
                "--config".into(),
                format!("source.ltsc-studio.registry=\"{}\"", profile.cargo_index),
            ]);
            options.extend(args);
            return Ok(options);
        }
        _ => {}
    }
    let insert = args.iter().position(|a| a == "--").unwrap_or(args.len());
    args.splice(insert..insert, options);
    Ok(args)
}
fn configure_network(
    command: &mut Command,
    invocation: &Invocation,
    profile: &crate::network::NetworkProfile,
) -> Result<()> {
    use crate::network::ProxyMode;
    crate::network::validate(profile)?;
    let Some(provider) = network_provider(invocation) else {
        return Ok(());
    };
    if provider == "uv" {
        for (name, _) in std::env::vars_os() {
            let name_text = name.to_string_lossy().to_ascii_uppercase();
            if name_text.starts_with("UV_INDEX")
                || matches!(
                    name_text.as_str(),
                    "UV_DEFAULT_INDEX"
                        | "UV_EXTRA_INDEX_URL"
                        | "UV_FIND_LINKS"
                        | "UV_NO_INDEX"
                        | "UV_INSECURE_HOST"
                        | "UV_NATIVE_TLS"
                        | "UV_NO_VERIFY_HASHES"
                )
            {
                command.env_remove(name);
            }
        }
    }
    if profile.mode == ProxyMode::Inherit {
        return Ok(());
    }
    for (name, _) in std::env::vars_os() {
        if matches!(
            name.to_string_lossy().to_ascii_uppercase().as_str(),
            "HTTP_PROXY" | "HTTPS_PROXY" | "ALL_PROXY" | "NO_PROXY" | "PROXY" | "CARGO_HTTP_PROXY"
        ) {
            command.env_remove(name);
        }
    }
    command.env_remove("PROXY").env_remove("proxy");
    let proxy = if profile.mode == ProxyMode::Manual {
        profile.proxy_url.as_str()
    } else {
        ""
    };
    let bypass = if profile.mode == ProxyMode::Direct {
        "*"
    } else {
        ""
    };
    for name in ["http_proxy", "HTTP_PROXY", "https_proxy", "HTTPS_PROXY"] {
        command.env(name, proxy);
    }
    for name in ["all_proxy", "ALL_PROXY"] {
        command.env(name, "");
    }
    for name in ["no_proxy", "NO_PROXY"] {
        command.env(name, bypass);
    }
    command.env("CARGO_HTTP_PROXY", proxy);
    Ok(())
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
#[cfg(any(windows, target_os = "macos", test))]
pub(crate) fn run_python_tools(
    invocation: &Invocation,
    cancel: &Arc<AtomicBool>,
    timeout: Duration,
) -> Result<Output> {
    run_configured(invocation, cancel, timeout, configure_python_tools)
}
#[cfg(any(windows, target_os = "macos", test))]
fn configure_python_tools(command: &mut Command) {
    for (name, _) in std::env::vars_os() {
        let upper = name.to_string_lossy().to_ascii_uppercase();
        if upper.starts_with("UV_")
            || upper.starts_with("PYTHON")
            || matches!(
                upper.as_str(),
                "CONDA_PREFIX" | "VIRTUAL_ENV" | "XDG_DATA_HOME" | "XDG_BIN_HOME"
            )
        {
            command.env_remove(name);
        }
    }
    command
        .env_remove("UV_OFFLINE")
        .env("UV_NO_CONFIG", "1")
        .env("UV_PYTHON_DOWNLOADS", "never");
}
#[cfg(any(windows, target_os = "macos", test))]
fn configure_python_inventory(command: &mut Command) {
    configure_python_tools(command);
    command.env("UV_OFFLINE", "1");
}
#[cfg(any(windows, target_os = "macos", test))]
pub(crate) fn run_python_inventory(
    invocation: &Invocation,
    cancel: &Arc<AtomicBool>,
    timeout: Duration,
) -> Result<Output> {
    run_configured(invocation, cancel, timeout, configure_python_inventory)
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
    let network = crate::network::active_profile();
    let args = if let Some(profile) = &network {
        network_arguments(invocation, profile)?
    } else {
        invocation.args.clone()
    };
    let mut command = Command::new(&invocation.program);
    command
        .args(&args)
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
    if let Some(profile) = &network {
        configure_network(&mut command, invocation, profile)?;
    }
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
    fn reviewed_network_rewrites_sources_and_proxy_before_package_delimiter() {
        use crate::network::{NetworkProfile, ProxyMode};
        let profile = NetworkProfile {
            mode: ProxyMode::Manual,
            proxy_url: "http://127.0.0.1:7890".into(),
            npm_registry: "https://packages.example/npm/".into(),
            python_index: "https://packages.example/simple/".into(),
            cargo_index: "sparse+https://packages.example/crates/".into(),
            ..NetworkProfile::default()
        };
        let make = |name: &str, args: &[&str]| Invocation {
            program: std::env::temp_dir().join(name),
            args: args.iter().map(|s| s.to_string()).collect(),
            npm_node: name == "node",
            vscode_node: false,
        };
        let npm = make(
            "node",
            &[
                "npm-cli.js",
                "install",
                "--registry=https://registry.npmjs.org/",
                "--ignore-scripts",
                "--",
                "typescript",
            ],
        );
        let args = network_arguments(&npm, &profile).unwrap();
        let delimiter = args.iter().position(|a| a == "--").unwrap();
        assert_eq!(&args[delimiter + 1..], ["typescript"]);
        assert!(args[..delimiter].contains(&"--registry=https://packages.example/npm/".into()));
        assert!(args[..delimiter].contains(&"--proxy=http://127.0.0.1:7890".into()));
        assert!(
            args.contains(&"--strict-ssl=true".into()) && args.contains(&"--ignore-scripts".into())
        );
        let uv = make(
            "uv",
            &[
                "tool",
                "install",
                "--no-config",
                "--no-python-downloads",
                "ruff",
            ],
        );
        assert!(
            network_arguments(&uv, &profile)
                .unwrap()
                .contains(&"--default-index=https://packages.example/simple/".into())
        );
        let uv_upgrade = make("uv", &["tool", "upgrade", "--no-config", "ruff"]);
        assert!(
            network_arguments(&uv_upgrade, &profile)
                .unwrap()
                .contains(&"--default-index=https://packages.example/simple/".into())
        );
        let cargo = make(
            "cargo",
            &["install", "--locked", "--registry", "crates-io", "ripgrep"],
        );
        let args = network_arguments(&cargo, &profile).unwrap();
        assert_eq!(args[0], "--config");
        assert!(args.contains(&"source.crates-io.replace-with=\"ltsc-studio\"".into()));
        assert!(args.contains(&"--locked".into()));
        let pip = make(
            "python.exe",
            &[
                "-E",
                "-P",
                "-m",
                "pip",
                "--isolated",
                "install",
                "--index-url=https://pypi.org/simple",
                "--only-binary=:all:",
                "--",
                "ruff",
            ],
        );
        let args = network_arguments(&pip, &profile).unwrap();
        assert!(args.contains(&"--index-url=https://packages.example/simple/".into()));
        assert!(
            args.contains(&"--only-binary=:all:".into()) && args.contains(&"--isolated".into())
        );
        let unsupported = make("winget.exe", &["install", "--id", "Git.Git"]);
        assert_eq!(
            network_arguments(&unsupported, &profile).unwrap(),
            unsupported.args
        );
    }

    #[test]
    fn direct_and_manual_network_environment_override_conflicting_routes_without_mutating_parent() {
        use crate::network::{NetworkProfile, ProxyMode};
        let invocation = Invocation {
            program: std::env::temp_dir().join("cargo"),
            args: vec![],
            vscode_node: false,
            npm_node: false,
        };
        let parent = std::env::var_os("HTTPS_PROXY");
        for mode in [ProxyMode::Direct, ProxyMode::Manual] {
            let profile = NetworkProfile {
                mode,
                proxy_url: if mode == ProxyMode::Manual {
                    "http://[::1]:7890".into()
                } else {
                    String::new()
                },
                ..NetworkProfile::default()
            };
            let mut command = Command::new("unused");
            command.env("PROXY", "http://old.invalid:80");
            configure_network(&mut command, &invocation, &profile).unwrap();
            let values: std::collections::BTreeMap<_, _> = command
                .get_envs()
                .map(|(k, v)| {
                    (
                        k.to_string_lossy().into_owned(),
                        v.map(|v| v.to_string_lossy().into_owned()),
                    )
                })
                .collect();
            assert_eq!(values["PROXY"], None);
            assert_eq!(
                values["CARGO_HTTP_PROXY"].as_deref(),
                Some(profile.proxy_url.as_str())
            );
            assert_eq!(values["http_proxy"], values["HTTPS_PROXY"]);
            assert_eq!(
                values["NO_PROXY"].as_deref(),
                Some(if mode == ProxyMode::Direct { "*" } else { "" })
            );
        }
        assert_eq!(std::env::var_os("HTTPS_PROXY"), parent);
        let mut unsupported = Command::new("unused");
        let command = Invocation {
            program: std::env::temp_dir().join("winget.exe"),
            ..invocation
        };
        configure_network(&mut unsupported, &command, &NetworkProfile::default()).unwrap();
        assert_eq!(unsupported.get_envs().count(), 0);
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
    fn uv_deployment_clears_offline_and_inherited_package_configuration() {
        let inherited_uv: Vec<_> = std::env::vars_os()
            .filter(|(name, _)| {
                name.to_string_lossy()
                    .to_ascii_uppercase()
                    .starts_with("UV_")
            })
            .map(|(name, _)| name)
            .collect();
        let mut command = Command::new("unused");
        configure_python_tools(&mut command);
        let env: std::collections::BTreeMap<_, _> = command.get_envs().collect();
        for name in inherited_uv {
            if !["UV_NO_CONFIG", "UV_PYTHON_DOWNLOADS"].contains(&name.to_string_lossy().as_ref()) {
                assert_eq!(env.get(name.as_os_str()), Some(&None), "{name:?}");
            }
        }
        assert_eq!(env.get(std::ffi::OsStr::new("UV_OFFLINE")), Some(&None));
        assert!(
            env.get(std::ffi::OsStr::new("UV_DEFAULT_INDEX"))
                .is_none_or(Option::is_none)
        );
        assert_eq!(
            env.get(std::ffi::OsStr::new("UV_NO_CONFIG")),
            Some(&Some(std::ffi::OsStr::new("1")))
        );
        assert_eq!(
            env.get(std::ffi::OsStr::new("UV_PYTHON_DOWNLOADS")),
            Some(&Some(std::ffi::OsStr::new("never")))
        );
    }

    #[test]
    fn uv_inventory_is_offline_and_uses_the_same_sanitized_store_paths_as_deployment() {
        let mut command = Command::new("unused");
        configure_python_inventory(&mut command);
        let env: std::collections::BTreeMap<_, _> = command.get_envs().collect();
        assert_eq!(
            env.get(std::ffi::OsStr::new("UV_OFFLINE")),
            Some(&Some(std::ffi::OsStr::new("1")))
        );
        assert_eq!(
            env.get(std::ffi::OsStr::new("UV_NO_CONFIG")),
            Some(&Some(std::ffi::OsStr::new("1")))
        );
        assert_eq!(
            env.get(std::ffi::OsStr::new("UV_PYTHON_DOWNLOADS")),
            Some(&Some(std::ffi::OsStr::new("never")))
        );
        for name in [
            "UV_TOOL_DIR",
            "UV_TOOL_BIN_DIR",
            "XDG_DATA_HOME",
            "XDG_BIN_HOME",
        ] {
            assert!(
                env.get(std::ffi::OsStr::new(name))
                    .is_none_or(Option::is_none)
            );
        }
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
