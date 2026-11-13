use std::io::Write;
use std::path::Path;
use std::process::Output;

use color_eyre::eyre::{Context, Result, bail};
use serde::Deserialize;

use crate::error::{PartialStep, TopgradeError};
use crate::execution_context::{ExecutionContext, RunType};
use crate::executor::ExecutorOutput;

// Documented WinGet HRESULTs. An unfiltered upgrade query can use these to
// report that no applicable candidates were found, rather than a process error.
pub(super) const UPDATE_NOT_APPLICABLE: u32 = 0x8A15_002B;
const NO_APPLICATIONS_FOUND: u32 = 0x8A15_0014;

pub(super) fn verify_after_upgrade(ctx: &ExecutionContext, winget: &Path) -> Result<()> {
    verify_with_run_type(ctx.run_type(), winget)
}

pub(super) fn verify_exact_after_upgrade(ctx: &ExecutionContext, winget: &Path, id: &str) -> Result<()> {
    verify_exact_with_run_type(ctx.run_type(), winget, id)
}

fn verify_exact_with_run_type(run_type: RunType, winget: &Path, id: &str) -> Result<()> {
    // `upgrade --id` would execute another upgrade. `list --upgrade-available`
    // is the read-only equivalent, limited to the package just maintained.
    let ExecutorOutput::Wet(output) = run_type
        .execute(winget)
        .args([
            "list",
            "--id",
            id,
            "--exact",
            "--source",
            "winget",
            "--upgrade-available",
            "--disable-interactivity",
        ])
        .output()?
    else {
        return Ok(());
    };
    std::io::stdout().lock().write_all(&output.stdout)?;
    std::io::stderr().lock().write_all(&output.stderr)?;
    verify_query_output(
        &output,
        &format!("winget list --id {id} --exact --source winget (verification)"),
        false,
    )
}

fn verify_with_run_type(run_type: RunType, winget: &Path) -> Result<()> {
    // A composite query can hide one failed source behind another source's
    // successful result. Inventory sources as JSON, then query each implicit
    // source separately. Explicit sources remain outside the default scope.
    // Ordinary executors ensure dry runs do not start these extra processes.
    let ExecutorOutput::Wet(inventory) = run_type.execute(winget).args(["source", "export"]).output()? else {
        return Ok(());
    };
    std::io::stderr().lock().write_all(&inventory.stderr)?;
    if !inventory.status.success() {
        return Err(TopgradeError::ProcessFailedWithOutput(
            "winget source export (verification)".into(),
            inventory.status,
            String::from_utf8_lossy(&inventory.stderr).into_owned(),
        )
        .into());
    }
    let sources = parse_sources(&inventory.stdout)?;
    verify_sources(&sources, |name| {
        let ExecutorOutput::Wet(output) = run_type
            .execute(winget)
            .args(["upgrade", "--source", name, "--disable-interactivity"])
            .output()?
        else {
            unreachable!("a dry run returns before source verification")
        };
        std::io::stdout().lock().write_all(&output.stdout)?;
        std::io::stderr().lock().write_all(&output.stderr)?;
        Ok(output)
    })
}

