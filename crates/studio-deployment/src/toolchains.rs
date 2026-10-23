//! Portable Rust channel requests; native execution never bootstraps rustup or selects a default.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::Read,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RustRequest {
    pub channel: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RustReport {
    pub requested: String,
    pub verified: bool,
    pub cancelled: bool,
    pub resolved_release: Option<String>,
    pub commit_hash: Option<String>,
    pub commit_date: Option<String>,
    pub detail: String,
}
pub fn validate(request: &RustRequest) -> Result<()> {
    let value = &request.channel;
    ensure!(
        value.len() <= 32
            && (matches!(value.as_str(), "stable" | "beta" | "nightly")
                || numeric_release(value)
                || value.strip_prefix("nightly-").is_some_and(valid_date)),
        "Choose stable, beta, nightly, an exact x.y.z release, or nightly-YYYY-MM-DD"
    );
    Ok(())
}
fn numeric_release(value: &str) -> bool {
    let parts: Vec<_> = value.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.len() <= 5
                && part.bytes().all(|b| b.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0'))
        })
}
fn valid_date(value: &str) -> bool {
    if value.len() != 10
        || value.as_bytes()[4] != b'-'
        || value.as_bytes()[7] != b'-'
        || !value
            .bytes()
            .enumerate()
            .all(|(i, b)| matches!(i, 4 | 7) || b.is_ascii_digit())
    {
        return false;
    }
    let year = value[0..4].parse::<u32>().unwrap_or(0);
    let month = value[5..7].parse::<usize>().unwrap_or(0);
    let day = value[8..10].parse::<u32>().unwrap_or(0);
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    year > 0 && (1..=12).contains(&month) && day > 0 && day <= days[month - 1]
}
pub fn rust_toolchain_toml(request: &RustRequest) -> Result<String> {
    validate(request)?;
    Ok(format!(
        "[toolchain]\nchannel = \"{}\"\nprofile = \"minimal\"\n",
        request.channel
    ))
}
pub fn mise_toml(request: &RustRequest) -> Result<String> {
    validate(request)?;
    Ok(format!(
        "# Version policy only; generate and review mise.lock with mise separately.\n[tools]\nrust = {{ version = \"{}\", profile = \"minimal\" }}\n",
        request.channel
    ))
}
pub fn save(request: &RustRequest, path: &Path) -> Result<()> {
    validate(request)?;
    studio_core::export_document(request, path)
}
pub fn load(path: &Path) -> Result<RustRequest> {
    let metadata = fs::symlink_metadata(path).context("Read Rust request metadata")?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Rust request must be a regular file, not a symlink"
    );
    let file = File::open(path).context("Open Rust request")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened = file.metadata()?;
        ensure!(
            opened.dev() == metadata.dev() && opened.ino() == metadata.ino(),
            "Rust request changed while opening"
        );
    }
    let mut bytes = Vec::new();
    file.take(studio_core::MAX_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= studio_core::MAX_INPUT_BYTES,
        "Rust request exceeds 1 MiB limit"
    );
    let request: RustRequest =
        serde_json::from_slice(&bytes).context("Invalid Rust request JSON")?;
    validate(&request)?;
    Ok(request)
}

