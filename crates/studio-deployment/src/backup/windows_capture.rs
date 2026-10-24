use super::*;
use crate::{
    engine::DeveloperTool,
    process::{Invocation, Output, Runner},
};
use std::sync::atomic::Ordering;

#[cfg(windows)]
struct Native;
#[cfg(windows)]
impl Runner for Native {
    fn run(&mut self, invocation: &Invocation, cancel: &Arc<AtomicBool>) -> Result<Output> {
        crate::process::run_inventory(invocation, cancel, std::time::Duration::from_secs(60))
    }
}

#[cfg(windows)]
pub(super) fn collect(cancel: Arc<AtomicBool>) -> Result<Snapshot> {
    let captured_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("System clock precedes Unix epoch")?
        .as_secs()
        .to_string();
    collect_with(
        &mut Native,
        crate::windows::developer_tool,
        crate::windows::pip_layout,
        cancel,
        captured_at,
    )
}

fn invocation(kind: &str, tool: &DeveloperTool) -> Result<Invocation> {
    ensure!(
        tool.program.is_absolute(),
        "Provider executable must be absolute"
    );
    if kind == "scoop" {
        return crate::scoop_tools::export_invocation(tool);
    }
    let args = match kind {
        "cargo" => {
            let root = tool
                .cargo_root
                .as_ref()
                .context("Missing fixed Cargo root")?;
            ensure!(root.is_absolute(), "Cargo root must be absolute");
            vec![
                "install".into(),
                "--list".into(),
                "--root".into(),
                root.to_string_lossy().into_owned(),
            ]
        }
        "uv" => [
            "tool",
            "list",
            "--no-config",
            "--no-progress",
            "--color",
            "never",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        "npm" => {
            let cli = tool.npm_cli.as_ref().context("Missing official npm CLI")?;
            let prefix = tool
                .npm_prefix
                .as_ref()
                .context("Missing fixed npm prefix")?;
            ensure!(
                cli.is_absolute() && prefix.is_absolute(),
                "npm paths must be absolute"
            );
            vec![
                cli.to_string_lossy().into_owned(),
                "ls".into(),
                "--global".into(),
                "--prefix".into(),
                prefix.to_string_lossy().into_owned(),
                "--registry=https://registry.npmjs.org/".into(),
                "--userconfig=NUL".into(),
                "--globalconfig=NUL.ltsc-global".into(),
                "--no-audit".into(),
                "--no-fund".into(),
                "--no-update-notifier".into(),
                "--no-progress".into(),
                "--color=false".into(),
                "--ignore-scripts".into(),
                "--offline".into(),
                "--json".into(),
                "--depth=0".into(),
                "--long".into(),
            ]
        }
        "vscode" => {
            let cli = tool
                .vscode_cli
                .as_ref()
                .context("Missing official VS Code CLI")?;
            ensure!(cli.is_absolute(), "VS Code CLI must be absolute");
            vec![
                cli.to_string_lossy().into_owned(),
                "--list-extensions".into(),
                "--show-versions".into(),
            ]
        }
        _ => anyhow::bail!("Unknown inventory provider"),
    };
    Ok(Invocation {
        program: tool.program.clone(),
        args,
        vscode_node: kind == "vscode",
        npm_node: kind == "npm",
    })
}

fn packages(kind: &str, text: &str, tool: &DeveloperTool) -> Result<(Vec<Package>, bool)> {
    ensure!(
        text.len() <= 1024 * 1024 && !text.contains('\u{fffd}'),
        "Incomplete command capture"
    );
    let (packages, omitted) = if kind == "scoop" {
        let inventory = super::scoop::parse_export(text)?;
        (inventory.packages, inventory.omitted)
    } else if kind == "npm" {
        let prefix = tool
            .npm_prefix
            .as_ref()
            .context("Missing fixed npm prefix")?;
        (
            crate::engine::npm_inventory(text, &prefix.to_string_lossy())?
                .into_iter()
                .map(|(name, version)| Package { name, version })
                .collect(),
            false,
        )
    } else {
        let (packages, omitted) = super::capture::parse_packages(kind, text, None)?;
        ensure!(!omitted, "Incomplete inventory");
        (packages, false)
    };
    ensure!(
        packages.len() <= MAX_PACKAGES,
        "Inventory exceeds package count limit"
    );
    for package in &packages {
        let valid = if kind == "scoop" {
            super::scoop::valid_identity(&package.name, &package.version)
        } else {
            valid_name(kind, &package.name) && valid_version(&package.version)
        };
        ensure!(valid, "Invalid package identity or version");
    }
    Ok((packages, omitted))
}

fn collect_with(
    runner: &mut impl Runner,
    mut developer_tool: impl FnMut(&str) -> Result<DeveloperTool>,
    mut pip_layout: impl FnMut() -> Result<crate::pip::Layout>,
    cancel: Arc<AtomicBool>,
    captured_at: String,
) -> Result<Snapshot> {
    let mut providers = Vec::new();
    for kind in WINDOWS_PROVIDERS {
        ensure!(
            !cancel.load(Ordering::Acquire),
            "Developer environment collection was cancelled"
        );
        let mut provider = ProviderSnapshot {
            kind: kind.into(),
            status: "failed".into(),
            packages: vec![],
            detail: CAPTURE_FAILED.into(),
        };
        if kind == "pip" {
            match pip_layout() {
                Err(error) if error.is::<MissingProvider>() => {
                    provider.status = "unavailable".into();
                    provider.detail = UNAVAILABLE.into();
                }
                Err(_) => {}
                Ok(layout) => {
                    if let Ok(packages) = crate::pip::inventory(layout, runner, &cancel) {
                        provider.packages = packages;
                        provider.status = "complete".into();
                        provider.detail = COMPLETE.into();
                    }
                }
            }
        } else {
            match developer_tool(kind) {
                Err(error) if error.is::<MissingProvider>() => {
                    provider.status = "unavailable".into();
                    provider.detail = UNAVAILABLE.into();
                }
                Err(_) => {}
                Ok(tool) => {
                    let command = invocation(kind, &tool)?;
                    match runner.run(&command, &cancel) {
                        Err(_) => {}
                        Ok(output) => {
                            ensure!(
                                !output.cancelled && !cancel.load(Ordering::Acquire),
                                "Developer environment collection was cancelled"
                            );
                            if output.timed_out {
                                provider.detail = TIMED_OUT.into();
                            } else if output.code != 0 {
                                provider.detail = FAILED.into();
                            } else if !output.stderr.trim().is_empty() {
                                provider.detail = INVALID.into();
                            } else {
                                match packages(kind, &output.stdout, &tool) {
                                    Err(_) => provider.detail = INVALID.into(),
                                    Ok((packages, omitted)) => {
                                        let has_packages = !packages.is_empty();
                                        provider.packages = packages;
                                        provider.status = if omitted && has_packages {
                                            "partial".into()
                                        } else if omitted {
                                            "failed".into()
                                        } else {
                                            "complete".into()
                                        };
                                        provider.detail = if omitted {
                                            OMITTED.into()
                                        } else {
                                            COMPLETE.into()
                                        };
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        ensure!(
            !cancel.load(Ordering::Acquire),
            "Developer environment collection was cancelled"
        );
        providers.push(provider);
    }
    let mut warnings = vec![INVENTORY.into(), LOCATIONS.into(), PYTHON_SCOPE.into()];
    if providers
        .iter()
        .any(|provider| provider.status != "complete")
    {
        warnings.push(PARTIAL.into());
    }
    let snapshot = Snapshot {
        schema_version: 2,
        captured_at,
        platform: "windows".into(),
        providers,
        warnings,
    };
    validate(&snapshot)?;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct Fake {
        outputs: VecDeque<Result<Output>>,
        commands: Vec<Invocation>,
        cancel_at: Option<usize>,
    }
    impl Runner for Fake {
        fn run(&mut self, invocation: &Invocation, cancel: &Arc<AtomicBool>) -> Result<Output> {
            self.commands.push(invocation.clone());
            if self.cancel_at == Some(self.commands.len()) {
                cancel.store(true, Ordering::Release);
            }
            self.outputs.pop_front().expect("unexpected command")
        }
    }
    fn output(text: &str) -> Result<Output> {
        Ok(Output {
            code: 0,
            stdout: text.into(),
            stderr: String::new(),
            timed_out: false,
            cancelled: false,
        })
    }
    fn fake(outputs: Vec<Result<Output>>) -> Fake {
        Fake {
            outputs: outputs.into(),
            commands: vec![],
            cancel_at: None,
        }
    }
    fn tool(kind: &str) -> Result<DeveloperTool> {
        let root = std::env::temp_dir().join("studio-fixed-provider");
        Ok(DeveloperTool {
            program: root.join(if kind == "vscode" {
                "Code.exe"
            } else {
                "provider.exe"
            }),
            cargo_root: Some(root.join("cargo")),
            vscode_cli: Some(root.join("cli.js")),
            npm_cli: Some(root.join("npm-cli.js")),
            npm_prefix: Some(root.join("npm")),
            scoop_script: (kind == "scoop").then(|| root.join("scoop.ps1")),
        })
    }
    fn missing_pip() -> Result<crate::pip::Layout> {
        Err(MissingProvider.into())
    }
    fn npm_fixture() -> String {
        let prefix = tool("npm").unwrap().npm_prefix.unwrap();
        serde_json::json!({"path": prefix, "dependencies": {"typescript": {"name":"typescript","version":"5.9.0","path":prefix.join("node_modules/typescript")}}}).to_string()
    }

    #[test]
    fn windows_snapshot_and_fixed_readonly_commands_are_complete_and_private() {
        let scoop_export = r#"{"buckets":[{"Name":"main","Source":"https://github.com/ScoopInstaller/Main"}],"apps":[{"Name":"7zip","Version":"24.09","Source":"main","Info":""}]}"#;
        let mut fake = fake(vec![
            output("ripgrep v15.0.0:\n    rg.exe\n"),
            output("ruff v0.14.0\n- ruff.exe\n"),
            output(&npm_fixture()),
            output("ms-python.python@2026.1.0\n"),
            output(scoop_export),
        ]);
        let snapshot = collect_with(
            &mut fake,
            tool,
            missing_pip,
            Arc::new(AtomicBool::new(false)),
            "1792720000".into(),
        )
        .unwrap();
        assert_eq!(snapshot.schema_version, 2);
        assert_eq!(snapshot.platform, "windows");
        assert_eq!(snapshot.providers.len(), 6);
        assert_eq!(
            snapshot
                .providers
                .iter()
                .filter(|provider| provider.status == "complete")
                .count(),
            5
        );
        assert_eq!(snapshot.providers[3].status, "unavailable");
        assert_eq!(snapshot.providers[0].packages[0].name, "ripgrep");
        assert_eq!(snapshot.providers[5].packages[0].name, "main/7zip");
        let serialized = serde_json::to_string(&snapshot).unwrap();
        assert!(!serialized.contains("studio-fixed-provider"));
        for command in fake.commands {
            assert!(command.program.is_absolute());
            assert!(!command.args.iter().any(|arg| matches!(
                arg.as_str(),
                "--install-extension" | "--locked" | "--upgrade" | "bootstrap"
            )));
            if command.npm_node {
                assert!(command.args.contains(&"--offline".into()));
                assert!(command.args.contains(&"--ignore-scripts".into()));
            }
            if command.vscode_node {
                assert!(command.args.contains(&"--show-versions".into()));
            }
            if command.args.contains(&"export".into()) {
                assert!(!command.args.contains(&"--config".into()));
            }
        }
    }

    #[test]
    fn windows_collection_includes_only_verified_pip_metadata_and_no_probe_paths() {
        let identity = crate::pip::tests::identity().to_string();
        let scoop_export = r#"{"buckets":[{"Name":"main","Source":"https://github.com/ScoopInstaller/Main"}],"apps":[{"Name":"7zip","Version":"24.09","Source":"main","Info":""}]}"#;
        let mut runner = fake(vec![
            output(""),
            output(""),
            output(&npm_fixture()),
            output(&identity),
            output(r#"[{"name":"PyWavelets","version":"1.9.0"}]"#),
            output(&identity),
            output(""),
            output(scoop_export),
        ]);
        let snapshot = collect_with(
            &mut runner,
            tool,
            || Ok(crate::pip::tests::layout()),
            Arc::new(AtomicBool::new(false)),
            "1".into(),
        )
        .unwrap();
        assert!(
            snapshot
                .providers
                .iter()
                .all(|provider| provider.status == "complete")
        );
        assert_eq!(snapshot.providers[3].packages[0].name, "pywavelets");
        assert_eq!(snapshot.warnings.len(), 3);
        assert_eq!(runner.commands.len(), 8);
        let text = serde_json::to_string(&snapshot).unwrap();
        assert!(!text.contains("Python314"));
        assert!(!text.contains("pip_origin"));
        assert!(!text.contains("studio-fixed-provider"));
        let mut runner = fake(vec![
            output(""),
            output(""),
            output(&npm_fixture()),
            output(&identity),
            output("{}"),
            output(""),
            output(scoop_export),
        ]);
        let snapshot = collect_with(
            &mut runner,
            tool,
            || Ok(crate::pip::tests::layout()),
            Arc::new(AtomicBool::new(false)),
            "1".into(),
        )
        .unwrap();
        assert_eq!(snapshot.providers[3].status, "failed");
        assert!(snapshot.providers[3].packages.is_empty());
    }

    #[test]
    fn incomplete_or_invalid_output_never_becomes_an_empty_complete_backup() {
        for bad in [
            "{}",
            r#"{"path":"wrong","dependencies":{}}"#,
            r#"{"path":"wrong","dependencies":{"pkg":{"name":"pkg","version":"1.0.0","path":"wrong"}}}"#,
        ] {
            assert!(packages("npm", bad, &tool("npm").unwrap()).is_err());
        }
        for (kind, text) in [
            ("cargo", "    orphan.exe\n"),
            ("uv", "- orphan.exe\n"),
            ("cargo", "pkg v1.0.0 (/private/project):\n"),
            ("vscode", "publisher.extension@1.0.0\n/private/secret"),
            ("uv", "friendly-bard v1.0\nfriendly_bard v2.0\n"),
        ] {
            assert!(
                packages(kind, text, &tool(kind).unwrap()).is_err(),
                "{kind}: {text}"
            );
        }
        let mut invalid = serde_json::from_str::<serde_json::Value>(&npm_fixture()).unwrap();
        invalid["dependencies"]["typescript"]["link"] = true.into();
        assert!(packages("npm", &invalid.to_string(), &tool("npm").unwrap()).is_err());
        let mut runner = fake(vec![
            output(""),
            output(""),
            output("{}"),
            output(""),
            output(
                r#"{"buckets":[{"Name":"main","Source":"https://github.com/ScoopInstaller/Main"}],"apps":[]}"#,
            ),
        ]);
        let snapshot = collect_with(
            &mut runner,
            tool,
            missing_pip,
            Arc::new(AtomicBool::new(false)),
            "1".into(),
        )
        .unwrap();
        assert_eq!(snapshot.providers[2].status, "failed");
        assert_eq!(snapshot.providers[2].detail, INVALID);
        assert!(snapshot.providers[2].packages.is_empty());
    }

    #[test]
    fn process_failures_stderr_and_timeouts_never_report_complete() {
        let scoop_export = r#"{"buckets":[{"Name":"main","Source":"https://github.com/ScoopInstaller/Main"}],"apps":[{"Name":"7zip","Version":"24.09","Source":"main","Info":""}]}"#;
        for failure in [
            Output {
                code: 1,
                stdout: String::new(),
                stderr: String::new(),
                cancelled: false,
                timed_out: false,
            },
            Output {
                code: 0,
                stdout: String::new(),
                stderr: "partial metadata /private/path".into(),
                cancelled: false,
                timed_out: false,
            },
            Output {
                code: 0,
                stdout: String::new(),
                stderr: String::new(),
                cancelled: false,
                timed_out: true,
            },
        ] {
            let mut runner = fake(vec![
                Ok(failure),
                output(""),
                output(&npm_fixture()),
                output(""),
                output(scoop_export),
            ]);
            let snapshot = collect_with(
                &mut runner,
                tool,
                missing_pip,
                Arc::new(AtomicBool::new(false)),
                "1".into(),
            )
            .unwrap();
            assert_eq!(snapshot.providers[0].status, "failed");
            assert!(snapshot.providers[0].packages.is_empty());
            assert!(
                !serde_json::to_string(&snapshot)
                    .unwrap()
                    .contains("/private/path")
            );
        }
    }

    #[test]
    fn discovery_failure_and_unavailable_are_distinct_and_cancellation_returns_no_snapshot() {
        let mut runner = fake(vec![]);
        let unavailable = collect_with(
            &mut runner,
            |_| Err(MissingProvider.into()),
            missing_pip,
            Arc::new(AtomicBool::new(false)),
            "1".into(),
        )
        .unwrap();
        assert!(
            unavailable
                .providers
                .iter()
                .all(|p| p.status == "unavailable")
        );
        assert!(runner.commands.is_empty());
        let failed = collect_with(
            &mut runner,
            |_| anyhow::bail!("permission denied /private/token"),
            || anyhow::bail!("invalid Python layout /private/path"),
            Arc::new(AtomicBool::new(false)),
            "1".into(),
        )
        .unwrap();
        assert!(failed.providers.iter().all(|p| p.status == "failed"));
        assert!(!serde_json::to_string(&failed).unwrap().contains("/private"));
        assert!(
            collect_with(
                &mut runner,
                tool,
                missing_pip,
                Arc::new(AtomicBool::new(true)),
                "1".into()
            )
            .is_err()
        );
        let mut runner = fake(vec![output("")]);
        runner.cancel_at = Some(1);
        assert!(
            collect_with(
                &mut runner,
                tool,
                missing_pip,
                Arc::new(AtomicBool::new(false)),
                "1".into()
            )
            .is_err()
        );
        assert_eq!(runner.commands.len(), 1);
    }
}
