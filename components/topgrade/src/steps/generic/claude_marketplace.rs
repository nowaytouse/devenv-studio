use std::{
    collections::HashSet,
    io::{self, Write as _},
    path::Path,
};

use color_eyre::eyre::{Result, WrapErr, ensure};
use serde::Deserialize;

use crate::{
    command::CommandExt as _,
    error::{NoAutomaticRetry, PartialStep},
    execution_context::ExecutionContext,
    executor::ExecutorOutput,
};

const SINGLE_FAILURE: &str = "1 marketplace could not be refreshed (see --debug): ";

fn reports_incomplete(stdout: &str, stderr: &str) -> bool {
    stdout.lines().chain(stderr.lines()).any(|line| {
        line.contains("marketplace could not be refreshed")
            || line.contains("marketplaces could not be refreshed")
            || line.contains("marketplaces, but not all")
    })
}

fn retry_target<'a>(
    stdout: &'a str,
    stderr: &'a str,
    code: Option<i32>,
    configured: &HashSet<String>,
) -> Option<&'a str> {
    if code != Some(1) {
        return None;
    }
    let mut target = None;
    for line in stdout.lines().chain(stderr.lines()).map(str::trim) {
        let line = line.strip_prefix("Updating marketplaces...").unwrap_or(line).trim();
        if line.is_empty()
            || line
                .strip_prefix("✘ Updated ")
                .and_then(|s| s.strip_suffix(" marketplaces, but not all"))
                .is_some_and(|n| n.parse::<u32>().is_ok())
        {
            continue;
        }
        let name = line.strip_prefix(SINGLE_FAILURE)?;
        if name.is_empty()
            || name.len() > 128
            || !name.as_bytes()[0].is_ascii_alphanumeric()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
            || !configured.contains(name)
            || target.replace(name).is_some()
        {
            return None;
        }
    }
    target
}

fn verified_target(stdout: &str, target: &str) -> bool {
    #[derive(Deserialize)]
    struct RefreshResult<'a> {
        command: &'a str,
        outcome: &'a str,
        marketplace: &'a str,
    }

    stdout
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .and_then(|line| serde_json::from_str::<RefreshResult<'_>>(line).ok())
        .is_some_and(|report| {
            report.command == "marketplace-update" && report.outcome == "ok" && report.marketplace == target
        })
}

pub(super) fn refresh(ctx: &ExecutionContext, claude: &Path, configured: &HashSet<String>) -> Result<()> {
    let ExecutorOutput::Wet(output) = ctx.execute(claude).args(["plugin", "marketplace", "update"]).output()? else {
        return Ok(());
    };
    io::stdout().write_all(&output.stdout)?;
    io::stdout().flush()?;
    io::stderr().write_all(&output.stderr)?;
    io::stderr().flush()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.success() {
        if reports_incomplete(&stdout, &stderr) {
            return Err(PartialStep("Claude reported an incomplete marketplace refresh despite exit 0".into()).into());
        }
        return Ok(());
    }

    let target = retry_target(&stdout, &stderr, output.status.code(), configured)
        .ok_or_else(|| NoAutomaticRetry(format!("Claude marketplace refresh failed ({})", output.status)))?;
    println!("Retrying Claude marketplace {target} individually");
    let retry = ctx
        .execute(claude)
        .args(["plugin", "marketplace", "update", target, "--json"])
        .output_checked_utf8()
        .wrap_err_with(|| NoAutomaticRetry(format!("Targeted refresh failed for {target}")))?;
    ensure!(
        verified_target(&retry.stdout, target) && !reports_incomplete(&retry.stdout, &retry.stderr),
        NoAutomaticRetry(format!("Claude did not verify refresh of {target}"))
    );
    println!("Refreshed Claude marketplace {target}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_partial_reports_in_either_stream() {
        assert!(reports_incomplete(
            "Updating marketplaces...1 marketplace could not be refreshed (see --debug): fixture",
            "",
        ));
        assert!(reports_incomplete("", "✘ Updated 12 marketplaces, but not all"));
        assert!(reports_incomplete(
            "",
            "2 marketplaces could not be refreshed (see --debug): a, b"
        ));
        assert!(!reports_incomplete("✓ Updated 12 marketplaces", ""));
    }

    #[test]
    fn retries_only_one_known_configured_name_from_a_complete_diagnostic() {
        let configured = HashSet::from([String::from("second.market"), String::from("--flag")]);
        let message = format!("Updating marketplaces...{SINGLE_FAILURE}second.market");
        assert_eq!(
            retry_target(&message, "✘ Updated 12 marketplaces, but not all", Some(1), &configured),
            Some("second.market")
        );
        for (out, err, code) in [
            (message.clone(), message.as_str(), Some(1)),
            (message.clone(), "other error", Some(1)),
            (message.clone(), "", Some(0)),
            (message.clone(), "", None),
            (message.replace("second.market", "unknown"), "", Some(1)),
            (message.replace("second.market", "--flag"), "", Some(1)),
            (message.replace("second.market", "a, b"), "", Some(1)),
        ] {
            assert!(retry_target(&out, err, code, &configured).is_none());
        }
    }

    #[test]
    fn needs_success_for_the_exact_marketplace_in_the_final_json_line() {
        let report = r#"{"command":"marketplace-update","outcome":"ok","marketplace":"fixture"}"#;
        assert!(verified_target(&format!("Refreshing…\n{report}\n"), "fixture"));
        assert!(!verified_target(&report.replace("ok", "failed"), "fixture"));
        assert!(!verified_target(report, "other"));
        assert!(!verified_target(&format!("{report}\nnot a JSON result"), "fixture"));
    }
}
