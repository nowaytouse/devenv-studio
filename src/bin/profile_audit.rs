#[cfg(target_os = "windows")]
#[allow(dead_code)]
#[path = "../config.rs"]
mod config;
#[cfg(target_os = "windows")]
#[allow(dead_code)]
#[path = "../inventory.rs"]
mod inventory;
#[cfg(target_os = "windows")]
#[allow(dead_code)]
#[path = "../utils.rs"]
mod utils;

#[cfg(target_os = "windows")]
use config::SetupProfile;
#[cfg(target_os = "windows")]
use utils::{run_native_cmd_timeout, CancellationToken};

#[cfg(target_os = "windows")]
fn main() -> anyhow::Result<()> {
    let profile = SetupProfile::load_default();
    profile.validate()?;

    let mut failures = Vec::new();
    for app in profile
        .packages
        .winget_core
        .iter()
        .chain(&profile.packages.winget_dev)
    {
        let result = run_native_cmd_timeout(
            "winget.exe",
            &[
                "search",
                "--id",
                &app.id,
                "-e",
                "--source",
                "winget",
                "--accept-source-agreements",
                "--disable-interactivity",
            ],
            120,
            &CancellationToken::default(),
        );
        if !(result.succeeded()
            && result
                .output
                .to_ascii_lowercase()
                .contains(&app.id.to_ascii_lowercase()))
        {
            failures.push(format!("WinGet {}: {}", app.id, result.diagnostic()));
        }
    }

    let checks = build_http_checks(&profile);
    failures.extend(run_http_checks(checks, 8));
    if failures.is_empty() {
        println!(
            "Profile 在线审计通过：{} WinGet，{} Scoop，{} Cargo，{} NPM，{} Pip/UV",
            profile.packages.winget_core.len() + profile.packages.winget_dev.len(),
            profile.packages.scoop_tools.len(),
            profile.packages.cargo_packages.len(),
            profile.packages.npm_globals.len(),
            profile.packages.pip_packages.len() + profile.packages.uv_tools.len()
        );
        return Ok(());
    }

    for failure in &failures {
        eprintln!("{failure}");
    }
    anyhow::bail!("Profile 在线审计失败：{} 项", failures.len())
}

#[cfg(any(target_os = "windows", test))]
#[derive(Debug)]
struct HttpCheck {
    label: String,
    urls: Vec<String>,
}

#[cfg(target_os = "windows")]
fn build_http_checks(profile: &SetupProfile) -> Vec<HttpCheck> {
    let mut checks = Vec::new();
    for package in &profile.packages.scoop_tools {
        checks.push(HttpCheck {
            label: format!("Scoop {package}"),
            urls: ["Main", "Extras", "Versions"]
                .into_iter()
                .map(|bucket| {
                    format!(
                        "https://raw.githubusercontent.com/ScoopInstaller/{bucket}/master/bucket/{package}.json"
                    )
                })
                .collect(),
        });
    }
    for package in &profile.packages.cargo_packages {
        checks.push(HttpCheck {
            label: format!("Cargo {package}"),
            urls: vec![format!("https://crates.io/api/v1/crates/{package}")],
        });
    }
    for package in &profile.packages.npm_globals {
        checks.push(HttpCheck {
            label: format!("NPM {package}"),
            urls: vec![format!(
                "https://registry.npmjs.org/{}",
                package.replace('/', "%2F")
            )],
        });
    }
    for package in profile
        .packages
        .pip_packages
        .iter()
        .chain(&profile.packages.uv_tools)
    {
        checks.push(HttpCheck {
            label: format!("PyPI {package}"),
            urls: vec![format!("https://pypi.org/pypi/{package}/json")],
        });
    }
    checks
}

