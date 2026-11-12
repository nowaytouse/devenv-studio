use std::{
    collections::BTreeSet,
    sync::{Arc, atomic::AtomicBool},
};

#[cfg(windows)]
use std::time::Duration;
#[cfg(any(windows, test))]
use std::{collections::BTreeMap, path::Path, sync::atomic::Ordering};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::backup::{self, Package};
#[cfg(any(windows, test))]
use crate::{
    engine::DeveloperTool,
    process::{Invocation, Output, Runner},
};

const MAX_SELECTED: usize = 128;
#[cfg(windows)]
const INVENTORY_TIMEOUT: Duration = Duration::from_secs(60);
#[cfg(windows)]
const INSTALL_TIMEOUT: Duration = Duration::from_secs(900);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub packages: Vec<Package>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageResult {
    pub name: String,
    pub requested: String,
    pub installed: Option<String>,
    pub action: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub requested: usize,
    pub verified: bool,
    pub cancelled: bool,
    pub packages: Vec<PackageResult>,
    pub detail: String,
}

#[cfg(any(windows, test))]
#[derive(Debug)]
struct Cancelled;

#[cfg(any(windows, test))]
impl std::fmt::Display for Cancelled {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Scoop redeployment was cancelled")
    }
}

#[cfg(any(windows, test))]
impl std::error::Error for Cancelled {}

pub fn validate(request: &Request) -> Result<()> {
    ensure!(
        !request.packages.is_empty() && request.packages.len() <= MAX_SELECTED,
        "Select between one and 128 Scoop packages"
    );
    let mut identities = BTreeSet::new();
    for package in &request.packages {
        ensure!(
            backup::scoop::valid_identity(&package.name, &package.version),
            "Invalid Scoop package identity or saved version"
        );
        let (_, app) = package
            .name
            .split_once('/')
            .context("Invalid Scoop package identity")?;
        ensure!(
            identities.insert(app.to_ascii_lowercase()),
            "Duplicate selected Scoop app name"
        );
    }
    Ok(())
}

#[cfg(any(windows, test))]
pub(crate) fn export_invocation(tool: &DeveloperTool) -> Result<Invocation> {
    let script = tool
        .scoop_script
        .as_ref()
        .context("Missing installed Scoop PowerShell script")?;
    ensure!(
        tool.program.is_absolute() && script.is_absolute(),
        "Scoop and PowerShell paths must be absolute"
    );
    Ok(Invocation {
        program: tool.program.clone(),
        args: vec![
            "-NoLogo".into(),
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-File".into(),
            script.to_string_lossy().into_owned(),
            "export".into(),
        ],
        vscode_node: false,
        npm_node: false,
    })
}

#[cfg(any(windows, test))]
fn install_invocation(tool: &DeveloperTool, package: &Package) -> Result<Invocation> {
    validate(&Request {
        packages: vec![package.clone()],
    })?;
    let mut invocation = export_invocation(tool)?;
    let action = invocation
        .args
        .last_mut()
        .context("Missing Scoop export action")?;
    *action = "install".into();
    invocation.args.push("--no-update-scoop".into());
    invocation
        .args
        .push(format!("{}@{}", package.name, package.version));
    Ok(invocation)
}

#[cfg(any(windows, test))]
fn step(
    runner: &mut impl Runner,
    invocation: &Invocation,
    cancel: &Arc<AtomicBool>,
) -> Result<Output> {
    if cancel.load(Ordering::Acquire) {
        return Err(Cancelled.into());
    }
    let output = runner.run(invocation, cancel)?;
    if output.cancelled || cancel.load(Ordering::Acquire) {
        return Err(Cancelled.into());
    }
    ensure!(!output.timed_out, "Scoop command timed out");
    ensure!(output.code == 0, "Scoop command failed");
    Ok(output)
}

#[cfg(any(windows, test))]
fn inventory(
    runner: &mut impl Runner,
    tool: &DeveloperTool,
    cancel: &Arc<AtomicBool>,
) -> Result<backup::scoop::Inventory> {
    let output = step(runner, &export_invocation(tool)?, cancel)?;
    ensure!(
        output.stderr.trim().is_empty(),
        "Scoop export emitted diagnostics; installed package state is incomplete"
    );
    backup::scoop::parse_export(&output.stdout)
}

