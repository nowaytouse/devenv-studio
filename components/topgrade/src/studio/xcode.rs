//! Select an installed, versioned Xcode beta in the terminal.

use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal, Read, Seek};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use color_eyre::eyre::{Context, Result, bail, ensure, eyre};
use nix::libc::{O_NOFOLLOW, O_NONBLOCK};
use regex::Regex;
use serde::Deserialize;

use super::{command, tool_command, wait_until};
use crate::command::CommandExt as _;

const XCODE_SELECT: &str = "/usr/bin/xcode-select";
const XCODEBUILD: &str = "/usr/bin/xcodebuild";
const PLUTIL: &str = "/usr/bin/plutil";
const SUDO: &str = "/usr/bin/sudo";
const PLIST_LIMIT: u64 = 4 * 1024 * 1024;
const VERSION_OUTPUT_LIMIT: u64 = 64 * 1024;

#[derive(Debug)]
struct Candidate {
    name: String,
    developer: PathBuf,
    key: (Vec<u64>, u64),
    build: String,
}

fn candidate(applications: &Path) -> Result<Option<Candidate>> {
    static NAME: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\AXcode-([0-9]+(?:\.[0-9]+)*)-Beta([0-9]+)-([0-9A-Za-z]+)\.app\z")
            .expect("valid Xcode bundle name pattern")
    });
    let mut best: Option<Candidate> = None;
    let mut ambiguous = false;
    for entry in fs::read_dir(applications)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some((name, parts)) = name
            .to_str()
            .and_then(|name| NAME.captures(name).map(|parts| (name, parts)))
        else {
            continue;
        };
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let developer = entry.path().canonicalize()?.join("Contents/Developer");
        match fs::symlink_metadata(&developer) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => continue,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        }
        ensure!(developer.canonicalize()? == developer, "Unsafe Xcode developer path");
        let key = (
            parts[1]
                .split('.')
                .map(str::parse)
                .collect::<std::result::Result<Vec<u64>, _>>()?,
            parts[2].parse()?,
        );
        match best.as_ref().map(|best| key.cmp(&best.key)) {
            Some(std::cmp::Ordering::Less) => {}
            Some(std::cmp::Ordering::Equal) => ambiguous = true,
            None | Some(std::cmp::Ordering::Greater) => {
                ambiguous = false;
                best = Some(Candidate {
                    name: name.to_owned(),
                    developer,
                    key,
                    build: parts[3].to_owned(),
                });
            }
        }
    }
    ensure!(
        !ambiguous,
        "Multiple Xcode bundles claim the highest version and beta number"
    );
    Ok(best)
}

fn plist(path: &Path, args: &[&str]) -> Result<String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() <= PLIST_LIMIT && path.canonicalize()? == path,
        "Unsafe Xcode property list: {}",
        path.display()
    );
    Ok(tool_command(Path::new(PLUTIL))?
        .args(args)
        .args(["--", "-"])
        .stdin(Stdio::from(file))
        .output_checked_utf8()?
        .stdout)
}

fn inspect_bundle(target: &Candidate) -> Result<()> {
    let contents = target
        .developer
        .parent()
        .ok_or_else(|| eyre!("Missing Xcode bundle contents"))?;
    let identifier = plist(
        &contents.join("Info.plist"),
        &["-extract", "CFBundleIdentifier", "raw", "-expect", "string"],
    )?;
    ensure!(
        identifier.trim() == "com.apple.dt.Xcode",
        "Unexpected Xcode bundle identifier"
    );

    #[derive(Deserialize)]
    struct Version {
        #[serde(rename = "ProductBuildVersion")]
        build: Option<String>,
    }
    let version = contents.join("version.plist");
    if super::exists(&version)? {
        let metadata: Version = serde_json::from_str(&plist(&version, &["-convert", "json", "-o", "-"])?)?;
        ensure!(
            metadata
                .build
                .as_deref()
                .is_none_or(|build| build.is_empty() || build == target.build),
            "Xcode version.plist and bundle name disagree on the build"
        );
    }
    Ok(())
}

fn active() -> Result<PathBuf> {
    let output = tool_command(Path::new(XCODE_SELECT))?.arg("-p").output_checked_utf8()?;
    let path = Path::new(output.stdout.trim());
    ensure!(
        path.is_absolute() && path.is_dir(),
        "Invalid active Xcode developer directory"
    );
    Ok(path.canonicalize()?)
}

fn read_output(mut file: File) -> Result<String> {
    file.rewind()?;
    let mut output = String::new();
    file.take(VERSION_OUTPUT_LIMIT + 1).read_to_string(&mut output)?;
    ensure!(
        output.len() as u64 <= VERSION_OUTPUT_LIMIT,
        "Xcode version output exceeds its limit"
    );
    Ok(output)
}

