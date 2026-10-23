//! Integrated compatibility backend imported from LTSC_Tools_Backup a8523ab.
//! Legacy resources and unauthenticated rollback execution are deliberately unavailable.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub mod config;
#[cfg_attr(not(windows), allow(dead_code))]
mod installer;
pub mod inventory;
#[cfg_attr(not(windows), allow(dead_code))]
mod platform;
#[cfg_attr(not(windows), allow(dead_code))]
pub mod rollback;
#[cfg_attr(not(windows), allow(dead_code))]
mod utils;
#[cfg_attr(not(windows), allow(dead_code))]
mod winget_bootstrap;

pub use config::{ExecutionTarget, NetworkMode, SetupConfig, SetupProfile};

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Options {
    pub extra_apps: bool,
    pub dev_tools: bool,
    pub git_shell: bool,
    pub editor: bool,
    pub npm_config: bool,
    pub ollama: bool,
    pub windows_features: bool,
    pub storage: bool,
    pub tweaks: bool,
}

#[derive(Debug, Clone)]
pub struct Review {
    pub summary: String,
    pub config: SetupConfig,
    canonical: String,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub succeeded: bool,
    pub cancelled: bool,
    pub summary: String,
    pub history: PathBuf,
    pub log: Vec<String>,
}

pub fn default_profile_json() -> Result<String> {
    let profile = SetupProfile::load_default();
    profile.validate()?;
    Ok(serde_json::to_string_pretty(&profile)?)
}