#[cfg(any(windows, test))]
fn run_recorded(
    runner: &mut impl Runner,
    tool: &DeveloperTool,
    request: &Request,
    cancel: &Arc<AtomicBool>,
    evidence_path: &Path,
    persist: &mut impl FnMut(&str, &serde_json::Value) -> Result<()>,
) -> Result<Report> {
    validate(request)?;
    let before = inventory(runner, tool, cancel)?;
    let existing: BTreeMap<_, _> = before
        .packages
        .iter()
        .map(|package| (package.name.to_ascii_lowercase(), package.version.as_str()))
        .collect();
    for package in &request.packages {
        let (bucket, app) = package
            .name
            .split_once('/')
            .context("Invalid Scoop package identity")?;
        ensure!(
            before.official_buckets.contains(bucket),
            "The official Scoop bucket {bucket} is not registered; it will not be added automatically"
        );
        ensure!(
            !before.packages.iter().any(|installed| {
                installed
                    .name
                    .split_once('/')
                    .is_some_and(|(installed_bucket, installed_app)| {
                        installed_bucket != bucket && installed_app.eq_ignore_ascii_case(app)
                    })
            }),
            "Scoop package {} has a same-name installation from another bucket",
            package.name
        );
        ensure!(
            !before.omitted_apps.contains(&app.to_ascii_lowercase())
                || existing.get(&package.name.to_ascii_lowercase())
                    == Some(&package.version.as_str()),
            "Existing Scoop state prevents reinstalling {}; review its installed source and status",
            package.name
        );
    }
    persist(
        "before.json",
        &serde_json::json!({
            "schema_version": 1,
            "provider": "scoop",
            "packages": &before.packages,
            "scope": "Installed packages from registered official Scoop buckets only; global and custom-source entries are excluded."
        }),
    )
    .context("Persist Scoop package baseline before deployment")?;
    persist(
        "request.json",
        &serde_json::json!({
            "schema_version": 1,
            "event": "started",
            "provider": "scoop",
            "request": request,
            "automatic_bucket_addition": false,
            "no_update_scoop": true,
            "automatic_rollback": false,
            "completion": "Unknown until result.json is present."
        }),
    )
    .context("Persist Scoop package request before deployment")?;

    let deployment = (|| {
        for package in &request.packages {
            if cancel.load(Ordering::Acquire) {
                return Err(Cancelled.into());
            }
            if existing
                .get(&package.name.to_ascii_lowercase())
                .is_some_and(|version| *version == package.version)
            {
                continue;
            }
            let invocation = install_invocation(tool, package)?;
            step(runner, &invocation, cancel)
                .with_context(|| format!("Install exact Scoop package {}", package.name))?;
        }
        let installed = inventory(runner, tool, cancel)?;
        let observed: BTreeMap<_, _> = installed
            .packages
            .iter()
            .map(|package| (package.name.to_ascii_lowercase(), package.version.as_str()))
            .collect();
        for package in &request.packages {
            ensure!(
                observed.get(&package.name.to_ascii_lowercase()) == Some(&package.version.as_str()),
                "Installed Scoop package does not match its saved exact version"
            );
        }
        Ok(installed.packages)
    })();

    let cancelled = deployment
        .as_ref()
        .is_err_and(|error: &anyhow::Error| error.is::<Cancelled>());
    let mut after_error = None;
    let after = match &deployment {
        Ok(packages) => Some(packages.clone()),
        Err(_) => match inventory(runner, tool, &Arc::new(AtomicBool::new(false))) {
            Ok(inventory) => Some(inventory.packages),
            Err(error) => {
                after_error = Some(format!("{error:#}"));
                None
            }
        },
    };
    if let Some(packages) = &after {
        persist(
            "after.json",
            &serde_json::json!({
                "schema_version": 1,
                "provider": "scoop",
                "packages": packages
            }),
        )
        .context("Persist Scoop package post-deployment inventory")?;
    }
    let observed: BTreeMap<_, _> = after
        .as_ref()
        .into_iter()
        .flatten()
        .map(|package| (package.name.to_ascii_lowercase(), package.version.as_str()))
        .collect();
    let packages = request
        .packages
        .iter()
        .map(|package| {
            let identity = package.name.to_ascii_lowercase();
            let installed = observed.get(&identity).map(|version| (*version).to_owned());
            let action = match &installed {
                None => "not installed",
                Some(version) if version == &package.version => {
                    if existing
                        .get(&identity)
                        .is_some_and(|before| *before == package.version)
                    {
                        "already satisfied"
                    } else {
                        "installed exact version"
                    }
                }
                Some(_) => "version mismatch",
            };
            PackageResult {
                name: package.name.clone(),
                requested: package.version.clone(),
                installed,
                action: action.into(),
            }
        })
        .collect::<Vec<_>>();
    let mut detail = if deployment.is_ok() {
        "Selected Scoop package identities and exact saved versions were read back from the registered official buckets. ".into()
    } else {
        format!(
            "Deployment did not finish with a verified package state. Partial changes may remain. {}{}",
            if cancelled {
                "The operation was cancelled. "
            } else {
                ""
            },
            after_error
                .as_deref()
                .map(|error| format!("Post-state read failed: {error}. "))
                .unwrap_or_default()
        )
    };
    detail.push_str("Scoop manifests may run installation scripts and install dependencies. No automatic rollback is available.");
    detail.push_str(&format!(" Evidence: {}.", evidence_path.display()));
    let report = Report {
        requested: request.packages.len(),
        verified: deployment.is_ok(),
        cancelled,
        packages,
        detail,
    };
    persist(
        "result.json",
        &serde_json::json!({
            "schema_version": 1,
            "event": "finished",
            "status": if report.verified { "verified" } else if cancelled { "cancelled" } else { "failed" },
            "report": report,
            "failure": deployment.as_ref().err().map(|error| format!("{error:#}")),
        }),
    )
    .context("Terminal Scoop evidence could not be persisted; the started record remains unresolved")?;
    if cancelled {
        return Ok(report);
    }
    deployment?;
    Ok(report)
}

