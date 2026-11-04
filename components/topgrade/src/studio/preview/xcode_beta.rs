use color_eyre::eyre::{Context, ContextCompat, Result, bail, ensure};
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
use crate::command::CommandExt as _;

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

fn logged(command: &mut Command, log: &mut File, fold_catalog: bool) -> Result<()> {
    static CATALOG_ITEM: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^\s*[0-9]+(?:\.[0-9]+){1,4}(?:\s+(?:Beta|Release Candidate|GM Seed|RC)(?:\s+[0-9]+)?)?\s+\([A-Za-z0-9]+\)(?:\s+\[[^]]+\])?(?:\s+\(Installed(?:, Selected)?\))?\s*$")
            .expect("valid Xcode version row")
    });
    let (pipe, writer) = io::pipe()?;
    let mut child = command
        .stdin(Stdio::inherit())
        .stdout(Stdio::from(writer.try_clone()?))
        .stderr(Stdio::from(writer))
        .spawn_checked()?;
    // Command keeps its own write ends; close them before reading to EOF.
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let result = (|| -> Result<()> {
        let mut reader = io::BufReader::new(pipe);
        let mut output = io::stdout().lock();
        let mut line = Vec::new();
        let mut omitted = 0;
        loop {
            line.clear();
            if reader.read_until(b'\n', &mut line)? == 0 {
                break;
            }
            let text = String::from_utf8_lossy(&line);
            let text = without_urls(&text);
            log.write_all(text.as_bytes())?;
            if fold_catalog && CATALOG_ITEM.is_match(&text) {
                omitted += 1;
            } else {
                output.write_all(text.as_bytes())?;
                output.flush()?;
            }
        }
        if omitted > 0 {
            writeln!(output, "[Xcode 版本目录已折叠：{omitted} 条，完整内容保存在日志中]")?;
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
    logged(
        xcodes(executable)?
            .args(["download", &name, "--directory"])
            .arg(&archives),
        log,
        false,
    )?;
    let archive = unique_entry(&archives, "xip", false)?;
    println!("=== 解压并验证官方 XIP ===");
    logged(
        tool_command(Path::new("/usr/bin/xip"))?
            .args(["--expand"])
            .arg(&archive)
            .current_dir(&expanded),
        log,
        false,
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

pub(in crate::studio) fn xcode_beta() -> Result<()> {
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
    println!("Xcode 完整运行日志：{}", path.display());
    println!("=== 更新 Xcode 版本目录 ===");
    logged(xcodes(&executable)?.arg("update"), &mut log, true)?;
    let (status, stdout, stderr) = bounded_output(
        xcodes(&executable)?.arg("list"),
        Duration::from_secs(120),
        8 * 1024 * 1024,
    )?;
    log.write_all(without_urls(&stdout).as_bytes())?;
    log.write_all(without_urls(&stderr).as_bytes())?;
    log.sync_all()?;
    ensure!(status.success(), "Xcodes catalog failed; see the private log");
    let target = parse_catalog(&stdout)?;
    let name = target.name();
    println!("目标：{name} ({})", target.build);
    let applications = Path::new(APPLICATIONS);
    if let Some(path) = installed(applications, &target.build)? {
        println!("Xcode: CURRENT — {name} ({})\n已安装：{}", target.build, path.display());
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        bail!("Xcode Beta installation requires a Terminal for Apple authentication");
    }
    install(&executable, applications, &home, &target, &mut log)
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
}
