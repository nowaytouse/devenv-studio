//! Terminal-only updates for reviewed macOS preview channels.

mod betterdisplay;
mod krita;
mod tourbox;
mod xcode_beta;

pub(super) use betterdisplay::betterdisplay;
pub(super) use krita::krita_next;
pub(super) use tourbox::tourbox_beta;
pub(super) use xcode_beta::xcode_beta;

use color_eyre::eyre::{Context, Result, bail, ensure};
use nix::libc::{O_NOFOLLOW, O_NONBLOCK};
use serde_json::Value;
use std::{
    ffi::CString,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{self, Read, Seek},
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

use super::{check_dir, check_file, tool_command, wait_until};
use crate::command::CommandExt as _;

const APPLICATIONS: &str = "/Applications";
const PLIST_LIMIT: u64 = 4 * 1024 * 1024;

fn trusted_https(url: &str, hosts: &[&str]) -> bool {
    let Some(authority) = url
        .strip_prefix("https://")
        .and_then(|rest| rest.split_once('/').map(|(host, _)| host))
    else {
        return false;
    };
    hosts.contains(&authority)
        && url.len() <= 2048
        && url
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'\\' | b'#'))
}

fn download_to(
    source: &str,
    hosts: &[&str],
    path: &Path,
    limit: u64,
    max_time: u64,
    head: bool,
    compressed: bool,
) -> Result<String> {
    let mut url = source.to_owned();
    for _ in 0..4 {
        ensure!(
            trusted_https(&url, hosts),
            "Preview download left its official HTTPS origin"
        );
        let mut curl = tool_command(Path::new("/usr/bin/curl"))?;
        curl.args([
            "--disable",
            "--silent",
            "--show-error",
            "--proto",
            "=https",
            "--connect-timeout",
            "20",
            "--retry",
            "2",
            "--max-time",
        ])
        .arg(max_time.to_string())
        .arg("--max-filesize")
        .arg(limit.to_string())
        .args(["--user-agent", "PreviewAppsWorkflow", "--output"])
        .arg(path)
        .args(["--write-out", "%{http_code}\n%{redirect_url}\n%header{last-modified}\n"]);
        if head {
            curl.arg("--head");
        } else if compressed {
            curl.arg("--compressed");
        }
        curl.args(["--", &url]);
        let (status, report, _) = bounded_output(&mut curl, Duration::from_secs(max_time * 3 + 20), 64 * 1024)?;
        ensure!(status.success(), "Preview download failed ({status})");
        let mut lines = report.split('\n');
        let status = lines.next().unwrap_or_default();
        let redirect = lines.next().unwrap_or_default();
        let modified = lines.next().unwrap_or_default();
        if matches!(status, "301" | "302" | "303" | "307" | "308") {
            url = redirect.to_owned();
            continue;
        }
        ensure!(
            status == "200" && redirect.is_empty(),
            "Unexpected preview download HTTP status {status}"
        );
        let downloaded = fs::symlink_metadata(path)?;
        ensure!(
            downloaded.is_file() && downloaded.len() > 0 && downloaded.len() <= limit,
            "Preview download has an invalid length or type"
        );
        return Ok(modified.to_owned());
    }
    bail!("Too many preview download redirects")
}

fn running(app: &Path, executable_suffix: &str) -> Result<bool> {
    let prefix = format!("{}/", app.to_string_lossy().to_ascii_lowercase());
    let (status, stdout, _) = bounded_output(
        tool_command(Path::new("/bin/ps"))?.args(["-awwxo", "comm="]),
        Duration::from_secs(15),
        4 * 1024 * 1024,
    )?;
    ensure!(status.success(), "Could not check for a running preview application");
    Ok(stdout.lines().any(|line| {
        let command = line.trim().to_ascii_lowercase();
        command.starts_with(&prefix) || command.ends_with(executable_suffix)
    }))
}

fn private_path(home: &Path, relative: &str) -> Result<PathBuf> {
    let mut path = home.to_owned();
    for component in Path::new(relative).components() {
        let std::path::Component::Normal(name) = component else {
            bail!("Invalid preview updater directory");
        };
        path.push(name);
        if let Err(error) = DirBuilder::new().mode(0o700).create(&path)
            && error.kind() != io::ErrorKind::AlreadyExists
        {
            return Err(error.into());
        }
        check_dir(&path, false)?;
    }
    ensure!(
        path.canonicalize()? == path,
        "Preview updater directory is not canonical"
    );
    Ok(path)
}

