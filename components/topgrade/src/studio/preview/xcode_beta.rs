use color_eyre::eyre::{Context, ContextCompat, Result, ensure};
use regex::Regex;
use std::{
    borrow::Cow,
    fs::{self, File},
    io::{self, BufRead, IsTerminal, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::LazyLock,
    time::Duration,
};

use super::{APPLICATIONS, bounded_output, bundle, move_exclusive, plist, private_path, signature, tool_command};
use crate::{command::CommandExt as _, error::PartialStep};

const ID: &str = "com.apple.dt.Xcode";
const REQUIREMENT: &str = "=anchor apple and identifier \"com.apple.dt.Xcode\"";

#[derive(Clone, Debug, Eq, PartialEq)]
struct Release {
    version: String,
    parts: [u32; 3],
    beta: u32,
    build: String,
}

impl Release {
    fn name(&self) -> String {
        if self.beta == 1 {
            format!("{} Beta", self.version)
        } else {
            format!("{} Beta {}", self.version, self.beta)
        }
    }

    fn destination(&self, applications: &Path) -> PathBuf {
        applications.join(format!("Xcode-{}-Beta{}-{}.app", self.version, self.beta, self.build))
    }
}

fn parse_catalog(text: &str) -> Result<Release> {
    static PATTERN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^\s*([0-9]{1,4}(?:\.[0-9]{1,4}){1,2})\s+Beta(?:\s+([0-9]{1,4}))?\s+\(([A-Za-z0-9]{2,32})\)\s+\[Apple Silicon\]")
            .expect("valid Xcode catalog pattern")
    });
    let mut candidates = Vec::new();
    for parts in text.lines().filter_map(|line| PATTERN.captures(line)) {
        let version = parts[1].to_owned();
        let mut numbers = [0; 3];
        for (slot, part) in numbers.iter_mut().zip(version.split('.')) {
            *slot = part.parse()?;
        }
        let beta = parts.get(2).map_or(Ok(1), |part| part.as_str().parse())?;
        ensure!(beta > 0, "Invalid Xcode beta number");
        candidates.push(Release {
            version,
            parts: numbers,
            beta,
            build: parts[3].to_owned(),
        });
    }
    candidates.sort_by_key(|release| (release.parts, release.beta));
    let newest = candidates
        .pop()
        .context("No Apple Silicon Xcode Beta in the xcodes catalog")?;
    ensure!(
        !candidates
            .iter()
            .any(|other| (other.parts, other.beta) == (newest.parts, newest.beta) && other.build != newest.build),
        "Conflicting builds for the newest Xcode Beta"
    );
    Ok(newest)
}

fn verify(app: &Path, build: &str) -> Result<()> {
    bundle(app, ID)?;
    signature(app, Some(REQUIREMENT))?;
    let developer = app.join("Contents/Developer");
    ensure!(
        developer.is_dir() && developer.canonicalize()? == developer,
        "Unsafe Xcode developer directory"
    );
    let (status, stdout, _) = bounded_output(
        tool_command(Path::new("/usr/bin/xcodebuild"))?
            .env("DEVELOPER_DIR", &developer)
            .arg("-version"),
        Duration::from_secs(60),
        64 * 1024,
    )?;
    ensure!(
        status.success()
            && stdout
                .lines()
                .filter_map(|line| line.strip_prefix("Build version "))
                .eq([build]),
        "Xcode did not report the exact expected build {build}"
    );
    Ok(())
}

