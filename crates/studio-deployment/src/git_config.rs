use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
#[cfg(any(windows, target_os = "macos", test))]
use std::sync::atomic::Ordering;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};
#[cfg(any(windows, target_os = "macos"))]
use std::{sync::Mutex, time::Duration};

const SCHEMA_VERSION: u32 = 1;
const MAX_PROFILE_BYTES: u64 = 64 * 1024;
const MAX_VALUE_BYTES: usize = 512;
#[cfg(any(windows, target_os = "macos"))]
const GIT_TIMEOUT: Duration = Duration::from_secs(10);
pub const KEYS: [&str; 9] = [
    "user.name",
    "user.email",
    "init.defaultBranch",
    "core.autocrlf",
    "core.eol",
    "core.longpaths",
    "pull.rebase",
    "fetch.prune",
    "push.default",
];

#[cfg(any(windows, target_os = "macos"))]
static APPLY_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub schema_version: u32,
    pub settings: BTreeMap<String, String>,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            settings: BTreeMap::new(),
        }
    }
}

impl Profile {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == SCHEMA_VERSION,
            "Unsupported Git profile schema"
        );
        ensure!(
            self.settings.len() <= KEYS.len(),
            "Git profile contains too many settings"
        );
        for (key, value) in &self.settings {
            ensure!(
                KEYS.contains(&key.as_str()),
                "Git profile contains an unsupported setting"
            );
            validate_value(key, value)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ApplyReport {
    pub schema_version: u32,
    pub selected: Vec<String>,
    pub changed: Vec<String>,
    pub verified: bool,
    pub cancelled: bool,
    pub detail: String,
    pub history: PathBuf,
}

pub fn save(profile: &Profile, path: &Path) -> Result<()> {
    profile.validate()?;
    studio_core::export_document(profile, path)
}

pub fn load(path: &Path) -> Result<Profile> {
    let bytes = read_profile(path)?;
    let profile: Profile = serde_json::from_slice(&bytes).context("Invalid Git profile JSON")?;
    profile.validate()?;
    Ok(profile)
}

pub fn capture(cancel: &Arc<AtomicBool>) -> Result<Profile> {
    #[cfg(any(target_os = "macos", windows))]
    {
        let git = git_program()?;
        let mut runner = NativeRunner;
        capture_with(&mut runner, &git, &KEYS, cancel)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = cancel;
        bail!("Git profile capture is supported on macOS and Windows only")
    }
}

pub fn apply(
    profile: &Profile,
    selected: &BTreeSet<String>,
    expected_current: &Profile,
    cancel: &Arc<AtomicBool>,
) -> Result<ApplyReport> {
    #[cfg(any(target_os = "macos", windows))]
    {
        let _guard = APPLY_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("Git configuration lock was poisoned"))?;
        let git = git_program()?;
        let mut runner = NativeRunner;
        apply_with(
            profile,
            selected,
            expected_current,
            &mut runner,
            &git,
            &studio_platform::data_directory()?,
            cancel,
        )
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (profile, selected, expected_current, cancel);
        bail!("Git profile application is supported on macOS and Windows only")
    }
}

fn validate_value(key: &str, value: &str) -> Result<()> {
    ensure!(
        value.len() <= MAX_VALUE_BYTES,
        "Git setting value is too long"
    );
    ensure!(
        !value.chars().any(char::is_control),
        "Git setting values cannot contain control characters"
    );
    if matches!(key, "user.name" | "user.email") {
        ensure!(
            !value.trim().is_empty(),
            "Git identity values cannot be empty"
        );
    }
    let valid = match key {
        "user.name" | "user.email" | "init.defaultBranch" => true,
        "core.autocrlf" => matches_any(
            value,
            &["true", "false", "yes", "no", "on", "off", "1", "0", "input"],
        ),
        "core.eol" => matches_any(value, &["lf", "crlf", "native"]),
        "core.longpaths" | "fetch.prune" => matches_any(
            value,
            &["true", "false", "yes", "no", "on", "off", "1", "0"],
        ),
        "pull.rebase" => matches_any(
            value,
            &[
                "true",
                "false",
                "yes",
                "no",
                "on",
                "off",
                "1",
                "0",
                "merges",
                "interactive",
            ],
        ),
        "push.default" => matches_any(
            value,
            &["nothing", "current", "upstream", "simple", "matching"],
        ),
        _ => false,
    };
    ensure!(valid, "Git setting has an unsupported value");
    Ok(())
}

