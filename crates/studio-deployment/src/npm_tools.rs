use crate::backup::{self, Package};
#[cfg(any(target_os = "macos", test))]
use anyhow::Context;
use anyhow::ensure;
use serde::{Deserialize, Serialize};
#[cfg(any(target_os = "macos", test))]
use std::{collections::BTreeMap, path::Path, sync::atomic::Ordering};
use std::{
    collections::BTreeSet,
    sync::{Arc, atomic::AtomicBool},
};

#[cfg(target_os = "macos")]
use std::time::Duration;

const MAX_REQUEST_BYTES: usize = 1024 * 1024;

#[cfg(target_os = "macos")]
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30 * 60);
#[cfg(any(target_os = "macos", test))]
const REGISTRY: &str = "https://registry.npmjs.org/";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// Packages must come from a complete, saved Mac inventory.
    pub packages: Vec<Package>,
    /// An empty value restores the recorded versions; a value selects an npm dist-tag.
    pub tag: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageResult {
    pub name: String,
    pub requested: String,
    pub installed: Option<String>,
    pub action: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub requested: usize,
    pub verified: bool,
    pub cancelled: bool,
    pub packages: Vec<PackageResult>,
    pub detail: String,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug)]
struct Cancelled;

#[cfg(any(target_os = "macos", test))]
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("npm global package redeployment was cancelled")
    }
}

#[cfg(any(target_os = "macos", test))]
impl std::error::Error for Cancelled {}

pub fn validate(request: &Request) -> anyhow::Result<()> {
    ensure!(
        !request.packages.is_empty() && request.packages.len() <= backup::MAX_PACKAGES,
        "Select between 1 and {} packages",
        backup::MAX_PACKAGES
    );
    let mut names = BTreeSet::new();
    for package in &request.packages {
        ensure!(
            backup::valid_name("npm", &package.name) && backup::valid_version(&package.version),
            "Invalid npm package identity or version"
        );
        ensure!(
            names.insert(package.name.to_ascii_lowercase()),
            "Duplicate npm package identity"
        );
    }
    if let Some(tag) = &request.tag {
        ensure!(valid_tag(tag), "Invalid npm dist-tag");
    }
    ensure!(
        serde_json::to_vec(request)?.len() <= MAX_REQUEST_BYTES,
        "npm request exceeds 1 MiB"
    );
    Ok(())
}

fn valid_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.len() <= 64
        && tag.as_bytes()[0].is_ascii_alphabetic()
        && !matches!(tag.as_bytes()[0], b'v' | b'V')
        && tag
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

#[cfg(any(target_os = "macos", test))]
fn base_args(cli: &Path, prefix: &Path) -> Vec<String> {
    vec![
        cli.to_string_lossy().into_owned(),
        format!("--prefix={}", prefix.display()),
        format!("--registry={REGISTRY}"),
        "--userconfig=/dev/null".into(),
        "--globalconfig=/dev/null.ltsc-global".into(),
        "--strict-ssl=true".into(),
        "--no-audit".into(),
        "--no-fund".into(),
        "--no-update-notifier".into(),
        "--no-progress".into(),
        "--color=false".into(),
        "--ignore-scripts".into(),
    ]
}

#[cfg(any(target_os = "macos", test))]
fn invocation(node: &Path, args: Vec<String>) -> crate::process::Invocation {
    crate::process::Invocation {
        program: node.to_path_buf(),
        args,
        vscode_node: false,
        npm_node: true,
    }
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy)]
struct NpmPaths<'a> {
    node: &'a Path,
    cli: &'a Path,
    prefix: &'a Path,
}

#[cfg(any(target_os = "macos", test))]
fn step(
    runner: &mut impl crate::process::Runner,
    node: &Path,
    cli: &Path,
    prefix: &Path,
    args: Vec<String>,
    cancel: &Arc<AtomicBool>,
) -> anyhow::Result<crate::process::Output> {
    ensure!(
        node.is_absolute() && cli.is_absolute() && prefix.is_absolute(),
        "npm paths must be absolute"
    );
    if cancel.load(Ordering::Acquire) {
        return Err(Cancelled.into());
    }
    let output = runner.run(&invocation(node, args), cancel)?;
    if output.cancelled || cancel.load(Ordering::Acquire) {
        return Err(Cancelled.into());
    }
    ensure!(
        !output.timed_out,
        "npm command timed out; installed state is unverified"
    );
    ensure!(
        output.code == 0,
        "npm command failed (exit code {}); output was omitted",
        output.code
    );
    Ok(output)
}

