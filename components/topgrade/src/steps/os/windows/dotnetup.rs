use std::fs;
use std::io::Write;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Output;

use color_eyre::eyre::{Context, Result, bail, eyre};
use serde_json::{Value, json};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{FOLDERID_Desktop, KNOWN_FOLDER_FLAG, SHGetKnownFolderPath};

use crate::execution_context::ExecutionContext;
use crate::executor::Executor;
use crate::terminal::print_separator;
use inventory::{validate_inventory, validate_manifest};

mod inventory;
mod recovery;
mod signature;

const ROOT: &str = r"D:\DevTools\DotnetDaily";
const INSTALL: &str = r"D:\DevTools\DotnetDaily\managed";
const MANIFEST: &str = r"D:\DevTools\DotnetDaily\dotnetup-manifest.json";
const REPARSE_POINT: u32 = 0x400;

fn manager() -> Result<PathBuf> {
    Ok(
        PathBuf::from(std::env::var_os("USERPROFILE").ok_or_else(|| eyre!("USERPROFILE is not set"))?)
            .join(r".local\bin\dotnetup.exe"),
    )
}

fn command(ctx: &ExecutionContext, program: &Path) -> Executor {
    let mut command = ctx.execute(program);
    command
        .env("DOTNET_CLI_TELEMETRY_OPTOUT", "1")
        .env("DOTNET_GENERATE_ASPNET_CERTIFICATE", "false")
        .env("DOTNET_NOLOGO", "1")
        .env("DOTNET_ADD_GLOBAL_TOOLS_TO_PATH", "false");
    command
}

