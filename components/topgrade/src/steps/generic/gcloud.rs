use crate::{
    command::display_output,
    error::{NoAutomaticRetry, SkipStep},
    execution_context::RunType,
    executor::ExecutorOutput,
    terminal::print_separator,
    utils::require,
};
use color_eyre::eyre::{Result, WrapErr};
use std::{thread, time::Duration};

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
    update_at(mode, &require("gcloud")?, thread::sleep)
}

fn update_at(mode: RunType, gcloud: &std::path::Path, mut wait: impl FnMut(Duration)) -> Result<()> {
    if gcloud.starts_with("/snap") {
        return Err(SkipStep("gcloud is managed by Snap".into()).into());
    }
    print_separator("gcloud");
    let mut delays = [2, 8].into_iter();
    loop {
        let ExecutorOutput::Wet(output) = mode
            .execute(gcloud)
            .args(["components", "update", "--quiet"])
            .output()
            .wrap_err(NoAutomaticRetry("Could not capture the gcloud component update".into()))?
        else {
            return Ok(());
        };
        display_output(&output).wrap_err(NoAutomaticRetry(
            "Could not display the gcloud component update result".into(),
        ))?;
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
        if !manifest_transport_failure(&format!("{stdout}\n{stderr}")) {
            return Err(NoAutomaticRetry(format!("gcloud component update failed ({})", output.status)).into());
        }
        let Some(seconds) = delays.next() else {
            return Err(NoAutomaticRetry(format!(
                "gcloud could not fetch the official manifest after three attempts ({}). Check this process's proxy settings and connectivity to dl.google.com.", output.status
            )).into());
        };
        eprintln!("Official gcloud manifest connection failed before updates began. Retrying in {seconds}s.");
        wait(Duration::from_secs(seconds));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn safe_retry_budget_survives_the_outer_runner() {
        use crate::{
            config::{CommandLineArgs, Config},
            execution_context::ExecutionContext,
            runner::{Runner, StepResult},
            step::Step,
        };
        use clap::Parser;
        use std::{fs, os::unix::fs::PermissionsExt};

        let directory = tempfile::tempdir().unwrap();
        let config_path = directory.path().join("config.toml");
        fs::write(&config_path, "[misc]\nask_retry = false\nauto_retry = 3\n").unwrap();
        let args = CommandLineArgs::parse_from([
            "topgrade",
            "--config",
            config_path.to_str().unwrap(),
            "--only",
            "gcloud",
        ]);
        let config = Config::load(args).unwrap();
        #[cfg(target_os = "linux")]
        let distribution = crate::steps::linux::Distribution::detect();
        let ctx = ExecutionContext::new(
            RunType::Wet,
            None,
            &config,
            #[cfg(target_os = "linux")]
            &distribution,
        );
        let manifest = "ERROR: gcloud crashed (SSLError): HTTPSConnectionPool(host='dl.google.com', port=443): Max retries exceeded with url: /dl/cloudsdk/channels/rapid/components-2.json (SSLEOFError: UNEXPECTED_EOF_WHILE_READING)";
        let executable = directory.path().join("gcloud");
        fs::write(&executable, format!(
            "#!/bin/sh\necho call >> \"$0.calls\"\n[ \"$(wc -l < \"$0.calls\")\" -ge 3 ] && exit 0\necho '{START}'\ncat >&2 <<'ERROR'\n{manifest}\nERROR\nexit 1\n"
        )).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        update_at(RunType::Wet, &executable, |_| {}).unwrap();
        for (stderr, status, expected_calls) in [
            (manifest, 1, 3),
            (GCLOUD_WORKAROUND, 0, 1),
            (
                "ERROR: (gcloud.components.update) The component [gsutil] failed to download.",
                1,
                1,
            ),
        ] {
            let executable = directory.path().join("gcloud");
            let counter = executable.with_extension("calls");
            fs::write(&counter, "").unwrap();
            let script = format!(
                "#!/bin/sh\necho call >> \"$0.calls\"\necho '{START}'\ncat >&2 <<'ERROR'\n{stderr}\nERROR\nexit {status}\n"
            );
            fs::write(&executable, script).unwrap();
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
            let mut runner = Runner::new(&ctx);
            runner
                .execute(Step::Gcloud, "gcloud", || {
                    let mut delays = Vec::new();
                    let result = update_at(ctx.run_type(), &executable, |d| delays.push(d.as_secs()));
                    if stderr == manifest {
                        assert_eq!(delays, [2, 8]);
                        assert!(result.as_ref().unwrap_err().to_string().contains("three attempts"));
                    } else {
                        assert!(delays.is_empty());
                    }
                    result
                })
                .unwrap();
            assert!(matches!(runner.report().as_slice(), [(_, StepResult::Failure)]));
            assert_eq!(
                fs::read_to_string(counter).unwrap().lines().count(),
                expected_calls,
                "{stderr}"
            );
        }
    }

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
