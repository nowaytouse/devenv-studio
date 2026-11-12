use std::{
    io::{self, Write as _},
    path::Path,
};

use color_eyre::eyre::{Result, ensure};
use serde::Deserialize;

use crate::{
    command::CommandExt as _, error::PartialStep, execution_context::ExecutionContext, executor::ExecutorOutput,
};

const TARGET: &str = "claude-code-plugins-plus";
const SINGLE_FAILURE: &str = "1 marketplace could not be refreshed (see --debug): claude-code-plugins-plus";

fn reports_incomplete(stdout: &str, stderr: &str) -> bool {
    stdout.lines().chain(stderr.lines()).any(|line| {
        line.contains("marketplace could not be refreshed")
            || line.contains("marketplaces could not be refreshed")
            || line.contains("marketplaces, but not all")
    })
}

fn recoverable(stdout: &str, stderr: &str, code: Option<i32>) -> bool {
    let mut failures = stdout
        .lines()
        .chain(stderr.lines())
        .map(str::trim)
        .map(|line| line.strip_prefix("Updating marketplaces...").unwrap_or(line).trim())
        .filter(|line| line.contains("could not be refreshed"));
    code == Some(1) && failures.next() == Some(SINGLE_FAILURE) && failures.next().is_none()
}

fn verified_target(stdout: &str) -> bool {
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
            report.command == "marketplace-update" && report.outcome == "ok" && report.marketplace == TARGET
        })
}

pub(super) fn refresh(ctx: &ExecutionContext, claude: &Path) -> Result<()> {
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

    ensure!(
        recoverable(&stdout, &stderr, output.status.code()),
        "Claude marketplace refresh failed ({})",
        output.status
    );
    // Claude's bulk command has no JSON mode. Recover only the single known diagnostic.
    println!("Retrying Claude marketplace {TARGET} individually");
    let retry = ctx
        .execute(claude)
        .args(["plugin", "marketplace", "update", TARGET, "--json"])
        .output_checked_utf8()?;
    ensure!(
        verified_target(&retry.stdout) && !reports_incomplete(&retry.stdout, &retry.stderr),
        "Claude did not verify refresh of {TARGET}"
    );
    println!("Refreshed Claude marketplace {TARGET}");
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
    fn retries_only_the_known_singular_failure() {
        assert!(recoverable("", SINGLE_FAILURE, Some(1)));
        assert!(recoverable(
            &format!("Updating marketplaces...{SINGLE_FAILURE}"),
            "",
            Some(1)
        ));
        assert!(!recoverable("", SINGLE_FAILURE, Some(0)));
        assert!(!recoverable("", SINGLE_FAILURE, None));
        assert!(!recoverable("", &SINGLE_FAILURE.replace(TARGET, "unrelated"), Some(1)));
        assert!(!recoverable(SINGLE_FAILURE, SINGLE_FAILURE, Some(1)));
        assert!(!recoverable(
            "",
            &format!("{SINGLE_FAILURE}, another-marketplace"),
            Some(1)
        ));
    }

    #[test]
    fn needs_success_for_the_exact_marketplace_in_the_final_json_line() {
        let report = r#"{"command":"marketplace-update","outcome":"ok","marketplace":"claude-code-plugins-plus"}"#;
        assert!(verified_target(&format!("Refreshing…\n{report}\n")));
        assert!(!verified_target(&report.replace("ok", "failed")));
        assert!(!verified_target(&report.replace(TARGET, "unrelated")));
        assert!(!verified_target(&format!("{report}\nnot a JSON result")));
    }
}
