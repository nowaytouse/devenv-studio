use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};

const MAX_REQUEST_BYTES: u64 = 64 * 1024;
const MAX_EXTENSIONS: usize = 256;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(20 * 60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionRequest {
    pub id: String,
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub schema_version: u32,
    pub extensions: Vec<ExtensionRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionResult {
    pub id: String,
    pub requested_version: Option<String>,
    pub installed_version: Option<String>,
    pub action: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub requested: usize,
    pub verified: bool,
    pub cancelled: bool,
    pub extensions: Vec<ExtensionResult>,
    pub detail: String,
}

#[derive(Debug)]
struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VS Code extension deployment was cancelled")
    }
}

impl std::error::Error for Cancelled {}

pub fn validate(request: &Request) -> anyhow::Result<()> {
    ensure!(
        request.schema_version == 1,
        "Unsupported VS Code request schema"
    );
    ensure!(
        !request.extensions.is_empty() && request.extensions.len() <= MAX_EXTENSIONS,
        "Select 1 to 256 VS Code extensions"
    );
    let mut ids = BTreeSet::new();
    for extension in &request.extensions {
        ensure!(
            crate::developer_ids::valid("vscode", &extension.id),
            "Invalid VS Code extension identity"
        );
        ensure!(
            ids.insert(extension.id.as_str()),
            "Duplicate VS Code extension identity"
        );
        if let Some(version) = &extension.version {
            ensure!(
                valid_version(version),
                "Invalid exact VS Code extension version"
            );
        }
    }
    Ok(())
}

fn valid_version(version: &str) -> bool {
    version.len() <= 128
        && version
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && version
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
        && version.bytes().any(|byte| byte.is_ascii_digit())
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".+-".contains(&byte))
}

pub fn save(request: &Request, path: &Path) -> anyhow::Result<()> {
    validate(request)?;
    studio_core::export_document(request, path)
}

pub fn load(path: &Path) -> anyhow::Result<Request> {
    let metadata = fs::symlink_metadata(path).context("Inspect VS Code extension request")?;
    ensure!(
        metadata.file_type().is_file(),
        "VS Code request must be a regular file"
    );
    ensure!(
        metadata.len() <= MAX_REQUEST_BYTES,
        "VS Code request exceeds the 64 KiB limit"
    );
    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .context("Open VS Code extension request")?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_REQUEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("Read VS Code extension request")?;
    ensure!(
        bytes.len() as u64 <= MAX_REQUEST_BYTES,
        "VS Code request exceeds the 64 KiB limit"
    );
    let request: Request =
        serde_json::from_slice(&bytes).context("Parse VS Code extension request")?;
    validate(&request)?;
    Ok(request)
}

pub fn parse_inventory(text: &str) -> anyhow::Result<Vec<crate::backup::Package>> {
    let mut packages = BTreeMap::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let (id, version) = line
            .rsplit_once('@')
            .context("VS Code returned an extension without a version")?;
        ensure!(
            crate::developer_ids::valid("vscode", id) && valid_version(version),
            "VS Code returned an invalid extension identity or version"
        );
        ensure!(
            packages.insert(id.to_owned(), version.to_owned()).is_none(),
            "VS Code returned a duplicate extension identity"
        );
    }
    Ok(packages
        .into_iter()
        .map(|(name, version)| crate::backup::Package { name, version })
        .collect())
}

fn invocation(electron: &Path, cli: &Path, args: &[&str]) -> crate::process::Invocation {
    let mut full_args = vec![cli.to_string_lossy().into_owned()];
    full_args.extend(args.iter().map(|arg| (*arg).to_owned()));
    crate::process::Invocation {
        program: electron.to_path_buf(),
        args: full_args,
        vscode_node: true,
        npm_node: false,
    }
}

fn step(
    runner: &mut impl crate::process::Runner,
    electron: &Path,
    cli: &Path,
    args: &[&str],
    cancel: &Arc<AtomicBool>,
) -> anyhow::Result<crate::process::Output> {
    ensure!(
        electron.is_absolute() && cli.is_absolute(),
        "VS Code paths must be absolute"
    );
    if cancel.load(Ordering::Acquire) {
        return Err(Cancelled.into());
    }
    let output = runner.run(&invocation(electron, cli, args), cancel)?;
    if output.cancelled || cancel.load(Ordering::Acquire) {
        return Err(Cancelled.into());
    }
    ensure!(
        !output.timed_out,
        "VS Code command timed out; installed state is unverified"
    );
    ensure!(
        output.code == 0,
        "VS Code command failed (exit code {}); output was omitted",
        output.code
    );
    Ok(output)
}