#[cfg(any(target_os = "macos", test))]
fn inventory(
    runner: &mut impl crate::process::Runner,
    node: &Path,
    cli: &Path,
    prefix: &Path,
    cancel: &Arc<AtomicBool>,
) -> anyhow::Result<Vec<Package>> {
    let mut args = base_args(cli, prefix);
    args.extend(
        [
            "ls",
            "--global",
            "--json",
            "--depth=0",
            "--long",
            "--offline",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    let output = step(runner, node, cli, prefix, args, cancel)?;
    ensure!(
        output.stderr.trim().is_empty(),
        "npm emitted diagnostics; installed package state is incomplete"
    );
    backup::parse_npm_inventory(&output.stdout, &prefix.join("lib"))
}

#[cfg(any(target_os = "macos", test))]
fn run_recorded(
    runner: &mut impl crate::process::Runner,
    paths: NpmPaths<'_>,
    request: &Request,
    cancel: &Arc<AtomicBool>,
    evidence_path: &Path,
    persist: &mut impl FnMut(&str, &serde_json::Value) -> anyhow::Result<()>,
) -> anyhow::Result<Report> {
    let NpmPaths { node, cli, prefix } = paths;
    validate(request)?;
    let before = inventory(runner, node, cli, prefix, cancel)?;
    persist(
        "before.json",
        &serde_json::json!({
            "schema_version": 1,
            "provider": "npm-global",
            "prefix": prefix,
            "packages": before,
            "scope": "The active Homebrew npm prefix only. Packages and binaries are not byte-identical backups."
        }),
    )
    .context("Persist npm global package baseline before deployment")?;
    persist(
        "request.json",
        &serde_json::json!({
            "schema_version": 1,
            "event": "started",
            "provider": "npm-global",
            "request": request,
            "install_scripts": "disabled",
            "automatic_rollback": false,
            "completion": "Unknown until result.json is present."
        }),
    )
    .context("Persist npm global package request before deployment")?;

    let existing: BTreeMap<_, _> = before
        .iter()
        .map(|package| (package.name.as_str(), package.version.as_str()))
        .collect();
    let deployment = (|| {
        for package in &request.packages {
            if cancel.load(Ordering::Acquire) {
                return Err(Cancelled.into());
            }
            if request.tag.is_none()
                && existing
                    .get(package.name.as_str())
                    .is_some_and(|version| *version == package.version)
            {
                continue;
            }
            let target = format!(
                "{}@{}",
                package.name,
                request.tag.as_deref().unwrap_or(&package.version)
            );
            let mut args = base_args(cli, prefix);
            args.extend(["install", "--global", "--no-save", "--"].map(str::to_owned));
            args.push(target);
            step(runner, node, cli, prefix, args, cancel)
                .with_context(|| format!("Install selected npm package {}", package.name))?;
        }
        let installed = inventory(runner, node, cli, prefix, cancel)?;
        let observed: BTreeMap<_, _> = installed
            .iter()
            .map(|package| (package.name.as_str(), package.version.as_str()))
            .collect();
        for package in &request.packages {
            let version = observed
                .get(package.name.as_str())
                .context("A selected npm package is missing after deployment")?;
            if request.tag.is_none() {
                ensure!(
                    *version == package.version,
                    "Installed npm package version does not match the saved version"
                );
            }
        }
        Ok(installed)
    })();

    let cancelled = deployment
        .as_ref()
        .is_err_and(|error: &anyhow::Error| error.is::<Cancelled>());
    let mut after_error = None;
    let after = match &deployment {
        Ok(packages) => Some(packages.clone()),
        Err(_) => match inventory(runner, node, cli, prefix, &Arc::new(AtomicBool::new(false))) {
            Ok(packages) => Some(packages),
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
                "provider": "npm-global",
                "prefix": prefix,
                "packages": packages
            }),
        )
        .context("Persist npm global package post-deployment inventory")?;
    }
    let observed: BTreeMap<_, _> = after
        .as_ref()
        .into_iter()
        .flatten()
        .map(|package| (package.name.as_str(), package.version.as_str()))
        .collect();
    let packages = request
        .packages
        .iter()
        .map(|package| {
            let installed = observed
                .get(package.name.as_str())
                .map(|version| (*version).to_owned());
            let requested = request
                .tag
                .clone()
                .unwrap_or_else(|| package.version.clone());
            let action = match (&installed, request.tag.is_some()) {
                (None, _) => "not installed",
                (Some(version), false) if version == &package.version => {
                    if existing
                        .get(package.name.as_str())
                        .is_some_and(|before| *before == package.version)
                    {
                        "already satisfied"
                    } else {
                        "installed exact version"
                    }
                }
                (Some(_), true) => "resolved npm dist-tag",
                (Some(_), false) => "version mismatch",
            };
            PackageResult {
                name: package.name.clone(),
                requested,
                installed,
                action: action.into(),
            }
        })
        .collect::<Vec<_>>();
    let mut detail = if deployment.is_ok() {
        if let Some(tag) = &request.tag {
            format!(
                "Selected package identities were read back from the active Homebrew npm prefix after resolving the {tag} dist-tag. "
            )
        } else {
            "Selected package identities and exact saved versions were read back from the active Homebrew npm prefix. ".into()
        }
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
    detail.push_str(
        " npm lifecycle scripts were disabled; packages that require install scripts may not work. No automatic rollback is available.",
    );
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
    .context("Terminal npm evidence could not be persisted; the started record remains unresolved")?;
    if cancelled {
        return Ok(report);
    }
    deployment?;
    Ok(report)
}

#[cfg(target_os = "macos")]
struct Native;

#[cfg(target_os = "macos")]
impl crate::process::Runner for Native {
    fn run(
        &mut self,
        invocation: &crate::process::Invocation,
        cancel: &Arc<AtomicBool>,
    ) -> anyhow::Result<crate::process::Output> {
        crate::process::run_bounded(invocation, cancel, COMMAND_TIMEOUT)
    }
}

pub fn configure(request: Request, cancel: Arc<AtomicBool>) -> anyhow::Result<Report> {
    validate(&request)?;
    #[cfg(target_os = "macos")]
    {
        let (node, cli, prefix) = backup::npm_cli_paths().context(
            "Install Node.js and npm with Homebrew before restoring npm global packages",
        )?;
        let paths = NpmPaths {
            node: &node,
            cli: &cli,
            prefix: &prefix,
        };
        let history = crate::environment::create_history(&studio_platform::data_directory()?)?;
        let profile = crate::network::load_current()?;
        crate::network::with_profile(&profile, || {
            crate::network::record_current(&history.join("network.json"))?;
            let mut runner = Native;
            run_recorded(
                &mut runner,
                paths,
                &request,
                &cancel,
                &history,
                &mut |name, value| studio_core::export_document(value, &history.join(name)),
            )
        })
        .with_context(|| {
            format!(
                "npm redeployment evidence: {}. Selected packages may be partly changed; inspect before retrying.",
                history.display()
            )
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = cancel;
        anyhow::bail!("Native npm global package deployment currently requires macOS")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{Invocation, Output, Runner};
    use std::{collections::VecDeque, sync::atomic::AtomicBool};

    struct FakeRunner {
        outputs: VecDeque<Output>,
        calls: Vec<Invocation>,
    }

    impl Runner for FakeRunner {
        fn run(
            &mut self,
            invocation: &Invocation,
            _cancel: &Arc<AtomicBool>,
        ) -> anyhow::Result<Output> {
            self.calls.push(invocation.clone());
            self.outputs
                .pop_front()
                .context("Unexpected npm command in test")
        }
    }

    fn test_paths(prefix: &Path) -> NpmPaths<'_> {
        NpmPaths {
            node: Path::new(if cfg!(windows) {
                r"C:\tools\node.exe"
            } else {
                "/opt/homebrew/bin/node"
            }),
            cli: Path::new(if cfg!(windows) {
                r"C:\tools\npm-cli.js"
            } else {
                "/opt/homebrew/lib/node_modules/npm/bin/npm-cli.js"
            }),
            prefix,
        }
    }

    fn result(code: u32, stdout: String) -> Output {
        Output {
            code,
            stdout,
            stderr: String::new(),
            cancelled: false,
            timed_out: false,
        }
    }

    fn listing(prefix: &Path, packages: &[(&str, &str)]) -> String {
        let dependencies: serde_json::Map<_, _> = packages
            .iter()
            .map(|(name, version)| {
                (
                    (*name).to_owned(),
                    serde_json::json!({ "version": version }),
                )
            })
            .collect();
        serde_json::json!({
            "path": prefix.join("lib").display().to_string(),
            "dependencies": dependencies,
        })
        .to_string()
    }

    fn request(tag: Option<&str>) -> Request {
        Request {
            packages: vec![Package {
                name: "@scope/tool".into(),
                version: "1.2.3-beta.1".into(),
            }],
            tag: tag.map(str::to_owned),
        }
    }

    #[test]
    fn request_requires_inventory_identities_and_safe_dist_tags() {
        assert!(validate(&request(Some("beta"))).is_ok());
        assert!(validate(&request(Some("nightly-2026-03"))).is_ok());
        assert!(validate(&request(Some("latest"))).is_ok());
        assert!(validate(&request(Some("v1.4"))).is_err());
        assert!(validate(&request(Some("1.4"))).is_err());
        let mut invalid = request(Some("../registry"));
        assert!(validate(&invalid).is_err());
        invalid = request(None);
        invalid.packages[0].name = "@scope/tool;touch".into();
        assert!(validate(&invalid).is_err());
        invalid = request(None);
        invalid.packages.push(invalid.packages[0].clone());
        assert!(validate(&invalid).is_err());
    }

    #[test]
    fn exact_saved_version_skips_matching_packages_and_is_read_back() {
        let prefix = std::env::temp_dir().join("devenv-studio-npm-fixture");
        let prefix = prefix.as_path();
        let inventory = listing(prefix, &[("@scope/tool", "1.2.3-beta.1")]);
        let mut runner = FakeRunner {
            outputs: [result(0, inventory.clone()), result(0, inventory)]
                .into_iter()
                .collect(),
            calls: vec![],
        };
        let mut evidence: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        let report = run_recorded(
            &mut runner,
            test_paths(prefix),
            &request(None),
            &Arc::new(AtomicBool::new(false)),
            &prefix.join("history"),
            &mut |name, value| {
                evidence.insert(name.into(), value.clone());
                Ok(())
            },
        )
        .unwrap();
        assert!(report.verified);
        assert_eq!(runner.calls.len(), 2);
        assert_eq!(evidence["result.json"]["status"], "verified");
        assert_eq!(report.packages[0].action, "already satisfied");
    }

    #[test]
    fn dist_tag_install_is_safe_and_requires_post_install_readback() {
        let prefix = std::env::temp_dir().join("devenv-studio-npm-fixture");
        let prefix = prefix.as_path();
        let before = listing(prefix, &[("@scope/tool", "1.2.3")]);
        let after = listing(prefix, &[("@scope/tool", "2.0.0-beta.1")]);
        let mut runner = FakeRunner {
            outputs: [
                result(0, before),
                result(0, String::new()),
                result(0, after),
            ]
            .into_iter()
            .collect(),
            calls: vec![],
        };
        let report = run_recorded(
            &mut runner,
            test_paths(prefix),
            &request(Some("beta")),
            &Arc::new(AtomicBool::new(false)),
            &prefix.join("history"),
            &mut |_, _| Ok(()),
        )
        .unwrap();
        assert!(report.verified);
        assert_eq!(
            report.packages[0].installed.as_deref(),
            Some("2.0.0-beta.1")
        );
        let args = &runner.calls[1].args;
        assert!(args.iter().any(|arg| arg == "--ignore-scripts"));
        assert!(
            args.iter()
                .any(|arg| arg == "--registry=https://registry.npmjs.org/")
        );
        assert!(args.iter().any(|arg| arg == "@scope/tool@beta"));
        assert!(args.iter().any(|arg| arg == "--"));
    }

    #[test]
    fn failed_install_records_observed_partial_state_and_never_claims_success() {
        let prefix = std::env::temp_dir().join("devenv-studio-npm-fixture");
        let prefix = prefix.as_path();
        let inventory = listing(prefix, &[("@scope/tool", "1.2.3")]);
        let mut runner = FakeRunner {
            outputs: [
                result(0, inventory.clone()),
                result(1, String::new()),
                result(0, inventory),
            ]
            .into_iter()
            .collect(),
            calls: vec![],
        };
        let mut evidence: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        assert!(
            run_recorded(
                &mut runner,
                test_paths(prefix),
                &request(Some("next")),
                &Arc::new(AtomicBool::new(false)),
                &prefix.join("history"),
                &mut |name, value| {
                    evidence.insert(name.into(), value.clone());
                    Ok(())
                },
            )
            .is_err()
        );
        assert_eq!(evidence["result.json"]["status"], "failed");
        assert!(evidence.contains_key("after.json"));
    }
}