fn checked(output: Output) -> Result<String> {
    if !output.status.success() {
        bail!(
            "dotnetup failed ({}): {}{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    if !output.stderr.is_empty() {
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
    }
    String::from_utf8(output.stdout).wrap_err("dotnetup returned invalid UTF-8")
}

fn safe_path(path: &Path, directory: bool) -> Result<()> {
    let metadata = fs::symlink_metadata(path).wrap_err_with(|| format!("Cannot inspect {}", path.display()))?;
    if metadata.file_attributes() & REPARSE_POINT != 0
        || if directory {
            !metadata.is_dir()
        } else {
            !metadata.is_file()
        }
    {
        bail!("Unexpected managed path: {}", path.display());
    }
    Ok(())
}

fn desktop() -> Result<PathBuf> {
    // The shell allocates this path with CoTaskMemAlloc.
    unsafe {
        let path = SHGetKnownFolderPath(&FOLDERID_Desktop, KNOWN_FOLDER_FLAG(0), None)?;
        let value = path.to_string();
        CoTaskMemFree(Some(path.0.cast()));
        Ok(PathBuf::from(value?))
    }
}

fn write_report(report: &Value) -> Result<()> {
    let dir = desktop()?.join(r"设备维护与记录\软件更新日志");
    fs::create_dir_all(&dir).wrap_err("Cannot create daily SDK report directory")?;
    let mut temp = tempfile::NamedTempFile::new_in(&dir).wrap_err("Cannot create daily SDK report")?;
    serde_json::to_writer_pretty(&mut temp, report)?;
    temp.write_all(b"\n")?;
    temp.flush()?;
    temp.persist(dir.join("DotnetDaily-latest.json"))
        .map_err(|e| e.error)
        .wrap_err("Cannot replace daily SDK report")?;
    Ok(())
}

pub(crate) fn daily(ctx: &ExecutionContext, check_only: bool) -> Result<()> {
    print_separator(".NET SDK (daily)");
    if ctx.run_type().dry() {
        println!(
            "Would {} the isolated daily SDK",
            if check_only { "check" } else { "update and verify" }
        );
        return Ok(());
    }
    let install = Path::new(INSTALL);
    let manifest = Path::new(MANIFEST);
    let mut report = json!({"Started": chrono::Local::now().to_rfc3339(), "Success": false, "CheckOnly": check_only, "Owner": "Official dotnetup", "InstallRoot": INSTALL, "Manifest": MANIFEST, "UpdateFeedChecked": false});
    let result = (|| -> Result<()> {
        for dir in [Path::new(r"D:\DevTools"), Path::new(ROOT), install] {
            safe_path(dir, true)?;
        }
        safe_path(manifest, false)?;
        let manager = manager()?;
        if !manager.is_file() {
            bail!("Official dotnetup manager is missing");
        }
        signature::verify_microsoft(&manager).wrap_err("Installed dotnetup manager signature verification failed")?;
        if check_only {
            let bytes = fs::read(manifest).wrap_err("Cannot read dotnetup manifest")?;
            validate_manifest(&bytes, install)?;
        } else {
            let output = recovery::update(&manager, Some((install, manifest)), || {
                Ok(command(ctx, &manager)
                    .args(["sdk", "update", "--install-path"])
                    .arg(install)
                    .arg("--manifest-path")
                    .arg(manifest)
                    .arg("--no-progress")
                    .output()?
                    .unwrap_wet())
            })?;
            let update = checked(output)?;
            print!("{update}");
            report["UpdateOutput"] = json!(update.lines().collect::<Vec<_>>());
            report["UpdateFeedChecked"] = json!(true);
            signature::verify_microsoft(&manager)
                .wrap_err("dotnetup manager signature verification failed after update")?;
        }
        let inventory = checked(
            command(ctx, &manager)
                .args(["list", "--install-path"])
                .arg(install)
                .arg("--manifest-path")
                .arg(manifest)
                .args(["--format", "Json"])
                .output()?
                .unwrap_wet(),
        )?;
        let inventory: Value = serde_json::from_str(&inventory).wrap_err("Invalid dotnetup inventory JSON")?;
        safe_path(&install.join("dotnet.exe"), false)?;
        let version = checked(
            command(ctx, &install.join("dotnet.exe"))
                .current_dir(install)
                .arg("--version")
                .output()?
                .unwrap_wet(),
        )?;
        let version = version.strip_suffix('\n').unwrap_or(&version);
        let version = version.strip_suffix('\r').unwrap_or(version);
        validate_inventory(&inventory, install, version)?;
        let manager_version = checked(command(ctx, &manager).arg("--version").output()?.unwrap_wet())?;
        if manager_version.trim().is_empty() {
            bail!("Verified dotnetup returned an empty version");
        }
        report["SDKVersion"] = json!(version);
        report["ManagerVersion"] = json!(manager_version.trim_end_matches(['\r', '\n']));
        report["ManagerSignature"] = json!("Valid");
        report["SDKHostSignature"] = json!(signature::signature_status(&install.join("dotnet.exe")));
        report["Verification"] = json!(
            "Official dotnetup verifies SHA512 for unsigned daily builds; no Windows policy or certificate checks were disabled."
        );
        report["Status"] = json!(if check_only {
            "HealthyInstalled; latest feed not checked"
        } else {
            "Updated rolling daily requirement"
        });
        report["Success"] = json!(true);
        Ok(())
    })();
    if let Err(error) = &result {
        report["Error"] = json!(format!("{error:#}"));
    }
    report["Completed"] = json!(chrono::Local::now().to_rfc3339());
    let report_result = write_report(&report);
    println!("{}", serde_json::to_string_pretty(&report)?);
    match (result, report_result) {
        (Err(error), Err(report_error)) => {
            Err(error.wrap_err(format!("Additionally failed to write report: {report_error:#}")))
        }
        (Err(error), _) => Err(error),
        (Ok(()), result) => result,
    }
}

pub(crate) fn preview(ctx: &ExecutionContext) -> Result<()> {
    print_separator("dotnetup (preview)");
    if ctx.run_type().dry() {
        println!("Would update and verify dotnetup preview");
        return Ok(());
    }
    let manager = manager()?;
    if !manager.is_file() {
        bail!("Expected official dotnetup preview manager is missing");
    }
    signature::verify_microsoft(&manager).wrap_err("Installed dotnetup manager signature verification failed")?;
    let output = recovery::update(&manager, None, || {
        Ok(command(ctx, &manager)
            .args(["self", "update", "--channel", "preview", "--no-progress"])
            .output()?
            .unwrap_wet())
    })?;
    print!("{}", checked(output)?);
    signature::verify_microsoft(&manager).wrap_err("Updated dotnetup manager signature verification failed")?;
    let version = checked(command(ctx, &manager).arg("--version").output()?.unwrap_wet())
        .wrap_err("Updated dotnetup verification failed")?;
    if version.trim().is_empty() {
        bail!("Updated dotnetup returned an empty version");
    }
    print!("{version}");
    Ok(())
}