/// Lists supported portable channel names only; custom links and foreign host toolchains are omitted.
pub fn capture(cancel: Arc<AtomicBool>) -> Result<Vec<String>> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        let program = rustup_program()?;
        let text = step(
            &mut Native(std::time::Duration::from_secs(30)),
            &program,
            &["toolchain", "list"],
            &cancel,
        )?;
        parse_list(&text)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = cancel;
        anyhow::bail!("Native Rust inventory requires macOS or Windows")
    }
}
pub fn configure(request: RustRequest, cancel: Arc<AtomicBool>) -> Result<RustReport> {
    validate(&request)?;
    #[cfg(any(windows, target_os = "macos"))]
    {
        let program = rustup_program()?;
        let history = crate::environment::create_history(&studio_platform::data_directory()?)?;
        crate::network::record_current(&history.join("network.json"))?;
        let mut report = run_recorded(
            &mut Native(std::time::Duration::from_secs(90 * 60)), &program, &request, &cancel,
            &mut |name, value| studio_core::export_document(value, &history.join(name)),
        ).with_context(|| format!(
            "Rust deployment evidence: {}. Partial installation may exist; inspect before retrying.", history.display()
        ))?;
        report
            .detail
            .push_str(&format!(" Execution evidence: {}", history.display()));
        Ok(report)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = cancel;
        anyhow::bail!("Native Rust deployment requires macOS or Windows")
    }
}
#[cfg(any(windows, target_os = "macos"))]
fn rustup_program() -> Result<std::path::PathBuf> {
    let variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let home = std::path::PathBuf::from(
        std::env::var_os(variable).context("Home directory is unavailable")?,
    );
    ensure!(
        home.is_absolute() && home.is_dir(),
        "A standard absolute home directory is required"
    );
    let program = home.join(".cargo/bin").join(if cfg!(windows) {
        "rustup.exe"
    } else {
        "rustup"
    });
    ensure!(
        program.is_file(),
        "Install rustup in the standard user .cargo/bin location first; no bootstrap is performed"
    );
    Ok(program)
}
#[cfg(any(windows, target_os = "macos"))]
struct Native(std::time::Duration);
#[cfg(any(windows, target_os = "macos"))]
impl crate::process::Runner for Native {
    fn run(
        &mut self,
        invocation: &crate::process::Invocation,
        cancel: &Arc<AtomicBool>,
    ) -> Result<crate::process::Output> {
        crate::process::run_toolchain(invocation, cancel, self.0)
    }
}
#[cfg(any(windows, target_os = "macos", test))]
#[derive(Debug)]
struct Cancelled;
#[cfg(any(windows, target_os = "macos", test))]
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Rust operation cancelled")
    }
}
#[cfg(any(windows, target_os = "macos", test))]
impl std::error::Error for Cancelled {}
#[cfg(any(windows, target_os = "macos", test))]
fn step(
    runner: &mut impl crate::process::Runner,
    program: &Path,
    args: &[&str],
    cancel: &Arc<AtomicBool>,
) -> Result<String> {
    use std::sync::atomic::Ordering;
    ensure!(program.is_absolute(), "Rustup location must be absolute");
    if cancel.load(Ordering::Acquire) {
        return Err(Cancelled.into());
    }
    let output = runner.run(
        &crate::process::Invocation {
            program: program.to_path_buf(),
            args: args.iter().map(|s| (*s).into()).collect(),
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
        "Rustup command timed out; installed state is unverified"
    );
    ensure!(
        output.code == 0,
        "Rustup command failed; installed state is unverified"
    );
    Ok(output.stdout)
}
#[cfg(any(windows, target_os = "macos", test))]
fn portable_name(value: &str) -> Option<&str> {
    const HOSTS: &[&str] = &[
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "x86_64-pc-windows-msvc",
        "i686-pc-windows-msvc",
        "aarch64-pc-windows-msvc",
        "x86_64-pc-windows-gnu",
        "i686-pc-windows-gnu",
        "aarch64-pc-windows-gnullvm",
        "x86_64-pc-windows-gnullvm",
        "i686-pc-windows-gnullvm",
    ];
    let valid = |channel: &str| {
        validate(&RustRequest {
            channel: channel.into(),
        })
        .is_ok()
    };
    if valid(value) {
        return Some(value);
    }
    HOSTS.iter().find_map(|host| {
        value
            .strip_suffix(host)
            .and_then(|s| s.strip_suffix('-'))
            .filter(|s| valid(s))
    })
}
#[cfg(any(windows, target_os = "macos", test))]
fn parse_list(text: &str) -> Result<Vec<String>> {
    ensure!(
        text.len() <= 65536 && text.lines().count() <= 256,
        "Rust inventory exceeds supported bounds"
    );
    if text.trim() == "no installed toolchains" {
        return Ok(Vec::new());
    }
    let mut names = std::collections::BTreeSet::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        ensure!(
            !line.chars().any(|c| c.is_control() && c != '\t'),
            "Invalid Rust inventory text"
        );
        let identity = line
            .split_whitespace()
            .next()
            .context("Invalid Rust inventory row")?;
        ensure!(
            identity.len() <= 128
                && identity
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)),
            "Invalid Rust toolchain identity"
        );
        let annotation = line
            .trim()
            .strip_prefix(identity)
            .unwrap_or_default()
            .trim();
        ensure!(
            matches!(
                annotation,
                "" | "(default)" | "(active)" | "(active, default)" | "(default, active)"
            ),
            "Unexpected Rust inventory annotation"
        );
        if let Some(name) = portable_name(identity) {
            names.insert(name.to_owned());
        }
    }
    Ok(names.into_iter().collect())
}
#[cfg(any(windows, target_os = "macos", test))]
fn read_default(text: &str) -> Result<&str> {
    let value = text.trim().strip_suffix(" (default)").context(
        "Rust default is unavailable or unsupported; configure a default in rustup first",
    )?;
    ensure!(
        portable_name(value).is_some(),
        "An existing official macOS or Windows default toolchain is required"
    );
    Ok(value)
}
#[cfg(any(windows, target_os = "macos", test))]
fn compiler_evidence(text: &str, request: &RustRequest) -> Result<(String, String, String)> {
    ensure!(
        text.len() <= 65536 && text.starts_with("rustc "),
        "Invalid compiler readback"
    );
    let field = |name: &str| -> Result<&str> {
        let mut values = text.lines().filter_map(|line| line.strip_prefix(name));
        let value = values.next().context("Compiler evidence is incomplete")?;
        ensure!(values.next().is_none(), "Duplicate compiler evidence field");
        Ok(value)
    };
    let release = field("release: ")?;
    ensure!(
        text.lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            == Some(release),
        "Compiler version header disagrees with release field"
    );
    let hash = field("commit-hash: ")?;
    let date = field("commit-date: ")?;
    let (base, suffix) = release.split_once('-').unwrap_or((release, ""));
    ensure!(
        numeric_release(base)
            && hash.len() == 40
            && hash.bytes().all(|b| b.is_ascii_hexdigit())
            && valid_date(date),
        "Invalid compiler identity"
    );
    let matches = match request.channel.as_str() {
        "stable" => suffix.is_empty(),
        "beta" => suffix
            .strip_prefix("beta.")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())),
        "nightly" => suffix == "nightly",
        value if value.starts_with("nightly-") => suffix == "nightly",
        value => release == value,
    };
    ensure!(
        matches,
        "Installed compiler does not match the requested release channel"
    );
    Ok((release.into(), hash.into(), date.into()))
}
#[cfg(any(windows, target_os = "macos", test))]
fn run_recorded(
    runner: &mut impl crate::process::Runner,
    program: &Path,
    request: &RustRequest,
    cancel: &Arc<AtomicBool>,
    persist: &mut impl FnMut(&str, &serde_json::Value) -> Result<()>,
) -> Result<RustReport> {
    validate(request)?;
    let before = parse_list(&step(runner, program, &["toolchain", "list"], cancel)?)?;
    let default_text = step(runner, program, &["default"], cancel)?;
    let default = read_default(&default_text)?;
    ensure!(
        before
            .iter()
            .any(|name| Some(name.as_str()) == portable_name(default)),
        "Existing default toolchain is missing from the baseline"
    );
    persist(
        "before.json",
        &serde_json::json!({
            "schema_version":1, "provider":"rustup", "supported_channels":before,
            "default":default, "custom_links_and_foreign_hosts_included":false,
        }),
    )
    .context("Persist Rust baseline before installation")?;
    persist("request.json", &serde_json::json!({
        "schema_version":1, "event":"started", "provider":"rustup", "requested":request,
        "profile":"minimal", "self_update":false,
        "completion":"Unknown until result.json is present. No automatic rollback is available.",
    })).context("Persist Rust request before installation")?;
    let result = (|| {
        step(
            runner,
            program,
            &[
                "toolchain",
                "install",
                &request.channel,
                "--profile",
                "minimal",
                "--no-self-update",
            ],
            cancel,
        )?;
        let installed = parse_list(&step(runner, program, &["toolchain", "list"], cancel)?)?;
        ensure!(
            installed.contains(&request.channel),
            "Requested toolchain identity is missing after installation"
        );
        ensure!(
            read_default(&step(runner, program, &["default"], cancel)?)? == default,
            "Rust default changed; inspect the environment before retrying"
        );
        let compiler = step(
            runner,
            program,
            &["run", &request.channel, "rustc", "--version", "--verbose"],
            cancel,
        )?;
        compiler_evidence(&compiler, request)
    })();
    let cancelled = result.as_ref().is_err_and(|e| e.is::<Cancelled>());
    let detail = if result.is_ok() {
        "Requested toolchain presence and compiler release/hash were verified. Rolling channels may resolve differently next time; project builds and bit-identical restoration were not verified."
    } else {
        "Rust deployment did not complete with verified compiler identity. Partial changes or running descendants may remain; inspect before retrying. No automatic rollback is available."
    };
    let mut report = RustReport {
        requested: request.channel.clone(),
        verified: result.is_ok(),
        cancelled,
        resolved_release: None,
        commit_hash: None,
        commit_date: None,
        detail: detail.into(),
    };
    if let Ok((release, hash, date)) = &result {
        report.resolved_release = Some(release.clone());
        report.commit_hash = Some(hash.clone());
        report.commit_date = Some(date.clone());
    }
    persist("result.json", &serde_json::json!({
        "schema_version":1, "event":"finished",
        "status":if report.verified { "verified" } else if cancelled { "cancelled" } else { "failed" },
        "report":report,
    })).context("Terminal Rust evidence could not be persisted; the started record remains unresolved")?;
    if cancelled {
        return Ok(report);
    }
    result?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{Invocation, Output, Runner};
    use std::{collections::VecDeque, path::PathBuf};
    #[test]
    fn request_rejects_paths_flags_host_suffixes_invalid_dates_and_unknown_fields() {
        for channel in ["stable", "beta", "nightly", "1.91.0", "nightly-2024-02-29"] {
            assert!(
                validate(&RustRequest {
                    channel: channel.into()
                })
                .is_ok(),
                "{channel}"
            );
        }
        for channel in [
            "",
            "latest",
            "HEAD",
            "my-link",
            "1.91",
            "01.2.3",
            "--help",
            "stable-x86_64-pc-windows-msvc",
            "../nightly",
            "https://x",
            "nightly\n1.2.3",
            "nightly-2025-02-29",
            "nightly-2024-13-01",
            "nightly-0000-01-01",
            "nightly-２０２４-01-01",
        ] {
            assert!(
                validate(&RustRequest {
                    channel: channel.into()
                })
                .is_err(),
                "{channel}"
            );
        }
        for json in [
            r#"{"channel":"stable","path":"/tmp"}"#,
            r#"{"channel":"stable","channel":"nightly"}"#,
        ] {
            assert!(serde_json::from_str::<RustRequest>(json).is_err());
        }
        let request = RustRequest {
            channel: "nightly-2024-02-29".into(),
        };
        assert_eq!(
            rust_toolchain_toml(&request).unwrap(),
            "[toolchain]\nchannel = \"nightly-2024-02-29\"\nprofile = \"minimal\"\n"
        );
        assert!(
            mise_toml(&request)
                .unwrap()
                .contains("rust = { version = \"nightly-2024-02-29\", profile = \"minimal\" }")
        );
    }
    #[test]
    fn request_files_roundtrip_without_overwrite_and_reject_oversize_or_symlinks() -> Result<()> {
        struct Directory(PathBuf);
        impl Drop for Directory {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let dir = Directory(std::env::temp_dir().join(format!(
                "studio-rust-request-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos()
            )));
        fs::create_dir(&dir.0)?;
        let path = dir.0.join("request.json");
        let request = RustRequest {
            channel: "nightly".into(),
        };
        save(&request, &path)?;
        assert_eq!(load(&path)?, request);
        assert!(save(&request, &path).is_err());
        let huge = dir.0.join("huge.json");
        fs::write(&huge, vec![b' '; studio_core::MAX_INPUT_BYTES as usize + 1])?;
        assert!(load(&huge).is_err());
        #[cfg(unix)]
        {
            let link = dir.0.join("link.json");
            std::os::unix::fs::symlink(&path, &link)?;
            assert!(load(&link).is_err());
        }
        Ok(())
    }
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
    fn compiler(release: &str) -> Output {
        output(&format!(
            "rustc {release} (abc 2024-02-28)\nrelease: {release}\ncommit-hash: 0123456789abcdef0123456789abcdef01234567\ncommit-date: 2024-02-28\n"
        ))
    }
    fn fake() -> Fake {
        Fake {
            calls: Vec::new(),
            outputs: VecDeque::from([
                output("stable-aarch64-apple-darwin (default)\n"),
                output("stable-aarch64-apple-darwin (default)\n"),
                output(""),
                output(
                    "stable-aarch64-apple-darwin (default)\nnightly-2024-02-29-aarch64-apple-darwin\n",
                ),
                output("stable-aarch64-apple-darwin (default)\n"),
                compiler("1.78.0-nightly"),
            ]),
        }
    }
    fn run(
        fake: &mut Fake,
        persist: &mut impl FnMut(&str, &serde_json::Value) -> Result<()>,
    ) -> Result<RustReport> {
        run_recorded(
            fake,
            &PathBuf::from(if cfg!(windows) {
                "C:\\rustup.exe"
            } else {
                "/rustup"
            }),
            &RustRequest {
                channel: "nightly-2024-02-29".into(),
            },
            &Arc::new(AtomicBool::new(false)),
            persist,
        )
    }
    #[test]
    fn successful_deployment_records_policy_and_independent_dated_channel_compiler_evidence() {
        let mut fake = fake();
        let mut records = Vec::new();
        let report = run(&mut fake, &mut |name, value| {
            records.push((name.to_owned(), value.clone()));
            Ok(())
        })
        .unwrap();
        assert!(report.verified && !report.cancelled);
        assert_eq!(report.resolved_release.as_deref(), Some("1.78.0-nightly"));
        assert_eq!(report.commit_date.as_deref(), Some("2024-02-28"));
        assert_eq!(
            records
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            ["before.json", "request.json", "result.json"]
        );
        assert_eq!(
            fake.calls[2].args,
            [
                "toolchain",
                "install",
                "nightly-2024-02-29",
                "--profile",
                "minimal",
                "--no-self-update"
            ]
        );
        assert_eq!(
            fake.calls[5].args,
            [
                "run",
                "nightly-2024-02-29",
                "rustc",
                "--version",
                "--verbose"
            ]
        );
        assert!(fake.calls.iter().all(|call| {
            !call
                .args
                .iter()
                .any(|v| matches!(v.as_str(), "uninstall" | "override" | "--default" | "self"))
        }));
    }
    #[test]
    fn persistence_failure_stops_install_and_failed_readback_never_verifies() {
        for fail_at in ["before.json", "request.json"] {
            let mut fake = fake();
            assert!(
                run(&mut fake, &mut |name, _| {
                    ensure!(name != fail_at, "disk full");
                    Ok(())
                })
                .is_err()
            );
            assert_eq!(fake.calls.len(), 2);
        }
        let mut fake = self::fake();
        fake.outputs[5] = compiler("1.78.0");
        let mut final_record = None;
        assert!(
            run(&mut fake, &mut |name, value| {
                if name == "result.json" {
                    final_record = Some(value.clone());
                }
                Ok(())
            })
            .is_err()
        );
        assert_eq!(final_record.unwrap()["status"], "failed");
        let mut fake = self::fake();
        assert!(
            run(&mut fake, &mut |name, _| {
                ensure!(name != "result.json", "disk full");
                Ok(())
            })
            .is_err()
        );
    }
    #[test]
    fn wrong_identity_no_default_command_failure_timeout_and_cancel_fail_closed() {
        for index in [0, 1, 2, 3, 4, 5] {
            let mut fake = fake();
            fake.outputs[index].code = 1;
            assert!(run(&mut fake, &mut |_, _| Ok(())).is_err());
            assert!(fake.calls.len() <= index + 1);
        }
        let mut fake = fake();
        fake.outputs[3] = output("nightly-aarch64-apple-darwin\n");
        assert!(run(&mut fake, &mut |_, _| Ok(())).is_err());
        let mut fake = self::fake();
        fake.outputs[2].timed_out = true;
        assert!(run(&mut fake, &mut |_, _| Ok(())).is_err());
        let mut fake = self::fake();
        fake.outputs[2].cancelled = true;
        let report = run(&mut fake, &mut |_, _| Ok(())).unwrap();
        assert!(report.cancelled && !report.verified);
        assert_eq!(fake.calls.len(), 3);
        let mut fake = self::fake();
        fake.outputs[4] = output("nightly-aarch64-apple-darwin (default)\n");
        assert!(run(&mut fake, &mut |_, _| Ok(())).is_err());
    }
    #[test]
    fn inventory_omits_custom_and_foreign_toolchains_and_evidence_checks_channel() {
        assert_eq!(parse_list("stable-x86_64-pc-windows-msvc (default)\nnightly-2024-02-29-aarch64-apple-darwin\ncustom\nstable-x86_64-unknown-linux-gnu\n").unwrap(),["nightly-2024-02-29","stable"]);
        assert!(read_default("no default toolchain is configured").is_err());
        assert!(parse_list("stable\u{1b}[0m").is_err());
        assert!(parse_list("stable unexpected stdout").is_err());
        assert!(parse_list("custom /private/path").is_err());
        for (channel, release, expected) in [
            ("stable", "1.91.0", true),
            ("beta", "1.92.0-beta.3", true),
            ("nightly", "1.93.0-nightly", true),
            ("1.91.0", "1.91.1", false),
            ("stable", "1.93.0-nightly", false),
        ] {
            assert_eq!(
                compiler_evidence(
                    &compiler(release).stdout,
                    &RustRequest {
                        channel: channel.into()
                    }
                )
                .is_ok(),
                expected
            );
        }
        assert!(
            compiler_evidence(
                "rustc fake\nrelease: 1.91.0\ncommit-hash: unknown\ncommit-date: unknown\n",
                &RustRequest {
                    channel: "stable".into()
                }
            )
            .is_err()
        );
    }
}