fn installed(applications: &Path, build: &str) -> Result<Option<PathBuf>> {
    let mut paths = fs::read_dir(applications)?.collect::<io::Result<Vec<_>>>()?;
    paths.sort_by_key(fs::DirEntry::file_name);
    for entry in paths {
        let name = entry.file_name();
        if !name
            .to_str()
            .is_some_and(|name| name.starts_with("Xcode") && name.ends_with(".app"))
            || !entry.file_type()?.is_dir()
        {
            continue;
        }
        let app = entry.path();
        let Ok(info) = bundle(&app, ID) else { continue };
        let version_plist = app.join("Contents/version.plist");
        let found = if super::super::exists(&version_plist)? {
            plist(&version_plist)?["ProductBuildVersion"]
                .as_str()
                .map(str::to_owned)
        } else {
            info["DTXcodeBuild"].as_str().map(str::to_owned)
        };
        if found.as_deref().is_some_and(|known| known != build) {
            continue;
        }
        if found.as_deref() == Some(build) {
            verify(&app, build)?;
            return Ok(Some(app));
        }
        // Old bundle layouts may omit the property-list build. Inspect a signed
        // candidate's tool before deciding whether the target is already installed.
        if verify(&app, build).is_ok() {
            return Ok(Some(app));
        }
    }
    Ok(None)
}

fn logged(command: &mut Command, log: &mut File) -> Result<()> {
    let (pipe, writer) = io::pipe()?;
    let mut child = command
        .stdout(Stdio::from(writer.try_clone()?))
        .stderr(Stdio::from(writer))
        .spawn_checked()?;
    // Command keeps its own write ends; close them before reading to EOF.
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let result = (|| -> Result<()> {
        let mut reader = io::BufReader::new(pipe);
        let mut output = io::stdout().lock();
        let mut line = Vec::new();
        loop {
            line.clear();
            if reader.read_until(b'\n', &mut line)? == 0 {
                break;
            }
            let text = String::from_utf8_lossy(&line);
            let text = without_urls(&text);
            log.write_all(text.as_bytes())?;
            output.write_all(text.as_bytes())?;
            output.flush()?;
        }
        log.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    result?;
    ensure!(status.success(), "Xcode command failed ({status}); see the private log");
    Ok(())
}

fn download(command: &mut Command, tty: &File) -> Result<()> {
    // Give Xcodes the real terminal for prompts without newlines and keep authentication out of logs.
    command
        .stdin(Stdio::from(tty.try_clone()?))
        .stdout(Stdio::from(tty.try_clone()?))
        .stderr(Stdio::from(tty.try_clone()?))
        .status_checked()
}

fn without_urls(text: &str) -> Cow<'_, str> {
    static URL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?i)\bhttps?://[^\s"'<>\\\x1b]+"#).expect("valid Xcode log URL pattern"));
    URL.replace_all(text, "[URL redacted]")
}

fn unique_entry(directory: &Path, extension: &str, want_directory: bool) -> Result<PathBuf> {
    let mut found = None;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.path().extension().is_some_and(|value| value == extension) {
            continue;
        }
        let kind = entry.file_type()?;
        ensure!(
            found.is_none() && if want_directory { kind.is_dir() } else { kind.is_file() },
            "Expected exactly one safe Xcode .{extension} entry"
        );
        found = Some(entry.path());
    }
    found.wrap_err_with(|| format!("No Xcode .{extension} found"))
}

