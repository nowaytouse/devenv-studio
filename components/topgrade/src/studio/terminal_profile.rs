//! Native managed-profile operations that keep terminal authorization semantics.
use super::xcode;
use crate::{command::CommandExt, execution_context::RunType, executor::ExecutorOutput, utils::require};
use color_eyre::eyre::{Result, WrapErr, bail, ensure};
use serde::Deserialize;
use std::{collections::BTreeSet, io::Write, path::Path, thread, time::Duration};

const MAS: &str = "/opt/homebrew/bin/mas";
const GCLOUD_WORKAROUND: &str = "WARNING: Known issue encountered updating Python modules.";
const START: &str = "Beginning update. This process may take several minutes.";

pub(super) fn run(id: &str, mode: RunType) -> Result<()> {
    let action: fn() -> Result<()> = match id {
        "gcloud_verified" => gcloud,
        "mas_verified" => mas,
        "xcode_select_beta" => xcode::run,
        _ => bail!("Unknown native profile operation: {id}"),
    };
    if mode.dry() {
        println!("Dry running: {id}");
        return Ok(());
    }
    action()
}

fn manifest_transport_failure(output: &str) -> bool {
    let mut starts = 0;
    let mut errors = 0;
    for line in output.lines() {
        match line {
            ""
            | "If you would like to report this issue, please run the following command:"
            | "  gcloud feedback"
            | "To check gcloud for common problems, please run the following command:"
            | "  gcloud info --run-diagnostics" => {}
            START => starts += 1,
            _ => {
                let reason = ["SSLError", "ConnectionError"].iter().any(|kind| {
                    line.starts_with(&format!(
                        "ERROR: gcloud crashed ({kind}): HTTPSConnectionPool(host='dl.google.com', port=443): Max retries exceeded with url: /dl/cloudsdk/channels/rapid/components-2.json "
                    ))
                });
                let known_failure = (line.contains("SSLEOFError") && line.contains("UNEXPECTED_EOF_WHILE_READING"))
                    || (line.contains("ConnectionResetError") && line.contains("Connection reset by peer"));
                if !reason || !known_failure {
                    return false;
                }
                errors += 1;
            }
        }
    }
    starts == 1 && errors == 1
}

fn gcloud() -> Result<()> {
    let executable = require("gcloud")?;
    for attempt in 0..2 {
        let ExecutorOutput::Wet(output) = RunType::Wet
            .execute(&executable)
            .args(["components", "update", "--quiet"])
            .output()?
        else {
            bail!("Unexpected dry result for gcloud");
        };
        std::io::stdout().write_all(&output.stdout)?;
        std::io::stderr().write_all(&output.stderr)?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        ensure!(
            !stdout.contains(GCLOUD_WORKAROUND) && !stderr.contains(GCLOUD_WORKAROUND),
            "gcloud reported its Python workaround; component update may not have run"
        );
        if output.status.success() {
            return Ok(());
        }
        let combined = format!("{stdout}\n{stderr}");
        ensure!(
            attempt == 0 && manifest_transport_failure(&combined),
            "gcloud component update failed ({})",
            output.status
        );
        eprintln!("Official gcloud manifest transport failed before component operations; retrying once");
        thread::sleep(Duration::from_secs(2));
    }
    bail!("gcloud did not finish");
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

fn mas_query(mode: &str, ids: Option<&[u64]>) -> Result<Vec<u64>> {
    let allowed = ids.map(|ids| ids.iter().copied().collect());
    let output = RunType::Wet
        .execute(Path::new(MAS))
        .args(["outdated", mode, "--json"])
        .args(ids.into_iter().flatten().map(u64::to_string))
        .output_checked_utf8()?;
    std::io::stderr().write_all(output.stderr.as_bytes())?;
    mas_ids(&output.stdout, allowed.as_ref())
}

fn mas() -> Result<()> {
    let candidates = mas_query("--inaccurate", None)?;
    if candidates.is_empty() {
        println!("MAS: current (inaccurate catalog found no candidates)");
        return Ok(());
    }
    let verified = mas_query("--accurate", Some(&candidates))?;
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
    let remaining = mas_query("--accurate", Some(&verified))?;
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
    fn gcloud_retries_only_the_official_pre_operation_envelope() {
        let error = "ERROR: gcloud crashed (SSLError): HTTPSConnectionPool(host='dl.google.com', port=443): Max retries exceeded with url: /dl/cloudsdk/channels/rapid/components-2.json (SSLEOFError: UNEXPECTED_EOF_WHILE_READING)";
        assert!(manifest_transport_failure(&format!("{START}\n{error}\n")));
        assert!(!manifest_transport_failure(&format!(
            "{START}\n{error}\nInstalling component"
        )));
        assert!(!manifest_transport_failure(&format!(
            "{START}\n{}",
            error.replace("dl.google.com", "example.test")
        )));
    }

    #[test]
    fn mas_rejects_duplicate_equal_and_unselected_candidates() {
        let line = r#"{"adamID":42,"version":"1","newVersion":"2"}"#;
        assert_eq!(mas_ids(line, Some(&BTreeSet::from([42]))).unwrap(), [42]);
        assert!(mas_ids(line, Some(&BTreeSet::from([43]))).is_err());
        assert!(mas_ids(&format!("{line}\n{line}"), None).is_err());
        assert!(mas_ids(&line.replace("\"2\"", "\"1\""), None).is_err());
        assert!(mas_ids(&line.replace("42", "true"), None).is_err());
    }
}
