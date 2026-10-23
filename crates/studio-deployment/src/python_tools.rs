//! Explicit uv-managed Python command-tool redeployment.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use std::collections::VecDeque;
#[cfg(any(windows, target_os = "macos"))]
use std::time::Duration;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::Read,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

const MAX_TOOLS: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PythonTool {
    pub name: String,
    /// An exact observed/requested PyPI version; None follows the current index release.
    pub version: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub schema_version: u32,
    pub tools: Vec<PythonTool>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolResult {
    pub name: String,
    pub requested_version: Option<String>,
    pub installed_version: String,
    pub action: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub requested: usize,
    pub verified: bool,
    pub cancelled: bool,
    pub tools: Vec<ToolResult>,
    pub detail: String,
}
pub fn validate(request: &Request) -> Result<()> {
    ensure!(
        request.schema_version == 1,
        "Unsupported Python tool request schema"
    );
    ensure!(
        !request.tools.is_empty() && request.tools.len() <= MAX_TOOLS,
        "Select 1 to 512 Python tools"
    );
    let mut names = BTreeSet::new();
    for tool in &request.tools {
        let normalized =
            crate::normalize_pip_id(&tool.name).context("Invalid PyPI package name")?;
        ensure!(
            crate::developer_ids::valid("uv", &tool.name),
            "Invalid PyPI package name"
        );
        ensure!(names.insert(normalized), "Duplicate Python tool identity");
        if let Some(version) = &tool.version {
            ensure!(
                !version.is_empty()
                    && version.len() <= 128
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
                        .all(|byte| { byte.is_ascii_alphanumeric() || b".+_!-".contains(&byte) }),
                "Use a plain exact PyPI version or select the rolling release"
            );
        }
    }
    Ok(())
}
pub fn save(request: &Request, path: &Path) -> Result<()> {
    validate(request)?;
    studio_core::export_document(request, path)
}
pub fn load(path: &Path) -> Result<Request> {
    let metadata = fs::symlink_metadata(path).context("Read Python tool request metadata")?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Python tool request must be a regular file"
    );
    let file = File::open(path).context("Open Python tool request")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened = file.metadata()?;
        ensure!(
            opened.dev() == metadata.dev() && opened.ino() == metadata.ino(),
            "Python tool request changed while opening"
        );
    }
    let mut bytes = Vec::new();
    file.take(studio_core::MAX_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= studio_core::MAX_INPUT_BYTES,
        "Python tool request exceeds 1 MiB limit"
    );
    let request: Request =
        serde_json::from_slice(&bytes).context("Invalid Python tool request JSON")?;
    validate(&request)?;
    Ok(request)
}
#[cfg(any(windows, target_os = "macos"))]
struct Native(Duration);
#[cfg(any(windows, target_os = "macos"))]
impl crate::process::Runner for Native {
    fn run(
        &mut self,
        invocation: &crate::process::Invocation,
        cancel: &Arc<AtomicBool>,
    ) -> Result<crate::process::Output> {
        crate::process::run_python_tools(invocation, cancel, self.0)
    }
}
#[cfg(any(windows, target_os = "macos"))]
struct InventoryRunner(Duration);
#[cfg(any(windows, target_os = "macos"))]
impl crate::process::Runner for InventoryRunner {
    fn run(
        &mut self,
        invocation: &crate::process::Invocation,
        cancel: &Arc<AtomicBool>,
    ) -> Result<crate::process::Output> {
        crate::process::run_python_inventory(invocation, cancel, self.0)
    }
}
#[cfg(any(windows, target_os = "macos", test))]
#[derive(Debug)]
struct Cancelled;
#[cfg(any(windows, target_os = "macos", test))]
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Python tool operation cancelled")
    }
}
#[cfg(any(windows, target_os = "macos", test))]
impl std::error::Error for Cancelled {}
#[cfg(any(windows, target_os = "macos", test))]
fn step(
    runner: &mut impl crate::process::Runner,
    program: &Path,
    args: &[String],
    cancel: &Arc<AtomicBool>,
) -> Result<String> {
    ensure!(program.is_absolute(), "uv location must be absolute");
    if cancel.load(Ordering::Acquire) {
        return Err(Cancelled.into());
    }
    let output = runner.run(
        &crate::process::Invocation {
            program: program.to_path_buf(),
            args: args.to_vec(),
            vscode_node: false,
            npm_node: false,
        },
        cancel,
    )?;
    if output.cancelled || cancel.load(Ordering::Acquire) {
        return Err(Cancelled.into());
    }
    ensure!(
        !output.timed_out,
        "uv command timed out; installed state is unverified"
    );
    ensure!(
        output.code == 0,
        "uv command failed; installed state is unverified"
    );
    Ok(output.stdout)
}
fn list_args() -> Vec<String> {
    [
        "tool",
        "list",
        "--no-config",
        "--no-progress",
        "--color",
        "never",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}
#[cfg(any(windows, target_os = "macos", test))]
fn inventory(
    runner: &mut impl crate::process::Runner,
    program: &Path,
    cancel: &Arc<AtomicBool>,
) -> Result<Vec<crate::backup::Package>> {
    crate::backup::parse_uv_output(&step(runner, program, &list_args(), cancel)?)
}
pub fn capture(cancel: Arc<AtomicBool>) -> Result<Vec<crate::backup::Package>> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        let program = crate::backup::uv_program()?;
        inventory(
            &mut InventoryRunner(Duration::from_secs(60)),
            &program,
            &cancel,
        )
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = cancel;
        anyhow::bail!("Native uv inventory requires macOS or Windows")
    }
}
pub fn configure(request: Request, cancel: Arc<AtomicBool>) -> Result<Report> {
    validate(&request)?;
    #[cfg(any(windows, target_os = "macos"))]
    {
        let program = crate::backup::uv_program()?;
        let history = crate::environment::create_history(&studio_platform::data_directory()?)?;
        crate::network::record_current(&history.join("network.json"))?;
        let report = run_recorded(
            &mut Native(Duration::from_secs(45 * 60)),
            &program,
            &request,
            &cancel,
            &mut |name, value| studio_core::export_document(value, &history.join(name)),
            Some(&history),
        )
        .with_context(|| format!(
            "Python tool deployment evidence: {}. Selected tools may be partly changed; inspect before retrying.",
            history.display()
        ))?;
        Ok(report)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = cancel;
        anyhow::bail!("Native uv deployment requires macOS or Windows")
    }
}
#[cfg(any(windows, target_os = "macos", test))]
fn run_recorded(
    runner: &mut impl crate::process::Runner,
    program: &Path,
    request: &Request,
    cancel: &Arc<AtomicBool>,
    persist: &mut impl FnMut(&str, &serde_json::Value) -> Result<()>,
    evidence_path: Option<&Path>,
) -> Result<Report> {
    validate(request)?;
    let before = inventory(runner, program, cancel)?;
    persist("before.json", &serde_json::json!({
        "schema_version":1, "provider":"uv-tool", "tools":before,
        "scope":"Only uv-managed command tools; pipx environments and project dependencies are separate."
    })).context("Persist Python tool baseline before installation")?;
    persist(
        "request.json",
        &serde_json::json!({
            "schema_version":1, "event":"started", "provider":"uv-tool", "request":request,
            "python_downloads":false, "automatic_rollback":false,
            "completion":"Unknown until result.json is present."
        }),
    )
    .context("Persist Python tool request before installation")?;

    let deployment: Result<(Vec<crate::backup::Package>, Vec<ToolResult>)> = (|| {
        let existing: BTreeMap<_, _> = before
            .iter()
            .map(|item| {
                (
                    crate::normalize_pip_id(&item.name).unwrap_or_default(),
                    item.version.as_str(),
                )
            })
            .collect();
        for tool in &request.tools {
            if cancel.load(Ordering::Acquire) {
                return Err(Cancelled.into());
            }
            let current = existing
                .get(&crate::normalize_pip_id(&tool.name).unwrap_or_default())
                .copied();
            if let (Some(requested), Some(installed)) = (tool.version.as_deref(), current)
                && requested == installed
            {
                continue;
            }
            let mut args: Vec<String> = ["tool"].into_iter().map(str::to_owned).collect();
            match (tool.version.as_deref(), current) {
                (Some(version), _) => {
                    args.push("install".into());
                    args.extend(
                        [
                            "--force",
                            "--no-config",
                            "--no-progress",
                            "--color",
                            "never",
                            "--no-python-downloads",
                        ]
                        .into_iter()
                        .map(str::to_owned),
                    );
                    args.push(format!("{}=={}", tool.name, version));
                }
                (None, Some(_)) => {
                    args.extend(
                        [
                            "install",
                            "--no-config",
                            "--no-progress",
                            "--color",
                            "never",
                            "--no-python-downloads",
                        ]
                        .into_iter()
                        .map(str::to_owned),
                    );
                    args.push(tool.name.clone());
                }
                (None, None) => {
                    args.extend(
                        [
                            "install",
                            "--no-config",
                            "--no-progress",
                            "--color",
                            "never",
                            "--no-python-downloads",
                        ]
                        .into_iter()
                        .map(str::to_owned),
                    );
                    args.push(tool.name.clone());
                }
            }
            step(runner, program, &args, cancel)
                .with_context(|| format!("Deploy selected Python tool {}", tool.name))?;
        }
        let installed = inventory(runner, program, cancel)?;
        let observed: BTreeMap<_, _> = installed
            .iter()
            .map(|item| {
                (
                    crate::normalize_pip_id(&item.name).unwrap_or_default(),
                    item,
                )
            })
            .collect();
        let mut results = Vec::with_capacity(request.tools.len());
        for tool in &request.tools {
            let item = observed
                .get(&crate::normalize_pip_id(&tool.name).unwrap_or_default())
                .context("A selected Python tool is missing after deployment")?;
            if let Some(version) = &tool.version {
                ensure!(
                    item.version == *version,
                    "Installed Python tool version does not match the exact request"
                );
            }
            let action = if tool.version.as_deref().is_some_and(|version| {
                existing
                    .get(&crate::normalize_pip_id(&tool.name).unwrap_or_default())
                    .is_some_and(|installed| *installed == version)
            }) {
                "already satisfied"
            } else if tool.version.is_some() {
                "installed exact version"
            } else {
                "deployed rolling request"
            };
            results.push(ToolResult {
                name: tool.name.clone(),
                requested_version: tool.version.clone(),
                installed_version: item.version.clone(),
                action: action.into(),
            });
        }
        Ok((installed, results))
    })();
    let cancelled = deployment
        .as_ref()
        .is_err_and(|error| error.is::<Cancelled>());
    let report = match &deployment {
        Ok((_, tools)) => Report {
            requested: request.tools.len(),
            verified: true,
            cancelled: false,
            tools: tools.clone(),
            detail: format!(
                "Selected uv tool identities were verified and fixed versions matched. Rolling requests resolve from the selected index at run time and may differ next run; project dependencies and bit-identical restoration were not verified.{}",
                evidence_path
                    .map(|path| format!(" Evidence: {}", path.display()))
                    .unwrap_or_default()
            ),
        },
        Err(_) => Report {
            requested: request.tools.len(),
            verified: false,
            cancelled,
            tools: Vec::new(),
            detail: format!(
                "Deployment did not complete with verified tool state. Partial changes may remain; no automatic rollback is available.{}",
                evidence_path
                    .map(|path| format!(" Evidence: {}", path.display()))
                    .unwrap_or_default()
            ),
        },
    };
    persist("result.json", &serde_json::json!({
        "schema_version":1, "event":"finished",
        "status":if report.verified { "verified" } else if cancelled { "cancelled" } else { "failed" },
        "report":report,
        "failure":deployment.as_ref().err().map(|error| format!("{error:#}")),
    })).context("Terminal Python tool evidence could not be persisted; the started record remains unresolved")?;
    if cancelled {
        return Ok(report);
    }
    deployment?;
    Ok(report)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{Invocation, Output, Runner};
    use std::path::PathBuf;

    struct Fake {
        calls: Vec<Invocation>,
        outputs: VecDeque<Output>,
    }
    impl Runner for Fake {
        fn run(&mut self, invocation: &Invocation, _: &Arc<AtomicBool>) -> Result<Output> {
            self.calls.push(invocation.clone());
            self.outputs.pop_front().context("Missing fake output")
        }
    }
    fn output(text: &str) -> Output {
        Output {
            code: 0,
            stdout: text.into(),
            stderr: String::new(),
            cancelled: false,
            timed_out: false,
        }
    }
    fn request() -> Request {
        Request {
            schema_version: 1,
            tools: vec![
                PythonTool {
                    name: "ruff".into(),
                    version: Some("0.14.0".into()),
                },
                PythonTool {
                    name: "mypy".into(),
                    version: None,
                },
                PythonTool {
                    name: "black".into(),
                    version: Some("25.0.0".into()),
                },
            ],
        }
    }
    #[test]
    fn request_accepts_exact_and_rolling_but_rejects_injection_duplicates_and_unknown_fields() {
        validate(&request()).unwrap();
        for name in [
            "",
            "../ruff",
            "-x",
            "ruff==1",
            "git+https://example.invalid/x",
            "ruff\n--help",
        ] {
            let request = Request {
                schema_version: 1,
                tools: vec![PythonTool {
                    name: name.into(),
                    version: None,
                }],
            };
            assert!(validate(&request).is_err(), "{name}");
        }
        for version in ["", "--help", "1.0;extra", "https://example.invalid"] {
            let request = Request {
                schema_version: 1,
                tools: vec![PythonTool {
                    name: "ruff".into(),
                    version: Some(version.into()),
                }],
            };
            assert!(validate(&request).is_err(), "{version}");
        }
        let duplicate = Request {
            schema_version: 1,
            tools: vec![
                PythonTool {
                    name: "friendly_name".into(),
                    version: None,
                },
                PythonTool {
                    name: "friendly.name".into(),
                    version: None,
                },
            ],
        };
        assert!(validate(&duplicate).is_err());
        assert!(
            serde_json::from_str::<Request>(r#"{"schema_version":1,"tools":[],"confirmed":true}"#)
                .is_err()
        );
    }
    #[test]
    fn request_roundtrip_preserves_exact_choices_and_export_is_no_clobber() -> Result<()> {
        struct Directory(PathBuf);
        impl Drop for Directory {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let directory = Directory(std::env::temp_dir().join(format!(
                "studio-python-request-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos()
            )));
        fs::create_dir(&directory.0)?;
        let path = directory.0.join("request.json");
        save(&request(), &path)?;
        assert_eq!(load(&path)?, request());
        assert!(save(&request(), &path).is_err());
        Ok(())
    }
    #[test]
    fn selected_tools_use_uv_only_and_exact_or_rolling_deployments_are_verified() {
        let mut fake = Fake {
            calls: Vec::new(),
            outputs: VecDeque::from([
                output("ruff v0.14.0\n- ruff\nmypy v1.18.0\n- mypy\n"),
                output(""),
                output(""),
                output("ruff v0.14.0\n- ruff\nmypy v1.19.0\n- mypy\nblack v25.0.0\n- black\n"),
            ]),
        };
        let mut records: Vec<(String, serde_json::Value)> = Vec::new();
        let report = run_recorded(
            &mut fake,
            &PathBuf::from(if cfg!(windows) { "C:\\uv.exe" } else { "/uv" }),
            &request(),
            &Arc::new(AtomicBool::new(false)),
            &mut |name, value| {
                records.push((name.into(), value.clone()));
                Ok(())
            },
            None,
        )
        .unwrap();
        assert!(report.verified);
        assert_eq!(
            report
                .tools
                .iter()
                .map(|item| item.installed_version.as_str())
                .collect::<Vec<_>>(),
            ["0.14.0", "1.19.0", "25.0.0"]
        );
        assert_eq!(fake.calls.len(), 4);
        assert_eq!(
            fake.calls[1].args,
            [
                "tool",
                "install",
                "--no-config",
                "--no-progress",
                "--color",
                "never",
                "--no-python-downloads",
                "mypy"
            ]
        );
        assert_eq!(
            fake.calls[2].args,
            [
                "tool",
                "install",
                "--force",
                "--no-config",
                "--no-progress",
                "--color",
                "never",
                "--no-python-downloads",
                "black==25.0.0"
            ]
        );
        assert_eq!(
            records
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            ["before.json", "request.json", "result.json"]
        );
    }
    #[test]
    fn failure_is_persisted_as_unverified_and_never_claims_success() {
        let mut fake = Fake {
            calls: Vec::new(),
            outputs: VecDeque::from([
                output(""),
                Output {
                    code: 1,
                    ..output("")
                },
            ]),
        };
        let mut terminal = None;
        assert!(
            run_recorded(
                &mut fake,
                &PathBuf::from("/uv"),
                &Request {
                    schema_version: 1,
                    tools: vec![PythonTool {
                        name: "ruff".into(),
                        version: None
                    }]
                },
                &Arc::new(AtomicBool::new(false)),
                &mut |name, value| {
                    if name == "result.json" {
                        terminal = Some(value.clone());
                    }
                    Ok(())
                },
                None,
            )
            .is_err()
        );
        assert_eq!(terminal.unwrap()["status"], "failed");
    }
    #[test]
    fn exact_installed_version_is_left_untouched_but_still_read_back() {
        let mut fake = Fake {
            calls: Vec::new(),
            outputs: VecDeque::from([
                output("ruff v0.14.0\n- ruff\n"),
                output("ruff v0.14.0\n- ruff\n"),
            ]),
        };
        let report = run_recorded(
            &mut fake,
            &PathBuf::from("/uv"),
            &Request {
                schema_version: 1,
                tools: vec![PythonTool {
                    name: "ruff".into(),
                    version: Some("0.14.0".into()),
                }],
            },
            &Arc::new(AtomicBool::new(false)),
            &mut |_, _| Ok(()),
            None,
        )
        .unwrap();
        assert!(report.verified);
        assert_eq!(fake.calls.len(), 2);
    }
}