/// Read-only. A review never creates files or probes installed tools.
pub fn review(
    profile_json: &str,
    target: ExecutionTarget,
    mode: NetworkMode,
    options: &Options,
) -> Result<Review> {
    ensure!(
        profile_json.len() <= 1_048_576,
        "Compatibility profile exceeds 1 MiB"
    );
    ensure!(
        !matches!(
            target,
            ExecutionTarget::AgentSkillsOnly | ExecutionTarget::RollbackLatest
        ),
        "Legacy Agent payloads and unauthenticated rollback commands are unavailable; review the preserved source history"
    );
    let profile: SetupProfile =
        serde_json::from_str(profile_json).context("Invalid compatibility profile JSON")?;
    profile.validate()?;
    let selected = match target {
        ExecutionTarget::FullSetup => *options != Options::default(),
        ExecutionTarget::DevToolsOnly => options.dev_tools || options.extra_apps,
        ExecutionTarget::NetworkOnly => true,
        ExecutionTarget::StorageOnly => options.storage,
        ExecutionTarget::WindowsFeaturesOnly => options.windows_features,
        ExecutionTarget::VSCodeExtensionsOnly => options.editor || options.npm_config,
        ExecutionTarget::SystemTweaksOnly => options.tweaks,
        _ => false,
    };
    ensure!(
        selected,
        "Choose the operation's corresponding scope before reviewing"
    );
    let allowed = match target {
        ExecutionTarget::FullSetup => options.clone(),
        ExecutionTarget::DevToolsOnly => Options {
            dev_tools: options.dev_tools,
            extra_apps: options.extra_apps,
            ..Options::default()
        },
        ExecutionTarget::NetworkOnly => Options::default(),
        ExecutionTarget::StorageOnly => Options {
            storage: options.storage,
            ..Options::default()
        },
        ExecutionTarget::WindowsFeaturesOnly => Options {
            windows_features: options.windows_features,
            ..Options::default()
        },
        ExecutionTarget::VSCodeExtensionsOnly => Options {
            editor: options.editor,
            npm_config: options.npm_config,
            ..Options::default()
        },
        ExecutionTarget::SystemTweaksOnly => Options {
            tweaks: options.tweaks,
            ..Options::default()
        },
        _ => Options::default(),
    };
    ensure!(
        *options == allowed,
        "Some selected scopes do not apply to this operation; deselect them or choose Full setup"
    );
    let system_changes = matches!(
        target,
        ExecutionTarget::NetworkOnly
            | ExecutionTarget::StorageOnly
            | ExecutionTarget::SystemTweaksOnly
    ) || (target == ExecutionTarget::FullSetup
        && (options.storage || options.tweaks));
    ensure!(
        !system_changes || profile.system_tweaks.create_rollback_journal,
        "Compatibility system changes require their original-value journal"
    );
    if options.storage {
        ensure!(
            profile.system_tweaks.enable_trim || profile.system_tweaks.optimize_storage,
            "Enable a storage operation in the profile"
        );
    }
    if options.windows_features {
        let features = &profile.windows_features;
        ensure!(
            features.enable_wsl2
                || features.enable_netfx3
                || features.enable_windows_sandbox
                || features.enable_hyper_v,
            "Enable a Windows feature in the profile"
        );
    }
    if options.ollama {
        ensure!(
            !profile.ollama_models.is_empty(),
            "Select an Ollama model in the profile"
        );
    }
    if options.git_shell && target == ExecutionTarget::FullSetup {
        ensure!(
            !profile.git_config.user_name.trim().is_empty()
                && !profile.git_config.user_email.trim().is_empty(),
            "Enter the Git identity explicitly before enabling Git/Shell configuration"
        );
    }
    let config = SetupConfig {
        include_curated_extras: options.extra_apps,
        include_dev_tools: options.dev_tools,
        include_agent_skills: false,
        include_docker_wsl: options.windows_features,
        include_storage_optimization: options.storage,
        include_deep_win_tweaks: options.tweaks,
        include_git_shell_configs: options.git_shell,
        include_vscode_extensions: options.editor,
        include_npmrc_config: options.npm_config,
        include_ollama_models: options.ollama,
        network_mode: mode,
        target_mode: target,
        profile,
        source_inventory: inventory::MacosInventory::load_embedded()?,
    };
    let packages = &config.profile.packages;
    let mut scopes = Vec::new();
    let full = target == ExecutionTarget::FullSetup;
    let packages_target = full || target == ExecutionTarget::DevToolsOnly;
    if packages_target && options.dev_tools {
        scopes.push(format!(
            "Development packages: WinGet {}, Scoop {}, Cargo {}, NPM {}, Pip {}, uv {}",
            packages.winget_dev.len(),
            packages.scoop_tools.len(),
            packages.cargo_packages.len(),
            packages.npm_globals.len(),
            packages.pip_packages.len(),
            packages.uv_tools.len()
        ));
    }
    if packages_target && options.extra_apps {
        scopes.push(format!(
            "Additional WinGet applications: {}; six legacy Store product requests",
            packages.winget_core.len()
        ));
    }
    if full && options.git_shell {
        scopes.push("Git identity/settings/ignore and PowerShell managed profile".into());
    }
    if (full || target == ExecutionTarget::VSCodeExtensionsOnly) && options.editor {
        scopes.push(format!(
            "VS Code/Cursor strict-JSON settings merge and {} extensions",
            config.profile.vscode_config.extensions.len()
        ));
    }
    if (full || target == ExecutionTarget::VSCodeExtensionsOnly) && options.npm_config {
        scopes.push("Persistent Cargo/Pip/NPM sources (HTTPS; originals preserved)".into());
    }
    if full && options.ollama {
        scopes.push(format!(
            "Ollama model downloads: {}",
            config.profile.ollama_models.len()
        ));
    }
    if (full && options.windows_features) || target == ExecutionTarget::WindowsFeaturesOnly {
        scopes.push("Selected Windows optional features".into());
    }
    if (full && options.storage) || target == ExecutionTarget::StorageOnly {
        scopes.push("Selected TRIM/Windows media-aware storage operations".into());
    }
    if (full && options.tweaks) || target == ExecutionTarget::SystemTweaksOnly {
        scopes.push("Legacy power/registry system tweaks; separate legacy journal".into());
    }
    if target == ExecutionTarget::NetworkOnly {
        scopes.push(format!("Legacy TCP/DNS operation: {mode}"));
    }
    let summary = format!(
        "{}\n{}\nWindows compatibility backend; no Agent payload deployment or legacy rollback execution. Package providers may run installer/build scripts. Packages, components and models are not uninstalled by settings recovery. File originals and the request/report are retained locally.",
        target.label(),
        scopes.join("\n")
    );
    let canonical = serde_json::to_string(&config)?;
    Ok(Review {
        summary,
        config,
        canonical,
    })
}

