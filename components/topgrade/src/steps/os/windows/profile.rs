//! Opt-in Windows preview channels previously maintained by standalone launchers.
use super::dotnetup::desktop;
use crate::{execution_context::ExecutionContext, step::Step, terminal::print_separator};
use color_eyre::eyre::{Context, Result, ensure, eyre};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

mod creative;
mod legacy;
mod ollama;
mod python;
mod scoop;

const LOG: &str = r"设备维护与记录\软件更新日志";
const FRONTIER: &str = r"设备维护与记录\前沿生态改进-20261115";

pub(crate) fn name(step: Step) -> &'static str {
    match step {
        Step::WindowsKritaNext => "Krita Next",
        Step::WindowsPowerToysPreview => "PowerToys preview",
        Step::WindowsPythonPreview => "Python preview (uv)",
        Step::WindowsPythonLauncher => "Python launcher registrations",
        Step::WindowsScoopRolling => "Scoop rolling runtimes",
        Step::WindowsNushellPlugins => "Nushell bundled plugins",
        Step::WindowsOllamaModels => "Local Ollama models",
        _ => unreachable!(),
    }
}

pub(crate) fn run(step: Step, ctx: &ExecutionContext) -> Result<()> {
    print_separator(name(step));
    if ctx.run_type().dry() {
        println!("Would inspect the installed {} profile", name(step));
        return Ok(());
    }
    let mode = ctx.run_type();
    let check = ctx.config().windows_profile_check_only();
    let dir = env_path("LOCALAPPDATA")?.join("Topgrade/locks");
    fs::create_dir_all(&dir)?;
    // Python preview verifies the same registration as the launcher step.
    let lock_name = if step == Step::WindowsPythonPreview {
        Step::WindowsPythonLauncher
    } else {
        step
    };
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join(format!("{lock_name:?}.lock")))?;
    lock.try_lock()
        .wrap_err("Another Windows profile operation is active")?;
    let (outcome, receipt) = match step {
        Step::WindowsKritaNext => (
            creative::krita(mode, check),
            Path::new(LOG).join("KritaNext-latest.json"),
        ),
        Step::WindowsPowerToysPreview => (
            creative::powertoys(mode, check),
            Path::new(FRONTIER).join("PowerToys/最近更新状态.json"),
        ),
        Step::WindowsPythonPreview => (
            python::preview(mode, check),
            Path::new(FRONTIER).join("Python/current-preview-state.json"),
        ),
        Step::WindowsPythonLauncher => (
            python::launcher(mode, check),
            Path::new(LOG).join("PythonLauncher-latest.json"),
        ),
        Step::WindowsScoopRolling => (
            scoop::rolling(mode, check),
            Path::new(LOG).join("ScoopRolling-latest.json"),
        ),
        Step::WindowsNushellPlugins => (
            scoop::plugins(mode, check),
            Path::new(LOG).join("NushellPlugins-latest.json"),
        ),
        Step::WindowsOllamaModels => (
            ollama::models(mode, check),
            Path::new(LOG).join("OllamaModels-latest.json"),
        ),
        _ => unreachable!(),
    };
    let report = match &outcome {
        Ok(value) => value.clone(),
        Err(err) => json!({"status":"Error","error":format!("{err:#}")}),
    };
    let mut report = report;
    report["check_only"] = json!(check);
    report["checked"] = json!(chrono::Utc::now().to_rfc3339());
    report["success"] = json!(outcome.is_ok());
    write_report(&desktop()?.join(receipt), &report)?;
    outcome?;
    println!("{}", field(&report, "status")?);
    Ok(())
}

fn write_report(path: &Path, value: &Value) -> Result<()> {
    let dir = path.parent().ok_or_else(|| eyre!("Missing report directory"))?;
    fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer_pretty(&mut tmp, value)?;
    tmp.write_all(b"\n")?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}
fn env_path(key: &str) -> Result<PathBuf> {
    let p = PathBuf::from(env::var_os(key).ok_or_else(|| eyre!("{key} is missing"))?);
    ensure!(p.is_absolute(), "{key} must be an absolute path");
    Ok(p)
}
fn scoop_root() -> Result<PathBuf> {
    if env::var_os("SCOOP").is_some() {
        env_path("SCOOP")
    } else {
        Ok(env_path("USERPROFILE")?.join("scoop"))
    }
}
fn sha(path: &Path) -> Result<String> {
    let mut state = Sha256::new();
    io::copy(&mut fs::File::open(path)?, &mut state)?;
    Ok(format!("{:x}", state.finalize()))
}
fn output(cmd: &mut crate::executor::Executor) -> Result<String> {
    use crate::command::CommandExt;
    let result = cmd
        .output_checked()
        .wrap_err_with(|| format!("Starting {:?}", cmd.get_program()))?;

    Ok(String::from_utf8(result.stdout)?.trim().to_owned())
}
fn read_json(path: &Path) -> Result<Value> {
    Ok(serde_json::from_reader(
        fs::File::open(path).wrap_err_with(|| format!("Reading {}", path.display()))?,
    )?)
}
fn field<'a>(data: &'a Value, name: &str) -> Result<&'a str> {
    data[name].as_str().ok_or_else(|| eyre!("Missing string field {name}"))
}
