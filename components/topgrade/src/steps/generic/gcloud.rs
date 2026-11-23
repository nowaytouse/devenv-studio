use crate::{
    error::{NoAutomaticRetry, SkipStep},
    execution_context::RunType,
    executor::ExecutorOutput,
    terminal::print_separator,
    utils::require,
};
use color_eyre::eyre::{Result, WrapErr, bail};
use std::{
    io::{self, Write},
    thread,
    time::Duration,
};

const GCLOUD_WORKAROUND: &str = "WARNING: Known issue encountered updating Python modules.";
const START: &str = "Beginning update. This process may take several minutes.";

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

pub(super) fn update(mode: RunType) -> Result<()> {
    let gcloud = require("gcloud")?;
    if gcloud.starts_with("/snap") {
        return Err(SkipStep("gcloud is managed by Snap".into()).into());
    }
    print_separator("gcloud");
    for attempt in 1..=2 {
        let ExecutorOutput::Wet(output) = mode
            .execute(&gcloud)
            .args(["components", "update", "--quiet"])
            .output()
            .wrap_err(NoAutomaticRetry("Could not capture the gcloud component update".into()))?
        else {
            return Ok(());
        };
        io::stdout().write_all(&output.stdout)?;
        io::stderr().write_all(&output.stderr)?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stdout.contains(GCLOUD_WORKAROUND) || stderr.contains(GCLOUD_WORKAROUND) {
            return Err(NoAutomaticRetry(
                "gcloud reported its Python workaround; component update may not have run".into(),
            )
            .into());
        }
        if stderr.contains("You cannot perform this action because the Google Cloud CLI component manager")
            && stderr.contains("is disabled for this installation")
        {
            return Err(
                SkipStep("The Google Cloud CLI component manager is disabled for this installation".into()).into(),
            );
        }
        if output.status.success() {
            return Ok(());
        }
        if attempt == 2 || !manifest_transport_failure(&format!("{stdout}\n{stderr}")) {
            return Err(NoAutomaticRetry(format!("gcloud component update failed ({})", output.status)).into());
        }
        eprintln!("Official gcloud manifest transport failed before component operations; retrying once");
        thread::sleep(Duration::from_secs(2));
    }
    bail!("gcloud did not finish")
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
}