fn matches_any(value: &str, choices: &[&str]) -> bool {
    choices
        .iter()
        .any(|choice| value.eq_ignore_ascii_case(choice))
}

fn read_profile(path: &Path) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).context("Inspect Git profile file")?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Git profile must be a regular file"
    );
    ensure!(
        metadata.len() <= MAX_PROFILE_BYTES,
        "Git profile exceeds 64 KiB"
    );
    let file = File::open(path).context("Open Git profile file")?;
    let opened = file.metadata()?;
    ensure!(
        opened.is_file(),
        "Opened Git profile must be a regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            opened.dev() == metadata.dev() && opened.ino() == metadata.ino(),
            "Git profile changed while opening"
        );
    }
    let current = fs::symlink_metadata(path)?;
    ensure!(
        current.is_file() && !current.file_type().is_symlink(),
        "Git profile changed while opening"
    );
    let mut bytes = Vec::new();
    file.take(MAX_PROFILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("Read Git profile")?;
    ensure!(
        bytes.len() as u64 <= MAX_PROFILE_BYTES,
        "Git profile exceeds 64 KiB"
    );
    Ok(bytes)
}

#[cfg(any(windows, target_os = "macos", test))]
fn capture_with(
    runner: &mut impl crate::process::Runner,
    git: &Path,
    keys: &[&str],
    cancel: &Arc<AtomicBool>,
) -> Result<Profile> {
    let mut profile = Profile::default();
    for key in keys {
        if let Some(value) = read_value(runner, git, key, cancel)? {
            profile.settings.insert((*key).to_owned(), value);
        }
    }
    profile.validate()?;
    Ok(profile)
}

#[cfg(any(windows, target_os = "macos", test))]
fn read_value(
    runner: &mut impl crate::process::Runner,
    git: &Path,
    key: &str,
    cancel: &Arc<AtomicBool>,
) -> Result<Option<String>> {
    let invocation = invocation(
        git,
        &[
            "config",
            "--global",
            "--no-includes",
            "--null",
            "--get",
            key,
        ]
        .map(str::to_owned),
    );
    let output = runner.run(&invocation, cancel)?;
    ensure!(!output.cancelled, "Git configuration read was cancelled");
    ensure!(!output.timed_out, "Git configuration read timed out");
    parse_get_value(output.code, &output.stdout, &output.stderr, key)
}

#[cfg(any(windows, target_os = "macos", test))]
fn parse_get_value(code: u32, stdout: &str, stderr: &str, key: &str) -> Result<Option<String>> {
    if code == 1 && stdout.is_empty() && stderr.is_empty() {
        return Ok(None);
    }
    ensure!(code == 0, "Git could not read {key} (exit {code})");
    let bytes = stdout.as_bytes();
    ensure!(
        bytes.last() == Some(&0),
        "Git returned an unterminated value for {key}"
    );
    ensure!(
        !bytes[..bytes.len() - 1].contains(&0),
        "Git returned multiple values for {key}"
    );
    let value = std::str::from_utf8(&bytes[..bytes.len() - 1])
        .context("Git setting is not valid UTF-8")?
        .to_owned();
    validate_value(key, &value)?;
    Ok(Some(value))
}

