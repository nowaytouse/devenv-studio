use std::{
    env,
    io::{self, Read},
    net::{SocketAddr, TcpStream},
    os::unix::{fs::MetadataExt, process::CommandExt as _},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{OnceLock, mpsc},
    thread,
    time::{Duration, Instant},
};

use color_eyre::eyre::{Context, ContextCompat, Result, ensure};
use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};

use crate::command::CommandExt as _;

pub(super) const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
pub(super) const UPDATE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const MAX_OUTPUT: u64 = 8 * 1024 * 1024;
const PROXY_KEYS: &[&str] = &[
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
];

#[expect(
    clippy::disallowed_methods,
    reason = "dependency discovery must never start tool shims"
)]
pub(super) fn find_tool(name: &str) -> Result<Option<PathBuf>> {
    let executable = match which_crate::which(name) {
        Ok(path) => path,
        Err(which_crate::Error::CannotFindBinaryPath) => return Ok(None),
        Err(error) => return Err(error).wrap_err_with(|| format!("Cannot resolve dependency tool {name}")),
    };
    let target = executable.canonicalize()?;
    let metadata = target.metadata()?;
    ensure!(
        executable.is_absolute() && metadata.is_file() && metadata.mode() & 0o111 != 0 && metadata.mode() & 0o022 == 0,
        "Unsafe dependency tool: {name}"
    );
    // Use the invocation path so tools such as rustup keep their shim names.
    Ok(Some(executable))
}

fn local_proxy(value: &str) -> Option<Vec<SocketAddr>> {
    let authority = value.split_once("://").map_or(value, |(_, rest)| rest);
    let authority = authority.split('/').next()?.rsplit('@').next()?;
    let (host, port) = authority.rsplit_once(':')?;
    let port = port.parse().ok()?;
    match host.to_ascii_lowercase().as_str() {
        "127.0.0.1" => Some(vec![SocketAddr::from(([127, 0, 0, 1], port))]),
        "[::1]" => Some(vec![SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], port))]),
        "localhost" => Some(vec![
            SocketAddr::from(([127, 0, 0, 1], port)),
            SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], port)),
        ]),
        _ => None,
    }
}

fn has_dead_loopback_proxy() -> bool {
    PROXY_KEYS.iter().any(|key| {
        env::var(key)
            .ok()
            .as_deref()
            .and_then(local_proxy)
            .is_some_and(|addresses| {
                addresses
                    .iter()
                    .all(|address| TcpStream::connect_timeout(address, Duration::from_millis(200)).is_err())
            })
    })
}

#[expect(
    clippy::disallowed_methods,
    reason = "trusted terminal package managers need the user's PATH and language toolchain settings"
)]
pub(super) fn command(executable: &Path, root: &Path) -> Command {
    let mut command = Command::new(executable);
    command
        .current_dir(root)
        .process_group(0)
        .stdin(Stdio::null())
        .env("HOMEBREW_NO_AUTO_UPDATE", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0");
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_PREFIX",
    ] {
        command.env_remove(key);
    }
    static DIRECT: OnceLock<bool> = OnceLock::new();
    if *DIRECT.get_or_init(has_dead_loopback_proxy) {
        for key in PROXY_KEYS {
            command.env_remove(key);
        }
        command.env("PROXY_AUTO_FALLBACK", "direct");
    }
    command
}

pub(super) fn pinact_auth(command: &mut Command, repo: &Path) -> Result<()> {
    if env::var_os("PINACT_GITHUB_TOKEN").is_none()
        && env::var_os("GITHUB_TOKEN").is_none()
        && let Some(gh) = find_tool("gh")?
    {
        let mut auth = self::command(&gh, repo);
        auth.args(["auth", "token"]).env("GH_PROMPT_DISABLED", "1");
        let auth = capture(auth, QUERY_TIMEOUT)?;
        let token = auth.stdout.trim();
        if auth.status.success() && !token.is_empty() && !token.contains(char::is_whitespace) {
            command.env("PINACT_GITHUB_TOKEN", token);
        }
    }
    Ok(())
}

pub(super) struct Output {
    pub(super) status: ExitStatus,
    pub(super) stdout: String,
    stderr: String,
}

impl Output {
    pub(super) fn contains_any(&self, tokens: &[&str]) -> bool {
        [&self.stdout, &self.stderr]
            .into_iter()
            .map(|stream| stream.to_ascii_lowercase())
            .any(|stream| tokens.iter().any(|token| stream.contains(token)))
    }
}

fn read_pipe(pipe: impl Read + Send + 'static) -> mpsc::Receiver<Result<String>> {
    let (send, receive) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let read = || -> Result<String> {
            let mut bytes = Vec::new();
            pipe.take(MAX_OUTPUT + 1).read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() as u64 <= MAX_OUTPUT,
                "Dependency command exceeded its output limit"
            );
            Ok(String::from_utf8_lossy(&bytes).into_owned())
        };
        let _ = send.send(read());
    });
    receive
}

pub(super) fn capture(mut command: Command, timeout: Duration) -> Result<Output> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn_checked()?;
    let pgid = Pid::from_raw(i32::try_from(child.id())?);
    let stop_group = || {
        if let Err(error) = killpg(pgid, Signal::SIGKILL)
            && error != nix::errno::Errno::ESRCH
        {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
        Ok(())
    };
    let result = (|| {
        let stdout = read_pipe(child.stdout.take().context("Missing dependency command stdout")?);
        let stderr = read_pipe(child.stderr.take().context("Missing dependency command stderr")?);
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            ensure!(!crate::ctrlc::interrupted(), "Dependency command interrupted");
            ensure!(
                started.elapsed() < timeout,
                "Dependency command exceeded its time limit"
            );
            thread::sleep(Duration::from_millis(100));
        };
        Ok(Output {
            status,
            stdout: stdout
                .recv_timeout(Duration::from_secs(2))
                .wrap_err("Dependency command stdout did not close")??,
            stderr: stderr
                .recv_timeout(Duration::from_secs(2))
                .wrap_err("Dependency command stderr did not close")??,
        })
    })();
    if result.is_err() {
        let termination = stop_group();
        let wait = child.wait();
        termination?;
        wait?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_literal_loopback_proxy_addresses_are_probed() {
        assert!(local_proxy("http://localhost:7890").is_some());
        assert!(local_proxy("http://user:pass@[::1]:7890").is_some());
        assert!(local_proxy("http://127.0.0.1.evil.example:7890").is_none());
        assert!(local_proxy("https://example.com:7890").is_none());
    }
}
