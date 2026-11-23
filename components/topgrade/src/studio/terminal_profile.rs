//! Native managed-profile operations that keep terminal authorization semantics.
use super::{gradle, headsync, npm_guard, preview, rolling, skills_profile, sqlite, xcode};
use crate::{command::CommandExt, execution_context::RunType};
use color_eyre::eyre::{Result, WrapErr, bail, ensure};
use serde::Deserialize;
use std::{collections::BTreeSet, io::Write, path::Path, thread, time::Duration};

const MAS: &str = "/opt/homebrew/bin/mas";
pub(super) fn run(id: &str, mode: RunType) -> Result<()> {
    let action: fn() -> Result<()> = match id {
        "betterdisplay_preview" => preview::betterdisplay,
        "brew_headsync" => headsync::run,
        "gradle_nightly" => gradle::run,
        "krita_next" => preview::krita_next,
        "mas_verified" => mas,
        "npm_bundled_security" => npm_guard::run,
        "rolling_projects" => || rolling::run(false),
        "skills_unattended" => skills_profile::update,
        "skills_coverage" => skills_profile::coverage,
        "sqlite_trunk" => || sqlite::run(false),
        "sqlite_python_trunk" => || sqlite::run(true),
        "tourbox_beta" => preview::tourbox_beta,
        "xcode_select_beta" => xcode::run,
        "xcode_beta_install" => || preview::xcode_beta(false),
        "xcode_beta_check" => || preview::xcode_beta(true),
        _ => bail!("Unknown native profile operation: {id}"),
    };
    if mode.dry() {
        println!("Dry running: {id}");
        if id == "rolling_projects" {
            return rolling::run(true);
        }
        return Ok(());
    }
    action()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MasRow {
    #[serde(rename = "adamID")]
    adam_id: u64,
    version: String,
    new_version: String,
}

fn mas_ids(stdout: &str, allowed: Option<&BTreeSet<u64>>) -> Result<Vec<u64>> {
    let mut seen = BTreeSet::new();
    stdout
        .lines()
        .enumerate()
        .map(|(index, line)| {
            let row: MasRow =
                serde_json::from_str(line).wrap_err_with(|| format!("Invalid MAS candidate on line {}", index + 1))?;
            ensure!(
                row.adam_id > 0 && seen.insert(row.adam_id),
                "Invalid or duplicate MAS candidate ID"
            );
            ensure!(
                !row.version.trim().is_empty() && !row.new_version.trim().is_empty() && row.version != row.new_version,
                "Invalid MAS candidate versions for {}",
                row.adam_id
            );
            ensure!(
                allowed.is_none_or(|ids| ids.contains(&row.adam_id)),
                "Unselected MAS candidate {}",
                row.adam_id
            );
            Ok(row.adam_id)
        })
        .collect()
}

fn mas_lookup_pending(status: Option<i32>, stdout: &str, stderr: &str, ids: &BTreeSet<u64>) -> bool {
    status == Some(1)
        && stdout.is_empty()
        && stderr
            .trim_end_matches(['\r', '\n'])
            .strip_prefix("Error: No installed apps with ADAM ID ")
            .and_then(|id| id.parse().ok())
            .is_some_and(|id| ids.contains(&id))
}

fn mas_query(mode: &str, ids: Option<&[u64]>, after_upgrade: bool) -> Result<Vec<u64>> {
    let allowed = ids.map(|ids| ids.iter().copied().collect::<BTreeSet<_>>());
    let delays = if after_upgrade { &[1, 2, 4, 8][..] } else { &[] };
    for attempt in 0..=delays.len() {
        ensure!(!crate::ctrlc::interrupted(), "MAS verification interrupted");
        let output = RunType::Wet
            .execute(Path::new(MAS))
            .args(["outdated", mode, "--json"])
            .args(ids.into_iter().flatten().map(u64::to_string))
            .output_checked_with_utf8(|output| {
                if output.status.success()
                    || (attempt < delays.len()
                        && allowed.as_ref().is_some_and(|allowed| {
                            mas_lookup_pending(output.status.code(), &output.stdout, &output.stderr, allowed)
                        }))
                {
                    Ok(())
                } else {
                    Err(())
                }
            })?;
        if output.status.success() {
            std::io::stderr().write_all(output.stderr.as_bytes())?;
            return mas_ids(&output.stdout, allowed.as_ref());
        }
        let delay = delays[attempt];
        println!("MAS: App Store inventory has not caught up; checking again in {delay}s");
        thread::sleep(Duration::from_secs(delay));
    }
    bail!("MAS verification did not complete")
}

fn mas() -> Result<()> {
    let candidates = mas_query("--inaccurate", None, false)?;
    if candidates.is_empty() {
        println!("MAS: current (inaccurate catalog found no candidates)");
        return Ok(());
    }
    let verified = mas_query("--accurate", Some(&candidates), false)?;
    if verified.is_empty() {
        println!("MAS: current after accurate App Store verification");
        return Ok(());
    }
    println!("MAS: accurately confirmed update candidates: {verified:?}");
    std::io::stdout().flush()?;
    RunType::Wet
        .execute(Path::new(MAS))
        .args(["upgrade", "--accurate"])
        .args(verified.iter().map(u64::to_string))
        .status_checked()?;
    let remaining = mas_query("--accurate", Some(&verified), true)?;
    ensure!(
        remaining.is_empty(),
        "MAS exited successfully but these updates remain: {remaining:?}"
    );
    println!("MAS: verified current after accurate update");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mas_rejects_duplicate_equal_and_unselected_candidates() {
        let line = r#"{"adamID":42,"version":"1","newVersion":"2"}"#;
        assert_eq!(mas_ids(line, Some(&BTreeSet::from([42]))).unwrap(), [42]);
        assert!(mas_ids(line, Some(&BTreeSet::from([43]))).is_err());
        assert!(mas_ids(&format!("{line}\n{line}"), None).is_err());
        assert!(mas_ids(&line.replace("\"2\"", "\"1\""), None).is_err());
        assert!(mas_ids(&line.replace("42", "true"), None).is_err());
    }

    #[test]
    fn mas_lookup_retry_requires_the_exact_requested_id_and_error() {
        let ids = BTreeSet::from([42, 43]);
        let missing = "Error: No installed apps with ADAM ID 42\n";
        assert!(mas_lookup_pending(Some(1), "", missing, &ids));
        assert!(!mas_lookup_pending(Some(2), "", missing, &ids));
        assert!(!mas_lookup_pending(Some(1), "{}", missing, &ids));
        assert!(!mas_lookup_pending(Some(1), "", &missing.replace("42", "44"), &ids));
        assert!(!mas_lookup_pending(
            Some(1),
            "",
            &format!("{missing}Network error"),
            &ids
        ));
    }
}
