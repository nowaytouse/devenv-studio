use super::*;
#[cfg(any(target_os = "macos", test))]
use crate::process::{Invocation, Output, Runner};
use serde::de::{MapAccess, Visitor};
use std::collections::BTreeMap;
#[cfg(any(target_os = "macos", test))]
use std::{path::PathBuf, sync::atomic::Ordering};

#[cfg(target_os = "macos")]
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
#[cfg(target_os = "macos")]
struct Native;
#[cfg(target_os = "macos")]
impl Runner for Native {
    fn run(&mut self, invocation: &Invocation, cancel: &Arc<AtomicBool>) -> Result<Output> {
        crate::process::run_bounded(invocation, cancel, TIMEOUT)
    }
}
#[cfg(any(target_os = "macos", test))]
struct Request {
    kind: &'static str,
    invocation: Option<Invocation>,
}
#[cfg(any(target_os = "macos", test))]
fn invocation(program: PathBuf, args: &[&str]) -> Invocation {
    Invocation {
        program,
        args: args.iter().map(|arg| (*arg).into()).collect(),
        vscode_node: false,
        npm_node: false,
    }
}
#[cfg(any(target_os = "macos", test))]
fn first(paths: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    paths
        .into_iter()
        .find(|path| path.is_absolute() && path.is_file())
}
#[cfg(any(target_os = "macos", test))]
pub(super) fn brew_runtime_ready(brew: &Path) -> bool {
    let check = || -> Result<bool> {
        let executable = brew.canonicalize()?;
        let repository = executable
            .parent()
            .and_then(Path::parent)
            .context("Invalid Homebrew location")?;
        let vendor = repository.join("Library/Homebrew/vendor");
        let mut version = String::new();
        File::open(vendor.join("portable-ruby-version"))?
            .take(65)
            .read_to_string(&mut version)?;
        let version = version.trim();
        ensure!(
            version.len() <= 64 && valid_version(version),
            "Invalid Homebrew runtime version"
        );
        let current = vendor.join("portable-ruby/current").canonicalize()?;
        let expected = vendor.join("portable-ruby").join(version).canonicalize()?;
        let metadata = fs::metadata(current.join("bin/ruby"))?;
        #[cfg(unix)]
        let executable = {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode() & 0o111 != 0
        };
        #[cfg(not(unix))]
        let executable = true;
        Ok(current == expected && metadata.is_file() && executable)
    };
    // Homebrew bootstraps/updates portable Ruby even for list; skip rather than trigger a download.
    check().unwrap_or(false)
}
#[cfg(any(target_os = "macos", test))]
pub(super) fn uv_program(home: &Path) -> Option<PathBuf> {
    first([
        home.join(".local/bin/uv"),
        home.join(".cargo/bin/uv"),
        "/opt/homebrew/bin/uv".into(),
        "/usr/local/bin/uv".into(),
    ])
}
#[cfg(any(target_os = "macos", test))]
fn requests(home: &Path) -> Vec<Request> {
    let brew = first([
        "/opt/homebrew/bin/brew".into(),
        "/usr/local/bin/brew".into(),
    ])
    .filter(|path| brew_runtime_ready(path));
    let cargo = first([home.join(".cargo/bin/cargo")]);
    let uv = uv_program(home);
    let npm = ["/opt/homebrew", "/usr/local"]
        .into_iter()
        .find_map(|prefix| {
            let prefix = PathBuf::from(prefix);
            let node = prefix.join("bin/node");
            let cli = prefix.join("lib/node_modules/npm/bin/npm-cli.js");
            if !node.is_file() || !cli.is_file() {
                return None;
            }
            let mut result = invocation(node, &[]);
            result.npm_node = true;
            result.args = vec![
                cli.to_string_lossy().into_owned(),
                "ls".into(),
                "--global".into(),
                "--json".into(),
                "--depth=0".into(),
                "--long".into(),
                "--ignore-scripts".into(),
                "--offline".into(),
                "--no-progress".into(),
                "--color=false".into(),
                "--no-update-notifier".into(),
                "--no-audit".into(),
                "--no-fund".into(),
                "--userconfig=/dev/null".into(),
                // npm rejects two sources with the same path; /dev is not user writable.
                "--globalconfig=/dev/null.ltsc-global".into(),
                format!("--prefix={}", prefix.display()),
            ];
            Some(result)
        });
    let vscode = [
        PathBuf::from("/Applications/Visual Studio Code.app"),
        home.join("Applications/Visual Studio Code.app"),
    ]
    .into_iter()
    .find_map(|app| {
        let electron = app.join("Contents/MacOS/Electron");
        let cli = app.join("Contents/Resources/app/out/cli.js");
        if !electron.is_file() || !cli.is_file() {
            return None;
        }
        let mut result = invocation(electron, &[]);
        result.vscode_node = true;
        result.args = vec![
            cli.to_string_lossy().into_owned(),
            "--list-extensions".into(),
            "--show-versions".into(),
        ];
        Some(result)
    });
    vec![
        Request {
            kind: "brew_formula",
            invocation: brew
                .clone()
                .map(|p| invocation(p, &["list", "--formula", "--versions"])),
        },
        Request {
            kind: "brew_cask",
            invocation: brew.map(|p| invocation(p, &["list", "--cask", "--versions"])),
        },
        Request {
            kind: "cargo",
            invocation: cargo.map(|p| {
                let mut result = invocation(p, &["install", "--list", "--root"]);
                result
                    .args
                    .push(home.join(".cargo").to_string_lossy().into_owned());
                result
            }),
        },
        Request {
            kind: "uv",
            invocation: uv.map(|p| {
                invocation(
                    p,
                    &[
                        "tool",
                        "list",
                        "--no-config",
                        "--no-progress",
                        "--color",
                        "never",
                    ],
                )
            }),
        },
        Request {
            kind: "npm",
            invocation: npm,
        },
        Request {
            kind: "vscode",
            invocation: vscode,
        },
    ]
}
#[cfg(target_os = "macos")]
pub(super) fn collect(cancel: Arc<AtomicBool>) -> Result<Snapshot> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("macOS home directory is unavailable")?;
    ensure!(
        home.is_absolute() && home.is_dir(),
        "macOS home directory is invalid"
    );
    let captured_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("System clock precedes Unix epoch")?
        .as_secs()
        .to_string();
    collect_with(&mut Native, requests(&home), cancel, captured_at)
}
#[cfg(any(target_os = "macos", test))]
fn collect_with(
    runner: &mut impl Runner,
    requests: Vec<Request>,
    cancel: Arc<AtomicBool>,
    captured_at: String,
) -> Result<Snapshot> {
    let mut providers = Vec::new();
    for request in requests {
        ensure!(
            !cancel.load(Ordering::Acquire),
            "Developer environment collection was cancelled"
        );
        let mut provider = ProviderSnapshot {
            kind: request.kind.into(),
            status: "unavailable".into(),
            packages: vec![],
            detail: if request.kind.starts_with("brew_") {
                BREW_UNAVAILABLE
            } else {
                UNAVAILABLE
            }
            .into(),
        };
        if let Some(invocation) = request.invocation {
            provider.status = "failed".into();
            match runner.run(&invocation, &cancel) {
                Err(_) => provider.detail = CAPTURE_FAILED.into(),
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
                        let npm_root = invocation
                            .args
                            .iter()
                            .find_map(|arg| arg.strip_prefix("--prefix="))
                            .map(|prefix| PathBuf::from(prefix).join("lib"));
                        match parse_packages(request.kind, &output.stdout, npm_root.as_deref()) {
                            Ok((packages, omitted)) => {
                                provider.packages = packages;
                                provider.status =
                                    if omitted { "failed" } else { "complete" }.into();
                                provider.detail = if omitted { OMITTED } else { COMPLETE }.into();
                            }
                            Err(_) => provider.detail = INVALID.into(),
                        }
                    }
                }
            }
        }
        providers.push(provider);
    }
    ensure!(
        !cancel.load(Ordering::Acquire),
        "Developer environment collection was cancelled"
    );
    let mut warnings = vec![INVENTORY.into(), LOCATIONS.into()];
    if providers.iter().any(|p| p.status != "complete") {
        warnings.push(PARTIAL.into());
    }
    let snapshot = Snapshot {
        schema_version: 1,
        captured_at,
        platform: "macos".into(),
        providers,
        warnings,
    };
    validate(&snapshot)?;
    Ok(snapshot)
}
fn package(kind: &str, name: &str, version: &str) -> Result<Package> {
    ensure!(
        valid_name(kind, name) && valid_version(version),
        "Unsupported package identity or version"
    );
    Ok(Package {
        name: name.into(),
        version: version.into(),
    })
}
pub(super) fn parse_packages(
    kind: &str,
    text: &str,
    npm_root: Option<&Path>,
) -> Result<(Vec<Package>, bool)> {
    ensure!(
        text.len() <= 1024 * 1024 && !text.contains('\u{fffd}'),
        "Incomplete or invalid command output"
    );
    let mut packages = Vec::new();
    let mut omitted = false;
    match kind {
        "brew_formula" | "brew_cask" => {
            for line in text.lines().filter(|line| !line.trim().is_empty()) {
                let fields: Vec<_> = line.split_whitespace().collect();
                ensure!(fields.len() >= 2, "Missing Homebrew version");
                if !valid_name(kind, fields[0]) || fields[1..].iter().any(|v| !valid_version(v)) {
                    omitted = true;
                    continue;
                }
                packages.push(package(kind, fields[0], &fields[1..].join(","))?);
            }
        }
        "cargo" | "uv" => {
            for line in text.lines().filter(|line| !line.trim().is_empty()) {
                if kind == "cargo" && line.starts_with(char::is_whitespace) {
                    ensure!(
                        !packages.is_empty(),
                        "Executable appears before a package header"
                    );
                    ensure!(
                        line.trim()
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)),
                        "Invalid Cargo executable"
                    );
                    continue;
                }
                if kind == "uv" && line.starts_with("- ") {
                    ensure!(
                        !packages.is_empty(),
                        "Executable appears before a package header"
                    );
                    ensure!(
                        line[2..]
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)),
                        "Invalid uv executable"
                    );
                    continue;
                }
                let line = if kind == "cargo" {
                    line.strip_suffix(':')
                        .context("Missing Cargo package marker")?
                } else {
                    line
                };
                let fields: Vec<_> = line.split_whitespace().collect();
                ensure!(fields.len() == 2, "Invalid package header");
                packages.push(package(
                    kind,
                    fields[0],
                    fields[1]
                        .strip_prefix('v')
                        .context("Missing package version")?,
                )?);
            }
        }
        "vscode" => {
            for line in text.lines().filter(|line| !line.trim().is_empty()) {
                let (name, version) = line.rsplit_once('@').context("Missing extension version")?;
                packages.push(package(kind, name, version)?);
            }
        }
        "npm" => {
            #[derive(Deserialize)]
            struct Listing {
                path: String,
                #[serde(default, deserialize_with = "unique_dependencies")]
                dependencies: BTreeMap<String, NpmPackage>,
                #[serde(default)]
                problems: Vec<serde_json::Value>,
                #[serde(default)]
                error: Option<serde_json::Value>,
            }
            let listing: Listing = serde_json::from_str(text).context("Invalid npm list JSON")?;
            ensure!(
                npm_root.is_some_and(|root| Path::new(&listing.path) == root),
                "npm root does not match selected standard installation"
            );
            ensure!(
                listing.problems.is_empty() && listing.error.is_none(),
                "npm reports incomplete metadata"
            );
            for (name, entry) in listing.dependencies {
                ensure!(
                    !entry.invalid && !entry.missing && entry.problems.is_empty(),
                    "npm package metadata is incomplete"
                );
                packages.push(package(kind, &name, &entry.version)?);
            }
        }
        _ => anyhow::bail!("Unsupported package provider"),
    }
    ensure!(
        packages.len() <= MAX_PACKAGES,
        "Package list exceeds count limit"
    );
    packages.sort_by_key(|package| package.name.to_ascii_lowercase());
    let mut names = BTreeSet::new();
    for package in &packages {
        ensure!(
            names.insert(identity_key(kind, &package.name)),
            "Duplicate package identity"
        );
    }
    Ok((packages, omitted))
}
#[derive(Deserialize)]
struct NpmPackage {
    version: String,
    #[serde(default)]
    invalid: bool,
    #[serde(default)]
    missing: bool,
    #[serde(default)]
    problems: Vec<serde_json::Value>,
}
fn unique_dependencies<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<BTreeMap<String, NpmPackage>, D::Error> {
    struct Dependencies;
    impl<'de> Visitor<'de> for Dependencies {
        type Value = BTreeMap<String, NpmPackage>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("unique npm dependencies")
        }
        fn visit_map<M: MapAccess<'de>>(
            self,
            mut map: M,
        ) -> std::result::Result<Self::Value, M::Error> {
            let mut result = BTreeMap::new();
            while let Some((key, value)) = map.next_entry::<String, NpmPackage>()? {
                if result.insert(key, value).is_some() {
                    return Err(serde::de::Error::custom("Duplicate npm identity"));
                }
            }
            Ok(result)
        }
    }
    d.deserialize_map(Dependencies)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    fn parse_packages(kind: &str, text: &str) -> Result<(Vec<Package>, bool)> {
        super::parse_packages(kind, text, Some(Path::new("/opt/homebrew/lib")))
    }
    struct Fake {
        outputs: VecDeque<Result<Output>>,
        calls: usize,
    }
    impl Runner for Fake {
        fn run(&mut self, _: &Invocation, _: &Arc<AtomicBool>) -> Result<Output> {
            self.calls += 1;
            self.outputs.pop_front().unwrap()
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
    fn fixture_requests() -> Vec<Request> {
        PROVIDERS
            .iter()
            .map(|kind| Request {
                kind,
                invocation: Some(invocation("/trusted/tool".into(), &[])),
            })
            .collect()
    }
    #[test]
    fn parses_multiple_brew_versions_cargo_uv_and_extensions_without_private_metadata() {
        let (brew, omitted) = parse_packages(
            "brew_formula",
            "openssl@3 3.5.1 3.5.2\nprivate/tap/tool 1.0\n",
        )
        .unwrap();
        assert!(omitted);
        assert_eq!(
            brew,
            vec![Package {
                name: "openssl@3".into(),
                version: "3.5.1,3.5.2".into()
            }]
        );
        assert_eq!(
            parse_packages("brew_cask", "visual-studio-code 1.100.0\n")
                .unwrap()
                .0[0]
                .version,
            "1.100.0"
        );
        assert_eq!(
            parse_packages("cargo", "ripgrep v14.1.0:\n    rg\n")
                .unwrap()
                .0[0]
                .name,
            "ripgrep"
        );
        assert_eq!(
            parse_packages("uv", "ruff v0.12.0\n- ruff\n").unwrap().0[0].version,
            "0.12.0"
        );
        assert_eq!(
            parse_packages("vscode", "rust-lang.rust-analyzer@0.3.1\n")
                .unwrap()
                .0
                .len(),
            1
        );
        for (kind, text) in [
            ("cargo", "tool v1.0.0 (/Users/private/repo):\n"),
            ("uv", "tool v1.0.0 (from https://private.example)\n"),
            ("vscode", "publisher.tool@https://private.example\n"),
            ("cargo", "tool v1.0.0:\n    tool\ntool v1.0.1:\n    tool\n"),
            ("uv", "tool v1.0.0\nsecret\u{fffd}\n"),
            ("cargo", "    rg\n"),
            ("uv", "- ruff\n"),
        ] {
            assert!(parse_packages(kind, text).is_err());
        }
    }
    #[test]
    fn npm_requires_versions_unique_names_exact_root_and_no_partial_metadata() {
        assert_eq!(parse_packages("npm", r#"{"path":"/opt/homebrew/lib","dependencies":{"typescript":{"version":"5.9.2"},"@scope/tool":{"version":"1.2.3"}}}"#).unwrap().0.len(), 2);
        assert!(
            parse_packages("npm", r#"{"path":"/opt/homebrew/lib"}"#)
                .unwrap()
                .0
                .is_empty()
        );
        for text in [
            r#"{}"#,
            r#"{"unrelated":true}"#,
            r#"{"path":"/wrong/lib","dependencies":{}}"#,
            r#"{"path":"/opt/homebrew/lib","dependencies":{"tool":{"version":"1.0"},"tool":{"version":"2.0"}}}"#,
            r#"{"path":"/opt/homebrew/lib","dependencies":{"tool":{"version":"file:/Users/private"}}}"#,
            r#"{"path":"/opt/homebrew/lib","dependencies":{"tool":{}}}"#,
            r#"{"path":"/opt/homebrew/lib","dependencies":{"tool":{"version":"1.0","missing":true}}}"#,
            r#"{"path":"/opt/homebrew/lib","problems":["bad"]}"#,
            r#"{"path":"/opt/homebrew/lib","dependencies":{},"dependencies":{}}"#,
        ] {
            assert!(parse_packages("npm", text).is_err());
        }
    }
    #[test]
    fn partial_failures_unavailable_and_cancel_are_explicit_and_sanitized() {
        let mut runner = Fake {
            calls: 0,
            outputs: VecDeque::from([
                Ok(output("rust 1.90.0\n")),
                Ok(Output {
                    code: 1,
                    stdout: "secret /Users/private".into(),
                    ..output("")
                }),
                Err(anyhow::anyhow!("secret /Users/private")),
                Ok(Output {
                    timed_out: true,
                    ..output("")
                }),
                Ok(output("not JSON /Users/private")),
            ]),
        };
        let mut requests = fixture_requests();
        requests[5].invocation = None;
        let snapshot = collect_with(
            &mut runner,
            requests,
            Arc::new(AtomicBool::new(false)),
            "123".into(),
        )
        .unwrap();
        assert_eq!(
            snapshot
                .providers
                .iter()
                .map(|p| p.status.as_str())
                .collect::<Vec<_>>(),
            [
                "complete",
                "failed",
                "failed",
                "failed",
                "failed",
                "unavailable"
            ]
        );
        assert!(
            !serde_json::to_string(&snapshot)
                .unwrap()
                .contains("/Users/")
        );
        let calls = runner.calls;
        assert!(
            collect_with(
                &mut runner,
                fixture_requests(),
                Arc::new(AtomicBool::new(true)),
                "123".into()
            )
            .is_err()
        );
        assert_eq!(runner.calls, calls);
        let mut runner = Fake {
            calls: 0,
            outputs: VecDeque::from([Ok(Output {
                cancelled: true,
                ..output("")
            })]),
        };
        assert!(
            collect_with(
                &mut runner,
                fixture_requests(),
                Arc::new(AtomicBool::new(false)),
                "123".into()
            )
            .is_err()
        );
        assert_eq!(runner.calls, 1);
    }
    #[cfg(unix)]
    #[test]
    fn homebrew_needing_runtime_bootstrap_is_not_invoked() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "ltsc-brew-runtime-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let vendor = root.join("Library/Homebrew/vendor");
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::create_dir_all(vendor.join("portable-ruby/4.0.1/bin")).unwrap();
        let brew = root.join("bin/brew");
        fs::write(&brew, b"fixture").unwrap();
        fs::write(vendor.join("portable-ruby-version"), b"4.0.1\n").unwrap();
        assert!(!brew_runtime_ready(&brew));
        fs::write(vendor.join("portable-ruby/4.0.1/bin/ruby"), b"fixture").unwrap();
        std::os::unix::fs::symlink("4.0.1", vendor.join("portable-ruby/current")).unwrap();
        assert!(!brew_runtime_ready(&brew));
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            vendor.join("portable-ruby/4.0.1/bin/ruby"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        assert!(brew_runtime_ready(&brew));
        fs::write(vendor.join("portable-ruby-version"), b"4.0.2\n").unwrap();
        assert!(!brew_runtime_ready(&brew));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn discovery_uses_only_fixed_locations_and_never_path_or_shell() {
        let requests = requests(Path::new("/missing-home-ltsc-fixture"));
        assert_eq!(requests.len(), 6);
        for request in requests {
            if let Some(inv) = request.invocation {
                assert!(inv.program.is_absolute());
                assert!(!inv.args.iter().any(|a| a == "-c" || a == "install"));
                if request.kind == "npm" {
                    assert!(inv.npm_node);
                    assert!(inv.args.iter().any(|a| a == "--ignore-scripts"));
                }
            }
        }
    }
}