fn version(developer: Option<&Path>, expected: &str) -> Result<String> {
    let stdout = tempfile::tempfile()?;
    let stderr = tempfile::tempfile()?;
    let mut command = tool_command(Path::new(XCODEBUILD))?;
    if let Some(path) = developer {
        command.env("DEVELOPER_DIR", path);
    }
    let mut child = command
        .arg("-version")
        .stdout(Stdio::from(stdout.try_clone()?))
        .stderr(Stdio::from(stderr.try_clone()?))
        .spawn_checked()?;
    let Some(status) = wait_until(&mut child, Instant::now() + Duration::from_secs(60))? else {
        let _ = child.kill();
        child.wait().wrap_err("Could not reap timed-out xcodebuild")?;
        bail!("Xcode version check timed out");
    };
    ensure!(
        status.success(),
        "xcodebuild -version failed ({status}): {}",
        read_output(stderr)?.trim()
    );
    let output = read_output(stdout)?;
    let build = format!("Build version {expected}");
    ensure!(
        output.lines().any(|line| line == build),
        "xcodebuild returned a different build"
    );
    Ok(output)
}

fn switch(developer: &Path) -> Result<()> {
    command(Path::new(SUDO))
        .args(["--", XCODE_SELECT, "--switch"])
        .arg(developer)
        .stdin(Stdio::inherit())
        .status_checked()
}

fn select(
    target: &Candidate,
    terminal: bool,
    mut active: impl FnMut() -> Result<PathBuf>,
    mut version: impl FnMut(Option<&Path>, &str) -> Result<String>,
    mut switch: impl FnMut(&Path) -> Result<()>,
) -> Result<()> {
    version(Some(&target.developer), &target.build).wrap_err("Could not validate the target Xcode")?;
    let previous = active().wrap_err("Could not determine the current developer directory for rollback")?;
    if previous == target.developer {
        println!("Xcode Auto Select: CURRENT — {}", target.name);
        return Ok(());
    }

    ensure!(
        terminal,
        "Xcode selection requires an interactive terminal for administrator authorization"
    );
    ensure!(
        active()? == previous,
        "Active developer directory changed before switching"
    );
    println!("Xcode Auto Select: SWITCHING → {}", target.name);

    let result = switch(&target.developer).and_then(|()| {
        ensure!(
            active()? == target.developer,
            "xcode-select did not select the target developer directory"
        );
        let output = version(None, &target.build)?;
        ensure!(
            active()? == target.developer,
            "Active developer directory changed during Xcode validation"
        );
        Ok(output)
    });

    match result {
        Ok(output) => {
            println!("Xcode Auto Select: SELECTED — {}\n{}", target.name, output.trim());
            Ok(())
        }
        Err(error) => {
            let context = match active() {
                Ok(current) if current == previous => "Previous developer directory is still active".to_owned(),
                Ok(current) if current == target.developer => {
                    println!("Xcode Auto Select: validation failed; restoring the previous developer directory");
                    match switch(&previous).and_then(|()| {
                        ensure!(active()? == previous, "the previous developer directory is not active");
                        Ok(())
                    }) {
                        Ok(()) => "Previous developer directory was restored and verified".to_owned(),
                        Err(rollback) => {
                            format!("Xcode rollback failed; inspect the developer directory manually: {rollback:#}")
                        }
                    }
                }
                Ok(_) => "Developer directory changed independently; inspect the active selection manually".to_owned(),
                Err(readback) => format!("Could not read the active developer directory after failure: {readback:#}"),
            };
            Err(error.wrap_err(context))
        }
    }
}

