use color_eyre::eyre::{Result, bail, ensure};
use serde_json::Value;
use std::{path::Path, process::Stdio, time::Duration};

use super::{APPLICATIONS, bounded_output, bundle, field, signature, team, tool_command};
use crate::command::CommandExt as _;

const ID: &str = "pro.betterdisplay.BetterDisplay";
const FEED: &str = "https://betterdisplay.pro/betterdisplay/sparkle/appcast.xml";

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
                "arm64_internal,arm64_pre,arm64",
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
    let probe = invoke("--probe")?;
    match probe {
        4 => println!("BetterDisplay: CURRENT — native probe found no update for requested channels"),
        0 => {
            let live = field(&bundle(&app, ID)?, "CFBundleVersion")?;
            ensure!(
                live == before && team(&app)? == expected_team,
                "BetterDisplay changed during its probe"
            );
            match invoke("--check-immediately")? {
                4 => println!("BetterDisplay: SPARKLE_NO_UPDATE (arm64_internal,arm64_pre,arm64 requested)"),
                0 => {
                    let after = field(&bundle(&app, ID)?, "CFBundleVersion")?;
                    signature(&app, None)?;
                    ensure!(
                        team(&app)? == expected_team,
                        "BetterDisplay signing team changed after update"
                    );
                    updated(&before, &after)?;
                    println!("BetterDisplay: UPDATED — build {before} -> {after}");
                }
                status => bail!("BetterDisplay: Sparkle CLI exit {status}"),
            }
        }
        status => bail!("BetterDisplay: Sparkle probe exit {status}"),
    }
    preference_hint();
    println!("BetterDisplay: INTERNAL_PREFERENCE_UNVERIFIED — check Settings > Application > Updates");
    Ok(())
}

fn updated(before: &str, after: &str) -> Result<()> {
    ensure!(
        before != after,
        "BetterDisplay: update was offered but build did not change ({after})"
    );
    Ok(())
}

fn preference_hint() {
    let read = || -> Result<Value> {
        let (status, plist, _) = bounded_output(
            tool_command(Path::new("/usr/bin/defaults"))?.args(["export", ID, "-"]),
            Duration::from_secs(8),
            super::PLIST_LIMIT,
        )?;
        ensure!(status.success(), "No externally readable BetterDisplay defaults");
        let mut temp = tempfile::NamedTempFile::new()?;
        std::io::Write::write_all(&mut temp, plist.as_bytes())?;
        super::plist(&temp.path().canonicalize()?)
    };
    let Some(values) = read().ok().and_then(|value| value.as_object().cloned()) else {
        println!("BetterDisplay: native update-channel preference is not externally readable");
        return;
    };
    let mut count = 0;
    for (key, value) in values {
        let normalized = key.to_ascii_lowercase().replace(['-', '_', ' '], "");
        if !["internal", "prerelease", "updatechannel"]
            .iter()
            .any(|term| normalized.contains(term))
            || key.len() > 100
            || key.chars().any(char::is_control)
        {
            continue;
        }
        if count < 6 {
            let description = match value.as_bool() {
                Some(true) => "ON",
                Some(false) => "OFF",
                None => "present (meaning unverified)",
            };
            println!("BetterDisplay: possible native setting {key} = {description} (key mapping UNVERIFIED)");
        }
        count += 1;
    }
    if count == 0 {
        println!("BetterDisplay: no identifiable internal-channel preference key");
    } else if count > 6 {
        println!("BetterDisplay: {} additional candidates omitted", count - 6);
    }
}

#[cfg(test)]
#[test]
fn successful_sparkle_update_requires_a_new_build() {
    assert!(updated("54070", "54070").is_err());
    assert!(updated("54070", "54080").is_ok());
}