fn install(executable: &Path, applications: &Path, home: &Path, target: &Release, log: &mut File) -> Result<()> {
    ensure!(
        io::stdin().is_terminal(),
        "Open Terminal to complete Apple authentication"
    );
    let tty = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .wrap_err("Open the controlling terminal for Apple authentication")?;
    let destination = target.destination(applications);
    ensure!(!super::super::exists(&destination)?, "Xcode destination already exists");
    let staging = tempfile::Builder::new()
        .prefix(".xcode-beta-")
        .tempdir_in(applications)
        .wrap_err("Creating a private /Applications staging directory requires write access")?;
    let downloads = home.join("Downloads");
    super::super::check_dir(&downloads, false)?;
    ensure!(
        downloads.canonicalize()? == downloads,
        "Unsafe Xcode Downloads directory"
    );
    let workspace = tempfile::Builder::new().prefix(".xcode-beta-").tempdir_in(downloads)?;
    let archives = workspace.path().join("archives");
    let expanded = workspace.path().join("expanded");
    fs::create_dir(&archives)?;
    fs::create_dir(&expanded)?;
    let name = target.name();
    println!("=== 自动下载 {name} ({}) ===", target.build);
    println!("如需登录，请在此终端完成 Apple 账户验证。登录与下载输出不写入日志。");
    let mut command = xcodes(executable)?;
    command
        .args(["download", &name, "--data-source", "xcodeReleases", "--directory"])
        .arg(&archives);
    writeln!(log, "Starting interactive download of {name} ({})", target.build)?;
    log.sync_all()?;
    download(&mut command, &tty)?;
    let archive = unique_entry(&archives, "xip", false)?;
    println!("=== 解压并验证官方 XIP ===");
    logged(
        tool_command(Path::new("/usr/bin/xip"))?
            .args(["--expand"])
            .arg(&archive)
            .current_dir(&expanded),
        log,
    )?;
    let source = unique_entry(&expanded, "app", true)?;
    verify(&source, &target.build)?;
    let candidate = staging.path().join("Xcode.app");
    tool_command(Path::new("/usr/bin/ditto"))?
        .arg(&source)
        .arg(&candidate)
        .status_checked()?;
    verify(&candidate, &target.build)?;
    let identity = fs::symlink_metadata(&candidate)?;
    move_exclusive(&candidate, &destination)?;
    let verify_install = File::open(applications)?
        .sync_all()
        .map_err(Into::into)
        .and_then(|()| verify(&destination, &target.build));
    if let Err(error) = verify_install {
        let current = fs::symlink_metadata(&destination)?;
        ensure!(
            current.dev() == identity.dev() && current.ino() == identity.ino(),
            "Installed Xcode path changed during verification; review before rollback: {error:#}"
        );
        move_exclusive(&destination, &candidate).wrap_err("Could not roll back unverified Xcode installation")?;
        File::open(applications)?.sync_all()?;
        return Err(error);
    }
    println!(
        "Xcode: INSTALLED — {name} ({})\n安装路径：{}",
        target.build,
        destination.display()
    );
    Ok(())
}

