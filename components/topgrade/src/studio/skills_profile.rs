//! Managed global skill maintenance and coverage.
mod cursor;
mod design;
mod npx;

use color_eyre::eyre::{Context, ContextCompat, Result, ensure};
use serde::Deserialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, DirBuilder, File},
    io::{self, Read, Write},
    os::unix::{fs::DirBuilderExt, fs::PermissionsExt, process::CommandExt as _},
    path::{Component, Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

use super::{
    account_home, check_dir, exists, open_file, proxy::Route, resolve_tool, terminate_group, tool_command, user_only,
};
use crate::command::CommandExt as _;

const LOCK_PATH: &str = ".agents/.skill-lock.json";
const DATA_LIMIT: u64 = 16 * 1024 * 1024;

pub(super) fn update() -> Result<()> {
    user_only()?;
    let home = account_home()?;
    npx::run(&home)?;
    design::sync(&home)
}

pub(super) fn coverage() -> Result<()> {
    user_only()?;
    let home = account_home()?;
    cursor::run(&home)?;
    design::verify(&home)
}

fn directory(home: &Path, relative: &Path, create: bool) -> Result<PathBuf> {
    let mut path = home.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(color_eyre::eyre::eyre!("Unsafe skill directory path"));
        };
        path.push(component);
        if create
            && let Err(error) = DirBuilder::new().mode(0o700).create(&path)
            && error.kind() != io::ErrorKind::AlreadyExists
        {
            return Err(error.into());
        }
        check_dir(&path, false)?;
    }
    Ok(path)
}

fn bounded(reader: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "Skill maintenance input exceeded its size limit"
    );
    Ok(bytes)
}

fn bytes(path: &Path, limit: u64) -> Result<Vec<u8>> {
    bounded(open_file(path, false, limit)?, limit)
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

fn atomic_write(path: &Path, payload: &[u8], mode: u32) -> Result<()> {
    let parent = path.parent().context("Missing skill file parent")?;
    check_dir(parent, false)?;
    ensure!(parent.canonicalize()? == parent, "Non-canonical skill file parent");
    if exists(path)? {
        open_file(path, false, DATA_LIMIT)?;
    }
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(payload)?;
    file.as_file().set_permissions(fs::Permissions::from_mode(mode))?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    sync_directory(parent)
}

use super::strict_json::parse as json;

struct Captured {
    status: ExitStatus,
    out: Vec<u8>,
    err: Vec<u8>,
}

fn capture(command: &mut Command, timeout: Duration, limit: u64) -> Result<Captured> {
    capture_with(command, timeout, limit, |reader, limit, _| bounded(reader, limit))
}

fn capture_with(
    command: &mut Command,
    timeout: Duration,
    limit: u64,
    read: impl Fn(&mut dyn Read, u64, bool) -> Result<Vec<u8>> + Send + Sync + 'static,
) -> Result<Captured> {
    fn pipe(
        mut reader: impl Read + Send + 'static,
        read: impl FnOnce(&mut dyn Read) -> Result<Vec<u8>> + Send + 'static,
    ) -> mpsc::Receiver<Result<Vec<u8>>> {
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(read(&mut reader));
        });
        receiver
    }

    let mut child = command
        .process_group(0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn_checked()?;
    let pgid = i32::try_from(child.id())?;
    let result = (|| {
        let read = Arc::new(read);
        let stdout = Arc::clone(&read);
        let out = pipe(
            child.stdout.take().context("Missing skill command stdout")?,
            move |reader| stdout(reader, limit, false),
        );
        let err = pipe(
            child.stderr.take().context("Missing skill command stderr")?,
            move |reader| read(reader, DATA_LIMIT, true),
        );
        let deadline = Instant::now() + timeout;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            ensure!(
                !crate::ctrlc::interrupted() && Instant::now() < deadline,
                "Skill maintenance command was interrupted or timed out"
            );
            thread::sleep(Duration::from_millis(50));
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        Ok(Captured {
            status,
            out: out.recv_timeout(deadline.saturating_duration_since(Instant::now()))??,
            err: err.recv_timeout(deadline.saturating_duration_since(Instant::now()))??,
        })
    })();
    if result.is_err() {
        terminate_group(&mut child, pgid)?;
    }
    result
}

fn command(home: &Path, executable: &str, route: Option<&Route>) -> Result<Command> {
    let mut command = tool_command(&resolve_tool(executable)?)?;
    command.env("HOME", home).current_dir(home);
    if executable == "gh" {
        for key in ["GH_TOKEN", "GITHUB_TOKEN"] {
            if let Ok(value) = std::env::var(key)
                && !value.is_empty()
            {
                ensure!(
                    !value.chars().any(char::is_whitespace),
                    "Malformed GitHub credential environment"
                );
                command.env(key, value);
            }
        }
    }
    if let Some(route) = route {
        route.apply(&mut command);
    }
    Ok(command)
}

fn npm_config(command: &mut Command, work: &Path) -> Result<()> {
    for (name, file) in [
        ("npm_config_userconfig", "user.npmrc"),
        ("npm_config_globalconfig", "global.npmrc"),
    ] {
        let path = work.join(file);
        File::create_new(&path)?;
        command.env(name, path);
    }
    command.args([
        "--ignore-scripts",
        "--strict-ssl=true",
        "--registry=https://registry.npmjs.org",
    ]);
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Row {
    skill_name: String,
    #[serde(rename = "sourceURL")]
    source_url: Option<String>,
    path: PathBuf,
    pinned: bool,
}

fn rows(home: &Path, route: Option<&Route>) -> Result<Vec<Row>> {
    let out = capture(
        command(home, "gh", route)?.args(["skill", "list", "--json", "skillName,sourceURL,pinned,path"]),
        Duration::from_secs(90),
        DATA_LIMIT,
    )?;
    ensure!(
        out.status.success(),
        "GitHub CLI skill discovery failed ({})",
        out.status
    );
    let rows: Vec<Row> = serde_json::from_value(json(&out.out)?)?;
    ensure!(
        !rows.is_empty() && rows.len() <= 4096,
        "Unbounded or empty GitHub CLI skill inventory"
    );
    Ok(rows)
}

fn lock_entries(lock: &Value) -> Result<&Map<String, Value>> {
    ensure!(
        lock.get("version").and_then(Value::as_u64) == Some(3),
        "Unsupported shared skill lock version"
    );
    lock.get("skills")
        .and_then(Value::as_object)
        .context("Shared skill lock has no skills object")
}

fn lock_path(home: &Path) -> Result<PathBuf> {
    ensure!(
        std::env::var_os("XDG_STATE_HOME").is_none_or(|value| value.is_empty()),
        "Non-default XDG_STATE_HOME changes the skill lock owner"
    );
    Ok(home.join(LOCK_PATH))
}

#[cfg(test)]
#[test]
fn rejects_nested_duplicate_skill_lock_keys() {
    assert!(json(br#"{"skills":{"design":{"source":"first","source":"second"}}}"#).is_err());
}