#[cfg(any(windows, target_os = "macos", test))]
fn check_branch(
    runner: &mut impl crate::process::Runner,
    git: &Path,
    value: &str,
    cancel: &Arc<AtomicBool>,
) -> Result<()> {
    ensure!(
        !value.starts_with('-'),
        "Git default branch cannot begin with a dash"
    );
    let invocation = invocation(
        git,
        &["check-ref-format", "--branch", value].map(str::to_owned),
    );
    let output = runner.run(&invocation, cancel)?;
    ensure!(!output.cancelled, "Git branch validation was cancelled");
    ensure!(!output.timed_out, "Git branch validation timed out");
    ensure!(output.code == 0, "Git default branch name is invalid");
    Ok(())
}

#[cfg(any(windows, target_os = "macos", test))]
fn apply_with(
    profile: &Profile,
    selected: &BTreeSet<String>,
    expected_current: &Profile,
    runner: &mut impl crate::process::Runner,
    git: &Path,
    data: &Path,
    cancel: &Arc<AtomicBool>,
) -> Result<ApplyReport> {
    profile.validate()?;
    expected_current.validate()?;
    ensure!(!selected.is_empty(), "Select at least one Git setting");
    for key in selected {
        ensure!(
            profile.settings.contains_key(key),
            "Selected Git setting has no target value"
        );
        ensure!(
            KEYS.contains(&key.as_str()),
            "Selected Git setting is unsupported"
        );
    }
    ensure!(
        !cancel.load(Ordering::Acquire),
        "Git configuration apply was cancelled before review"
    );

    let requested = select(profile, selected);
    if let Some(branch) = requested.settings.get("init.defaultBranch") {
        check_branch(runner, git, branch, cancel)?;
    }
    let selected_keys: Vec<_> = selected.iter().map(String::as_str).collect();
    let before = capture_with(runner, git, &selected_keys, cancel)?;
    for key in selected {
        ensure!(
            before.settings.get(key) == expected_current.settings.get(key),
            "Git setting {key} changed after review; refresh the current comparison"
        );
    }

    let history = crate::environment::create_history(data)?;
    studio_core::export_document(&before, &history.join("before.json"))?;
    studio_core::export_document(&requested, &history.join("request.json"))?;

    let mut operation_error = None;
    for key in selected {
        if cancel.load(Ordering::Acquire) {
            operation_error = Some("Git configuration apply was cancelled".to_owned());
            break;
        }
        let Some(value) = requested.settings.get(key) else {
            operation_error = Some(format!("Selected Git setting {key} has no target value"));
            break;
        };
        if before.settings.get(key) == Some(value) {
            continue;
        }
        match read_value(runner, git, key, cancel) {
            Ok(current) if current == expected_current.settings.get(key).cloned() => {}
            Ok(_) => {
                operation_error = Some(format!(
                    "Git setting {key} changed during apply; later settings were not written"
                ));
                break;
            }
            Err(error) => {
                operation_error =
                    Some(format!("Could not recheck {key} before writing: {error:#}"));
                break;
            }
        }
        let invocation = invocation(
            git,
            &[
                "config",
                "--global",
                "--no-includes",
                "--replace-all",
                key,
                value,
            ]
            .map(str::to_owned),
        );
        match runner.run(&invocation, cancel) {
            Ok(output) if output.code == 0 && !output.cancelled && !output.timed_out => {}
            Ok(output) if output.cancelled => {
                operation_error = Some(format!("Apply was cancelled while writing {key}"));
                break;
            }
            Ok(output) if output.timed_out => {
                operation_error = Some(format!(
                    "Writing {key} timed out; its final state is uncertain"
                ));
                break;
            }
            Ok(output) => {
                operation_error = Some(format!("Git could not write {key} (exit {})", output.code));
                break;
            }
            Err(error) => {
                operation_error = Some(format!("Git could not write {key}: {error:#}"));
                break;
            }
        }
    }

    let verify_cancel = Arc::new(AtomicBool::new(false));
    let after = capture_with(runner, git, &selected_keys, &verify_cancel);
    if let Ok(after) = &after {
        studio_core::export_document(after, &history.join("after.json"))?;
    }
    let verified = operation_error.is_none()
        && after
            .as_ref()
            .is_ok_and(|actual| actual.settings == requested.settings);
    let cancelled = cancel.load(Ordering::Acquire);
    let changed = after.as_ref().map_or_else(
        |_| Vec::new(),
        |actual| {
            selected
                .iter()
                .filter(|key| before.settings.get(*key) != actual.settings.get(*key))
                .cloned()
                .collect()
        },
    );
    let detail = if verified {
        "All selected Git settings match the requested values.".to_owned()
    } else if let Some(error) = operation_error {
        format!("{error}; later settings were not written.")
    } else if let Err(error) = &after {
        format!("Git readback failed: {error:#}")
    } else {
        "Git readback did not match the requested values.".to_owned()
    };
    let report = ApplyReport {
        schema_version: SCHEMA_VERSION,
        selected: selected.iter().cloned().collect(),
        changed,
        verified,
        cancelled,
        detail,
        history: history.clone(),
    };
    studio_core::export_document(&report, &history.join("result.json"))
        .with_context(|| format!("Save Git configuration result in {}", history.display()))?;
    Ok(report)
}