fn xcodes(executable: &Path) -> Result<Command> {
    let mut command = tool_command(executable)?;
    for name in ["XCODES_USERNAME", "XCODES_PASSWORD"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    Ok(command)
}

fn catalog(executable: &Path, log: &mut File) -> Result<Release> {
    // Xcodes refreshes its public catalog when stale without authenticating.
    let (status, stdout, stderr) = bounded_output(
        tool_command(executable)?.args([
            "list",
            "--architecture",
            "arm64",
            "--data-source",
            "xcodeReleases",
            "--no-color",
        ]),
        Duration::from_secs(120),
        8 * 1024 * 1024,
    )?;
    log.write_all(without_urls(&stdout).as_bytes())?;
    log.write_all(without_urls(&stderr).as_bytes())?;
    log.sync_all()?;
    ensure!(
        status.success(),
        "Xcodes catalog failed ({status}); see the private log"
    );
    parse_catalog(&stdout)
}

pub(in crate::studio) fn xcode_beta(check_only: bool) -> Result<()> {
    super::super::user_only()?;
    let home = super::super::account_home()?;
    let _lock = super::lock(&home, "xcode-beta-updater.lock")?;
    let executable = super::super::resolve_tool("xcodes")?;
    let logs = private_path(&home, "Library/Logs/TopgradeUpdater")?;
    let (mut log, path) = tempfile::Builder::new()
        .prefix("xcode-beta-")
        .suffix(".log")
        .tempfile_in(logs)?
        .keep()?;
    println!("Xcode 检查与解包日志：{}", path.display());
    let target = catalog(&executable, &mut log)?;
    let name = target.name();
    println!("目标：{name} ({})", target.build);
    let applications = Path::new(APPLICATIONS);
    if let Some(path) = installed(applications, &target.build)? {
        println!("Xcode: CURRENT — {name} ({})\n已安装：{}", target.build, path.display());
        return Ok(());
    }
    if check_only {
        println!("Xcode: UPDATE_AVAILABLE — {name} ({})", target.build);
        return Ok(());
    }
    install(&executable, applications, &home, &target, &mut log).map_err(|error| {
        crate::ctrlc::unset_interrupted();
        PartialStep(format!(
            "Xcode Beta {name} ({}) remains pending: {error:#}. No automatic authentication retry.",
            target.build
        ))
        .into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooses_numeric_arm64_beta_and_rejects_conflicting_builds() -> Result<()> {
        let catalog = "28.1 Beta (28B1) [Intel]\n27.3 Beta 2 (27C2) [Apple Silicon]\n27.3 Beta 11 (27C11) [Apple Silicon]\n27.3 Release Candidate (27C100) [Apple Silicon]";
        let target = parse_catalog(catalog)?;
        assert_eq!(target.name(), "27.3 Beta 11");
        assert_eq!(target.build, "27C11");
        assert!(parse_catalog(&format!("{catalog}\n27.3 Beta 11 (27C12) [Apple Silicon]")).is_err());
        Ok(())
    }

    #[test]
    fn publishing_preserves_an_existing_destination() -> Result<()> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("candidate");
        let destination = root.path().join("existing");
        fs::write(&source, b"new")?;
        fs::write(&destination, b"old")?;
        assert!(move_exclusive(&source, &destination).is_err());
        assert_eq!(fs::read(destination)?, b"old");
        assert_eq!(fs::read(source)?, b"new");
        Ok(())
    }

    #[test]
    fn download_logs_do_not_keep_authentication_urls() {
        let text = "Downloading Xcode (https://downloads.example/path?access_token=secret)\n";
        let safe = without_urls(text);
        assert!(safe.starts_with("Downloading Xcode"));
        assert!(safe.contains("[URL redacted]"));
        assert!(!safe.contains("secret"));
    }

    #[test]
    fn catalog_uses_the_cached_public_list_without_login_or_forced_refresh() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let xcodes = dir.path().join("xcodes");
        fs::write(
            &xcodes,
            concat!(
                "#!/bin/sh\n",
                "test \"$*\" = 'list --architecture arm64 --data-source xcodeReleases --no-color' || exit 95\n",
                "printf '%s\\n' '27.3 Beta 2 (27C5035i) [Apple Silicon]'\n",
            ),
        )
        .unwrap();
        fs::set_permissions(&xcodes, fs::Permissions::from_mode(0o700)).unwrap();
        let mut log = tempfile::tempfile().unwrap();
        assert_eq!(catalog(&xcodes, &mut log).unwrap().build, "27C5035i");
    }

    #[test]
    fn authentication_prompt_without_newline_can_be_answered_on_a_terminal() {
        use nix::libc;
        use std::io::Read;
        use std::os::fd::{AsRawFd, FromRawFd};

        let mut master = -1;
        let mut slave = -1;
        // openpty returns two owned descriptors on success.
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            },
            0
        );
        let mut master = unsafe { File::from_raw_fd(master) };
        let tty = unsafe { File::from_raw_fd(slave) };
        crate::studio::set_close_on_exec(master.as_raw_fd(), true).unwrap();
        crate::studio::set_close_on_exec(tty.as_raw_fd(), true).unwrap();
        let user = std::thread::spawn(move || {
            let mut poll = libc::pollfd {
                fd: master.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            assert_eq!(unsafe { libc::poll(&mut poll, 1, 5000) }, 1);
            let mut prompt = [0; 19];
            master.read_exact(&mut prompt).unwrap();
            assert_eq!(&prompt, b"Verification code: ");
            master.write_all(b"123456\n").unwrap();
            // Keep the PTY available until the child has processed the answer.
            master.read_exact(&mut [0]).unwrap();
            master
        });
        download(
            tool_command(Path::new("/bin/sh")).unwrap().args([
                "-c",
                r#"
                test -t 0 && test -t 1 && test -t 2 || exit 96
                stty -echo
                printf 'Verification code: ' >&2
                IFS= read -r otp || exit 97
                test "$otp" = 123456 || exit 98
                printf 'X' >&2
            "#,
            ]),
            &tty,
        )
        .unwrap();
        user.join().unwrap();
    }
}
