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
    pub dism: PathBuf,
    pub sfc: PathBuf,
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
            (tools.dism.clone(), args)
        }
        "maintenance" => {
            ensure!(!inspect, "Maintenance detection does not run scans");
            match task.id.as_str() {
                "maintenance-check-health" => (
                    tools.dism.clone(),
                    vec![
                        "/English".into(),
                        "/Online".into(),
                        "/Cleanup-Image".into(),
                        "/CheckHealth".into(),
                        "/NoRestart".into(),
                    ],
                ),
                "maintenance-scan-health" => (
                    tools.dism.clone(),
                    vec![
                        "/English".into(),
                        "/Online".into(),
                        "/Cleanup-Image".into(),
                        "/ScanHealth".into(),
                        "/NoRestart".into(),
                    ],
                ),
                "maintenance-restore-health" => (
                    tools.dism.clone(),
                    vec![
                        "/English".into(),
                        "/Online".into(),
                        "/Cleanup-Image".into(),
                        "/RestoreHealth".into(),
                        "/NoRestart".into(),
                    ],
                ),
                "maintenance-sfc-verify" => (tools.sfc.clone(), vec!["/verifyonly".into()]),
                _ => anyhow::bail!("Unsupported maintenance task"),
            }
        }
        _ => anyhow::bail!("Unsupported deployment kind"),
    };
    Ok(Invocation { program, args })
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
            detail: "尚未运行维护命令；检查状态不会自动启动扫描或修复。".into(),
        });
    }
    let output = runner.run(&invocation(tools, task, true)?, cancel)?;
    complete_output(&output)?;
    let state = if task.kind == "package" {
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
        "Task command failed: {}. Licences/source terms are never auto-accepted; complete any required agreement manually using official WinGet, then inspect again.",
        output.diagnostic()
    );
    Ok(reboot)
}
fn completed_with_reboot(task: &Task, code: u32) -> bool {
    code == 3010 || (task.kind == "package" && code == 0x8a150109)
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
                serde_json::json!({"event":"intent","task_id":task.id,"program":command.program,"args":command.args,"warning":"No automatic rollback. OS servicing and installer descendants may continue after cancellation."}),
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
            dism: "/trusted/dism.exe".into(),
            sfc: "/trusted/sfc.exe".into(),
        }
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
}
