use std::{fs, path::Path, process::Output};

use super::inventory::validate_manifest;
use regex::Regex;
use sha2::{Digest, Sha256};

#[cfg(windows)]
pub(super) fn update(
    manager: &Path,
    daily: Option<(&Path, &Path)>,
    run: impl FnMut() -> color_eyre::Result<Output>,
) -> color_eyre::Result<Output> {
    update_with_probe(manager, daily, run, reachable)
}

fn update_with_probe(
    manager: &Path,
    daily: Option<(&Path, &Path)>,
    mut run: impl FnMut() -> color_eyre::Result<Output>,
    mut probe: impl FnMut(&str) -> bool,
) -> color_eyre::Result<Output> {
    use color_eyre::eyre::WrapErr;
    let digest = Sha256::digest(fs::read(manager).wrap_err("Cannot fingerprint dotnetup before update")?);
    let manifest = if let Some((install, path)) = daily {
        let bytes = fs::read(path).wrap_err("Cannot read daily SDK manifest")?;
        let retryable = validate_manifest(&bytes, install)?;
        Some((path, bytes, retryable))
    } else {
        None
    };
    let first = run()?;
    if first.status.success() {
        return Ok(first);
    }
    let message = format!(
        "{}\n{}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    let normalized = message.split_whitespace().collect::<Vec<_>>().join(" ");
    let pattern = if daily.is_some() {
        r"\AFailed to update \.NET SDK daily\. Error: Failed to resolve daily channel via (https://aka\.ms/dotnet/[0-9]+\.[0-9]+/daily/dotnet-sdk-win-x64\.zip): The SSL connection could not be established, see inner exception\.\z"
    } else {
        r"\AError: Failed to resolve preview channel via (https://aka\.ms/dotnet/dotnetup/preview/dotnetup-win-x64\.exe): The SSL connection could not be established, see inner exception\.\z"
    };
    let Some(captures) = Regex::new(pattern).expect("constant regex").captures(&normalized) else {
        return Ok(first);
    };
    if manifest.as_ref().is_some_and(|(_, _, retryable)| !retryable) {
        return Ok(first);
    }
    eprintln!("dotnetup could not resolve its Microsoft channel. Checking connectivity with HEAD.");
    if !probe(&captures[1]) {
        return Ok(first);
    }
    if fs::read(manager).ok().map(Sha256::digest) != Some(digest)
        || manifest.is_some_and(|(path, bytes, _)| fs::read(path).ok() != Some(bytes))
    {
        return Ok(first);
    }
    eprintln!("dotnetup channel reachable again; retrying the same update once.");
    let retry = run().wrap_err_with(|| {
        format!(
            "First update failed ({}): {normalized}; could not start retry",
            first.status
        )
    })?;
    if !retry.status.success() {
        color_eyre::eyre::bail!(
            "First update failed ({}): {normalized}\nRetry failed ({}): {}{}",
            first.status,
            retry.status,
            String::from_utf8_lossy(&retry.stdout),
            String::from_utf8_lossy(&retry.stderr)
        );
    }
    Ok(retry)
}

#[cfg(windows)]
fn reachable(url: &str) -> bool {
    use std::time::Duration;
    let Ok(client) = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
    else {
        return false;
    };
    for attempt in 0..2 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_secs(2));
        }
        let Ok(mut current) = reqwest::Url::parse(url) else {
            return false;
        };
        let start = std::time::Instant::now();
        for hop in 0..=10 {
            let Some(remaining) = Duration::from_secs(15).checked_sub(start.elapsed()) else {
                break;
            };
            let host = current.host_str().unwrap_or_default();
            let response = match client.head(current.clone()).timeout(remaining).send() {
                Ok(response) => response,
                Err(error) => {
                    // Redirect URLs can contain signed query parameters.
                    eprintln!("Channel probe at {host} failed: {}", error.without_url());
                    break;
                }
            };
            if matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
                let Some(location) = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                else {
                    break;
                };
                let Ok(next) = current.join(location) else { break };
                if next.scheme() != "https" || !next.username().is_empty() || next.password().is_some() {
                    break;
                }
                current = next;
                continue;
            }
            let html = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| {
                    v.split(';')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .eq_ignore_ascii_case("text/html")
                });
            if hop > 0 && response.status().is_success() && current.host_str() != Some("aka.ms") && !html {
                return true;
            }
            eprintln!(
                "Channel probe at {host} returned {} without a usable download redirect.",
                response.status()
            );
            break;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;
    #[cfg(windows)]
    use std::os::windows::process::ExitStatusExt;
    use std::process::ExitStatus;

    fn output(success: bool, message: &str) -> Output {
        Output {
            status: ExitStatus::from_raw(if success { 0 } else { 256 }),
            stdout: Vec::new(),
            stderr: message.as_bytes().to_vec(),
        }
    }

    const PREVIEW: &str = "Error: Failed to resolve preview channel via https://aka.ms/dotnet/dotnetup/preview/dotnetup-win-x64.exe: The SSL connection could not be established, see inner exception.";
    const DAILY: &str = "Failed to update .NET SDK daily. Error: Failed to resolve daily channel via https://aka.ms/dotnet/10.0/daily/dotnet-sdk-win-x64.zip: The SSL connection could not be established, see inner exception.";

    #[test]
    fn retry_only_exact_failure_and_preserve_original_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let manager = dir.path().join("manager");
        fs::write(&manager, "original").unwrap();
        for (message, reachable, retry_success, expected_calls) in [
            (PREVIEW, true, true, 2),
            (PREVIEW, false, true, 1),
            (PREVIEW, true, false, 2),
            ("unrelated TLS failure", true, true, 1),
        ] {
            let mut calls = 0;
            let result = update_with_probe(
                &manager,
                None,
                || {
                    calls += 1;
                    Ok(if calls == 1 {
                        output(false, message)
                    } else {
                        output(retry_success, "retry")
                    })
                },
                |_| reachable,
            );
            assert_eq!(calls, expected_calls);
            if expected_calls == 2 && !retry_success {
                let error = result.unwrap_err().to_string();
                assert!(error.contains(message) && error.contains("Retry failed") && error.ends_with("retry"));
                continue;
            }
            let result = result.unwrap();
            if expected_calls == 2 && retry_success {
                assert!(result.status.success());
            } else {
                assert_eq!(result.stderr, message.as_bytes());
            }
        }
    }

    #[test]
    fn rejects_unsafe_or_changed_daily_and_changed_manager() {
        let dir = tempfile::tempdir().unwrap();
        let manager = dir.path().join("manager");
        let manifest = dir.path().join("manifest");
        let install = dir.path().join("sdk");
        let valid = serde_json::json!({"dotnetRoots":[{"path":install.to_str().unwrap(),"architecture":"x64","installSpecs":[{"component":"SDK","versionOrChannel":"daily","installSource":"Explicit"}]}]}).to_string();
        for scenario in 0..4 {
            fs::write(&manager, "original").unwrap();
            let multi_spec = valid.replace(r#""installSpecs":["#, r#""installSpecs":[{"component":"Runtime"},"#);
            fs::write(&manifest, if scenario == 0 { &multi_spec } else { &valid }).unwrap();
            let mut calls = 0;
            let result = update_with_probe(
                &manager,
                Some((&install, &manifest)),
                || {
                    calls += 1;
                    Ok(if calls == 1 {
                        output(false, DAILY)
                    } else {
                        output(true, "ok")
                    })
                },
                |_| {
                    if scenario == 1 {
                        fs::write(&manager, "changed").unwrap();
                    }
                    if scenario == 2 {
                        fs::write(&manifest, "{}").unwrap();
                    }
                    true
                },
            )
            .unwrap();
            assert_eq!(calls, if scenario == 3 { 2 } else { 1 });
            assert_eq!(result.status.success(), scenario == 3);
        }
    }
}