fn lock(home: &Path, name: &str) -> Result<File> {
    let path = private_path(home, ".cache")?.join(name);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(&path)?;
    // Earlier updaters created these empty lock files with the account's umask.
    check_file(&file, &path, false, 0)?;
    file.try_lock().wrap_err("Another preview update is running")?;
    Ok(file)
}

fn bounded_output(command: &mut Command, timeout: Duration, limit: u64) -> Result<(ExitStatus, String, String)> {
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let mut child = command
        .stdout(Stdio::from(stdout.try_clone()?))
        .stderr(Stdio::from(stderr.try_clone()?))
        .spawn_checked()?;
    let Some(status) = wait_until(&mut child, Instant::now() + timeout)? else {
        let _ = child.kill();
        child.wait().wrap_err("Reap timed-out preview updater probe")?;
        bail!("Preview updater probe timed out");
    };
    let read = |file: &mut File| -> Result<String> {
        ensure!(
            file.metadata()?.len() <= limit,
            "Preview updater output exceeds its limit"
        );
        file.rewind()?;
        let mut bytes = Vec::new();
        file.take(limit + 1).read_to_end(&mut bytes)?;
        ensure!(bytes.len() as u64 <= limit, "Preview updater output exceeds its limit");
        Ok(String::from_utf8(bytes)?)
    };
    Ok((status, read(&mut stdout)?, read(&mut stderr)?))
}

fn plist(path: &Path) -> Result<Value> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(path)?;
    ensure!(
        file.metadata()?.is_file() && file.metadata()?.len() <= PLIST_LIMIT && path.canonicalize()? == path,
        "Unsafe application property list: {}",
        path.display()
    );
    let (status, stdout, _) = bounded_output(
        tool_command(Path::new("/usr/bin/plutil"))?
            .args(["-convert", "json", "-o", "-", "--", "-"])
            .stdin(Stdio::from(file)),
        Duration::from_secs(15),
        PLIST_LIMIT,
    )?;
    ensure!(
        status.success(),
        "Application property list is invalid: {}",
        path.display()
    );
    Ok(serde_json::from_str(&stdout)?)
}

fn field(info: &Value, key: &str) -> Result<String> {
    let value = match &info[key] {
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        _ => bail!("Missing or malformed application field {key}"),
    };
    ensure!(
        !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control),
        "Invalid application field {key}"
    );
    Ok(value)
}

fn bundle(app: &Path, expected: &str) -> Result<Value> {
    ensure!(
        fs::symlink_metadata(app)?.is_dir() && app.canonicalize()? == app,
        "Unsafe application path: {}",
        app.display()
    );
    let info = plist(&app.join("Contents/Info.plist"))?;
    ensure!(
        info["CFBundleIdentifier"].as_str() == Some(expected),
        "Unexpected bundle identifier"
    );
    Ok(info)
}

fn signature(app: &Path, requirement: Option<&str>) -> Result<()> {
    let mut command = tool_command(Path::new("/usr/bin/codesign"))?;
    command.args(["--verify", "--deep", "--strict"]);
    if let Some(requirement) = requirement {
        command.args(["-R", requirement]);
    }
    command.arg(app).status_checked()
}

fn team(app: &Path) -> Result<String> {
    let (status, stdout, stderr) = bounded_output(
        tool_command(Path::new("/usr/bin/codesign"))?
            .args(["-dv", "--verbose=4"])
            .arg(app),
        Duration::from_secs(30),
        64 * 1024,
    )?;
    ensure!(status.success(), "Could not read application signing identity");
    let teams: Vec<_> = stdout
        .lines()
        .chain(stderr.lines())
        .filter_map(|line| line.strip_prefix("TeamIdentifier="))
        .collect();
    ensure!(
        matches!(teams.as_slice(), [team] if team.len() == 10
            && team.bytes().all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())),
        "Application has no unambiguous Developer ID team"
    );
    Ok(teams[0].into())
}

fn move_exclusive(source: &Path, target: &Path) -> Result<()> {
    let source = CString::new(source.as_os_str().as_bytes())?;
    let target = CString::new(target.as_os_str().as_bytes())?;
    // Both paths are on the same volume, and Darwin commits the no-replace rename atomically.
    let result = unsafe { nix::libc::renamex_np(source.as_ptr(), target.as_ptr(), nix::libc::RENAME_EXCL) };
    ensure!(
        result == 0,
        "Could not publish preview app: {}",
        io::Error::last_os_error()
    );
    Ok(())
}
