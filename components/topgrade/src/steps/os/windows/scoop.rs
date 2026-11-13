use std::path::Path;

use color_eyre::eyre::{Context, Result, bail};
use serde::Deserialize;

use crate::command::CommandExt;
use crate::error::PartialStep;
use crate::execution_context::ExecutionContext;
use crate::terminal::print_separator;
use crate::utils::require;

// Invoking Scoop's .cmd shim can lose a nested PowerShell script's exit code.
// The entry point and arguments are encoded as literal PowerShell strings below.
const DISPATCH: &str = r"& {
param([string] $ScoopScript, [Parameter(ValueFromRemainingArguments=$true)] [string[]] $ScoopArgs)
$global:LASTEXITCODE = 0
& $ScoopScript @ScoopArgs
$ok = $?
$code = $LASTEXITCODE
if ($code -ne 0) { exit $code }
if (-not $ok) { exit 1 }
exit 0
}";

// `status --local` is a read-only public command. Its success-stream objects
// have invariant property names; only informational host output is suppressed.
const STATUS: &str = r"& {
param([string] $ScoopScript)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false
$global:LASTEXITCODE = 0
$items = @(& $ScoopScript status --local 6>$null)
$ok = $?
$code = $LASTEXITCODE
if ($code -ne 0) { exit $code }
if (-not $ok) { exit 1 }
$rows = @($items | ForEach-Object {
    if (-not $_.PSObject.Properties['Name'] -or -not $_.PSObject.Properties['Info'] -or
        -not $_.PSObject.Properties['Installed Version'] -or -not $_.PSObject.Properties['Latest Version']) {
        throw 'Unexpected Scoop status object; update completion cannot be verified'
    }
    [PSCustomObject]@{
        name = [string]$_.Name
        installed = [string]$_.'Installed Version'
        latest = [string]$_.'Latest Version'
        held = [bool]($_.Info -match '(^|, )Held package(,|$)')
        failed = [bool]($_.Info -match '(^|, )Install failed(,|$)')
        removed = [bool]($_.Info -match '(^|, )Manifest removed(,|$)')
        missing_dependencies = [string]$_.'Missing Dependencies'
    }
})
ConvertTo-Json -InputObject @($rows) -Depth 4 -Compress
}";

pub(super) fn run(ctx: &ExecutionContext) -> Result<()> {
    let scoop = require("scoop")?;
    let script = scoop.with_extension("ps1");
    if !script.is_file() {
        bail!(
            "Cannot verify Scoop's PowerShell entry point beside {}",
            scoop.display()
        );
    }
    let powershell = ctx.require_powershell()?;
    print_separator("Scoop");

    let dispatch = |args: &[&str]| {
        powershell
            .build_command(ctx, &literal_command(DISPATCH, &script, args)?, false)?
            .status_checked()
    };
    dispatch(&["update"])?;
    dispatch(&["update", "*"])?;
    if !ctx.run_type().dry() {
        let status = powershell
            .build_command(ctx, &literal_command(STATUS, &script, &[])?, false)?
            .output_checked_utf8()?;
        eprint!("{}", status.stderr);
        verify_status(&status.stdout)?;
    }
    if ctx.config().cleanup() {
        dispatch(&["cleanup", "*"])?;
        dispatch(&["cache", "rm", "-a"])?;
    }
    Ok(())
}