/// The root application's worker owns this call; only the event collector uses a helper thread.
pub fn execute(review: Review, cancel: Arc<AtomicBool>) -> Result<Report> {
    ensure!(
        !cancel.load(Ordering::Acquire),
        "Compatibility operation cancelled before execution"
    );
    ensure!(
        cfg!(windows),
        "Compatibility deployment requires a supported Windows host; this host was not changed"
    );
    let host = studio_platform::host();
    ensure!(
        host.can_apply && host.target.is_some(),
        "Unsupported Windows host: {}",
        host.explanation
    );
    ensure!(
        serde_json::to_string(&review.config)? == review.canonical,
        "Reviewed configuration was changed; review it again"
    );
    review.config.profile.validate()?;
    ensure!(
        !matches!(
            review.config.target_mode,
            ExecutionTarget::AgentSkillsOnly | ExecutionTarget::RollbackLatest
        ),
        "This legacy target is unavailable"
    );
    if review.config.target_mode.requires_admin() {
        ensure!(
            utils::is_admin(),
            "This compatibility operation requires an administrator"
        );
    }
    let _lock = studio_platform::acquire_mutation_lock()?;
    ensure!(
        !cancel.load(Ordering::Acquire),
        "Compatibility operation cancelled before mutation"
    );
    let history_root = studio_platform::data_directory()?.join("compatibility-runs");
    utils::reject_links(&history_root)?;
    std::fs::create_dir_all(&history_root)?;
    let history = history_root.join(format!(
        "run-{}-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%S%fZ"),
        std::process::id()
    ));
    std::fs::create_dir(&history)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&history, std::fs::Permissions::from_mode(0o700))?;
    }
    utils::write_atomic(&history.join("request.json"), review.canonical.as_bytes())?;
    let report = snapshot_files(&review.config, &history)
        .and_then(|()| execute_events(review.config, cancel, history.clone()));
    match report {
        Ok(report) => {
            utils::write_atomic(
                &history.join("report.json"),
                &serde_json::to_vec_pretty(&report)?,
            )?;
            Ok(report)
        }
        Err(error) => {
            let failure = Report {
                succeeded: false,
                cancelled: false,
                summary: format!("Compatibility execution failed: {error:#}"),
                history: history.clone(),
                log: Vec::new(),
            };
            utils::write_atomic(
                &history.join("report.json"),
                &serde_json::to_vec_pretty(&failure)?,
            )?;
            Err(error.context(format!("History: {}", history.display())))
        }
    }
}

fn snapshot_files(config: &SetupConfig, history: &std::path::Path) -> Result<()> {
    let home =
        PathBuf::from(std::env::var_os("USERPROFILE").context("USERPROFILE is unavailable")?);
    let appdata = PathBuf::from(std::env::var_os("APPDATA").context("APPDATA is unavailable")?);
    let mut paths = Vec::new();
    let full = config.target_mode == ExecutionTarget::FullSetup;
    if full && config.include_git_shell_configs {
        paths.extend([
            home.join(".gitconfig"),
            home.join(".config/git/ignore"),
            home.join("Documents/PowerShell/Microsoft.PowerShell_profile.ps1"),
        ]);
    }
    if (full || config.target_mode == ExecutionTarget::VSCodeExtensionsOnly)
        && config.include_npmrc_config
    {
        paths.extend([
            home.join(".cargo/config.toml"),
            home.join(".npmrc"),
            appdata.join("pip/pip.ini"),
        ]);
    }
    if (full || config.target_mode == ExecutionTarget::VSCodeExtensionsOnly)
        && config.include_vscode_extensions
    {
        paths.extend([
            appdata.join("Code/User/settings.json"),
            appdata.join("Cursor/User/settings.json"),
        ]);
    }
    let mut baselines = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        utils::reject_links(path)?;
        let backup = match utils::read_bytes_bounded(path) {
            Ok(bytes) => {
                let name = format!("original-{index}.bin");
                utils::write_atomic(&history.join(&name), &bytes)?;
                Some(name)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error).with_context(|| format!("Read original {}", path.display()));
            }
        };
        baselines.push(serde_json::json!({"path":path,"original":backup}));
    }
    utils::write_atomic(
        &history.join("originals.json"),
        &serde_json::to_vec_pretty(&baselines)?,
    )?;
    Ok(())
}