pub(super) fn run() -> Result<()> {
    let Some(target) = candidate(Path::new("/Applications"))? else {
        println!("Xcode Auto Select: SKIPPED — 未发现已安装的版本化 Beta");
        return Ok(());
    };
    inspect_bundle(&target)?;
    let terminal = io::stdin().is_terminal() && io::stderr().is_terminal();
    select(&target, terminal, active, version, switch)
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::os::unix::fs::symlink;

    use super::*;

    fn bundle(root: &Path, name: &str) -> PathBuf {
        let developer = root.join(name).join("Contents/Developer");
        fs::create_dir_all(&developer).unwrap();
        developer.canonicalize().unwrap()
    }

    fn target() -> Candidate {
        Candidate {
            name: "Xcode-27.3-Beta1-27C5020n.app".into(),
            developer: "/Applications/Xcode-27.3-Beta1-27C5020n.app/Contents/Developer".into(),
            key: (vec![27, 3], 1),
            build: "27C5020n".into(),
        }
    }

    fn verified(_: Option<&Path>, build: &str) -> Result<String> {
        assert_eq!(build, "27C5020n");
        Ok(format!("Xcode 27.3\nBuild version {build}\n"))
    }

    #[test]
    fn chooses_highest_numeric_version_and_beta_from_real_versioned_directories() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert!(candidate(root).unwrap().is_none());
        bundle(root, "Xcode-27.2-Beta10-27B5038j.app");
        bundle(root, "Xcode-27.10-Beta2-27Z5020n.app");
        let latest = bundle(root, "Xcode-27.10-Beta10-27Z5030n.app");
        let unversioned = bundle(root, "Xcode-beta.app");
        symlink(
            unversioned.ancestors().nth(2).unwrap(),
            root.join("Xcode-99.0-Beta9-99Z999.app"),
        )
        .unwrap();
        fs::create_dir(root.join("Xcode-29.0-Beta1-29C5020n.app")).unwrap();

        let selected = candidate(root).unwrap().unwrap();
        assert_eq!(selected.developer, latest);
        assert_eq!(selected.build, "27Z5030n");
        bundle(root, "Xcode-27.10-Beta10-27Z5031n.app");
        assert!(
            candidate(root)
                .unwrap_err()
                .to_string()
                .contains("Multiple Xcode bundles")
        );
    }

    #[test]
    fn plist_identity_and_build_are_checked_by_system_plutil() {
        let dir = tempfile::tempdir().unwrap();
        let mut target = target();
        target.developer = bundle(dir.path(), &target.name);
        let contents = target.developer.parent().unwrap();
        let info = contents.join("Info.plist");
        let version = contents.join("version.plist");
        let write = |path: &Path, key: &str, value: &str| {
            fs::write(
                path,
                format!(
                    r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>{key}</key><string>{value}</string></dict></plist>"#
                ),
            )
            .unwrap();
        };

        write(&info, "CFBundleIdentifier", "com.apple.dt.Xcode");
        inspect_bundle(&target).unwrap();
        write(&version, "ProductBuildVersion", "27C5020n");
        inspect_bundle(&target).unwrap();
        write(&version, "ProductBuildVersion", "different");
        assert!(inspect_bundle(&target).unwrap_err().to_string().contains("disagree"));
        write(&info, "CFBundleIdentifier", "com.example.Other");
        assert!(inspect_bundle(&target).unwrap_err().to_string().contains("identifier"));
    }

    #[test]
    fn no_tty_is_a_noop_only_when_the_target_is_already_current() {
        let target = target();
        select(
            &target,
            false,
            || Ok(target.developer.clone()),
            verified,
            |_| panic!("already current must not switch"),
        )
        .unwrap();

        let error = select(
            &target,
            false,
            || Ok("/previous".into()),
            verified,
            |_| panic!("noninteractive invocation must not switch"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("interactive terminal"));
    }

    #[test]
    fn pre_switch_readback_detects_an_independent_change() {
        let target = target();
        let mut active = ["/previous", "/independent"].into_iter();
        let error = select(
            &target,
            true,
            || Ok(active.next().unwrap().into()),
            verified,
            |_| panic!("do not overwrite an independent switch"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("changed before switching"));
    }

    #[test]
    fn a_failed_postcondition_restores_the_previous_selection_and_returns_error() {
        let target = target();
        let previous = PathBuf::from("/previous");
        let current = RefCell::new(previous.clone());
        let mut calls = Vec::new();
        let error = select(
            &target,
            true,
            || Ok(current.borrow().clone()),
            |developer, build| {
                if developer.is_none() {
                    bail!("simulated default build mismatch");
                }
                verified(developer, build)
            },
            |developer| {
                calls.push(developer.to_path_buf());
                current.replace(developer.to_path_buf());
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(calls, [target.developer, previous.clone()]);
        assert_eq!(current.into_inner(), previous);
        let report = format!("{error:#}");
        assert!(report.contains("restored and verified") && report.contains("simulated default build mismatch"));
    }

    #[test]
    fn successful_switch_checks_build_and_active_path() {
        let target = target();
        let current = RefCell::new(PathBuf::from("/previous"));
        let version_checks = Cell::new(0);
        select(
            &target,
            true,
            || Ok(current.borrow().clone()),
            |developer, build| {
                let count = version_checks.get();
                assert_eq!(
                    developer,
                    if count == 0 {
                        Some(target.developer.as_path())
                    } else {
                        None
                    }
                );
                version_checks.set(count + 1);
                verified(developer, build)
            },
            |developer| {
                current.replace(developer.to_path_buf());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(version_checks.get(), 2);
        assert_eq!(current.into_inner(), target.developer);
    }

    #[test]
    fn rollback_does_not_overwrite_an_independent_selection_and_reports_its_own_failure() {
        let target = target();
        for independent in [false, true] {
            let current = RefCell::new(PathBuf::from("/previous"));
            let mut calls = 0;
            let error = select(
                &target,
                true,
                || Ok(current.borrow().clone()),
                |developer, build| {
                    if developer.is_none() {
                        if independent {
                            current.replace("/independent".into());
                        }
                        bail!("simulated build mismatch");
                    }
                    verified(developer, build)
                },
                |developer| {
                    calls += 1;
                    if developer != target.developer {
                        bail!("simulated rollback failure");
                    }
                    current.replace(developer.to_path_buf());
                    Ok(())
                },
            )
            .unwrap_err();
            let report = format!("{error:#}");
            if independent {
                assert!(report.contains("changed independently"));
                assert_eq!(calls, 1);
                assert_eq!(current.into_inner(), Path::new("/independent"));
            } else {
                assert!(report.contains("rollback failed") && report.contains("simulated build mismatch"));
                assert_eq!(calls, 2);
                assert_eq!(current.into_inner(), target.developer);
            }
        }
    }
}
