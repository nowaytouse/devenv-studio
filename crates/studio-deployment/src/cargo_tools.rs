use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
#[cfg(any(target_os = "macos", test))]
use std::{collections::BTreeMap, sync::atomic::Ordering};
use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::Read,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};
#[cfg(target_os = "macos")]
use std::{path::PathBuf, sync::atomic::AtomicU64, time::Duration};

const MAX_REQUEST_BYTES: u64 = 64 * 1024;
const MAX_TOOLS: usize = 8;
#[cfg(target_os = "macos")]
const CARGO_TIMEOUT: Duration = Duration::from_secs(45 * 60);
#[cfg(target_os = "macos")]
const GIT_TIMEOUT: Duration = Duration::from_secs(2 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CargoTool {
    pub name: &'static str,
    pub repository: &'static str,
}

pub const TOOLS: [CargoTool; MAX_TOOLS] = [
    CargoTool {
        name: "ripgrep",
        repository: "https://github.com/BurntSushi/ripgrep.git",
    },
    CargoTool {
        name: "fd-find",
        repository: "https://github.com/sharkdp/fd.git",
    },
    CargoTool {
        name: "bat",
        repository: "https://github.com/sharkdp/bat.git",
    },
    CargoTool {
        name: "eza",
        repository: "https://github.com/eza-community/eza.git",
    },
    CargoTool {
        name: "zoxide",
        repository: "https://github.com/ajeetdsouza/zoxide.git",
    },
    CargoTool {
        name: "starship",
        repository: "https://github.com/starship/starship.git",
    },
    CargoTool {
        name: "just",
        repository: "https://github.com/casey/just.git",
    },
    CargoTool {
        name: "git-delta",
        repository: "https://github.com/dandavison/delta.git",
    },
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolRequest {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub git_head: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub schema_version: u32,
    pub tools: Vec<ToolRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolResult {
    pub name: String,
    pub requested_version: Option<String>,
    pub installed_version: String,
    pub source_revision: Option<String>,
    pub action: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
        "Unsupported Cargo tool request schema"
    );
    ensure!(
        !request.tools.is_empty() && request.tools.len() <= MAX_TOOLS,
        "Select 1 to 8 Cargo tools"
    );
    let mut names = BTreeSet::new();
    for tool in &request.tools {
        ensure!(
            TOOLS.iter().any(|allowed| allowed.name == tool.name),
            "Cargo tool is not in the reviewed catalog"
        );
        ensure!(
            names.insert(tool.name.as_str()),
            "Duplicate Cargo tool identity"
        );
        if let Some(version) = &tool.version {
            ensure!(
                !tool.git_head && valid_version(version),
                "Use an exact Cargo version or choose Git HEAD"
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

pub fn save(request: &Request, path: &Path) -> Result<()> {
    validate(request)?;
    studio_core::export_document(request, path)
}

pub fn load(path: &Path) -> Result<Request> {
    let metadata = fs::symlink_metadata(path).context("Inspect Cargo tool request")?;
    ensure!(
        metadata.file_type().is_file(),
        "Cargo tool request must be a regular file"
    );
    ensure!(
        metadata.len() <= MAX_REQUEST_BYTES,
        "Cargo tool request exceeds the 64 KiB limit"
    );
    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .context("Open Cargo tool request")?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_REQUEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("Read Cargo tool request")?;
    ensure!(
        bytes.len() as u64 <= MAX_REQUEST_BYTES,
        "Cargo tool request exceeds the 64 KiB limit"
    );
    let request: Request = serde_json::from_slice(&bytes).context("Parse Cargo tool request")?;
    validate(&request)?;
    Ok(request)
}

#[cfg(any(windows, target_os = "macos", test))]
pub fn parse_inventory(text: &str) -> Result<Vec<crate::backup::Package>> {
    crate::backup::parse_cargo_output(text)
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug)]
struct Cancelled;

#[cfg(any(target_os = "macos", test))]
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Cargo tool deployment cancelled")
    }
}

#[cfg(any(target_os = "macos", test))]
impl std::error::Error for Cancelled {}

#[cfg(target_os = "macos")]
struct CargoHome {
    path: PathBuf,
}

#[cfg(target_os = "macos")]
impl CargoHome {
    fn create() -> Result<Self> {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir();
        ensure!(root.is_absolute(), "Temporary directory must be absolute");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .context("System clock precedes Unix epoch")?
            .as_nanos();
        for _ in 0..32 {
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = root.join(format!(
                "devenv-studio-cargo-{}-{now}-{sequence}",
                std::process::id()
            ));
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                result => result.context("Create isolated Cargo home")?,
            }
            ensure!(
                fs::symlink_metadata(&path)?.file_type().is_dir(),
                "Isolated Cargo home must be a real directory"
            );
            return Ok(Self { path });
        }
        anyhow::bail!("Unable to create an unused isolated Cargo home")
    }

    fn cleanup(&mut self) -> Result<()> {
        match fs::symlink_metadata(&self.path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("Inspect isolated Cargo home before cleanup"),
            Ok(metadata) => {
                ensure!(
                    metadata.file_type().is_dir(),
                    "Isolated Cargo home changed type before cleanup"
                );
                fs::remove_dir_all(&self.path).context("Remove isolated Cargo cache")?;
            }
        }
        self.path.clear();
        Ok(())
    }
}

#[cfg(target_os = "macos")]
impl Drop for CargoHome {
    fn drop(&mut self) {
        if !self.path.as_os_str().is_empty() {
            let _ = self.cleanup();
        }
    }
}

#[cfg(target_os = "macos")]
struct Native {
    git: Option<PathBuf>,
    cargo_home: PathBuf,
}

#[cfg(target_os = "macos")]
impl crate::process::Runner for Native {
    fn run(
        &mut self,
        invocation: &crate::process::Invocation,
        cancel: &Arc<AtomicBool>,
    ) -> Result<crate::process::Output> {
        if self
            .git
            .as_ref()
            .is_some_and(|git| git == &invocation.program)
        {
            crate::process::run_git_tools(invocation, cancel, GIT_TIMEOUT, &self.cargo_home)
        } else {
            crate::process::run_cargo_tools(invocation, cancel, CARGO_TIMEOUT, &self.cargo_home)
        }
    }
}

#[cfg(any(target_os = "macos", test))]
fn invocation(program: &Path, args: &[String]) -> crate::process::Invocation {
    crate::process::Invocation {
        program: program.to_path_buf(),
        args: args.to_vec(),
        vscode_node: false,
        npm_node: false,
    }
}

#[cfg(any(target_os = "macos", test))]
fn step(
    runner: &mut impl crate::process::Runner,
    program: &Path,
    args: &[String],
    cancel: &Arc<AtomicBool>,
) -> Result<crate::process::Output> {
    ensure!(
        program.is_absolute(),
        "Cargo and Git executables must use absolute paths"
    );
    if cancel.load(Ordering::Acquire) {
        return Err(Cancelled.into());
    }
    let output = runner.run(&invocation(program, args), cancel)?;
    if output.cancelled || cancel.load(Ordering::Acquire) {
        return Err(Cancelled.into());
    }
    ensure!(
        !output.timed_out,
        "Cargo or Git command timed out; installed state is unverified"
    );
    ensure!(
        output.code == 0,
        "Cargo or Git command failed (exit code {}); command output was omitted because build scripts can print private data",
        output.code
    );
    Ok(output)
}

#[cfg(any(target_os = "macos", test))]
fn inventory(
    runner: &mut impl crate::process::Runner,
    cargo: &Path,
    root: &Path,
    cancel: &Arc<AtomicBool>,
) -> Result<Vec<crate::backup::Package>> {
    let args = vec![
        "install".into(),
        "--list".into(),
        "--root".into(),
        root.to_string_lossy().into_owned(),
    ];
    parse_inventory(&step(runner, cargo, &args, cancel)?.stdout)
}

#[cfg(any(target_os = "macos", test))]
fn git_revision(
    runner: &mut impl crate::process::Runner,
    git: &Path,
    repository: &str,
    cancel: &Arc<AtomicBool>,
) -> Result<String> {
    let args = vec![
        "ls-remote".into(),
        "--symref".into(),
        repository.into(),
        "HEAD".into(),
    ];
    let stdout = step(runner, git, &args, cancel)?.stdout;
    let revisions: Vec<_> = stdout
        .lines()
        .filter_map(|line| {
            let (revision, reference) = line.split_once('\t')?;
            (reference == "HEAD" && valid_revision(revision)).then_some(revision.to_owned())
        })
        .collect();
    ensure!(
        revisions.len() == 1,
        "Git upstream did not return one full HEAD commit"
    );
    Ok(revisions[0].clone())
}

#[cfg(any(target_os = "macos", test))]
fn valid_revision(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(any(target_os = "macos", test))]
struct RunPaths<'a> {
    cargo: &'a Path,
    git: Option<&'a Path>,
    root: &'a Path,
    evidence_path: &'a Path,
}

#[cfg(any(target_os = "macos", test))]
fn run_recorded(
    runner: &mut impl crate::process::Runner,
    paths: RunPaths<'_>,
    request: &Request,
    cancel: &Arc<AtomicBool>,
    persist: &mut impl FnMut(&str, &serde_json::Value) -> Result<()>,
    cleanup: impl FnOnce() -> Result<()>,
) -> Result<Report> {
    validate(request)?;
    let before = inventory(runner, paths.cargo, paths.root, cancel)?;
    persist(
        "before.json",
        &serde_json::json!({
            "schema_version": 1,
            "provider": "cargo-install",
            "tools": before,
            "scope": "Standard per-user Cargo install root; package source and build inputs are not a byte-identical environment snapshot."
        }),
    )
    .context("Persist Cargo tool baseline before deployment")?;
    persist(
        "request.json",
        &serde_json::json!({
            "schema_version": 1,
            "event": "started",
            "provider": "cargo-install",
            "request": request,
            "installation_root": paths.root,
            "automatic_rollback": false,
            "completion": "Unknown until result.json is present."
        }),
    )
    .context("Persist Cargo tool request before deployment")?;

    let deployment: Result<(Vec<crate::backup::Package>, Vec<ToolResult>)> = (|| {
        let mut revisions = BTreeMap::new();
        for tool in request.tools.iter().filter(|tool| tool.git_head) {
            let git = paths
                .git
                .context("Git HEAD requires a local Git executable")?;
            let repository = TOOLS
                .iter()
                .find(|candidate| candidate.name == tool.name)
                .context("Cargo tool is not in the reviewed catalog")?
                .repository;
            revisions.insert(
                tool.name.clone(),
                git_revision(runner, git, repository, cancel)?,
            );
        }
        persist(
            "resolved.json",
            &serde_json::json!({ "schema_version": 1, "git_head_revisions": revisions }),
        )
        .context("Persist resolved Cargo Git revisions before installation")?;

        let current: BTreeMap<_, _> = before
            .iter()
            .map(|item| (item.name.as_str(), item.version.as_str()))
            .collect();
        for tool in &request.tools {
            if cancel.load(Ordering::Acquire) {
                return Err(Cancelled.into());
            }
            if tool.version.as_deref().is_some_and(|version| {
                current
                    .get(tool.name.as_str())
                    .is_some_and(|installed| *installed == version)
            }) {
                continue;
            }
            let mut args = vec![
                "install".into(),
                "--locked".into(),
                "--root".into(),
                paths.root.to_string_lossy().into_owned(),
            ];
            if tool.git_head {
                let repository = TOOLS
                    .iter()
                    .find(|candidate| candidate.name == tool.name)
                    .context("Cargo tool is not in the reviewed catalog")?
                    .repository;
                let revision = revisions
                    .get(&tool.name)
                    .context("Resolved Cargo Git revision is missing")?;
                args.extend([
                    "--force".into(),
                    "--git".into(),
                    repository.into(),
                    "--rev".into(),
                    revision.clone(),
                ]);
            } else if let Some(version) = &tool.version {
                args.extend(["--version".into(), version.clone()]);
            }
            args.push(tool.name.clone());
            step(runner, paths.cargo, &args, cancel)
                .with_context(|| format!("Deploy Cargo tool {}", tool.name))?;
        }

        let installed = inventory(runner, paths.cargo, paths.root, cancel)?;
        let observed: BTreeMap<_, _> = installed
            .iter()
            .map(|item| (item.name.as_str(), item))
            .collect();
        let mut tools = Vec::with_capacity(request.tools.len());
        for tool in &request.tools {
            let item = observed
                .get(tool.name.as_str())
                .context("A selected Cargo tool is missing after deployment")?;
            if let Some(version) = &tool.version {
                ensure!(
                    item.version == *version,
                    "Installed Cargo tool version does not match the exact request"
                );
            }
            tools.push(ToolResult {
                name: tool.name.clone(),
                requested_version: tool.version.clone(),
                installed_version: item.version.clone(),
                source_revision: revisions.get(&tool.name).cloned(),
                action: if tool.git_head {
                    "installed from pinned upstream HEAD".into()
                } else if tool.version.is_some() {
                    "installed exact version".into()
                } else {
                    "deployed rolling crates.io request".into()
                },
            });
        }
        Ok((installed, tools))
    })();

    let cancelled = deployment
        .as_ref()
        .is_err_and(|error| error.is::<Cancelled>());
    let mut after_error = None;
    let after = match &deployment {
        Ok((installed, _)) => Some(installed.clone()),
        Err(_) if !cancelled => {
            match inventory(
                runner,
                paths.cargo,
                paths.root,
                &Arc::new(AtomicBool::new(false)),
            ) {
                Ok(installed) => Some(installed),
                Err(error) => {
                    after_error = Some(format!("{error:#}"));
                    None
                }
            }
        }
        Err(_) => None,
    };
    if let Some(after) = &after {
        persist(
            "after.json",
            &serde_json::json!({ "schema_version": 1, "provider": "cargo-install", "tools": after }),
        )
        .context("Persist Cargo tool post-deployment inventory")?;
    }
    let cleanup_error = cleanup().err().map(|error| format!("{error:#}"));
    let mut detail = if deployment.is_ok() {
        "Selected package identities were read back after deployment and exact registry versions matched. Rolling releases resolve through the reviewed Cargo registry at run time. Git HEAD commands were pinned to the recorded commit; Cargo's installed-package list exposes package version, not a binary commit fingerprint. Package build scripts ran as the current user; changes are not automatically rolled back.".to_owned()
    } else {
        format!(
            "Deployment did not finish with a verified package state. {}{} No automatic rollback is available.",
            if cancelled {
                "The operation was cancelled; post-state was not read. "
            } else {
                "Partial changes may remain. "
            },
            after_error
                .as_deref()
                .map(|error| format!("Post-state read failed: {error}. "))
                .unwrap_or_default()
        )
    };
    if let Some(error) = cleanup_error {
        detail.push_str(&format!(" Isolated Cargo cache cleanup failed: {error}."));
    }
    detail.push_str(&format!(" Evidence: {}", paths.evidence_path.display()));
    let report = match &deployment {
        Ok((_, tools)) => Report {
            requested: request.tools.len(),
            verified: true,
            cancelled: false,
            tools: tools.clone(),
            detail,
        },
        Err(_) => Report {
            requested: request.tools.len(),
            verified: false,
            cancelled,
            tools: Vec::new(),
            detail,
        },
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
    .context("Terminal Cargo tool evidence could not be persisted; the started record remains unresolved")?;
    if cancelled {
        return Ok(report);
    }
    deployment?;
    Ok(report)
}

#[cfg(target_os = "macos")]
pub fn configure(request: Request, cancel: Arc<AtomicBool>) -> Result<Report> {
    validate(&request)?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("macOS home directory is unavailable")?;
    ensure!(
        home.is_absolute() && home.is_dir(),
        "macOS home directory is invalid"
    );
    let cargo = home.join(".cargo/bin/cargo");
    ensure!(
        cargo.is_file(),
        "Install a per-user Rust toolchain with Cargo before deploying Cargo tools"
    );
    let git = if request.tools.iter().any(|tool| tool.git_head) {
        [
            "/opt/homebrew/bin/git",
            "/usr/local/bin/git",
            "/Library/Developer/CommandLineTools/usr/bin/git",
            "/Applications/Xcode.app/Contents/Developer/usr/bin/git",
        ]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
        .context("Git HEAD requires Homebrew Git, Xcode, or Command Line Tools")?
        .into()
    } else {
        None
    };
    let root = home.join(".cargo");
    let history = crate::environment::create_history(&studio_platform::data_directory()?)?;
    crate::network::record_current(&history.join("network.json"))?;
    let mut cargo_home = CargoHome::create()?;
    let mut runner = Native {
        git: git.clone(),
        cargo_home: cargo_home.path.clone(),
    };
    run_recorded(
        &mut runner,
        RunPaths {
            cargo: &cargo,
            git: git.as_deref(),
            root: &root,
            evidence_path: &history,
        },
        &request,
        &cancel,
        &mut |name, value| studio_core::export_document(value, &history.join(name)),
        || cargo_home.cleanup()
    )
    .with_context(|| format!("Cargo tool deployment evidence: {}. Selected packages may be partly changed; inspect before retrying.", history.display()))
}

#[cfg(not(target_os = "macos"))]
pub fn configure(_request: Request, _cancel: Arc<AtomicBool>) -> Result<Report> {
    anyhow::bail!("Native Cargo tool deployment currently requires macOS")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[test]
    fn request_allows_latest_exact_prerelease_and_reviewed_git_head_only() {
        let request = Request {
            schema_version: 1,
            tools: vec![
                ToolRequest {
                    name: "ripgrep".into(),
                    version: None,
                    git_head: false,
                },
                ToolRequest {
                    name: "fd-find".into(),
                    version: Some("10.0.0-beta.1".into()),
                    git_head: false,
                },
                ToolRequest {
                    name: "bat".into(),
                    version: None,
                    git_head: true,
                },
            ],
        };
        assert!(validate(&request).is_ok());
        let mut invalid = request.clone();
        invalid.tools[0].name = "arbitrary-crate".into();
        assert!(validate(&invalid).is_err());
        let mut invalid = request.clone();
        invalid.tools[2].version = Some("1.2.3".into());
        assert!(validate(&invalid).is_err());
    }

    #[test]
    fn git_head_requires_one_full_commit_hash() {
        assert!(valid_revision(&"a".repeat(40)));
        assert!(valid_revision(&"b".repeat(64)));
        assert!(!valid_revision("deadbeef"));
        assert!(!valid_revision(&"z".repeat(40)));
    }

    #[derive(Default)]
    struct FakeRunner {
        outputs: VecDeque<String>,
        calls: Vec<crate::process::Invocation>,
    }

    impl crate::process::Runner for FakeRunner {
        fn run(
            &mut self,
            invocation: &crate::process::Invocation,
            _: &Arc<AtomicBool>,
        ) -> Result<crate::process::Output> {
            self.calls.push(invocation.clone());
            Ok(crate::process::Output {
                code: 0,
                stdout: self.outputs.pop_front().unwrap_or_default(),
                stderr: String::new(),
                cancelled: false,
                timed_out: false,
            })
        }
    }

    #[test]
    fn git_install_is_pinned_and_read_back_before_verified() {
        let revision = "0123456789abcdef0123456789abcdef01234567";
        let mut runner = FakeRunner {
            outputs: [
                "ripgrep v15.1.0:\n    rg\n".into(),
                format!("ref: refs/heads/master\\tHEAD\\n{revision}\\tHEAD\\n")
                    .replace("\\t", "\t")
                    .replace("\\n", "\n"),
                String::new(),
                "ripgrep v15.2.0:\n    rg\n".into(),
            ]
            .into(),
            ..Default::default()
        };
        let request = Request {
            schema_version: 1,
            tools: vec![ToolRequest {
                name: "ripgrep".into(),
                version: None,
                git_head: true,
            }],
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let mut evidence = BTreeMap::new();
        let root = std::env::temp_dir().join("devenv-studio-cargo-fixture");
        let report = run_recorded(
            &mut runner,
            RunPaths {
                cargo: &root.join("bin/cargo"),
                git: Some(&root.join("bin/git")),
                root: &root,
                evidence_path: &root.join("history"),
            },
            &request,
            &cancel,
            &mut |name, value| {
                evidence.insert(name.to_owned(), value.clone());
                Ok(())
            },
            || Ok(()),
        )
        .unwrap();
        assert!(report.verified);
        assert_eq!(report.tools[0].installed_version, "15.2.0");
        assert_eq!(report.tools[0].source_revision.as_deref(), Some(revision));
        let install = &runner.calls[2].args;
        assert!(install.windows(2).any(|pair| pair == ["--rev", revision]));
        assert!(evidence.contains_key("before.json"));
        assert!(evidence.contains_key("resolved.json"));
        assert!(evidence.contains_key("after.json"));
        assert!(evidence.contains_key("result.json"));
    }
}