#[cfg(target_os = "windows")]
fn run_http_checks(checks: Vec<HttpCheck>, workers: usize) -> Vec<String> {
    run_http_checks_with(checks, workers, |check| {
        let mut diagnostics = Vec::new();
        let found = check.urls.iter().any(|url| {
            let result = run_native_cmd_timeout(
                "curl.exe",
                &[
                    "--fail",
                    "--silent",
                    "--show-error",
                    "--location",
                    "--retry",
                    "2",
                    "--retry-all-errors",
                    "--retry-delay",
                    "1",
                    "--user-agent",
                    "LTSCWorkspace/2.2 (+https://github.com/nowaytouse/LTSC_Tools_Backup)",
                    "--max-time",
                    "20",
                    "--output",
                    "NUL",
                    url,
                ],
                45,
                &CancellationToken::default(),
            );
            if result.succeeded() {
                true
            } else {
                diagnostics.push(format!("{url}: {}", result.diagnostic()));
                false
            }
        });
        (!found).then(|| format!("{}: {}", check.label, diagnostics.join(" | ")))
    })
}

#[cfg(any(target_os = "windows", test))]
fn run_http_checks_with<F>(checks: Vec<HttpCheck>, workers: usize, check_one: F) -> Vec<String>
where
    F: Fn(&HttpCheck) -> Option<String> + Send + Sync + 'static,
{
    use std::collections::VecDeque;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    };

    let expected = checks.len();
    if workers == 0 {
        return vec![format!(
            "HTTP audit started 0 workers; {expected} checks missing"
        )];
    }
    let queue = Arc::new(Mutex::new(VecDeque::from(checks)));
    let completed = Arc::new(AtomicUsize::new(0));
    let queue_poisoned = Arc::new(AtomicBool::new(false));
    let check_one = Arc::new(check_one);
    let mut threads = Vec::new();
    for _ in 0..workers {
        let queue = Arc::clone(&queue);
        let completed = Arc::clone(&completed);
        let queue_poisoned = Arc::clone(&queue_poisoned);
        let check_one = Arc::clone(&check_one);
        threads.push(std::thread::spawn(move || {
            let mut failures = Vec::new();
            loop {
                let mut queue = match queue.lock() {
                    Ok(queue) => queue,
                    Err(poisoned) => {
                        queue_poisoned.store(true, Ordering::Relaxed);
                        poisoned.into_inner()
                    }
                };
                let Some(check) = queue.pop_front() else {
                    break;
                };
                drop(queue);
                let failure = check_one(&check);
                completed.fetch_add(1, Ordering::Relaxed);
                if let Some(failure) = failure {
                    failures.push(failure);
                }
            }
            failures
        }));
    }
    let mut failures = Vec::new();
    for thread in threads {
        match thread.join() {
            Ok(worker_failures) => failures.extend(worker_failures),
            Err(_) => failures.push("HTTP audit worker panicked".to_string()),
        }
    }
    let completed = completed.load(Ordering::Relaxed);
    if queue_poisoned.load(Ordering::Relaxed) {
        failures.push("HTTP audit work queue was poisoned".to_string());
    }
    if completed != expected {
        failures.push(format!(
            "HTTP audit incomplete: {completed}/{expected} checks completed"
        ));
    }
    failures
}

#[cfg(not(target_os = "windows"))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("profile_audit 需要 Windows/WinGet，完整在线审计由 Windows CI 执行")
}

#[cfg(test)]
mod tests {
    use super::{run_http_checks_with, HttpCheck};

    fn check(label: &str) -> HttpCheck {
        HttpCheck {
            label: label.to_string(),
            urls: vec![format!("https://example.invalid/{label}")],
        }
    }

    #[test]
    fn worker_panic_fails_audit_and_reports_missing_checks() {
        let failures = run_http_checks_with(vec![check("panic"), check("pending")], 1, |check| {
            if check.label == "panic" {
                panic!("simulated checker panic");
            }
            None
        });

        assert!(failures
            .iter()
            .any(|failure| failure.contains("worker panicked")));
        assert!(failures
            .iter()
            .any(|failure| failure.contains("0/2 checks completed")));
    }

    #[test]
    fn successful_workers_complete_every_check() {
        assert!(
            run_http_checks_with(vec![check("one"), check("two")], 2, |check| {
                assert_eq!(
                    check.urls,
                    [format!("https://example.invalid/{}", check.label)]
                );
                None
            })
            .is_empty()
        );
    }

    #[test]
    fn zero_workers_fails_nonempty_audit() {
        let failures = run_http_checks_with(vec![check("one")], 0, |_| None);
        assert!(failures.iter().any(|failure| failure.contains("0 workers")));
    }
}
