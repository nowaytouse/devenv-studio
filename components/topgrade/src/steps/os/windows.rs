use std::io::IsTerminal;
use std::path::Path;
use std::{ffi::OsStr, process::Command};

use color_eyre::eyre::{Context, Result, bail};
use etcetera::base_strategy::BaseStrategy;
use rust_i18n::t;
use tracing::debug;

use crate::command::CommandExt;
use crate::config::UpdatesAutoReboot;
use crate::execution_context::ExecutionContext;
use crate::step::Step;
use crate::terminal::{print_separator, print_warning};
use crate::utils::{require, which};
use crate::{error::SkipStep, steps::git::RepoStep};

mod scoop;
mod winget;
mod wsl_update;

pub fn run_chocolatey(ctx: &ExecutionContext) -> Result<()> {
    let choco = require("choco")?;
    let yes = ctx.config().yes(Step::Chocolatey);

    print_separator("Chocolatey");

    let sudo = ctx.require_sudo()?;

    sudo.execute(ctx, &choco)?
        .args(["upgrade", "all"])
        .arg_if(yes, "--yes")
        .status_checked()
}

pub fn run_winget(ctx: &ExecutionContext) -> Result<()> {
    let winget = require("winget")?;

    print_separator("WinGet");

    ctx.execute(&winget).args(["source", "update"]).status_checked()?;

    let mut command = if ctx.config().winget_use_sudo() {
        let sudo = ctx.require_sudo()?;
        sudo.execute(ctx, &winget)?
    } else {
        ctx.execute(&winget)
    };

    let mut args = vec!["upgrade", "--all"];
    if ctx.config().winget_silent_install() {
        args.push("--silent");
    }

    command.args(args).status_checked()?;

    winget::verify_after_upgrade(ctx, &winget)
}

pub fn run_scoop(ctx: &ExecutionContext) -> Result<()> {
    scoop::run(ctx)
}

pub fn update_wsl(ctx: &ExecutionContext) -> Result<()> {
    if !is_wsl_installed()? {
        return Err(SkipStep(t!("WSL not installed").to_string()).into());
    }

    let wsl = require("wsl")?;

    print_separator(t!("Update WSL"));

    wsl_update::run(ctx, &wsl)
}

/// Detect if WSL is installed or not.
///
/// For WSL, we cannot simply check if command `wsl` is installed as on newer
/// versions of Windows (since Windows 10 version 2004), this command is
/// installed by default.
///
/// Modern WSL reports its runtime version even without Linux distributions.
/// Older inbox WSL versions lack `--version`, so fall back to their distro list.
fn is_wsl_installed() -> Result<bool> {
    if let Some(wsl) = which("wsl")? {
        // Don't use `output_checked` as an execution failure log is not wanted
        #[expect(clippy::disallowed_methods)]
        let installed = probe_wsl_runtime(|argument| {
            Command::new(&wsl)
                .arg(argument)
                .output()
                .map(|output| output.status.success())
        })?;
        return Ok(installed);
    }

    Ok(false)
}

fn probe_wsl_runtime(mut probe: impl FnMut(&str) -> std::io::Result<bool>) -> std::io::Result<bool> {
    if probe("--version")? {
        return Ok(true);
    }
    probe("-l")
}

