use color_eyre::eyre::{Result, bail, ensure};
use std::{path::Path, process::Stdio};

use super::{APPLICATIONS, bundle, field, signature, team, tool_command};
use crate::command::CommandExt as _;

const ID: &str = "pro.betterdisplay.BetterDisplay";
const FEED: &str = "https://betterdisplay.pro/betterdisplay/sparkle/appcast.xml";
const CHANNELS: &str = "arm64_internal,arm64_pre,arm64";

pub(in crate::studio) fn betterdisplay() -> Result<()> {
    super::super::user_only()?;
    let home = super::super::account_home()?;
    let _lock = super::lock(&home, "betterdisplay-preview.lock")?;
    let executable = home.join(".local/opt/sparkle-cli/sparkle.app/Contents/MacOS/sparkle");
    super::super::open_file(&executable, false, 64 * 1024 * 1024)?;
    signature(&executable, None)?;
    let app = Path::new(APPLICATIONS).join("BetterDisplay.app");
    let before = field(&bundle(&app, ID)?, "CFBundleVersion")?;
    signature(&app, None)?;
    let expected_team = team(&app)?;
    let invoke = |mode: &str| -> Result<i32> {
        let status = tool_command(&executable)?
            .arg(&app)
            .args([
                "--channels",
                CHANNELS,
                "--feed-url",
                FEED,
                "--user-agent-name",
                "PreviewAppsWorkflow",
                mode,
            ])
            .stdin(Stdio::inherit())
            .spawn_checked()?
            .wait()?;
        status
            .code()
            .ok_or_else(|| color_eyre::eyre::eyre!("Sparkle terminated by signal"))
    };

    println!("BetterDisplay: Checking Sparkle channels {CHANNELS} using the official feed");
    check_with_sparkle(
        invoke,
        || {
            let live = field(&bundle(&app, ID)?, "CFBundleVersion")?;
            ensure!(
                live == before && team(&app)? == expected_team,
                "BetterDisplay changed during its probe"
            );
            Ok(())
        },
        || {
            let after = field(&bundle(&app, ID)?, "CFBundleVersion")?;
            signature(&app, None)?;
            ensure!(
                team(&app)? == expected_team,
                "BetterDisplay signing team changed after update"
            );
            updated(&before, &after)?;
            println!("BetterDisplay: UPDATED — build {before} -> {after}");
            Ok(())
        },
    )
}

fn check_with_sparkle(
    mut invoke: impl FnMut(&str) -> Result<i32>,
    verify_before_update: impl FnOnce() -> Result<()>,
    verify_after_update: impl FnOnce() -> Result<()>,
) -> Result<()> {
    match invoke("--probe")? {
        4 => println!("BetterDisplay: CURRENT — no update available from Sparkle for the requested channels"),
        0 => {
            verify_before_update()?;
            match invoke("--check-immediately")? {
                4 => println!("BetterDisplay: CURRENT — Sparkle found no update on recheck"),
                0 => verify_after_update()?,
                status => bail!("BetterDisplay: Sparkle CLI exit {status}"),
            }
        }
        status => bail!("BetterDisplay: Sparkle probe exit {status}"),
    }
    Ok(())
}

fn updated(before: &str, after: &str) -> Result<()> {
    ensure!(
        before != after,
        "BetterDisplay: update was offered but build did not change ({after})"
    );
    Ok(())
}

#[cfg(test)]
#[test]
fn probe_no_update_completes_without_installation() {
    for status in [4, 1, 6] {
        let result = check_with_sparkle(
            |mode| {
                assert_eq!(mode, "--probe");
                Ok(status)
            },
            || panic!("unexpected update"),
            || panic!("unexpected installation"),
        );
        assert_eq!(result.is_ok(), status == 4);
    }
}

#[cfg(test)]
#[test]
fn install_exit_zero_requires_successful_readback() {
    let mut calls = ["--probe", "--check-immediately"].into_iter();
    let error = check_with_sparkle(
        |mode| {
            assert_eq!(Some(mode), calls.next());
            Ok(0)
        },
        || Ok(()),
        || updated("54070", "54070"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("build did not change"));
    assert!(calls.next().is_none());
}

#[cfg(test)]
#[test]
fn successful_sparkle_update_requires_a_new_build() {
    assert!(updated("54070", "54070").is_err());
    assert!(updated("54070", "54080").is_ok());
}