#[derive(Deserialize)]
struct WinGetSource {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Explicit", default)]
    explicit: bool,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SourceExport {
    Source(WinGetSource),
    Sources(Vec<WinGetSource>),
}

fn parse_sources(output: &[u8]) -> Result<Vec<WinGetSource>> {
    // WinGet emits one JSON object per source; also accept a JSON array.
    let mut sources = Vec::new();
    for item in serde_json::Deserializer::from_slice(output).into_iter::<SourceExport>() {
        match item.wrap_err("Could not read WinGet's source inventory; upgrade verification is incomplete")? {
            SourceExport::Source(source) => sources.push(source),
            SourceExport::Sources(items) => sources.extend(items),
        }
    }
    let mut names = std::collections::HashSet::new();
    for source in &sources {
        if source.name.trim().is_empty() || !names.insert(source.name.to_lowercase()) {
            bail!("WinGet source inventory contains an empty or duplicate name; upgrade verification is incomplete");
        }
    }
    Ok(sources)
}

fn verify_sources(sources: &[WinGetSource], mut query: impl FnMut(&str) -> Result<Output>) -> Result<()> {
    let mut sources = sources.iter().filter(|source| !source.explicit).peekable();
    if sources.peek().is_none() {
        return Err(
            PartialStep("WinGet did not provide an implicit source to verify remaining upgrades".into()).into(),
        );
    }
    let mut pending = Vec::new();
    let mut failure = None;
    for source in sources {
        println!("Checking remaining WinGet upgrades from {}", source.name);
        let command = format!(
            "winget upgrade --source {} --disable-interactivity (verification)",
            source.name
        );
        let result = query(&source.name)
            .wrap_err_with(|| command.clone())
            .and_then(|output| verify_query_output(&output, &command, true));
        if let Err(error) = result {
            if error.downcast_ref::<PartialStep>().is_some() {
                pending.push(source.name.as_str());
            } else if failure.is_none() {
                failure = Some(error);
            } else {
                eprintln!("{error:#}");
            }
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }
    if !pending.is_empty() {
        return Err(PartialStep(format!(
            "WinGet still lists upgrade candidates from: {}. Review the package tables above.",
            pending.join(", ")
        ))
        .into());
    }
    Ok(())
}

fn verify_query_output(output: &Output, command: &str, allow_no_applications: bool) -> Result<()> {
    let no_candidates =
        output.status.code().map(|code| code as u32).is_some_and(|code| {
            code == UPDATE_NOT_APPLICABLE || (allow_no_applications && code == NO_APPLICATIONS_FOUND)
        });
    if !output.status.success() && !no_candidates {
        return Err(TopgradeError::ProcessFailedWithOutput(
            command.into(),
            output.status,
            format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
        )
        .into());
    }

    if contains_upgrade_table(&output.stdout) {
        return Err(PartialStep(
            "WinGet still lists upgrade candidates after the update command. \
             The update is incomplete; review the package table above. \
             User pins and packages with unknown versions retain WinGet's default exclusions."
                .into(),
        )
        .into());
    }
    Ok(())
}

fn contains_upgrade_table(output: &[u8]) -> bool {
    let decoded = String::from_utf8_lossy(output);
    let plain = console::strip_ansi_codes(&decoded);
    let mut previous = "";
    for line in plain.lines() {
        let line = line.trim();
        // WinGet's candidate table uses a full-width ASCII separator below its
        // four/five column headings. Detect the layout, not translated headings,
        // warning text, package names, or localized "unknown version" labels.
        if line.len() >= 8 && line.bytes().all(|byte| byte == b'-') && previous.split_whitespace().count() >= 4 {
            return true;
        }
        if !line.is_empty() {
            previous = line;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{
        NO_APPLICATIONS_FOUND, UPDATE_NOT_APPLICABLE, parse_sources, verify_exact_with_run_type, verify_sources,
        verify_with_run_type,
    };
    use crate::error::{PartialStep, TopgradeError};
    use crate::execution_context::RunType;
    use std::os::windows::process::ExitStatusExt;
    use std::path::Path;
    use std::process::{ExitStatus, Output};

    fn output(code: u32, stdout: &str, stderr: &str) -> Output {
        Output {
            status: ExitStatus::from_raw(code),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    fn verify_query_output(output: &Output) -> color_eyre::eyre::Result<()> {
        super::verify_query_output(output, "test source verification", true)
    }

    #[test]
    fn remaining_candidates_are_partial_even_when_the_query_exits_zero() {
        let output = output(
            0,
            "Name Id Version Available Source\n-----------------------------------\nExample Example.Tool 1.0 2.0 winget\n",
            "",
        );
        assert!(
            verify_query_output(&output)
                .unwrap_err()
                .downcast_ref::<PartialStep>()
                .is_some()
        );
    }

    #[test]
    fn translated_headings_and_ansi_do_not_hide_a_remaining_table() {
        for headings in [
            "名称 标识符 版本 可用版本 源",
            "Nom Identifiant Version Disponible Source",
        ] {
            let stdout = format!(
                "\u{1b}[32m{headings}\u{1b}[0m\n-----------------------------------\nExample Example.Tool 1.0 2.0 winget\n"
            );
            assert!(
                verify_query_output(&output(0, &stdout, ""))
                    .unwrap_err()
                    .downcast_ref::<PartialStep>()
                    .is_some()
            );
        }
    }

    #[test]
    fn normal_exclusions_without_candidates_do_not_require_translation() {
        assert!(verify_query_output(&output(0, "未知版本项目按原设置保留。\n用户的版本限制继续生效。\n", "")).is_ok());
    }

    #[test]
    fn documented_empty_query_results_are_not_process_failures() {
        for code in [UPDATE_NOT_APPLICABLE, NO_APPLICATIONS_FOUND] {
            assert!(verify_query_output(&output(code, "", "")).is_ok());
        }
    }

    #[test]
    fn a_failed_query_keeps_the_exit_status_and_diagnostics() {
        let error = verify_query_output(&output(5, "", "source unavailable")).unwrap_err();
        match error.downcast_ref::<TopgradeError>() {
            Some(TopgradeError::ProcessFailedWithOutput(_, status, details)) => {
                assert_eq!(status.code(), Some(5));
                assert!(details.contains("source unavailable"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn dry_run_does_not_execute_the_verification_query() {
        assert!(verify_with_run_type(RunType::Dry, Path::new("nonexistent-winget-verification-fixture.exe")).is_ok());
        assert!(
            verify_exact_with_run_type(
                RunType::Dry,
                Path::new("nonexistent-winget-verification-fixture.exe"),
                "Microsoft.WSL"
            )
            .is_ok()
        );
    }

    #[test]
    fn source_inventory_accepts_json_streams_and_preserves_explicit_scope() {
        let sources =
            parse_sources(b"{\"Name\":\"community\",\"Explicit\":false}\n{\"Name\":\"opt-in\",\"Explicit\":true}\n")
                .unwrap();
        let mut queried = Vec::new();
        verify_sources(&sources, |name| {
            queried.push(name.to_owned());
            Ok(output(0, "", ""))
        })
        .unwrap();
        assert_eq!(queried, ["community"]);
        assert_eq!(parse_sources(b"[{\"Name\":\"community\"}]").unwrap().len(), 1);
    }

    #[test]
    fn checks_all_sources_and_prefers_a_hard_failure_to_incomplete_upgrades() {
        let sources =
            parse_sources(b"[{\"Name\":\"pending\"},{\"Name\":\"unavailable\"},{\"Name\":\"healthy\"}]").unwrap();
        let mut checked = Vec::new();
        let result = verify_sources(&sources, |name| {
            checked.push(name.to_owned());
            Ok(match name {
                "pending" => output(
                    0,
                    "Name Id Version Available\n-----------------------------\nExample Example.Tool 1 2\n",
                    "",
                ),
                "unavailable" => output(5, "", "source failure"),
                _ => output(0, "", ""),
            })
        });
        let error = result.unwrap_err();
        assert!(error.downcast_ref::<TopgradeError>().is_some());
        assert!(error.to_string().contains("unavailable"));
        assert_eq!(checked, ["pending", "unavailable", "healthy"]);
    }

    #[test]
    fn exact_query_requires_an_installed_match() {
        let command = "winget list --id Microsoft.WSL --exact --source winget (verification)";
        let error = super::verify_query_output(&output(NO_APPLICATIONS_FOUND, "", ""), command, false).unwrap_err();
        assert!(error.downcast_ref::<TopgradeError>().is_some());
        assert!(error.to_string().contains("Microsoft.WSL"));
        assert!(super::verify_query_output(&output(0, "", ""), command, false).is_ok());
    }

    #[test]
    fn missing_or_ambiguous_source_inventory_is_not_reported_complete() {
        assert!(parse_sources(b"not JSON").is_err());
        assert!(parse_sources(b"[{\"Name\":\"same\"},{\"Name\":\"SAME\"}]").is_err());
        assert!(parse_sources(b"[{\"Name\":\"\"}]").is_err());
        assert!(verify_sources(&[], |_| panic!("no source may be queried")).is_err());
    }
}