#[cfg(windows)]
struct Native;

#[cfg(windows)]
impl Runner for Native {
    fn run(&mut self, invocation: &Invocation, cancel: &Arc<AtomicBool>) -> Result<Output> {
        let timeout = if invocation.args.last().is_some_and(|arg| arg == "export") {
            INVENTORY_TIMEOUT
        } else {
            INSTALL_TIMEOUT
        };
        crate::process::run_inventory(invocation, cancel, timeout)
    }
}

pub fn configure(request: Request, cancel: Arc<AtomicBool>) -> Result<Report> {
    validate(&request)?;
    #[cfg(windows)]
    {
        let tool = crate::windows::developer_tool("scoop")?;
        let history = crate::environment::create_history(&studio_platform::data_directory()?)?;
        let profile = crate::network::load_current()?;
        crate::network::with_profile(&profile, || {
            crate::network::record_current(&history.join("network.json"))?;
            let mut runner = Native;
            run_recorded(
                &mut runner,
                &tool,
                &request,
                &cancel,
                &history,
                &mut |name, value| studio_core::export_document(value, &history.join(name)),
            )
        })
        .with_context(|| {
            format!(
                "Scoop redeployment evidence: {}. Selected packages may be partly changed; inspect before retrying.",
                history.display()
            )
        })
    }
    #[cfg(not(windows))]
    {
        let _ = cancel;
        anyhow::bail!("Native Scoop package deployment requires Windows")
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    struct FakeRunner {
        outputs: VecDeque<Output>,
        commands: Vec<Invocation>,
    }

    impl Runner for FakeRunner {
        fn run(&mut self, invocation: &Invocation, _cancel: &Arc<AtomicBool>) -> Result<Output> {
            self.commands.push(invocation.clone());
            self.outputs
                .pop_front()
                .context("Missing fake Scoop command output")
        }
    }

    fn output(stdout: &str, code: u32) -> Output {
        Output {
            code,
            stdout: stdout.into(),
            stderr: String::new(),
            cancelled: false,
            timed_out: false,
        }
    }

    fn export(version: Option<&str>) -> String {
        let apps = version.map_or_else(
            || "[]".into(),
            |version| {
                format!(
                    r#"[{{"Name":"test-tool","Version":"{version}","Source":"main","Info":""}}]"#
                )
            },
        );
        format!(
            r#"{{"buckets":[{{"Name":"main","Source":"https://github.com/ScoopInstaller/Main"}}],"apps":{apps}}}"#
        )
    }

    fn tool() -> DeveloperTool {
        let root = std::env::temp_dir().join("devenv-studio-scoop-test");
        DeveloperTool {
            program: root.join("powershell.exe"),
            cargo_root: None,
            vscode_cli: None,
            npm_cli: None,
            npm_prefix: None,
            scoop_script: Some(root.join("scoop.ps1")),
        }
    }

    fn request() -> Request {
        Request {
            packages: vec![Package {
                name: "main/test-tool".into(),
                version: "1.2.3".into(),
            }],
        }
    }

    #[test]
    fn request_requires_safe_saved_style_identities_and_unique_packages() {
        assert!(validate(&request()).is_ok());
        let mut invalid = request();
        invalid.packages[0].name = "custom/tool".into();
        assert!(validate(&invalid).is_err());
        let mut invalid = request();
        invalid.packages[0].version = "1.2.3;Invoke-Expression".into();
        assert!(validate(&invalid).is_err());
        let mut duplicate = request();
        duplicate.packages.push(duplicate.packages[0].clone());
        assert!(validate(&duplicate).is_err());
        duplicate.packages[1].name = "extras/Test-Tool".into();
        assert!(validate(&duplicate).is_err());
    }

    #[test]
    fn exact_install_uses_only_registered_bucket_and_reads_back_saved_version() {
        let mut runner = FakeRunner {
            outputs: VecDeque::from([
                output(&export(None), 0),
                output("installed", 0),
                output(&export(Some("1.2.3")), 0),
            ]),
            commands: Vec::new(),
        };
        let mut evidence = BTreeMap::new();
        let report = run_recorded(
            &mut runner,
            &tool(),
            &request(),
            &Arc::new(AtomicBool::new(false)),
            Path::new("/evidence"),
            &mut |name, value| {
                evidence.insert(name.to_owned(), value.clone());
                Ok(())
            },
        )
        .unwrap();
        assert!(report.verified);
        assert_eq!(report.packages[0].action, "installed exact version");
        assert!(runner.commands[0].args.contains(&"export".into()));
        assert!(runner.commands[1].args.contains(&"install".into()));
        assert!(
            runner.commands[1]
                .args
                .contains(&"--no-update-scoop".into())
        );
        assert!(
            runner.commands[1]
                .args
                .contains(&"main/test-tool@1.2.3".into())
        );
        assert!(!runner.commands[1].args.contains(&"import".into()));
        assert!(!runner.commands[1].args.contains(&"--global".into()));
        assert!(evidence.contains_key("before.json"));
        assert!(evidence.contains_key("request.json"));
        assert!(evidence.contains_key("after.json"));
        assert!(evidence.contains_key("result.json"));
    }

    #[test]
    fn missing_official_bucket_stops_before_install() {
        let other = r#"{"buckets":[],"apps":[]}"#;
        let mut runner = FakeRunner {
            outputs: VecDeque::from([output(other, 0)]),
            commands: Vec::new(),
        };
        let error = run_recorded(
            &mut runner,
            &tool(),
            &request(),
            &Arc::new(AtomicBool::new(false)),
            Path::new("/evidence"),
            &mut |_, _| Ok(()),
        )
        .unwrap_err();
        assert!(error.to_string().contains("not registered"));
        assert_eq!(runner.commands.len(), 1);
    }

    #[test]
    fn conflicting_same_name_stops_before_installing_any_selected_package() {
        for (entry, expected) in [
            (
                r#"{"Name":"Test-Tool","Version":"1.2.2","Source":"main","Info":"Held package"}"#,
                "prevents reinstalling main/test-tool",
            ),
            (
                r#"{"Name":"test-tool","Version":"1.2.3","Source":"extras","Info":"Global install"}"#,
                "prevents reinstalling main/test-tool",
            ),
            (
                r#"{"Name":"test-tool","Version":"1.2.2","Source":"https://example.invalid/app.json"}"#,
                "prevents reinstalling main/test-tool",
            ),
            (
                r#"{"Name":"test-tool","Version":"1.2.2","Info":"Install failed"}"#,
                "prevents reinstalling main/test-tool",
            ),
            (
                r#"{"Name":"Test-Tool","Version":"1.2.3","Source":"extras"}"#,
                "has a same-name installation from another bucket",
            ),
        ] {
            let before = format!(
                r#"{{"buckets":[{{"Name":"main","Source":"https://github.com/ScoopInstaller/Main"}},{{"Name":"extras","Source":"https://github.com/ScoopInstaller/Extras"}}],"apps":[{entry}]}}"#
            );
            let mut runner = FakeRunner {
                outputs: VecDeque::from([output(&before, 0)]),
                commands: Vec::new(),
            };
            let mut request = request();
            request.packages.insert(
                0,
                Package {
                    name: "main/unrelated".into(),
                    version: "1.0".into(),
                },
            );
            let error = run_recorded(
                &mut runner,
                &tool(),
                &request,
                &Arc::new(AtomicBool::new(false)),
                Path::new("/evidence"),
                &mut |_, _| panic!("preflight must not write deployment records"),
            )
            .unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
            assert_eq!(runner.commands.len(), 1);
            assert_eq!(runner.commands[0].args.last().unwrap(), "export");
        }
    }

    #[test]
    fn verified_exact_install_is_reused_alongside_omitted_same_name() {
        let inventory = r#"{"buckets":[{"Name":"main","Source":"https://github.com/ScoopInstaller/Main"}],"apps":[{"Name":"Test-Tool","Version":"1.2.3","Source":"main"},{"Name":"test-tool","Version":"2.0","Source":"main","Info":"Global install"}]}"#;
        let mut runner = FakeRunner {
            outputs: VecDeque::from([output(inventory, 0), output(inventory, 0)]),
            commands: Vec::new(),
        };
        let report = run_recorded(
            &mut runner,
            &tool(),
            &request(),
            &Arc::new(AtomicBool::new(false)),
            Path::new("/evidence"),
            &mut |_, _| Ok(()),
        )
        .unwrap();
        assert!(report.verified);
        assert_eq!(report.packages[0].action, "already satisfied");
        assert_eq!(runner.commands.len(), 2);
        assert!(
            runner
                .commands
                .iter()
                .all(|command| command.args.last().unwrap() == "export")
        );
    }
}