#[cfg(any(windows, target_os = "macos", test))]
fn select(profile: &Profile, selected: &BTreeSet<String>) -> Profile {
    Profile {
        schema_version: profile.schema_version,
        settings: profile
            .settings
            .iter()
            .filter(|(key, _)| selected.contains(*key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    }
}

#[cfg(any(windows, target_os = "macos", test))]
fn invocation(program: &Path, args: &[String]) -> crate::process::Invocation {
    crate::process::Invocation {
        program: program.to_path_buf(),
        args: args.to_vec(),
        vscode_node: false,
        npm_node: false,
    }
}

#[cfg(any(windows, target_os = "macos"))]
struct NativeRunner;

#[cfg(any(windows, target_os = "macos"))]
impl crate::process::Runner for NativeRunner {
    fn run(
        &mut self,
        invocation: &crate::process::Invocation,
        cancel: &Arc<AtomicBool>,
    ) -> Result<crate::process::Output> {
        crate::process::run_git_config(invocation, cancel, GIT_TIMEOUT)
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn git_program() -> Result<PathBuf> {
    let executable = if cfg!(windows) { "git.exe" } else { "git" };
    let mut candidates = Vec::new();
    #[cfg(target_os = "macos")]
    candidates.extend([
        PathBuf::from("/opt/homebrew/bin/git"),
        PathBuf::from("/usr/local/bin/git"),
        PathBuf::from("/usr/bin/git"),
    ]);
    #[cfg(windows)]
    {
        if let Some(program_files) = std::env::var_os("ProgramFiles") {
            candidates.push(PathBuf::from(program_files).join("Git/cmd/git.exe"));
        }
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            candidates.push(PathBuf::from(local_app_data).join("Programs/Git/cmd/git.exe"));
        }
        if let Some(home) = std::env::var_os("USERPROFILE") {
            candidates.push(PathBuf::from(home).join("scoop/shims/git.exe"));
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(
            std::env::split_paths(&path)
                .filter(|directory| directory.is_absolute())
                .map(|directory| directory.join(executable)),
        );
    }
    for candidate in candidates {
        let Ok(program) = candidate.canonicalize() else {
            continue;
        };
        if fs::metadata(&program).is_ok_and(|metadata| metadata.is_file()) {
            return Ok(program);
        }
    }
    bail!("Install Git in a standard location or add its executable to PATH")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Result<Self> {
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
            let path = std::env::temp_dir().join(format!(
                "devenv-git-profile-{}-{now}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path)?;
            Ok(Self(path))
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[derive(Default)]
    struct FakeRunner {
        values: BTreeMap<String, String>,
        calls: Vec<Vec<String>>,
        fail_write: Option<String>,
        invalid_branch: bool,
    }

    impl crate::process::Runner for FakeRunner {
        fn run(
            &mut self,
            invocation: &crate::process::Invocation,
            _: &Arc<AtomicBool>,
        ) -> Result<crate::process::Output> {
            self.calls.push(invocation.args.clone());
            let args = &invocation.args;
            let output = |code, stdout: String, stderr: String| crate::process::Output {
                code,
                stdout,
                stderr,
                cancelled: false,
                timed_out: false,
            };
            if args.first().is_some_and(|arg| arg == "check-ref-format") {
                return Ok(output(
                    if self.invalid_branch { 1 } else { 0 },
                    String::new(),
                    String::new(),
                ));
            }
            if args.iter().any(|arg| arg == "--get") {
                let key = args.last().context("Missing setting name")?;
                return Ok(match self.values.get(key) {
                    Some(value) => output(0, format!("{value}\0"), String::new()),
                    None => output(1, String::new(), String::new()),
                });
            }
            if args.iter().any(|arg| arg == "--replace-all") {
                let key = args.get(4).context("Missing setting name")?.clone();
                let value = args.get(5).context("Missing setting value")?.clone();
                if self.fail_write.as_deref() == Some(&key) {
                    return Ok(output(4, String::new(), "write denied".into()));
                }
                self.values.insert(key, value);
                return Ok(output(0, String::new(), String::new()));
            }
            bail!("Unexpected fake Git invocation")
        }
    }

    fn profile(entries: &[(&str, &str)]) -> Profile {
        Profile {
            schema_version: SCHEMA_VERSION,
            settings: entries
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
        }
    }

    fn selected(keys: &[&str]) -> BTreeSet<String> {
        keys.iter().map(|key| (*key).to_owned()).collect()
    }

    #[test]
    fn profile_roundtrip_is_allowlisted_bounded_and_no_clobber() -> Result<()> {
        let directory = TestDirectory::new()?;
        let path = directory.0.join("git-profile.json");
        let profile = profile(&[("user.name", "Example User"), ("core.autocrlf", "input")]);
        save(&profile, &path)?;
        assert_eq!(load(&path)?, profile);
        assert!(save(&profile, &path).is_err());
        assert!(
            Profile {
                settings: BTreeMap::from([("credential.helper".into(), "store".into())]),
                ..Profile::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            Profile {
                settings: BTreeMap::from([("core.autocrlf".into(), "unsafe".into())]),
                ..Profile::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            serde_json::from_str::<Profile>(r#"{"schema_version":1,"settings":{},"aliases":{}}"#)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn capture_reads_only_the_supported_global_keys() -> Result<()> {
        let mut runner = FakeRunner {
            values: BTreeMap::from([
                ("user.name".into(), "Example User".into()),
                ("user.email".into(), "example@example.test".into()),
                ("credential.helper".into(), "store".into()),
            ]),
            ..FakeRunner::default()
        };
        let git = Path::new("/usr/bin/git");
        let cancel = Arc::new(AtomicBool::new(false));
        let captured = capture_with(&mut runner, git, &KEYS, &cancel)?;
        assert_eq!(captured.settings.len(), 2);
        assert!(captured.settings.contains_key("user.name"));
        assert!(captured.settings.contains_key("user.email"));
        assert_eq!(runner.calls.len(), KEYS.len());
        assert!(
            runner
                .calls
                .iter()
                .all(|args| args.contains(&"--no-includes".into()))
        );
        Ok(())
    }

    #[test]
    fn stale_review_stops_before_any_write() -> Result<()> {
        let directory = TestDirectory::new()?;
        let target = profile(&[("user.name", "New Name")]);
        let expected = profile(&[("user.name", "Old Name")]);
        let mut runner = FakeRunner {
            values: BTreeMap::from([("user.name".into(), "Changed Elsewhere".into())]),
            ..FakeRunner::default()
        };
        let result = apply_with(
            &target,
            &selected(&["user.name"]),
            &expected,
            &mut runner,
            Path::new("/usr/bin/git"),
            &directory.0,
            &Arc::new(AtomicBool::new(false)),
        );
        assert!(result.is_err());
        assert!(
            runner
                .calls
                .iter()
                .all(|args| !args.iter().any(|arg| arg == "--replace-all"))
        );
        assert_eq!(fs::read_dir(&directory.0)?.count(), 0);
        Ok(())
    }

    #[test]
    fn selected_settings_are_journaled_and_verified() -> Result<()> {
        let directory = TestDirectory::new()?;
        let target = profile(&[("user.name", "New Name"), ("fetch.prune", "true")]);
        let expected = profile(&[("user.name", "Old Name"), ("fetch.prune", "false")]);
        let mut runner = FakeRunner {
            values: BTreeMap::from([
                ("user.name".into(), "Old Name".into()),
                ("fetch.prune".into(), "false".into()),
            ]),
            ..FakeRunner::default()
        };
        let report = apply_with(
            &target,
            &selected(&["user.name"]),
            &expected,
            &mut runner,
            Path::new("/usr/bin/git"),
            &directory.0,
            &Arc::new(AtomicBool::new(false)),
        )?;
        assert!(report.verified);
        assert_eq!(report.changed, vec!["user.name".to_owned()]);
        assert_eq!(
            runner.values.get("user.name").map(String::as_str),
            Some("New Name")
        );
        assert_eq!(
            runner.values.get("fetch.prune").map(String::as_str),
            Some("false")
        );
        assert_eq!(
            load(&report.history.join("request.json"))?.settings,
            target
                .settings
                .into_iter()
                .filter(|(key, _)| key == "user.name")
                .collect()
        );
        assert!(report.history.join("before.json").is_file());
        assert!(report.history.join("after.json").is_file());
        assert!(report.history.join("result.json").is_file());
        Ok(())
    }

    #[test]
    fn partial_failure_stops_later_writes_and_records_readback() -> Result<()> {
        let directory = TestDirectory::new()?;
        let target = profile(&[("core.autocrlf", "true"), ("user.name", "New Name")]);
        let expected = profile(&[("core.autocrlf", "false"), ("user.name", "Old Name")]);
        let mut runner = FakeRunner {
            values: BTreeMap::from([
                ("core.autocrlf".into(), "false".into()),
                ("user.name".into(), "Old Name".into()),
            ]),
            fail_write: Some("user.name".into()),
            ..FakeRunner::default()
        };
        let report = apply_with(
            &target,
            &selected(&["core.autocrlf", "user.name"]),
            &expected,
            &mut runner,
            Path::new("/usr/bin/git"),
            &directory.0,
            &Arc::new(AtomicBool::new(false)),
        )?;
        assert!(!report.verified);
        assert!(report.detail.contains("could not write user.name"));
        assert_eq!(report.changed, vec!["core.autocrlf".to_owned()]);
        assert_eq!(
            runner.values.get("core.autocrlf").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            runner.values.get("user.name").map(String::as_str),
            Some("Old Name")
        );
        assert!(
            load(&report.history.join("after.json"))?
                .settings
                .contains_key("core.autocrlf")
        );
        Ok(())
    }

    #[test]
    fn invalid_default_branch_is_rejected_before_any_configuration_write() -> Result<()> {
        let directory = TestDirectory::new()?;
        let target = profile(&[("init.defaultBranch", "bad branch")]);
        let mut runner = FakeRunner {
            invalid_branch: true,
            ..FakeRunner::default()
        };
        let result = apply_with(
            &target,
            &selected(&["init.defaultBranch"]),
            &Profile::default(),
            &mut runner,
            Path::new("/usr/bin/git"),
            &directory.0,
            &Arc::new(AtomicBool::new(false)),
        );
        assert!(result.is_err());
        assert!(
            runner
                .calls
                .iter()
                .all(|args| !args.iter().any(|arg| arg == "--replace-all"))
        );
        assert_eq!(fs::read_dir(&directory.0)?.count(), 0);
        Ok(())
    }
}