fn literal_command(body: &str, script: &Path, args: &[&str]) -> Result<String> {
    let path = script
        .to_str()
        .ok_or_else(|| color_eyre::eyre::eyre!("Scoop script path must be valid UTF-8"))?;
    let mut command = body.to_owned();
    for value in std::iter::once(path).chain(args.iter().copied()) {
        command.push_str(" '");
        // PowerShell also recognizes curly single quotes as string delimiters.
        for character in value.chars() {
            if matches!(character, '\'' | '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}') {
                command.push(character);
            }
            command.push(character);
        }
        command.push('\'');
    }
    Ok(command)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PackageStatus {
    name: String,
    installed: String,
    latest: String,
    held: bool,
    failed: bool,
    removed: bool,
    missing_dependencies: String,
}

fn verify_status(json: &str) -> Result<()> {
    let rows: Vec<PackageStatus> = serde_json::from_str(json).wrap_err("Could not verify Scoop status JSON")?;
    let mut unresolved = Vec::new();
    for row in rows {
        if row.name.trim().is_empty() {
            bail!("Scoop status returned a package without a name");
        }
        if row.failed || row.removed || !row.missing_dependencies.trim().is_empty() {
            unresolved.push(format!(
                "{} (failed={}, manifest_removed={}, missing_dependencies={})",
                row.name, row.failed, row.removed, row.missing_dependencies
            ));
        } else if !row.held && !row.latest.is_empty() && row.installed != row.latest {
            unresolved.push(format!("{} ({} -> {})", row.name, row.installed, row.latest));
        }
    }
    if !unresolved.is_empty() {
        return Err(PartialStep(format!(
            "Scoop still reports {} unresolved package(s) after updating: {}",
            unresolved.len(),
            unresolved.join("; ")
        ))
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution_context::RunType;
    use crate::executor::ExecutorOutput;

    #[test]
    fn outdated_packages_after_zero_exit_are_incomplete() {
        let error = verify_status(
            r#"[{"name":"example","installed":"1","latest":"2","held":false,"failed":false,"removed":false,"missing_dependencies":""}]"#,
        )
        .unwrap_err();
        assert!(error.downcast_ref::<PartialStep>().is_some());
        assert!(error.to_string().contains("example (1 -> 2)"));
    }

    #[test]
    fn holds_are_respected_but_do_not_hide_broken_installs() {
        for (failed, expected_ok) in [(false, true), (true, false)] {
            let json = format!(
                r#"[{{"name":"example","installed":"1","latest":"2","held":true,"failed":{failed},"removed":false,"missing_dependencies":""}}]"#
            );
            assert_eq!(verify_status(&json).is_ok(), expected_ok);
        }
        assert!(verify_status("[]").is_ok());
    }

    #[test]
    fn invalid_or_incomplete_status_cannot_report_success() {
        for json in ["", "Everything is ok!", "{}", r#"[{"name":"example"}]"#] {
            assert!(verify_status(json).is_err());
        }
    }

    fn invoke_fixture(run_type: RunType, script: &Path, args: &[&str], command: &str) -> Result<ExecutorOutput> {
        let powershell = require("powershell.exe")?;
        run_type
            .execute(powershell)
            .args(["-NoProfile", "-Command"])
            .arg(literal_command(command, script, args)?)
            .output()
    }

    #[test]
    fn nested_script_exit_code_and_literal_arguments_are_preserved() -> Result<()> {
        let directory = tempfile::Builder::new().prefix("scoop fixture ' ‘ ’ ‚ ‛ ").tempdir()?;
        let script = directory.path().join("fake scoop.ps1");
        std::fs::write(
            &script,
            "if ($args[0] -eq 'update' -and $args[1] -eq '*') { exit 23 }; exit 17",
        )?;
        let ExecutorOutput::Wet(output) = invoke_fixture(RunType::Wet, &script, &["update", "*"], DISPATCH)? else {
            panic!("Fixture did not run");
        };
        assert_eq!(
            output.status.code(),
            Some(23),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn status_serializes_objects_and_suppresses_only_host_information() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let script = directory.path().join("fake scoop.ps1");
        std::fs::write(
            &script,
            "Write-Host 'localized status text'; [PSCustomObject]@{Name='example'; 'Installed Version'='1'; 'Latest Version'='2'; 'Missing Dependencies'=''; Info=''}; exit 0",
        )?;
        let ExecutorOutput::Wet(output) = invoke_fixture(RunType::Wet, &script, &[], STATUS)? else {
            panic!("Fixture did not run");
        };
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let json = String::from_utf8(output.stdout)?;
        assert!(!json.contains("localized status text"));
        assert!(
            verify_status(&json)
                .unwrap_err()
                .downcast_ref::<PartialStep>()
                .is_some()
        );
        Ok(())
    }

    #[test]
    fn dry_run_does_not_execute_update_or_status() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let script = directory.path().join("must not run.ps1");
        for command in [DISPATCH, STATUS] {
            assert!(matches!(
                invoke_fixture(RunType::Dry, &script, &["update", "*"], command)?,
                ExecutorOutput::Dry
            ));
        }
        Ok(())
    }
}
