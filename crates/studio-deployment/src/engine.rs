use crate::{
    DeploymentEvent, DeploymentPlan, Detection, Task,
    process::{Invocation, Output, Runner},
};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
};

pub(crate) struct Tools {
    pub winget: Option<PathBuf>,
    pub dism: Option<PathBuf>,
    pub sfc: Option<PathBuf>,
    pub defrag: Option<PathBuf>,
    pub system_volume: Option<String>,
    pub developer: fn(&str) -> Result<DeveloperTool>,
}
pub(crate) struct DeveloperTool {
    pub program: PathBuf,
    pub cargo_root: Option<PathBuf>,
    pub vscode_cli: Option<PathBuf>,
    pub npm_cli: Option<PathBuf>,
    pub npm_prefix: Option<PathBuf>,
}
#[derive(Debug, Default)]
pub(crate) struct Outcome {
    pub completed: usize,
    pub failed: usize,
    pub cancelled: bool,
    pub restart_required: bool,
    pub detail: String,
}
pub(crate) trait History {
    fn append(&mut self, value: &serde_json::Value) -> Result<()>;
}
fn log(history: &mut impl History, value: impl Serialize) -> Result<()> {
    history
        .append(&serde_json::to_value(value)?)
        .context("Persist deployment history; no further command may run")
}
fn invocation(tools: &Tools, task: &Task, inspect: bool) -> Result<Invocation> {
    if matches!(task.kind.as_str(), "cargo" | "uv" | "vscode" | "npm") {
        return developer_invocation(tools, task, inspect);
    }
    let (program, args) = match task.kind.as_str() {
        "package" => {
            let id = task.package_id.as_ref().context("Missing package ID")?;
            let mut args = vec![
                if inspect { "list" } else { "install" }.into(),
                "--id".into(),
                id.clone(),
                "--exact".into(),
                "--source".into(),
                "winget".into(),
                "--disable-interactivity".into(),
            ];
            if !inspect {
                args.extend(["--silent".into(), "--no-upgrade".into()]);
            }
            (tools.winget.clone().context("WinGet unavailable. Install or register Microsoft's App Installer using https://learn.microsoft.com/windows/package-manager/winget/ ; this app never bootstraps a package manager")?,args)
        }
        "feature" => {
            let name = task.feature_name.as_ref().context("Missing feature name")?;
            let mut args = vec![
                "/English".into(),
                "/Online".into(),
                if inspect {
                    "/Get-FeatureInfo"
                } else {
                    "/Enable-Feature"
                }
                .into(),
                format!("/FeatureName:{name}"),
            ];
            if !inspect {
                args.extend(["/All".into(), "/NoRestart".into()]);
            }
            (tools.dism.clone().context("DISM is unavailable")?, args)
        }
        "maintenance" => {
            ensure!(!inspect, "Maintenance detection does not run scans");
            match task.id.as_str() {
                "maintenance-check-health" => (
                    tools.dism.clone().context("DISM is unavailable")?,
                    vec![
                        "/English".into(),
                        "/Online".into(),
                        "/Cleanup-Image".into(),
                        "/CheckHealth".into(),
                        "/NoRestart".into(),
                    ],
                ),
                "maintenance-scan-health" => (
                    tools.dism.clone().context("DISM is unavailable")?,
                    vec![
                        "/English".into(),
                        "/Online".into(),
                        "/Cleanup-Image".into(),
                        "/ScanHealth".into(),
                        "/NoRestart".into(),
                    ],
                ),
                "maintenance-restore-health" => (
                    tools.dism.clone().context("DISM is unavailable")?,
                    vec![
                        "/English".into(),
                        "/Online".into(),
                        "/Cleanup-Image".into(),
                        "/RestoreHealth".into(),
                        "/NoRestart".into(),
                    ],
                ),
                "maintenance-sfc-verify" => (
                    tools.sfc.clone().context("SFC is unavailable")?,
                    vec!["/verifyonly".into()],
                ),
                id if crate::storage::is_task(id) => (
                    tools
                        .defrag
                        .clone()
                        .context("Native defrag is unavailable")?,
                    crate::storage::arguments(id, tools.system_volume.as_deref())?,
                ),
                _ => anyhow::bail!("Unsupported maintenance task"),
            }
        }
        _ => anyhow::bail!("Unsupported deployment kind"),
    };
    Ok(Invocation {
        program,
        args,
        vscode_node: false,
        npm_node: false,
    })
}
fn developer_invocation(tools: &Tools, task: &Task, inspect: bool) -> Result<Invocation> {
    let id = task
        .package_id
        .as_deref()
        .context("Missing developer package ID")?;
    ensure!(
        crate::developer_ids::valid(&task.kind, id),
        "Invalid developer package identifier"
    );
    // Resolution is deliberately per task, after any selected WinGet prerequisites.
    let tool = (tools.developer)(&task.kind)?;
    ensure!(
        tool.program.is_absolute(),
        "Developer provider must have an absolute path"
    );
    let args = match task.kind.as_str() {
        "cargo" => {
            let root = tool
                .cargo_root
                .context("Missing fixed Cargo install root")?;
            ensure!(root.is_absolute(), "Cargo install root must be absolute");
            let mut args = vec![
                "install".into(),
                "--root".into(),
                root.to_string_lossy().into_owned(),
            ];
            if inspect {
                args.push("--list".into());
            } else {
                args.extend([
                    "--locked".into(),
                    "--registry".into(),
                    "crates-io".into(),
                    id.into(),
                ]);
            }
            args
        }
        "uv" => {
            let mut args = vec![
                "tool".into(),
                if inspect { "list" } else { "install" }.into(),
                "--no-config".into(),
                "--no-progress".into(),
                "--color".into(),
                "never".into(),
            ];
            if !inspect {
                args.extend(["--no-python-downloads".into(), id.into()]);
            }
            args
        }
        "npm" => {
            let cli = tool.npm_cli.context("Missing official npm-cli.js")?;
            let prefix = tool.npm_prefix.context("Missing fixed user npm prefix")?;
            ensure!(
                cli.is_absolute() && prefix.is_absolute(),
                "npm CLI and user prefix must be absolute"
            );
            let mut args = vec![
                cli.to_string_lossy().into_owned(),
                if inspect { "ls" } else { "install" }.into(),
                "--global".into(),
                "--prefix".into(),
                prefix.to_string_lossy().into_owned(),
                "--registry=https://registry.npmjs.org/".into(),
                "--userconfig=NUL".into(),
                // Distinct null-device spellings avoid npm rejecting duplicate config sources.
                "--globalconfig=NUL.ltsc-global".into(),
                "--no-audit".into(),
                "--no-fund".into(),
                "--no-update-notifier".into(),
                "--no-progress".into(),
                "--color=false".into(),
                "--ignore-scripts".into(),
            ];
            if inspect {
                args.extend(["--json".into(), "--depth=0".into(), "--long".into()]);
            } else {
                args.extend(["--".into(), id.into()]);
            }
            args
        }
        "vscode" => {
            let cli = tool.vscode_cli.context("Missing official VS Code cli.js")?;
            ensure!(
                cli.is_absolute(),
                "VS Code cli.js must have an absolute path"
            );
            let mut args = vec![
                cli.to_string_lossy().into_owned(),
                if inspect {
                    "--list-extensions"
                } else {
                    "--install-extension"
                }
                .into(),
            ];
            if !inspect {
                args.push(id.into());
            }
            args
        }
        _ => anyhow::bail!("Unsupported developer provider"),
    };
    Ok(Invocation {
        program: tool.program,
        args,
        vscode_node: task.kind == "vscode",
        npm_node: task.kind == "npm",
    })
}
fn installed_developer(task: &Task, stdout: &str) -> Result<bool> {
    let wanted = task
        .package_id
        .as_deref()
        .context("Missing developer package ID")?;
    let mut found = false;
    let mut listed_header = false;
    for raw in stdout.lines().filter(|line| !line.trim().is_empty()) {
        if task.kind == "vscode" {
            let id = raw.trim();
            ensure!(
                crate::developer_ids::valid("vscode", id),
                "VS Code returned an unrecognized extension list line"
            );
            found |= id.eq_ignore_ascii_case(wanted);
        } else {
            if raw.starts_with(char::is_whitespace) || (task.kind == "uv" && raw.starts_with('-')) {
                ensure!(
                    listed_header,
                    "Provider listed an executable without its package identity"
                );
                continue;
            }
            let line = raw.trim();
            let Some((id, version)) = line.split_once(" v") else {
                anyhow::bail!("Provider returned an unrecognized installed-tool list line");
            };
            ensure!(
                crate::developer_ids::valid(&task.kind, id)
                    && version.as_bytes().first().is_some_and(u8::is_ascii_digit)
                    && (task.kind != "cargo" || line.ends_with(':')),
                "Malformed installed-tool identity or version"
            );
            listed_header = true;
            // Cargo crate IDs are exact; Python distribution names normalize [-_.].
            found |= if task.kind == "uv" {
                normalize_python(id) == normalize_python(wanted)
            } else {
                id == wanted
            };
        }
    }
    Ok(found)
}
#[derive(serde::Deserialize)]
struct NpmList {
    path: String,
    #[serde(default, deserialize_with = "npm_dependencies")]
    dependencies: std::collections::BTreeMap<String, NpmPackage>,
    #[serde(default)]
    problems: Vec<String>,
    #[serde(default)]
    error: Option<serde_json::Value>,
    #[serde(default)]
    invalid: Option<serde_json::Value>,
    #[serde(default)]
    missing: bool,
}
#[derive(serde::Deserialize)]
struct NpmPackage {
    name: String,
    version: String,
    path: String,
    #[serde(default)]
    resolved: Option<String>,
    #[serde(default)]
    link: bool,
    #[serde(default)]
    problems: Vec<String>,
    #[serde(default)]
    error: Option<serde_json::Value>,
    #[serde(default)]
    invalid: Option<serde_json::Value>,
    #[serde(default)]
    missing: bool,
}
fn npm_dependencies<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<std::collections::BTreeMap<String, NpmPackage>, D::Error> {
    struct UniquePackages;
    impl<'de> serde::de::Visitor<'de> for UniquePackages {
        type Value = std::collections::BTreeMap<String, NpmPackage>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("an object of unique npm package identities")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut map: M,
        ) -> std::result::Result<Self::Value, M::Error> {
            let mut packages = std::collections::BTreeMap::new();
            while let Some((name, package)) = map.next_entry::<String, NpmPackage>()? {
                if packages.insert(name, package).is_some() {
                    return Err(serde::de::Error::custom("Duplicate npm package identity"));
                }
            }
            Ok(packages)
        }
    }
    deserializer.deserialize_map(UniquePackages)
}
fn npm_path_matches(actual: &str, expected: &str) -> bool {
    actual
        .replace('\\', "/")
        .trim_end_matches('/')
        .eq_ignore_ascii_case(expected.replace('\\', "/").trim_end_matches('/'))
}
fn installed_npm(wanted: &str, stdout: &str, prefix: &str) -> Result<bool> {
    let list: NpmList =
        serde_json::from_str(stdout).context("Parse complete npm ls --json --long output")?;
    ensure!(
        npm_path_matches(&list.path, prefix),
        "npm inspected a different global prefix"
    );
    ensure!(
        list.error.is_none() && list.invalid.is_none() && !list.missing && list.problems.is_empty(),
        "npm reported an incomplete or invalid installed-package tree"
    );
    for (id, package) in &list.dependencies {
        ensure!(
            crate::developer_ids::valid("npm", id)
                && package.name == *id
                && !package.version.is_empty()
                && package.version.len() <= 128
                && package.version.as_bytes()[0].is_ascii_digit()
                && package
                    .version
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b".+-".contains(&b))
                && npm_path_matches(
                    &package.path,
                    &format!("{}/node_modules/{id}", prefix.trim_end_matches(['/', '\\']))
                )
                && !package.link
                && !package.resolved.as_deref().is_some_and(|resolved| resolved
                    .starts_with("file:")
                    || resolved.starts_with("link:"))
                && package.error.is_none()
                && package.invalid.is_none()
                && !package.missing
                && package.problems.is_empty(),
            "npm returned an incomplete, invalid or mismatched installed-package identity"
        );
    }
    Ok(list.dependencies.contains_key(wanted))
}
fn normalize_python(value: &str) -> String {
    let mut normalized = String::new();
    for c in value.chars() {
        if matches!(c, '-' | '_' | '.') {
            if !normalized.ends_with('-') {
                normalized.push('-');
            }
        } else {
            normalized.push(c.to_ascii_lowercase());
        }
    }
    normalized
}
fn complete_output(output: &Output) -> Result<()> {
    ensure!(
        !output.cancelled,
        "Command cancelled; operating-system servicing or installer descendants may continue"
    );
    ensure!(
        !output.timed_out,
        "Command timed out; operating-system servicing or installer descendants may continue"
    );
    Ok(())
}
/// DISM /English State field must be a unique anchored field, never an incidental substring.
fn feature_state(stdout: &str) -> Result<&str> {
    let states: Vec<_> = stdout
        .lines()
        .filter_map(|line| line.trim().split_once(':'))
        .filter(|(key, _)| key.trim() == "State")
        .map(|(_, state)| state.trim())
        .collect();
    ensure!(
        states.len() == 1,
        "DISM did not return one unambiguous English State field"
    );
    match states[0] {
        "Enabled" => Ok("enabled"),
        "Enable Pending" | "EnablePending" => Ok("enable_pending"),
        "Disabled" => Ok("disabled"),
        "Disabled with Payload Removed" => Ok("payload_removed"),
        "Disable Pending" | "DisablePending" => Ok("disable_pending"),
        other => anyhow::bail!("Unsupported DISM feature state: {other}"),
    }
}
pub(crate) fn detect(
    task: &Task,
    tools: &Tools,
    runner: &mut impl Runner,
    cancel: &Arc<AtomicBool>,
) -> Result<Detection> {
    if task.kind == "maintenance" {
        return Ok(Detection {
            id: task.id.clone(),
            state: "ready".into(),
            detail: "尚未运行维护命令；检查状态不会自动启动扫描、修复、分析或磁盘优化。".into(),
        });
    }
    let command = invocation(tools, task, true)?;
    let output = runner.run(&command, cancel)?;
    complete_output(&output)?;
    let state = if matches!(task.kind.as_str(), "cargo" | "uv" | "vscode" | "npm") {
        ensure!(
            output.code == 0,
            "Developer provider inspection failed, not an absent tool: {}",
            output.diagnostic()
        );
        let installed = if task.kind == "npm" {
            let prefix = command
                .args
                .windows(2)
                .find(|pair| pair[0] == "--prefix")
                .context("Missing npm inspection prefix")?;
            installed_npm(
                task.package_id
                    .as_deref()
                    .context("Missing npm package ID")?,
                &output.stdout,
                &prefix[1],
            )?
        } else {
            installed_developer(task, &output.stdout)?
        };
        if installed {
            "installed"
        } else {
            "not_installed"
        }
    } else if task.kind == "package" {
        // Microsoft AppInstallerErrors.h: NO_APPLICATIONS_FOUND, not arbitrary nonzero.
        if output.code == 0x8a150014 {
            "not_installed"
        } else {
            ensure!(
                output.code == 0,
                "WinGet list failed (not an absent package): {}",
                output.diagnostic()
            );
            let id = task.package_id.as_deref().context("Missing package ID")?;
            ensure!(
                output
                    .stdout
                    .lines()
                    .any(|line| line.split_whitespace().any(|token| token == id)),
                "WinGet list returned success without the exact package ID; installed state is unverified: {}",
                output.diagnostic()
            );
            "installed"
        }
    } else {
        ensure!(
            output.code == 0,
            "DISM inspection failed: {}",
            output.diagnostic()
        );
        feature_state(&output.stdout)?
    };
    Ok(Detection {
        id: task.id.clone(),
        state: state.into(),
        detail: output.diagnostic(),
    })
}
fn checked_success(task: &Task, output: &Output) -> Result<bool> {
    complete_output(output)?;
    let reboot = completed_with_reboot(task, output.code);
    let already =
        task.kind == "package" && matches!(output.code, 0x8a150061 | 0x8a15010d | 0x8a15002b);
    ensure!(
        output.code == 0 || reboot || already,
        "Task command failed: {}. No installer/provider agreements are automatically accepted; review the provider prerequisite and recorded output before retrying.",
        output.diagnostic()
    );
    Ok(reboot)
}
fn completed_with_reboot(task: &Task, code: u32) -> bool {
    (matches!(task.kind.as_str(), "package" | "feature" | "maintenance")
        && !crate::storage::is_task(&task.id)
        && code == 3010)
        || (task.kind == "package" && code == 0x8a150109)
}
fn any_reboot_required(task: &Task, code: u32) -> bool {
    completed_with_reboot(task, code)
        || (task.kind == "package" && matches!(code, 0x8a15010a | 0x8a15010b))
}
pub(crate) fn run_plan(
    plan: &DeploymentPlan,
    tools: &Tools,
    runner: &mut impl Runner,
    history: &mut impl History,
    cancel: &Arc<AtomicBool>,
    events: Option<&Sender<DeploymentEvent>>,
) -> Result<Outcome> {
    log(
        history,
        serde_json::json!({"event":"plan","schema_version":1,"plan":plan,"rollback":false}),
    )?;
    let mut outcome = Outcome::default();
    let total = plan.tasks.len();
    for task in &plan.tasks {
        if cancel.load(Ordering::Acquire) {
            outcome.cancelled = true;
            break;
        }
        if let Some(events) = events {
            let _ = events.send(DeploymentEvent {
                task_id: task.id.clone(),
                message: format!("检查 {}", task.title),
                completed: outcome.completed,
                total,
            });
        }
        let result = (|| -> Result<(bool, String)> {
            let before = detect(task, tools, runner, cancel)?;
            log(
                history,
                serde_json::json!({"event":"inspection","task_id":task.id,"state":before.state,"detail":before.detail}),
            )?;
            if matches!(
                before.state.as_str(),
                "installed" | "enabled" | "enable_pending"
            ) {
                outcome.restart_required |= before.state == "enable_pending";
                return Ok((
                    before.state == "enable_pending",
                    format!(
                        "{} 已处于 {}，未重新运行安装或启用命令。",
                        task.title, before.state
                    ),
                ));
            }
            ensure!(
                before.state != "disable_pending",
                "Feature disable is pending; restart Windows and inspect again before enabling this feature"
            );
            if cancel.load(Ordering::Acquire) {
                return Ok((false, "cancelled before command".into()));
            }
            let command = invocation(tools, task, false)?;
            log(
                history,
                serde_json::json!({"event":"intent","task_id":task.id,"program":command.program,"args":command.args,"environment":if command.vscode_node {"ELECTRON_RUN_AS_NODE=1; VSCODE_DEV and VSCODE_PORTABLE removed"}else if command.npm_node {"NPM_CONFIG_*, NODE_OPTIONS and NODE_PATH removed; user/global npmrc disabled; fixed user prefix and registry; lifecycle scripts disabled"}else{"inherited"},"warning":"No automatic rollback. OS servicing and installer descendants may continue after cancellation."}),
            )?;
            if cancel.load(Ordering::Acquire) {
                return Ok((false, "cancelled before command".into()));
            }
            if let Some(events) = events {
                let _ = events.send(DeploymentEvent {
                    task_id: task.id.clone(),
                    message: format!("执行 {}；可取消，系统后台操作可能继续", task.title),
                    completed: outcome.completed,
                    total,
                });
            }
            let output = runner
                .run(&command, cancel)
                .context("Run deployment command; task may have partially changed the system")?;
            outcome.restart_required |= any_reboot_required(task, output.code);
            // Preserve captured output even for timeout, cancellation and nonzero exit.
            log(
                history,
                serde_json::json!({"event":"command_result","task_id":task.id,"detail":output.diagnostic()}),
            )?;
            let mut reboot = checked_success(task, &output)?;
            if task.kind == "maintenance" {
                if crate::storage::is_task(&task.id) {
                    return Ok((
                        false,
                        format!(
                            "存储维护命令完成，退出码 {}；请查看 Windows 输出。此结果不代表所有卷均已优化、磁盘健康或性能改善；不提供优化回滚。",
                            output.code
                        ),
                    ));
                }
                return Ok((
                    reboot,
                    format!(
                        "命令完成，退出码 {}。此结果不代表系统无损坏或修复已被验证；请查看本历史的输出及 DISM/CBS 日志。",
                        output.code
                    ),
                ));
            }
            let after = detect(task, tools, runner, cancel).context(
                "Post-command readback failed; installation/feature state is unverified",
            )?;
            log(
                history,
                serde_json::json!({"event":"verification","task_id":task.id,"state":after.state,"detail":after.detail}),
            )?;
            ensure!(
                matches!(
                    after.state.as_str(),
                    "installed" | "enabled" | "enable_pending"
                ),
                "Command exited successfully, but actual result is {}; task remains unverified",
                after.state
            );
            reboot |= after.state == "enable_pending";
            Ok((reboot, format!("{} 已验证：{}", task.title, after.state)))
        })();
        match result {
            Ok((restart, detail)) => {
                if cancel.load(Ordering::Acquire) {
                    outcome.cancelled = true;
                    log(
                        history,
                        serde_json::json!({"event":"cancelled","task_id":task.id,"detail":detail,"partial_state_possible":true}),
                    )?;
                    break;
                }
                outcome.completed += 1;
                outcome.restart_required |= restart;
                outcome.detail = detail.clone();
                log(
                    history,
                    serde_json::json!({"event":"completed","task_id":task.id,"restart_required":restart,"detail":detail}),
                )?;
                if let Some(events) = events {
                    let _ = events.send(DeploymentEvent {
                        task_id: task.id.clone(),
                        message: detail,
                        completed: outcome.completed,
                        total,
                    });
                }
            }
            Err(error) => {
                outcome.cancelled = cancel.load(Ordering::Acquire);
                outcome.failed = usize::from(!outcome.cancelled);
                outcome.detail = format!("{}: {error:#}", task.title);
                log(
                    history,
                    serde_json::json!({"event":if outcome.cancelled {"cancelled"}else{"failed"},"task_id":task.id,"detail":outcome.detail,"partial_state_possible":true}),
                )?;
                if let Some(events) = events {
                    let _ = events.send(DeploymentEvent {
                        task_id: task.id.clone(),
                        message: outcome.detail.clone(),
                        completed: outcome.completed,
                        total,
                    });
                }
                break;
            }
        }
    }
    log(
        history,
        serde_json::json!({"event":"finished","completed":outcome.completed,"failed":outcome.failed,"cancelled":outcome.cancelled,"restart_required":outcome.restart_required}),
    )?;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, VecDeque};
    struct Fake {
        outputs: VecDeque<Result<Output>>,
        calls: Vec<Invocation>,
        cancel: Option<Arc<AtomicBool>>,
    }
    impl Runner for Fake {
        fn run(&mut self, inv: &Invocation, _: &Arc<AtomicBool>) -> Result<Output> {
            self.calls.push(inv.clone());
            if self.calls.len() == 2
                && let Some(cancel) = &self.cancel
            {
                cancel.store(true, Ordering::Release);
            }
            self.outputs.pop_front().context("No fake output")?
        }
    }
    #[derive(Default)]
    struct Log {
        values: Vec<serde_json::Value>,
        fail: Option<usize>,
    }
    impl History for Log {
        fn append(&mut self, value: &serde_json::Value) -> Result<()> {
            ensure!(
                self.fail != Some(self.values.len()),
                "injected history failure"
            );
            self.values.push(value.clone());
            Ok(())
        }
    }
    fn output(code: u32, text: &str) -> Result<Output> {
        Ok(Output {
            code,
            stdout: text.into(),
            stderr: String::new(),
            cancelled: false,
            timed_out: false,
        })
    }
    fn tools() -> Tools {
        Tools {
            winget: Some("/trusted/winget.exe".into()),
            dism: Some("/trusted/dism.exe".into()),
            sfc: Some("/trusted/sfc.exe".into()),
            defrag: Some("/trusted/defrag.exe".into()),
            system_volume: Some(r"\\?\Volume{928842df-5a01-11de-a85c-806e6f6e6963}\".into()),
            developer: fake_developer,
        }
    }
    fn fake_developer(kind: &str) -> Result<DeveloperTool> {
        Ok(DeveloperTool {
            program: format!(
                "/trusted/{}",
                match kind {
                    "vscode" => "Code.exe",
                    "npm" => "node.exe",
                    _ => kind,
                }
            )
            .into(),
            cargo_root: (kind == "cargo").then(|| PathBuf::from("/trusted/.cargo")),
            vscode_cli: (kind == "vscode")
                .then(|| PathBuf::from("/trusted/resources/app/out/cli.js")),
            npm_cli: (kind == "npm")
                .then(|| PathBuf::from("/trusted/node_modules/npm/bin/npm-cli.js")),
            npm_prefix: (kind == "npm").then(|| PathBuf::from("/user/npm")),
        })
    }
    fn plan(kind: &str) -> DeploymentPlan {
        let c = crate::catalog().unwrap();
        let task = c.tasks.iter().find(|t| t.kind == kind).unwrap();
        crate::build_plan(
            studio_core::Target::Win10Ltsc2021,
            &BTreeSet::from([task.id.clone()]),
        )
        .unwrap()
    }
    #[test]
    fn history_failure_prevents_mutation_and_all_commands_if_header_fails() {
        for fail in [0, 2] {
            let p = plan("package");
            let mut fake = Fake {
                outputs: VecDeque::from([output(0x8a150014, "")]),
                calls: vec![],
                cancel: None,
            };
            let mut log = Log {
                fail: Some(fail),
                ..Default::default()
            };
            assert!(
                run_plan(
                    &p,
                    &tools(),
                    &mut fake,
                    &mut log,
                    &Arc::new(AtomicBool::new(false)),
                    None
                )
                .is_err()
            );
            assert!(fake.calls.iter().all(|i| i.args[0] == "list"));
            if fail == 0 {
                assert!(fake.calls.is_empty());
            }
        }
    }
    #[test]
    fn installer_zero_without_readback_is_failure() {
        let p = plan("package");
        let mut fake = Fake {
            outputs: VecDeque::from([
                output(0x8a150014, ""),
                output(0, "installed"),
                output(0x8a150014, ""),
            ]),
            calls: vec![],
            cancel: None,
        };
        let mut log = Log::default();
        let out = run_plan(
            &p,
            &tools(),
            &mut fake,
            &mut log,
            &Arc::new(AtomicBool::new(false)),
            None,
        )
        .unwrap();
        assert_eq!(out.failed, 1);
        assert_eq!(out.completed, 0);
        assert!(log.values.iter().any(|v| v["event"] == "intent"));
    }
    #[test]
    fn successful_package_requires_exact_id_and_never_accepts_terms() {
        let p = plan("package");
        let id = p.tasks[0].package_id.as_ref().unwrap();
        let mut fake = Fake {
            outputs: VecDeque::from([
                output(0x8a150014, ""),
                output(0, "done"),
                output(0, &format!("Name {id} 1.0 winget")),
            ]),
            calls: vec![],
            cancel: None,
        };
        let out = run_plan(
            &p,
            &tools(),
            &mut fake,
            &mut Log::default(),
            &Arc::new(AtomicBool::new(false)),
            None,
        )
        .unwrap();
        assert_eq!(out.completed, 1);
        assert!(
            !fake
                .calls
                .iter()
                .flat_map(|c| &c.args)
                .any(|a| a.starts_with("--accept-"))
        );
    }
    #[test]
    fn detection_failure_is_not_missing() {
        let p = plan("package");
        let mut fake = Fake {
            outputs: VecDeque::from([output(0x8a150046, "source terms")]),
            calls: vec![],
            cancel: None,
        };
        assert!(
            detect(
                &p.tasks[0],
                &tools(),
                &mut fake,
                &Arc::new(AtomicBool::new(false))
            )
            .is_err()
        );
        assert_eq!(fake.calls.len(), 1);
    }
    #[test]
    fn feature_anchored_state_and_pending_verification() {
        assert!(
            feature_state("Description : Enabled feature\nState : Disabled")
                .is_ok_and(|s| s == "disabled")
        );
        assert!(feature_state("Description : Enabled").is_err());
        assert!(feature_state("State : Disabled\nState : Enabled").is_err());
        let p = plan("feature");
        let mut fake = Fake {
            outputs: VecDeque::from([
                output(0, "State : Disabled"),
                output(3010, "done"),
                output(0, "State : Enable Pending"),
            ]),
            calls: vec![],
            cancel: None,
        };
        let out = run_plan(
            &p,
            &tools(),
            &mut fake,
            &mut Log::default(),
            &Arc::new(AtomicBool::new(false)),
            None,
        )
        .unwrap();
        assert_eq!(out.completed, 1);
        assert!(out.restart_required);
    }
    #[test]
    fn failed_verification_preserves_explicit_restart_requirement() {
        let p = plan("package");
        let mut fake = Fake {
            outputs: VecDeque::from([
                output(0x8a150014, ""),
                output(0x8a150109, "restart needed"),
                output(0x8a150014, ""),
            ]),
            calls: vec![],
            cancel: None,
        };
        let out = run_plan(
            &p,
            &tools(),
            &mut fake,
            &mut Log::default(),
            &Arc::new(AtomicBool::new(false)),
            None,
        )
        .unwrap();
        assert_eq!(out.failed, 1);
        assert_eq!(out.completed, 0);
        assert!(out.restart_required);
    }
    #[test]
    fn process_failure_timeout_and_nonzero_stop() {
        for result in [
            Err(anyhow::anyhow!("spawn failed")),
            output(5, "access denied"),
            Ok(Output {
                timed_out: true,
                ..output(0, "").unwrap()
            }),
        ] {
            let p = plan("maintenance");
            let mut fake = Fake {
                outputs: VecDeque::from([result]),
                calls: vec![],
                cancel: None,
            };
            let out = run_plan(
                &p,
                &tools(),
                &mut fake,
                &mut Log::default(),
                &Arc::new(AtomicBool::new(false)),
                None,
            )
            .unwrap();
            assert_eq!(out.failed, 1);
            assert_eq!(out.completed, 0);
        }
    }
    #[test]
    fn cancellation_does_not_claim_completion() {
        let p = plan("package");
        let cancel = Arc::new(AtomicBool::new(false));
        let mut fake = Fake {
            outputs: VecDeque::from([
                output(0x8a150014, ""),
                output(0, "done"),
                output(0, "unverified"),
            ]),
            calls: vec![],
            cancel: Some(cancel.clone()),
        };
        let out = run_plan(&p, &tools(), &mut fake, &mut Log::default(), &cancel, None).unwrap();
        assert!(out.cancelled);
        assert_eq!(out.completed, 0);
    }
    #[test]
    fn unknown_maintenance_and_kinds_are_not_commands() {
        let mut task = plan("maintenance").tasks.remove(0);
        task.id = "maintenance-delete-files".into();
        assert!(invocation(&tools(), &task, false).is_err());
        task.kind = "shell".into();
        assert!(invocation(&tools(), &task, false).is_err());
    }
    #[test]
    fn storage_maintenance_is_opt_in_and_does_not_claim_optimization_verification() {
        for id in [
            "storage-analyze-system",
            "storage-optimize-system",
            "storage-analyze-all",
            "storage-optimize-all",
        ] {
            let mut p = plan("maintenance");
            p.tasks[0].id = id.into();
            p.tasks[0].category = "storage".into();
            let cancel = Arc::new(AtomicBool::new(false));
            let mut fake = Fake {
                outputs: VecDeque::from([output(0, "Windows report")]),
                calls: vec![],
                cancel: None,
            };
            let mut chosen_tools = tools();
            chosen_tools.dism = None;
            chosen_tools.sfc = None;
            let detection = detect(&p.tasks[0], &chosen_tools, &mut fake, &cancel).unwrap();
            assert_eq!(detection.state, "ready");
            assert!(fake.calls.is_empty());
            let mut history = Log::default();
            let out = run_plan(&p, &chosen_tools, &mut fake, &mut history, &cancel, None).unwrap();
            assert_eq!(
                (out.completed, out.failed, out.restart_required),
                (1, 0, false)
            );
            assert_eq!(fake.calls.len(), 1);
            assert_eq!(fake.calls[0].program, PathBuf::from("/trusted/defrag.exe"));
            assert!(
                !fake.calls[0]
                    .args
                    .iter()
                    .any(|arg| matches!(arg.as_str(), "/H" | "/M" | "/D"))
            );
            let intent = history
                .values
                .iter()
                .find(|event| event["event"] == "intent")
                .unwrap();
            assert_eq!(intent["program"], "/trusted/defrag.exe");
            assert_eq!(
                intent["args"],
                serde_json::to_value(&fake.calls[0].args).unwrap()
            );
            assert!(out.detail.contains("不代表"));
            assert!(
                history
                    .values
                    .iter()
                    .all(|event| event["event"] != "verification")
            );
            for result in [
                output(3010, "unexpected status"),
                output(5, "denied"),
                Ok(Output {
                    timed_out: true,
                    ..output(0, "").unwrap()
                }),
                Err(anyhow::anyhow!("incomplete output capture")),
            ] {
                let mut fake = Fake {
                    outputs: VecDeque::from([result]),
                    calls: vec![],
                    cancel: None,
                };
                let out = run_plan(
                    &p,
                    &chosen_tools,
                    &mut fake,
                    &mut Log::default(),
                    &cancel,
                    None,
                )
                .unwrap();
                assert_eq!(
                    (out.completed, out.failed, out.restart_required),
                    (0, 1, false)
                );
            }
            let cancelled = Arc::new(AtomicBool::new(true));
            let mut fake = Fake {
                outputs: VecDeque::new(),
                calls: vec![],
                cancel: None,
            };
            let outcome = run_plan(
                &p,
                &chosen_tools,
                &mut fake,
                &mut Log::default(),
                &cancelled,
                None,
            )
            .unwrap();
            assert!(outcome.cancelled);
            assert_eq!(outcome.completed, 0);
            assert!(fake.calls.is_empty());
        }
    }
    fn developer_task(kind: &str, id: &str) -> Task {
        let mut task = plan("package").tasks.remove(0);
        task.id = format!("test-{kind}");
        task.kind = kind.into();
        task.category = "developer".into();
        task.requires_admin = false;
        task.package_id = Some(id.into());
        task.feature_name = None;
        task
    }
    #[test]
    fn developer_argv_is_direct_exact_and_uses_fixed_vscode_environment() {
        let cargo = invocation(&tools(), &developer_task("cargo", "ripgrep"), false).unwrap();
        assert_eq!(
            cargo.args,
            [
                "install",
                "--root",
                "/trusted/.cargo",
                "--locked",
                "--registry",
                "crates-io",
                "ripgrep"
            ]
        );
        assert!(!cargo.vscode_node);
        let uv = invocation(&tools(), &developer_task("uv", "ruff"), false).unwrap();
        assert_eq!(uv.args.last().unwrap(), "ruff");
        assert!(uv.args.contains(&"--no-python-downloads".into()));
        let vscode = invocation(
            &tools(),
            &developer_task("vscode", "rust-lang.rust-analyzer"),
            false,
        )
        .unwrap();
        assert_eq!(vscode.program, PathBuf::from("/trusted/Code.exe"));
        assert_eq!(
            vscode.args,
            [
                "/trusted/resources/app/out/cli.js",
                "--install-extension",
                "rust-lang.rust-analyzer"
            ]
        );
        assert!(vscode.vscode_node);
        for kind in ["cargo", "uv", "vscode"] {
            assert!(invocation(&tools(), &developer_task(kind, "--version"), false).is_err());
        }
    }
    #[test]
    fn developer_detection_matches_exact_listed_identity_not_substrings() {
        for (kind, id, present, other) in [
            (
                "cargo",
                "ripgrep",
                "ripgrep v14.1.0:\n    rg\n",
                "ripgrep-all v1.0.0:\n    rga\n",
            ),
            (
                "uv",
                "pre-commit",
                "pre_commit v4.0.0\n- pre-commit\n",
                "pre-commit-hooks v5.0.0\n- hook\n",
            ),
            (
                "vscode",
                "ms-python.python",
                "ms-python.python\n",
                "ms-python.python-debugger\n",
            ),
        ] {
            let task = developer_task(kind, id);
            assert!(installed_developer(&task, present).unwrap());
            assert!(!installed_developer(&task, other).unwrap());
            assert!(installed_developer(&task, "installation succeeded").is_err());
            let mut fake = Fake {
                outputs: VecDeque::from([output(5, "provider error")]),
                calls: vec![],
                cancel: None,
            };
            assert!(
                detect(
                    &task,
                    &tools(),
                    &mut fake,
                    &Arc::new(AtomicBool::new(false))
                )
                .is_err()
            );
        }
    }
    #[test]
    fn developer_zero_exit_requires_readback_and_cancellation_is_partial() {
        for kind in ["cargo", "uv", "vscode"] {
            let task = developer_task(
                kind,
                match kind {
                    "cargo" => "ripgrep",
                    "uv" => "ruff",
                    _ => "ms-python.python",
                },
            );
            let plan = DeploymentPlan {
                schema_version: 1,
                catalog_revision: "fixture".into(),
                target: studio_core::Target::Win11Ltsc2024,
                tasks: vec![task],
            };
            let mut fake = Fake {
                outputs: VecDeque::from([output(0, ""), output(0, "done"), output(0, "")]),
                calls: vec![],
                cancel: None,
            };
            let outcome = run_plan(
                &plan,
                &tools(),
                &mut fake,
                &mut Log::default(),
                &Arc::new(AtomicBool::new(false)),
                None,
            )
            .unwrap();
            assert_eq!(outcome.failed, 1);
            assert_eq!(outcome.completed, 0);
            let cancel = Arc::new(AtomicBool::new(false));
            let mut fake = Fake {
                outputs: VecDeque::from([output(0, ""), output(0, "done"), output(0, "")]),
                calls: vec![],
                cancel: Some(cancel.clone()),
            };
            let outcome = run_plan(
                &plan,
                &tools(),
                &mut fake,
                &mut Log::default(),
                &cancel,
                None,
            )
            .unwrap();
            assert!(outcome.cancelled);
            assert_eq!(outcome.completed, 0);
        }
    }
    fn npm_list(id: Option<&str>) -> String {
        let mut list = serde_json::json!({"path":"/user/npm","name":"npm","extraneous":false,"_dependencies":{},"devDependencies":{},"peerDependencies":{}});
        if let Some(id) = id {
            list["dependencies"] = serde_json::json!({id:{"name":id,"version":"1.2.3","path":format!("/user/npm/node_modules/{id}"),"overridden":false}});
        }
        list.to_string()
    }
    #[test]
    fn npm_arguments_use_one_fixed_user_prefix_and_no_lifecycle_scripts() {
        for inspect in [true, false] {
            let command =
                invocation(&tools(), &developer_task("npm", "@scope/tool"), inspect).unwrap();
            assert_eq!(command.program, PathBuf::from("/trusted/node.exe"));
            assert!(command.npm_node && !command.vscode_node);
            assert_eq!(
                &command.args[..14],
                [
                    "/trusted/node_modules/npm/bin/npm-cli.js",
                    if inspect { "ls" } else { "install" },
                    "--global",
                    "--prefix",
                    "/user/npm",
                    "--registry=https://registry.npmjs.org/",
                    "--userconfig=NUL",
                    "--globalconfig=NUL.ltsc-global",
                    "--no-audit",
                    "--no-fund",
                    "--no-update-notifier",
                    "--no-progress",
                    "--color=false",
                    "--ignore-scripts",
                ]
            );
            if inspect {
                assert_eq!(&command.args[14..], ["--json", "--depth=0", "--long"]);
            } else {
                assert_eq!(&command.args[14..], ["--", "@scope/tool"]);
            }
        }
        fn missing(_: &str) -> Result<DeveloperTool> {
            anyhow::bail!("Node/npm files unavailable")
        }
        let mut absent = tools();
        absent.developer = missing;
        let mut fake = Fake {
            outputs: VecDeque::new(),
            calls: vec![],
            cancel: None,
        };
        assert!(
            detect(
                &developer_task("npm", "typescript"),
                &absent,
                &mut fake,
                &Arc::new(AtomicBool::new(false))
            )
            .is_err()
        );
        assert!(fake.calls.is_empty());
    }
    #[test]
    fn npm_json_requires_exact_complete_identity_and_same_prefix() {
        for id in ["typescript", "@scope/tool"] {
            assert!(installed_npm(id, &npm_list(Some(id)), "/user/npm").unwrap());
            assert!(!installed_npm(id, &npm_list(None), "/user/npm").unwrap());
            assert!(!installed_npm(id, &npm_list(Some("other-tool")), "/user/npm").unwrap());
            assert!(installed_npm(id, &npm_list(Some(id)), "/other/npm").is_err());
            let original: serde_json::Value = serde_json::from_str(&npm_list(Some(id))).unwrap();
            for (field, value) in [
                ("name", serde_json::json!("other-tool")),
                ("version", serde_json::json!("")),
                ("version", serde_json::json!("file:../package")),
                (
                    "path",
                    serde_json::json!("/system/nodejs/node_modules/tool"),
                ),
                ("missing", serde_json::json!(true)),
                ("link", serde_json::json!(true)),
                ("resolved", serde_json::json!("file:../other")),
                ("invalid", serde_json::json!(true)),
                ("problems", serde_json::json!(["missing dependency"])),
                ("error", serde_json::json!({"code":"ELSPROBLEMS"})),
            ] {
                let mut list = original.clone();
                list["dependencies"][id][field] = value;
                assert!(
                    installed_npm(id, &list.to_string(), "/user/npm").is_err(),
                    "{field}"
                );
            }
            for field in ["name", "version", "path"] {
                let mut list = original.clone();
                list["dependencies"][id]
                    .as_object_mut()
                    .unwrap()
                    .remove(field);
                assert!(
                    installed_npm(id, &list.to_string(), "/user/npm").is_err(),
                    "{field}"
                );
            }
        }
        for text in [
            "",
            "[]",
            "{}",
            "null",
            r#"{"path":"/user/npm","dependencies":null}"#,
            r#"{"path":"/user/npm","dependencies":[]}"#,
            r#"{"path":"/user/npm","error":{}}"#,
            r#"{"path":"/user/npm","invalid":true}"#,
            r#"{"path":"/user/npm","problems":["invalid"]}"#,
            r#"{"path":"/user/npm","dependencies":{"tool":{},"tool":{}}}"#,
            r#"{"path":"/user/npm","path":"/other/npm"}"#,
        ] {
            assert!(installed_npm("tool", text, "/user/npm").is_err(), "{text}");
        }
        assert!(npm_path_matches(
            r"C:\Users\User\AppData\Roaming\npm",
            "c:/users/user/appdata/roaming/npm/"
        ));
    }
    #[test]
    fn npm_install_zero_exit_requires_readback_and_existing_tools_are_preserved() {
        let task = developer_task("npm", "typescript");
        let plan = DeploymentPlan {
            schema_version: 1,
            catalog_revision: "fixture".into(),
            target: studio_core::Target::Win11Ltsc2024,
            tasks: vec![task],
        };
        for after in [
            npm_list(None),
            "{}".into(),
            r#"{"path":"/user/npm","error":{}}"#.into(),
        ] {
            let mut fake = Fake {
                outputs: VecDeque::from([
                    output(0, &npm_list(None)),
                    output(0, "installed"),
                    output(0, &after),
                ]),
                calls: vec![],
                cancel: None,
            };
            let mut log = Log::default();
            let outcome = run_plan(
                &plan,
                &tools(),
                &mut fake,
                &mut log,
                &Arc::new(AtomicBool::new(false)),
                None,
            )
            .unwrap();
            assert_eq!((outcome.completed, outcome.failed), (0, 1));
            assert_eq!(fake.calls.len(), 3);
            assert!(!log.values.iter().any(|value| value["event"] == "completed"));
        }
        let mut fake = Fake {
            outputs: VecDeque::from([output(0, &npm_list(Some("typescript")))]),
            calls: vec![],
            cancel: None,
        };
        let outcome = run_plan(
            &plan,
            &tools(),
            &mut fake,
            &mut Log::default(),
            &Arc::new(AtomicBool::new(false)),
            None,
        )
        .unwrap();
        assert_eq!((outcome.completed, outcome.failed), (1, 0));
        assert_eq!(fake.calls.len(), 1);
        let mut fake = Fake {
            outputs: VecDeque::from([output(5, &npm_list(Some("typescript")))]),
            calls: vec![],
            cancel: None,
        };
        let outcome = run_plan(
            &plan,
            &tools(),
            &mut fake,
            &mut Log::default(),
            &Arc::new(AtomicBool::new(false)),
            None,
        )
        .unwrap();
        assert_eq!((outcome.completed, outcome.failed), (0, 1));
        assert_eq!(fake.calls.len(), 1);
    }
    #[test]
    fn providers_are_resolved_after_selected_prerequisite_tasks() {
        use std::sync::atomic::AtomicUsize;
        static RESOLVED: AtomicUsize = AtomicUsize::new(0);
        fn late(kind: &str) -> Result<DeveloperTool> {
            RESOLVED.fetch_add(1, Ordering::Relaxed);
            fake_developer(kind)
        }
        let mut tools = tools();
        tools.developer = late;
        let cargo = developer_task("cargo", "ripgrep");
        let mut plan = plan("package");
        let package = plan.tasks[0].package_id.clone().unwrap();
        plan.tasks.push(cargo);
        struct Ordered {
            calls: usize,
            package: String,
        }
        impl Runner for Ordered {
            fn run(&mut self, _: &Invocation, _: &Arc<AtomicBool>) -> Result<Output> {
                self.calls += 1;
                if self.calls == 1 {
                    assert_eq!(RESOLVED.load(Ordering::Relaxed), 0);
                    output(0, &self.package)
                } else {
                    assert!(RESOLVED.load(Ordering::Relaxed) > 0);
                    output(0, "ripgrep v14.1.0:\n    rg\n")
                }
            }
        }
        RESOLVED.store(0, Ordering::Relaxed);
        let outcome = run_plan(
            &plan,
            &tools,
            &mut Ordered { calls: 0, package },
            &mut Log::default(),
            &Arc::new(AtomicBool::new(false)),
            None,
        )
        .unwrap();
        assert_eq!(outcome.completed, 2);
    }
}