fn get_wsl_distributions(ctx: &ExecutionContext, wsl: &Path) -> Result<Vec<String>> {
    let output = ctx
        .execute(wsl)
        .always()
        .args(["--list", "-q"])
        .output_checked_utf8()?
        .stdout;
    Ok(output
        .lines()
        .map(|x| x.replace(['\u{0}', '\r'], "").trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect())
}

fn upgrade_wsl_distribution(wsl: &Path, dist: &str, ctx: &ExecutionContext) -> Result<()> {
    let topgrade = ctx
        .execute(wsl)
        .always()
        .args(["-d", dist, "bash", "-lc", "which topgrade"])
        .output_checked_utf8()
        .map_err(|_| SkipStep(t!("Could not find Topgrade installed in WSL").to_string()))?
        .stdout // The normal output from `which topgrade` appends a newline, so we trim it here.
        .trim_end()
        .to_owned();

    // WSL joins the arguments following the Linux command into a command line,
    // so arguments appended after the `bash -lc` script are not reliably passed
    // through as Bash positional parameters. Keep the complete script in one
    // argument and shell-quote the values that originated outside this function.
    let script = wsl_topgrade_script(dist, &topgrade, ctx.config().verbose(), ctx.config().yes(Step::Wsl));

    ctx.execute(wsl)
        .args(["-d", dist, "bash", "-lc", &script])
        .status_checked()
}

fn wsl_topgrade_script(dist: &str, topgrade: &str, verbose: bool, yes: bool) -> String {
    let mut script = format!(
        "TOPGRADE_PREFIX={} exec {}",
        shell_words::quote(dist),
        shell_words::quote(topgrade)
    );
    if verbose {
        script.push_str(" -v");
    }
    if yes {
        script.push_str(" -y");
    }
    script
}

pub fn run_wsl_topgrade(ctx: &ExecutionContext) -> Result<()> {
    let wsl = require("wsl")?;
    // A runtime can be installed without any distributions. Keep distribution
    // updates separate from the runtime-only WslUpdate step.
    #[expect(clippy::disallowed_methods)]
    let can_list_distributions = Command::new(&wsl).arg("-l").output()?.status.success();
    if !can_list_distributions {
        return Err(SkipStep("Could not list installed WSL distributions".to_owned()).into());
    }
    let wsl_distributions = get_wsl_distributions(ctx, &wsl)?;
    let mut ran = false;
    let mut first_error = None;

    debug!("WSL distributions: {:?}", wsl_distributions);

    for distribution in wsl_distributions {
        let result = upgrade_wsl_distribution(&wsl, &distribution, ctx);
        debug!("Upgrading {:?}: {:?}", distribution, result);
        match result {
            Ok(()) => ran = true,
            Err(e) if e.is::<SkipStep>() => continue,
            Err(e) => {
                if first_error.is_none() {
                    first_error = Some(e);
                }
            }
        }
    }

    if let Some(error) = first_error {
        Err(error)
    } else if ran {
        Ok(())
    } else {
        Err(SkipStep(t!("Could not find Topgrade in any WSL distribution").to_string()).into())
    }
}

pub fn windows_update(ctx: &ExecutionContext) -> Result<()> {
    if windows_update_needs_interactive_selection(
        ctx.config().accept_all_windows_updates(),
        std::io::stdin().is_terminal(),
    ) {
        let reason = t!(
            "Windows Update skipped: manual selection requires an interactive terminal (windows.accept_all_updates = false)"
        );
        print_warning(&reason);
        return Err(SkipStep(reason.to_string()).into());
    }
    let powershell = ctx.require_powershell()?;

    print_separator(t!("Windows Update"));

    if !powershell.has_module(ctx, "PSWindowsUpdate") {
        print_warning(t!(
            "The PSWindowsUpdate PowerShell module isn't installed so Topgrade can't run Windows Update.\nInstall PSWindowsUpdate by running `Install-Module PSWindowsUpdate` in PowerShell."
        ));

        return Err(SkipStep(t!("PSWindowsUpdate is not installed").to_string()).into());
    }

    let cmd = windows_update_command(
        ctx.config().accept_all_windows_updates(),
        ctx.config().windows_updates_auto_reboot(),
        ctx.config().updates_exclude_drivers(),
    );
    powershell.build_command(ctx, &cmd, true)?.status_checked()
}

fn windows_update_needs_interactive_selection(accept_all: bool, interactive: bool) -> bool {
    !accept_all && !interactive
}

fn windows_update_command(accept_all: bool, reboot: UpdatesAutoReboot, exclude_drivers: bool) -> String {
    let mut cmd = "Import-Module PSWindowsUpdate; Install-WindowsUpdate -Verbose".to_owned();
    if exclude_drivers {
        cmd.push_str(" -UpdateType Software");
    }
    if accept_all {
        cmd.push_str(" -AcceptAll");
    }
    match reboot {
        UpdatesAutoReboot::Yes => cmd.push_str(" -AutoReboot"),
        UpdatesAutoReboot::No => cmd.push_str(" -IgnoreReboot"),
        UpdatesAutoReboot::Ask => (), // Prompting is the default for Install-WindowsUpdate
    }
    cmd
}

const STORE_SCAN_COMMAND: &str = r#"$ErrorActionPreference = 'Stop'; (Get-CimInstance -Namespace "Root\cimv2\mdm\dmmap" -ClassName "MDM_EnterpriseModernAppManagement_AppManagement01" | Invoke-CimMethod -MethodName UpdateScanMethod).ReturnValue"#;

fn validate_store_scan_result(output: &str) -> Result<()> {
    let result = output
        .trim()
        .parse::<u32>()
        .wrap_err("Microsoft Store update scan did not return one numeric CIM ReturnValue")?;
    if result != 0 {
        bail!("Microsoft Store update scan failed with CIM ReturnValue {result}");
    }
    Ok(())
}

pub fn microsoft_store(ctx: &ExecutionContext) -> Result<()> {
    if let Some(store) = which("store")? {
        print_separator("Microsoft Store (`store` CLI)");

        return ctx
            .execute(store)
            .arg("updates")
            .arg_if(ctx.config().yes(Step::MicrosoftStore), "--apply")
            .status_checked();
    }

    debug!("`store` not available, falling back to powershell/UpdateScanMethod-based updates");

    let powershell = ctx.require_powershell()?;

    print_separator("Microsoft Store (UpdateScanMethod)");

    println!("{}", t!("Scanning for updates..."));

    // Scan for updates using the MDM UpdateScanMethod
    // This method is also available for non-MDM devices
    let output = powershell
        .build_command(ctx, STORE_SCAN_COMMAND, true)?
        .output_checked_utf8()?;
    validate_store_scan_result(&output.stdout)?;
    println!(
        "{}",
        t!("Microsoft Store asynchronous update scan requested; app upgrades have not been verified")
    );
    Ok(())
}

pub fn reboot(_ctx: &ExecutionContext) -> Result<()> {
    system_shutdown::reboot().map_err(Into::into)
}

pub fn shutdown(_ctx: &ExecutionContext) -> Result<()> {
    system_shutdown::shutdown().map_err(Into::into)
}

pub fn insert_startup_scripts(ctx: &ExecutionContext, git_repos: &mut RepoStep) -> Result<()> {
    let startup_dir = crate::WINDOWS_DIRS
        .data_dir()
        .join("Microsoft\\Windows\\Start Menu\\Programs\\Startup");
    for entry in std::fs::read_dir(&startup_dir)?.flatten() {
        let path = entry.path();
        if path.extension().and_then(OsStr::to_str) == Some("lnk")
            && let Ok(lnk) = parselnk::Lnk::try_from(Path::new(&path))
        {
            debug!("Startup link: {:?}", lnk);
            if let Some(path) = lnk.relative_path() {
                git_repos.insert_if_repo(ctx, startup_dir.join(path));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        probe_wsl_runtime, validate_store_scan_result, windows_update_command,
        windows_update_needs_interactive_selection, wsl_topgrade_script,
    };
    use crate::config::UpdatesAutoReboot;

    #[test]
    fn wsl_topgrade_script_quotes_dynamic_values_and_forwards_flags() {
        assert_eq!(
            wsl_topgrade_script("Distro's name", "/home/user/top grade", true, true),
            "TOPGRADE_PREFIX='Distro'\\''s name' exec '/home/user/top grade' -v -y"
        );
    }

    #[test]
    fn modern_wsl_does_not_require_an_installed_distribution() {
        let mut called = Vec::new();
        assert!(
            probe_wsl_runtime(|argument| {
                called.push(argument.to_owned());
                assert_eq!(argument, "--version", "modern WSL must not probe its empty distro list");
                Ok(true)
            })
            .unwrap()
        );
        assert_eq!(called, ["--version"]);
    }

    #[test]
    fn older_wsl_falls_back_to_its_distro_list() {
        for expected in [false, true] {
            let mut called = Vec::new();
            let installed = probe_wsl_runtime(|argument| {
                called.push(argument.to_owned());
                Ok(argument == "-l" && expected)
            })
            .unwrap();
            assert_eq!(installed, expected);
            assert_eq!(called, ["--version", "-l"]);
        }
        assert!(probe_wsl_runtime(|_| Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))).is_err());
    }

    #[test]
    fn windows_update_filters_preserve_acceptance_and_reboot_options() {
        let original = "Import-Module PSWindowsUpdate; Install-WindowsUpdate -Verbose";
        assert_eq!(
            windows_update_command(true, UpdatesAutoReboot::No, false),
            format!("{original} -AcceptAll -IgnoreReboot")
        );
        assert_eq!(
            windows_update_command(true, UpdatesAutoReboot::No, true),
            format!("{original} -UpdateType Software -AcceptAll -IgnoreReboot")
        );
        assert_eq!(
            windows_update_command(false, UpdatesAutoReboot::Yes, true),
            format!("{original} -UpdateType Software -AutoReboot")
        );
        assert_eq!(
            windows_update_command(false, UpdatesAutoReboot::Ask, true),
            format!("{original} -UpdateType Software")
        );
    }

    #[test]
    fn unattended_windows_update_does_not_wait_for_manual_selection() {
        assert!(windows_update_needs_interactive_selection(false, false));
        assert!(!windows_update_needs_interactive_selection(false, true));
        assert!(!windows_update_needs_interactive_selection(true, false));
        assert!(!windows_update_needs_interactive_selection(true, true));
    }

    #[test]
    fn store_scan_requires_successful_cim_return_value() {
        assert!(validate_store_scan_result("0\r\n").is_ok());
        for output in ["1", "2147749891", "", "0\n0", "not a result"] {
            assert!(
                validate_store_scan_result(output).is_err(),
                "accepted invalid CIM result: {output:?}"
            );
        }
    }
}