fn execute_events(
    config: SetupConfig,
    cancel: Arc<AtomicBool>,
    history: PathBuf,
) -> Result<Report> {
    use installer::{SetupEvent, SetupOutcome};
    use std::io::Write;
    let event_file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(history.join("events.log"))?;
    let (sender, receiver) = std::sync::mpsc::sync_channel(64);
    let collector_cancel = cancel.clone();
    let collector = std::thread::spawn(move || {
        let mut file = event_file;
        let mut log = std::collections::VecDeque::new();
        let mut log_bytes = 0;
        let mut persisted = 0;
        let mut display_truncated = false;
        let mut file_truncated = false;
        let mut outcome = None;
        let mut io_error = None;
        for event in receiver {
            let line = match event {
                SetupEvent::Log(message) => {
                    format!("{} {:?}: {}", message.time, message.level, message.message)
                }
                SetupEvent::Finished(summary) => {
                    outcome = Some(summary);
                    continue;
                }
                SetupEvent::Progress(value) => {
                    let _ = value;
                    continue;
                }
            };
            log_bytes += line.len();
            log.push_back(line.clone());
            while log_bytes > 524_288 {
                if let Some(old) = log.pop_front() {
                    log_bytes -= old.len();
                    display_truncated = true;
                }
            }
            if io_error.is_none() && !file_truncated {
                if persisted + line.len() + 1 > 8_388_480 {
                    file_truncated = true;
                    if let Err(error) = writeln!(
                        file,
                        "[Log truncated at 8 MiB; final result remains in report.json]"
                    ) {
                        io_error = Some(error);
                        collector_cancel.store(true, Ordering::Release);
                    }
                } else {
                    match writeln!(file, "{line}") {
                        Ok(()) => persisted += line.len() + 1,
                        Err(error) => {
                            io_error = Some(error);
                            collector_cancel.store(true, Ordering::Release);
                        }
                    }
                }
            }
        }
        if io_error.is_none()
            && let Err(error) = file.sync_all()
        {
            io_error = Some(error);
        }
        if display_truncated {
            log.push_front("[Earlier display entries truncated at 512 KiB]".into());
        }
        if file_truncated {
            log.push_back("[events.log truncated at 8 MiB]".into());
        }
        (outcome, log.into_iter().collect::<Vec<_>>(), io_error)
    });
    installer::run_setup_worker(
        sender,
        config,
        utils::CancellationToken::from_shared(cancel),
    );
    let (outcome, mut log, io_error) = collector
        .join()
        .map_err(|_| anyhow::anyhow!("Compatibility event collector crashed"))?;
    if let Some(error) = io_error {
        return Err(error).context("Persist compatibility execution log");
    }
    let outcome = outcome.context("Compatibility worker did not return a final result")?;
    log.push("Log display retains at most 512 KiB; events.log retains at most 8 MiB.".into());
    Ok(Report {
        succeeded: outcome.outcome == SetupOutcome::Succeeded,
        cancelled: outcome.outcome == SetupOutcome::Cancelled,
        summary: format!(
            "{}; {} errors, {} warnings, {:.1}s",
            outcome.outcome.label(),
            outcome.errors,
            outcome.warnings,
            outcome.elapsed.as_secs_f64()
        ),
        history,
        log,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_review_does_not_enable_an_operation_or_private_identity() {
        let json = default_profile_json().unwrap();
        assert!(!json.contains("@users.noreply.github.com"));
        assert!(
            review(
                &json,
                ExecutionTarget::DevToolsOnly,
                NetworkMode::Basic,
                &Options::default()
            )
            .is_err()
        );
        let profile: SetupProfile = serde_json::from_str(&json).unwrap();
        assert!(profile.git_config.user_name.is_empty());
        assert!(profile.git_config.user_email.is_empty());
        assert!(
            profile
                .environment_mirrors
                .npm_registry
                .starts_with("https://registry.npmjs.org")
        );
    }

    #[test]
    fn unsafe_targets_and_provider_arguments_are_rejected() {
        let json = default_profile_json().unwrap();
        for target in [
            ExecutionTarget::AgentSkillsOnly,
            ExecutionTarget::RollbackLatest,
        ] {
            assert!(review(&json, target, NetworkMode::Basic, &Options::default()).is_err());
        }
        let mut unknown: serde_json::Value = serde_json::from_str(&json).unwrap();
        unknown["packages"]["custom_script"] = serde_json::json!("ignored?");
        assert!(
            review(
                &unknown.to_string(),
                ExecutionTarget::DevToolsOnly,
                NetworkMode::Basic,
                &Options {
                    dev_tools: true,
                    ..Options::default()
                }
            )
            .is_err()
        );
        for value in [
            "--config",
            "https://example.invalid/a",
            "../payload",
            "x;cmd",
            "C:/payload",
        ] {
            let mut profile: SetupProfile = serde_json::from_str(&json).unwrap();
            profile.packages.cargo_packages = vec![value.into()];
            assert!(profile.validate().is_err(), "{value}");
        }
    }

    #[test]
    fn reviewed_domains_are_explicit_and_non_windows_execution_has_no_history() {
        let options = Options {
            dev_tools: true,
            ..Options::default()
        };
        let review = review(
            &default_profile_json().unwrap(),
            ExecutionTarget::DevToolsOnly,
            NetworkMode::Basic,
            &options,
        )
        .unwrap();
        assert!(review.summary.contains("Scoop 88"));
        assert!(!review.config.include_npmrc_config);
        let invalid = Options {
            dev_tools: true,
            git_shell: true,
            ..Options::default()
        };
        assert!(
            super::review(
                &default_profile_json().unwrap(),
                ExecutionTarget::DevToolsOnly,
                NetworkMode::Basic,
                &invalid
            )
            .is_err()
        );
        let cancelled = Arc::new(AtomicBool::new(true));
        assert!(execute(review.clone(), cancelled).is_err());
        if !cfg!(windows) {
            assert!(execute(review, Arc::new(AtomicBool::new(false))).is_err());
        }
    }
}