fn inventory(
    runner: &mut impl crate::process::Runner,
    electron: &Path,
    cli: &Path,
    cancel: &Arc<AtomicBool>,
) -> anyhow::Result<Vec<crate::backup::Package>> {
    let output = step(
        runner,
        electron,
        cli,
        &["--list-extensions", "--show-versions"],
        cancel,
    )?;
    parse_inventory(&output.stdout)
}

fn run_recorded(
    runner: &mut impl crate::process::Runner,
    electron: &Path,
    cli: &Path,
    request: &Request,
    cancel: &Arc<AtomicBool>,
    evidence_path: &Path,
    persist: &mut impl FnMut(&str, &serde_json::Value) -> anyhow::Result<()>,
) -> anyhow::Result<Report> {
    validate(request)?;
    let before = inventory(runner, electron, cli, cancel)?;
    persist(
        "before.json",
        &serde_json::json!({
            "schema_version": 1,
            "provider": "vscode-default-profile",
            "extensions": before,
            "scope": "The default stable Visual Studio Code user profile only. Extension packages and settings are not byte-identical machine backups."
        }),
    )
    .context("Persist VS Code extension baseline before deployment")?;
    persist(
        "request.json",
        &serde_json::json!({
            "schema_version": 1,
            "event": "started",
            "provider": "vscode-default-profile",
            "request": request,
            "automatic_rollback": false,
            "completion": "Unknown until result.json is present."
        }),
    )
    .context("Persist VS Code extension request before deployment")?;

    let existing: BTreeMap<_, _> = before
        .iter()
        .map(|package| (package.name.as_str(), package.version.as_str()))
        .collect();
    let deployment = (|| {
        for extension in &request.extensions {
            if cancel.load(Ordering::Acquire) {
                return Err(Cancelled.into());
            }
            if extension.version.as_deref().is_some_and(|version| {
                existing
                    .get(extension.id.as_str())
                    .is_some_and(|installed| *installed == version)
            }) {
                continue;
            }
            let target = extension
                .version
                .as_ref()
                .map(|version| format!("{}@{version}", extension.id))
                .unwrap_or_else(|| extension.id.clone());
            step(
                runner,
                electron,
                cli,
                &["--install-extension", &target, "--force"],
                cancel,
            )
            .with_context(|| format!("Install selected VS Code extension {}", extension.id))?;
        }
        let installed = inventory(runner, electron, cli, cancel)?;
        let observed: BTreeMap<_, _> = installed
            .iter()
            .map(|package| (package.name.as_str(), package.version.as_str()))
            .collect();
        for extension in &request.extensions {
            let version = observed
                .get(extension.id.as_str())
                .context("A selected VS Code extension is missing after deployment")?;
            if let Some(requested) = &extension.version {
                ensure!(
                    *version == requested,
                    "Installed VS Code extension version does not match the exact request"
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
        Err(_) => match inventory(runner, electron, cli, &Arc::new(AtomicBool::new(false))) {
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
                "provider": "vscode-default-profile",
                "extensions": packages
            }),
        )
        .context("Persist VS Code extension post-deployment inventory")?;
    }
    let observed: BTreeMap<_, _> = after
        .as_ref()
        .into_iter()
        .flatten()
        .map(|package| (package.name.as_str(), package.version.as_str()))
        .collect();
    let extensions = request
        .extensions
        .iter()
        .map(|extension| {
            let installed_version = observed
                .get(extension.id.as_str())
                .map(|value| (*value).to_owned());
            let action = match (&extension.version, &installed_version) {
                (Some(requested), Some(installed)) if requested == installed => {
                    if existing
                        .get(extension.id.as_str())
                        .is_some_and(|value| *value == requested)
                    {
                        "already satisfied"
                    } else {
                        "installed exact version"
                    }
                }
                (Some(_), Some(_)) => "version mismatch",
                (Some(_), None) | (None, None) => "not installed",
                (None, Some(_)) => "installed Marketplace latest",
            };
            ExtensionResult {
                id: extension.id.clone(),
                requested_version: extension.version.clone(),
                installed_version,
                action: action.into(),
            }
        })
        .collect::<Vec<_>>();
    let mut detail = if deployment.is_ok() {
        "Selected extension identities were read back from the default stable Visual Studio Code profile; exact versions matched, while rolling requests resolved through the configured Marketplace at run time. Extensions run with VS Code's privileges. No automatic rollback is available.".to_owned()
    } else {
        format!(
            "Deployment did not finish with a verified extension state. Partial changes may remain. {}{}No automatic rollback is available.",
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
    detail.push_str(&format!(" Evidence: {}", evidence_path.display()));
    let report = Report {
        requested: request.extensions.len(),
        verified: deployment.is_ok(),
        cancelled,
        extensions,
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
    .context("Terminal VS Code evidence could not be persisted; the started record remains unresolved")?;
    if cancelled {
        return Ok(report);
    }
    deployment?;
    Ok(report)
}

#[cfg(target_os = "macos")]
pub fn configure(request: Request, cancel: Arc<AtomicBool>) -> anyhow::Result<Report> {
    validate(&request)?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("macOS home directory is unavailable")?;
    ensure!(
        home.is_absolute() && home.is_dir(),
        "macOS home directory is invalid"
    );
    let (electron, cli) = crate::backup::vscode_cli_paths(&home)
        .context("Install stable Visual Studio Code in /Applications or ~/Applications first")?;
    let history = crate::environment::create_history(&studio_platform::data_directory()?)?;
    crate::network::record_current(&history.join("network.json"))?;
    let mut runner = Native;
    run_recorded(
        &mut runner,
        &electron,
        &cli,
        &request,
        &cancel,
        &history,
        &mut |name, value| studio_core::export_document(value, &history.join(name)),
    )
    .with_context(|| {
        format!(
            "VS Code extension deployment evidence: {}. Selected extensions may be partly changed; inspect before retrying.",
            history.display()
        )
    })
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

#[cfg(not(target_os = "macos"))]
pub fn configure(_request: Request, _cancel: Arc<AtomicBool>) -> anyhow::Result<Report> {
    anyhow::bail!("Native VS Code extension deployment currently requires macOS")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn request(extensions: Vec<ExtensionRequest>) -> Request {
        Request {
            schema_version: 1,
            extensions,
        }
    }

    fn extension(id: &str, version: Option<&str>) -> ExtensionRequest {
        ExtensionRequest {
            id: id.into(),
            version: version.map(str::to_owned),
        }
    }

    #[test]
    fn requests_are_opt_in_unique_and_accept_exact_prereleases() {
        assert!(
            validate(&request(vec![
                extension("rust-lang.rust-analyzer", Some("0.4.2100-beta.1")),
                extension("ms-python.python", None),
            ]))
            .is_ok()
        );
        assert!(validate(&request(Vec::new())).is_err());
        assert!(
            validate(&request(vec![
                extension("publisher.extension", None),
                extension("publisher.extension", None),
            ]))
            .is_err()
        );
        assert!(
            validate(&request(vec![extension(
                "publisher.extension",
                Some("1.2.3;--force"),
            )]))
            .is_err()
        );
        assert!(validate(&request(vec![extension("bad/id", None)])).is_err());
    }

    #[test]
    fn inventory_parser_requires_valid_unique_versioned_ids() {
        let parsed =
            parse_inventory("rust-lang.rust-analyzer@0.4.2100\nms-python.python@2026.1.0\n")
                .unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].name, "ms-python.python");
        assert!(parse_inventory("publisher.extension\n").is_err());
        assert!(parse_inventory("publisher.extension@1.0.0\npublisher.extension@1.0.0\n").is_err());
    }

    #[test]
    fn request_save_load_is_validated_and_never_overwrites() -> anyhow::Result<()> {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!("devenv-vscode-request-{sequence}"));
        fs::create_dir(&directory)?;
        let path = directory.join("extensions.json");
        let request = request(vec![extension("rust-lang.rust-analyzer", Some("0.4.2100"))]);
        save(&request, &path)?;
        assert_eq!(load(&path)?, request);
        assert!(save(&request, &path).is_err());
        fs::remove_dir_all(directory)?;
        Ok(())
    }

    struct FakeRunner {
        installed: BTreeMap<String, String>,
        installs: Vec<String>,
        fail_install: bool,
    }

    impl FakeRunner {
        fn new(installed: &[(&str, &str)]) -> Self {
            Self {
                installed: installed
                    .iter()
                    .map(|(id, version)| ((*id).into(), (*version).into()))
                    .collect(),
                installs: Vec::new(),
                fail_install: false,
            }
        }
    }

    impl crate::process::Runner for FakeRunner {
        fn run(
            &mut self,
            invocation: &crate::process::Invocation,
            _cancel: &Arc<AtomicBool>,
        ) -> anyhow::Result<crate::process::Output> {
            let (code, stdout) = match invocation.args.get(1).map(String::as_str) {
                Some("--list-extensions") => (
                    0,
                    self.installed
                        .iter()
                        .map(|(id, version)| format!("{id}@{version}\n"))
                        .collect(),
                ),
                Some("--install-extension") => {
                    let target = invocation
                        .args
                        .get(2)
                        .context("missing extension target")?
                        .clone();
                    self.installs.push(target.clone());
                    if self.fail_install {
                        (1, String::new())
                    } else {
                        let (id, version) = target
                            .rsplit_once('@')
                            .map(|(id, version)| (id, version.to_owned()))
                            .unwrap_or((target.as_str(), "9.9.9".into()));
                        self.installed.insert(id.into(), version);
                        (0, String::new())
                    }
                }
                _ => anyhow::bail!("unexpected fake VS Code invocation"),
            };
            Ok(crate::process::Output {
                code,
                stdout,
                stderr: String::new(),
                cancelled: false,
                timed_out: false,
            })
        }
    }

    fn run_recorded_for_test(
        runner: &mut FakeRunner,
        request: &Request,
        evidence: &mut BTreeMap<String, serde_json::Value>,
    ) -> anyhow::Result<Report> {
        let cancel = Arc::new(AtomicBool::new(false));
        let electron =
            PathBuf::from("/Applications/Visual Studio Code.app/Contents/MacOS/Electron");
        let cli =
            PathBuf::from("/Applications/Visual Studio Code.app/Contents/Resources/app/out/cli.js");
        run_recorded(
            runner,
            &electron,
            &cli,
            request,
            &cancel,
            Path::new("/private/history"),
            &mut |name, value| {
                evidence.insert(name.into(), value.clone());
                Ok(())
            },
        )
    }

    #[test]
    fn exact_and_rolling_installations_are_read_back_before_success() -> anyhow::Result<()> {
        let mut runner = FakeRunner::new(&[]);
        let request = request(vec![
            extension("rust-lang.rust-analyzer", Some("0.4.2100-beta.1")),
            extension("ms-python.python", None),
        ]);
        let mut evidence = BTreeMap::new();
        let report = run_recorded_for_test(&mut runner, &request, &mut evidence)?;
        assert!(report.verified);
        assert_eq!(
            runner.installs,
            [
                "rust-lang.rust-analyzer@0.4.2100-beta.1",
                "ms-python.python"
            ]
        );
        assert_eq!(
            report.extensions[0].installed_version.as_deref(),
            Some("0.4.2100-beta.1")
        );
        assert_eq!(
            report.extensions[1].installed_version.as_deref(),
            Some("9.9.9")
        );
        assert_eq!(evidence["result.json"]["status"], "verified");
        Ok(())
    }

    #[test]
    fn already_satisfied_exact_extension_is_not_reinstalled() -> anyhow::Result<()> {
        let mut runner = FakeRunner::new(&[("rust-lang.rust-analyzer", "0.4.2100")]);
        let request = request(vec![extension("rust-lang.rust-analyzer", Some("0.4.2100"))]);
        let mut evidence = BTreeMap::new();
        let report = run_recorded_for_test(&mut runner, &request, &mut evidence)?;
        assert!(report.verified);
        assert!(runner.installs.is_empty());
        assert_eq!(report.extensions[0].action, "already satisfied");
        Ok(())
    }

    #[test]
    fn install_failure_records_partial_state_and_never_claims_success() {
        let mut runner = FakeRunner::new(&[]);
        runner.fail_install = true;
        let request = request(vec![extension("ms-python.python", None)]);
        let mut evidence = BTreeMap::new();
        assert!(run_recorded_for_test(&mut runner, &request, &mut evidence).is_err());
        assert_eq!(evidence["result.json"]["status"], "failed");
        assert_eq!(evidence["result.json"]["report"]["verified"], false);
        assert!(evidence.contains_key("after.json"));
    }
}
