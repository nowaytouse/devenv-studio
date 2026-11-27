use super::{env_path, field, scoop_root};
use crate::{command::CommandExt as _, execution_context::RunType, executor::Executor};
use color_eyre::eyre::{Context, Result, ensure, eyre};
use reqwest::{blocking::Client, redirect::Policy};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    os::windows::process::CommandExt,
    process::{Child, Command},
    thread,
    time::{Duration, Instant},
};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

const HOST: &str = "127.0.0.1:11434";
struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn api(client: &Client, path: &str) -> Result<Value> {
    Ok(client
        .get(format!("http://{HOST}/api/{path}"))
        .send()?
        .error_for_status()?
        .json()?)
}
fn healthy(client: &Client) -> bool {
    api(client, "version")
        .ok()
        .is_some_and(|v| v["version"].as_str().is_some_and(|s| !s.is_empty()))
}
fn model_names(tags: &Value) -> Result<BTreeSet<String>> {
    let list = tags["models"]
        .as_array()
        .ok_or_else(|| eyre!("Invalid Ollama model inventory"))?;
    let mut names = BTreeSet::new();
    for item in list {
        let name = field(item, "name")?;
        ensure!(
            name.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._:/@+-".contains(&b)),
            "Invalid installed model name"
        );
        names.insert(name.to_owned());
    }
    Ok(names)
}
pub(super) fn models(mode: RunType, check: bool) -> Result<Value> {
    let exe = scoop_root()?.join("apps/ollama/current/ollama.exe");
    ensure!(exe.is_file(), "Missing Scoop Ollama installation");
    let client = Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(2))
        .build()?;
    let logs = env_path("LOCALAPPDATA")?.join("Topgrade");
    fs::create_dir_all(&logs)?;
    let mut journal = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(logs.join("Update-OllamaModels.log"))?;
    let mut server = None;
    let run = format!(
        "OllamaModels-{}-{}",
        chrono::Utc::now().timestamp_millis(),
        std::process::id()
    );
    let logged = |operation: &str| -> Result<Command> {
        let (Executor::Wet(mut cmd) | Executor::Damp(mut cmd)) = mode.execute(&exe) else {
            return Err(eyre!("Unexpected dry-run child process"));
        };
        cmd.env("OLLAMA_HOST", HOST)
            .creation_flags(0x08000000)
            .stdout(fs::File::create_new(
                logs.join(format!("{run}.{operation}.stdout.log")),
            )?)
            .stderr(fs::File::create_new(
                logs.join(format!("{run}.{operation}.stderr.log")),
            )?);
        Ok(cmd)
    };
    if !healthy(&client) {
        let mut sys = System::new()?;
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
        );
        let existing = sys.processes().values().any(|p| {
            p.name().eq_ignore_ascii_case("ollama.exe")
                && (p.cmd().is_empty() || p.cmd().iter().any(|s| s.eq_ignore_ascii_case("serve")))
        });
        if !existing {
            let child = logged("server")?.arg("serve").spawn_checked()?;
            server = Some(Server(child));
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        while !healthy(&client) {
            ensure!(Instant::now() < deadline, "Local Ollama server startup timed out");
            if let Some(Server(child)) = server.as_mut() {
                ensure!(
                    child.try_wait()?.is_none(),
                    "Temporary Ollama server exited before becoming ready"
                );
            }
            thread::sleep(Duration::from_millis(200));
        }
    }
    let names = model_names(&api(&client, "tags")?)?;
    writeln!(journal, "{run}: {} installed models, check_only={check}", names.len())?;
    let mut updated = 0;
    if !check {
        for (index, name) in names.iter().enumerate() {
            if !model_names(&api(&client, "tags")?)?.contains(name) {
                continue;
            }
            let mut child = logged(&format!("pull-{index}"))?
                .args(["pull", name])
                .spawn_checked()
                .wrap_err("Starting Ollama pull")?;
            let rc = child.wait()?;
            writeln!(journal, "{run}: pull-{index} returned {rc}")?;
            ensure!(
                rc.success(),
                "Installed model update failed ({rc}); see local Ollama logs for {run}"
            );
            updated += 1;
        }
    }
    if let Some(Server(child)) = server.as_mut()
        && child.try_wait()?.is_none()
    {
        child.kill()?;
        child.wait()?;
    }
    Ok(json!({"status":"Verified","installed":names.len(),"pulled":updated}))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reject_unexpected_model_names_before_any_pull() {
        assert!(model_names(&json!({"models":[{"name":"family:tag"},{"name":"--remote"}]})).is_err());
        assert_eq!(
            model_names(&json!({"models":[{"name":"family:tag"},{"name":"family:tag"}]}))
                .unwrap()
                .len(),
            1
        );
    }
}
