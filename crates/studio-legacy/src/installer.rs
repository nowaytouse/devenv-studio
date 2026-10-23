use crate::config::*;
use crate::platform::{
    RegistryHive, get_registry_dword, prepend_user_path, set_registry_dword, set_user_environment,
};
use crate::rollback::{RollbackAction, RollbackJournal};
use crate::utils::{
    CancellationToken, CommandResult, CommandState, LogLevel, LogMessage, run_native_cmd,
    run_native_cmd_timeout, update_managed_block, upsert_ini_values,
};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::mpsc::SyncSender as Sender;
use std::time::{Duration, Instant};

const MANAGED_START: &str = "# >>> LTSC Tools managed >>>";
const MANAGED_END: &str = "# <<< LTSC Tools managed <<<";

#[derive(Debug, Clone)]
pub enum SetupEvent {
    Log(LogMessage),
    Progress(f32),
    Finished(SetupSummary),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupOutcome {
    Succeeded,
    CompletedWithErrors,
    Cancelled,
    Crashed,
}

impl SetupOutcome {
    pub fn label(self) -> &'static str {
        match self {
            Self::Succeeded => "已完成",
            Self::CompletedWithErrors => "完成，但有失败项",
            Self::Cancelled => "已取消",
            Self::Crashed => "异常终止",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SetupSummary {
    pub outcome: SetupOutcome,
    pub errors: usize,
    pub warnings: usize,
    pub elapsed: Duration,
}

impl SetupSummary {
    pub fn crashed(elapsed: Duration) -> Self {
        Self {
            outcome: SetupOutcome::Crashed,
            errors: 1,
            warnings: 0,
            elapsed,
        }
    }
}

pub fn run_setup_worker(
    tx: Sender<SetupEvent>,
    config: SetupConfig,
    cancellation: CancellationToken,
) {
    let started = Instant::now();
    let config = match config.resolved() {
        Ok(config) => config,
        Err(error) => {
            let _ = tx.send(SetupEvent::Log(LogMessage::new(
                LogLevel::Error,
                format!("部署计划无法生成：{error}"),
            )));
            let _ = tx.send(SetupEvent::Finished(SetupSummary {
                outcome: SetupOutcome::CompletedWithErrors,
                errors: 1,
                warnings: 0,
                elapsed: started.elapsed(),
            }));
            return;
        }
    };
    let engine_tx = tx.clone();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        SetupEngine::new(engine_tx, config, cancellation).run()
    }));
    let summary = match result {
        Ok(summary) => summary,
        Err(payload) => {
            let message = panic_payload_message(payload);
            let _ = tx.send(SetupEvent::Log(LogMessage::new(
                LogLevel::Error,
                format!("部署线程异常终止：{message}"),
            )));
            SetupSummary::crashed(started.elapsed())
        }
    };
    let _ = tx.send(SetupEvent::Finished(summary));
}

#[derive(Debug, Clone, Copy, Default)]
struct ManagerAvailability {
    winget: bool,
    scoop: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScoopGitAction {
    UseExisting,
    InstallWithWinget,
    WingetRequired,
}

fn scoop_git_action(git_available: bool, winget_available: bool) -> ScoopGitAction {
    if git_available {
        ScoopGitAction::UseExisting
    } else if winget_available {
        ScoopGitAction::InstallWithWinget
    } else {
        ScoopGitAction::WingetRequired
    }
}

pub struct SetupEngine {
    tx: Sender<SetupEvent>,
    config: SetupConfig,
    cancellation: CancellationToken,
    errors: Cell<usize>,
    warnings: Cell<usize>,
    cancellation_reported: Cell<bool>,
    rollback: RefCell<Option<RollbackJournal>>,
}

impl SetupEngine {
    pub fn new(
        tx: Sender<SetupEvent>,
        config: SetupConfig,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            tx,
            config,
            cancellation,
            errors: Cell::new(0),
            warnings: Cell::new(0),
            cancellation_reported: Cell::new(false),
            rollback: RefCell::new(None),
        }
    }

    pub fn run(&self) -> SetupSummary {
        let started = Instant::now();
        self.emit(
            LogLevel::Start,
            format!(
                "开始“{}”；配置版本 {}",
                self.config.target_mode.label(),
                self.config.profile.profile_version
            ),
        );

        if matches!(
            self.config.target_mode,
            ExecutionTarget::RollbackLatest | ExecutionTarget::AgentSkillsOnly
        ) {
            self.log(
                LogLevel::Error,
                "旧 Agent 资源和未认证回滚命令不可执行；请审阅保留的源历史。",
            );
        } else {
            match self.config.profile.validate() {
                Ok(()) => {
                    if !self.stopped() {
                        self.begin_rollback_journal();
                        match self.config.target_mode {
                            ExecutionTarget::FullSetup => self.run_all_steps(),
                            ExecutionTarget::NetworkOnly => self.step_network(),
                            ExecutionTarget::StorageOnly => self.step_storage(),
                            ExecutionTarget::WindowsFeaturesOnly => self.step_windows_features(),
                            ExecutionTarget::AgentSkillsOnly => self.step_agent_skills(),
                            ExecutionTarget::DevToolsOnly => self.run_dev_tools_only(),
                            ExecutionTarget::VSCodeExtensionsOnly => {
                                if self.config.include_npmrc_config {
                                    self.step_environment_mirrors();
                                }
                                if !self.stopped() {
                                    self.step_vscode_and_tools_config(0.1, 0.95);
                                }
                            }
                            ExecutionTarget::SystemTweaksOnly => self.step_deep_win_tweaks(),
                            ExecutionTarget::RollbackLatest => unreachable!(),
                        }
                    }
                }
                Err(error) => self.log(LogLevel::Error, format!("配置校验失败：{error}")),
            }
        }

        self.finish_rollback_journal();

        let cancelled = self.cancellation.is_cancelled();
        if !cancelled {
            self.progress(1.0);
        }
        let outcome = if cancelled {
            SetupOutcome::Cancelled
        } else if self.errors.get() == 0 {
            SetupOutcome::Succeeded
        } else {
            SetupOutcome::CompletedWithErrors
        };
        let summary = SetupSummary {
            outcome,
            errors: self.errors.get(),
            warnings: self.warnings.get(),
            elapsed: started.elapsed(),
        };

        match outcome {
            SetupOutcome::Succeeded => self.emit(
                LogLevel::End,
                "任务完成；需要重启的系统设置会在下次启动后生效。",
            ),
            SetupOutcome::CompletedWithErrors => self.emit(
                LogLevel::Error,
                format!(
                    "任务已结束：{} 项失败，{} 项警告。失败不会再让 GUI 卡在运行状态。",
                    summary.errors, summary.warnings
                ),
            ),
            SetupOutcome::Cancelled => {
                self.emit(LogLevel::Warn, "任务已取消，当前子进程树已终止。")
            }
            SetupOutcome::Crashed => {}
        }
        summary
    }

    fn begin_rollback_journal(&self) {
        if !self.config.profile.system_tweaks.create_rollback_journal
            || !matches!(
                self.config.target_mode,
                ExecutionTarget::FullSetup
                    | ExecutionTarget::NetworkOnly
                    | ExecutionTarget::StorageOnly
                    | ExecutionTarget::SystemTweaksOnly
            )
        {
            return;
        }
        match RollbackJournal::create(&self.config.profile.profile_version) {
            Ok(journal) => {
                self.emit(
                    LogLevel::Ok,
                    format!("回滚账本：{}", journal.path().display()),
                );
                *self.rollback.borrow_mut() = Some(journal);
            }
            Err(error) => {
                self.log(
                    LogLevel::Error,
                    format!("无法创建回滚账本，已停止系统调优：{error}"),
                );
                self.cancellation.cancel();
            }
        }
    }

    fn finish_rollback_journal(&self) {
        let mut rollback = self.rollback.borrow_mut();
        let Some(journal) = rollback.as_mut() else {
            return;
        };
        match journal.finish() {
            Ok(()) => self.emit(
                LogLevel::Ok,
                format!("回滚账本已封存：{}", journal.path().display()),
            ),
            Err(error) => self.log(LogLevel::Error, format!("回滚账本封存失败：{error}")),
        }
    }

    fn record_rollback(&self, action: RollbackAction) -> bool {
        let mut rollback = self.rollback.borrow_mut();
        let Some(journal) = rollback.as_mut() else {
            return true;
        };
        match journal.record(action) {
            Ok(_) => true,
            Err(error) => {
                self.log(
                    LogLevel::Error,
                    format!("回滚账本写入失败，已停止后续系统变更：{error}"),
                );
                self.cancellation.cancel();
                false
            }
        }
    }

    fn run_all_steps(&self) {
        let managers = if self.config.include_dev_tools || self.config.include_curated_extras {
            self.step_package_managers()
        } else {
            ManagerAvailability::default()
        };
        if self.stopped() {
            return;
        }
        if self.config.include_docker_wsl {
            self.step_windows_features();
        }
        if self.stopped() {
            return;
        }
        if self.config.include_npmrc_config {
            self.step_environment_mirrors();
        }
        if self.stopped() {
            return;
        }
        if self.config.include_curated_extras {
            self.step_core_apps(managers.winget, 0.1, 0.2);
        }
        if self.stopped() {
            return;
        }
        if self.config.include_dev_tools {
            self.step_dev_suite(managers, 0.2, 0.7);
        }
        if self.stopped() {
            return;
        }
        if self.config.include_curated_extras {
            self.step_uwp_apps(managers.winget, 0.7, 0.76);
        }
        if self.stopped() {
            return;
        }
        if self.config.include_git_shell_configs {
            self.step_git_shell_configs();
        }
        if self.stopped() {
            return;
        }
        if self.config.include_vscode_extensions || self.config.include_npmrc_config {
            self.step_vscode_and_tools_config(0.76, 0.84);
        }
        if self.stopped() {
            return;
        }
        if self.config.include_ollama_models {
            self.step_ollama_models();
        }
        if self.stopped() {
            return;
        }
        if self.config.include_storage_optimization {
            self.step_storage();
        }
        if self.stopped() {
            return;
        }
        if self.config.include_deep_win_tweaks {
            self.step_deep_win_tweaks();
        }
        self.progress(0.99);
    }

    fn run_dev_tools_only(&self) {
        let managers = self.step_package_managers();
        if self.stopped() {
            return;
        }
        if self.config.include_curated_extras {
            self.step_core_apps(managers.winget, 0.1, 0.2);
        }
        if self.stopped() {
            return;
        }
        if self.config.include_dev_tools {
            self.step_dev_suite(managers, 0.2, 0.9);
        }
        if self.stopped() {
            return;
        }
        if self.config.include_curated_extras {
            self.step_uwp_apps(managers.winget, 0.9, 0.99);
        }
    }

    fn emit(&self, level: LogLevel, message: impl Into<String>) {
        let _ = self
            .tx
            .send(SetupEvent::Log(LogMessage::new(level, message)));
    }

    fn log(&self, level: LogLevel, message: impl Into<String>) {
        match level {
            LogLevel::Error => self.errors.set(self.errors.get() + 1),
            LogLevel::Warn => self.warnings.set(self.warnings.get() + 1),
            _ => {}
        }
        self.emit(level, message);
    }

    fn progress(&self, value: f32) {
        let _ = self.tx.send(SetupEvent::Progress(value.clamp(0.0, 1.0)));
    }

    fn stopped(&self) -> bool {
        if !self.cancellation.is_cancelled() {
            return false;
        }
        if !self.cancellation_reported.replace(true) {
            self.log(LogLevel::Warn, "收到取消请求，正在停止当前任务…");
        }
        true
    }

    fn command(&self, program: &str, args: &[&str], timeout_secs: u64) -> CommandResult {
        run_native_cmd_timeout(program, args, timeout_secs, &self.cancellation)
    }

    fn execute(&self, label: &str, program: &str, args: &[&str], timeout_secs: u64) -> bool {
        if self.stopped() {
            return false;
        }
        self.log(
            LogLevel::Info,
            format!("[CMD] {label} · {} {}", program, args.join(" ")),
        );
        let result = self.command(program, args, timeout_secs);
        if result.succeeded() {
            self.log(LogLevel::Ok, format!("{label}：完成"));
            true
        } else if result.cancelled() {
            self.stopped();
            false
        } else {
            self.log(
                LogLevel::Error,
                format!("{label}：失败（{}）", result.diagnostic()),
            );
            false
        }
    }

    fn step_network(&self) {
        self.log(
            LogLevel::Start,
            format!("网络设置 · {}", self.config.network_mode),
        );
        if self.config.network_mode != NetworkMode::Basic {
            self.snapshot_netsh_tcp();
            if self.stopped() {
                return;
            }
        }
        if !self.execute("刷新 DNS 缓存", "ipconfig.exe", &["/flushdns"], 45) {
            return;
        }

        if matches!(
            self.config.network_mode,
            NetworkMode::Optimized | NetworkMode::Extreme
        ) && !self.execute(
            "启用 TCP 自动调优",
            "netsh.exe",
            &[
                "interface",
                "tcp",
                "set",
                "global",
                "autotuninglevel=normal",
            ],
            45,
        ) {
            return;
        }

        if self.config.network_mode == NetworkMode::Extreme {
            if !self.execute(
                "启用 RSS/RSC 与 TCP Fast Open",
                "netsh.exe",
                &[
                    "interface",
                    "tcp",
                    "set",
                    "global",
                    "rss=enabled",
                    "rsc=enabled",
                    "fastopen=enabled",
                ],
                45,
            ) {
                return;
            }
            if !self.execute(
                "设置 Internet 模板为 CTCP",
                "netsh.exe",
                &[
                    "interface",
                    "tcp",
                    "set",
                    "supplemental",
                    "template=internet",
                    "congestionprovider=ctcp",
                ],
                45,
            ) {
                return;
            }
            self.execute(
                "启用 ECN",
                "netsh.exe",
                &["interface", "tcp", "set", "global", "ecncapability=enabled"],
                45,
            );
        }
    }

    fn snapshot_netsh_tcp(&self) {
        let path = {
            let rollback = self.rollback.borrow();
            rollback
                .as_ref()
                .map(|journal| journal.path().with_extension("netsh.txt"))
        };
        let Some(path) = path else {
            return;
        };
        let snapshot = self.command("netsh.exe", &["interface", "tcp", "dump"], 45);
        if !snapshot.succeeded() || snapshot.output.trim().is_empty() {
            self.log(
                LogLevel::Error,
                format!("无法备份 TCP 设置：{}", snapshot.diagnostic()),
            );
            self.cancellation.cancel();
            return;
        }
        if let Err(error) = crate::utils::write_preserving(&path, snapshot.output.as_bytes()) {
            self.log(LogLevel::Error, format!("TCP 备份写入失败：{error}"));
            self.cancellation.cancel();
            return;
        }
        self.record_rollback(RollbackAction::Command {
            label: "恢复 TCP 设置".into(),
            program: "netsh.exe".into(),
            args: vec!["exec".into(), path.to_string_lossy().into_owned()],
        });
    }

    fn step_package_managers(&self) -> ManagerAvailability {
        self.log(LogLevel::Start, "包管理器预检与原生引导");
        let packages = &self.config.profile.packages;
        let need_scoop = self.config.include_dev_tools && !packages.scoop_tools.is_empty();
        let need_winget = (self.config.include_curated_extras && !packages.winget_core.is_empty())
            || (self.config.include_dev_tools && !packages.winget_dev.is_empty())
            || self.config.include_curated_extras
            || need_scoop;
        let winget = need_winget && self.ensure_winget();
        let scoop = need_scoop && self.ensure_scoop(winget);
        ManagerAvailability { winget, scoop }
    }

    fn ensure_winget(&self) -> bool {
        let current = self.command("winget.exe", &["--version"], 30);
        if current.succeeded() {
            self.log(
                LogLevel::Ok,
                format!("WinGet 已就绪：{}", current.output.trim()),
            );
            return true;
        }
        if self.stopped() {
            return false;
        }

        self.log(
            LogLevel::Warn,
            "未发现 WinGet；下载官方安装包和依赖，为当前用户安装。",
        );
        if let Err(error) = self.bootstrap_winget() {
            if !self.stopped() {
                self.log(LogLevel::Error, format!("WinGet 原生引导失败：{error:#}"));
            }
            return false;
        }

        let verified = self.command("winget.exe", &["--version"], 45);
        if verified.succeeded() {
            self.log(
                LogLevel::Ok,
                format!("WinGet 已安装：{}", verified.output.trim()),
            );
            true
        } else {
            self.log(
                LogLevel::Error,
                format!(
                    "WinGet 已安装，但当前用户别名尚不可用：{}",
                    verified.diagnostic()
                ),
            );
            false
        }
    }

    fn bootstrap_winget(&self) -> anyhow::Result<()> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let root = std::env::temp_dir().join(format!("ltsc-winget-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&root)?;
        let result = (|| -> anyhow::Result<()> {
            let release = self.command(
                "curl.exe",
                &[
                    "--fail",
                    "--location",
                    "--silent",
                    "--show-error",
                    "--max-time",
                    "60",
                    "https://api.github.com/repos/microsoft/winget-cli/releases/latest",
                ],
                75,
            );
            if !release.succeeded() {
                anyhow::bail!("无法读取 WinGet 最新稳定版：{}", release.diagnostic());
            }
            let metadata: serde_json::Value = serde_json::from_str(&release.output)?;
            let tag = metadata["tag_name"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("WinGet 发布信息缺少版本号"))?;
            if !tag.starts_with('v')
                || !tag
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b".-_".contains(&byte))
            {
                anyhow::bail!("WinGet 发布版本号格式异常：{tag}");
            }
            let bundle = root.join("Microsoft.DesktopAppInstaller_8wekyb3d8bbwe.msixbundle");
            let archive = root.join("DesktopAppInstaller_Dependencies.zip");
            for (label, path) in [("WinGet 安装包", &bundle), ("WinGet 依赖", &archive)] {
                self.log(LogLevel::Info, format!("正在下载{label}（可取消）"));
                let name = path.file_name().unwrap().to_string_lossy();
                let url = format!(
                    "https://github.com/microsoft/winget-cli/releases/download/{tag}/{name}"
                );
                let destination = path.to_string_lossy();
                let download = self.command(
                    "curl.exe",
                    &[
                        "--fail",
                        "--location",
                        "--retry",
                        "3",
                        "--connect-timeout",
                        "20",
                        "--max-time",
                        "600",
                        "--output",
                        &destination,
                        &url,
                    ],
                    630,
                );
                if !download.succeeded() {
                    anyhow::bail!("{label}下载失败：{}", download.diagnostic());
                }
                if std::fs::metadata(path)?.len() < 1024 {
                    anyhow::bail!("{label}下载内容异常短");
                }
            }
            self.log(LogLevel::Info, "正在展开 WinGet 依赖");
            let archive_text = archive.to_string_lossy();
            let root_text = root.to_string_lossy();
            let extract = self.command("tar.exe", &["-xf", &archive_text, "-C", &root_text], 180);
            if !extract.succeeded() {
                anyhow::bail!("WinGet 依赖解包失败：{}", extract.diagnostic());
            }
            let mut dependencies = std::fs::read_dir(root.join("x64"))?
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<std::io::Result<Vec<_>>>()?;
            dependencies.retain(|path| {
                path.is_file()
                    && path.extension().is_some_and(|extension| {
                        let extension = extension.to_string_lossy();
                        extension.eq_ignore_ascii_case("appx")
                            || extension.eq_ignore_ascii_case("msix")
                            || extension.eq_ignore_ascii_case("msixbundle")
                    })
            });
            dependencies.sort();
            if dependencies.is_empty() {
                anyhow::bail!("官方依赖包中没有 x64 MSIX/AppX，停止安装");
            }
            self.log(
                LogLevel::Info,
                format!(
                    "通过 Windows 部署接口安装 WinGet 和 {} 个依赖",
                    dependencies.len()
                ),
            );
            crate::winget_bootstrap::install(&bundle, &dependencies, &self.cancellation)
        })();
        if let Err(error) = std::fs::remove_dir_all(&root) {
            self.log(
                LogLevel::Warn,
                format!("WinGet 临时下载目录未能清理（{}）：{error}", root.display()),
            );
        }
        result
    }

    fn ensure_scoop(&self, winget_available: bool) -> bool {
        let current = self.command("scoop.cmd", &["--version"], 30);
        if current.succeeded() {
            if !self.configure_scoop_buckets() {
                return false;
            }
            self.log(
                LogLevel::Ok,
                format!(
                    "Scoop 已就绪：{}",
                    current.output.lines().next().unwrap_or("unknown")
                ),
            );
            return true;
        }
        if self.stopped() {
            return false;
        }
        self.log(
            LogLevel::Info,
            "Scoop 未安装；Rust 将直接部署官方仓库与 shim，不执行远程安装脚本。",
        );
        let git_probe = self.command("git", &["--version"], 30);
        match &git_probe.state {
            CommandState::Cancelled => {
                self.stopped();
                return false;
            }
            CommandState::TimedOut | CommandState::WaitFailed | CommandState::OutputFailed => {
                self.log(
                    LogLevel::Error,
                    format!("Git 状态检查未正常完成：{}", git_probe.diagnostic()),
                );
                return false;
            }
            CommandState::Exited if !git_probe.succeeded() => {
                self.log(
                    LogLevel::Error,
                    format!("Git 已启动但版本检查失败：{}", git_probe.diagnostic()),
                );
                return false;
            }
            CommandState::Exited | CommandState::SpawnFailed => {}
        }
        let git_available = git_probe.succeeded();
        match scoop_git_action(git_available, winget_available) {
            ScoopGitAction::UseExisting => {
                self.log(LogLevel::Ok, "Scoop 引导使用现有 Git。");
            }
            ScoopGitAction::InstallWithWinget => {
                if !self.install_winget_app("Git.Git", "Git") {
                    if !self.stopped() {
                        self.log(LogLevel::Error, "无法安装 Git，Scoop 引导已停止。");
                    }
                    return false;
                }
                if !self.command("git", &["--version"], 30).succeeded() {
                    if !self.stopped() {
                        self.log(
                            LogLevel::Error,
                            "Git 安装完成但当前进程仍无法解析 git.exe；请重启后重试。",
                        );
                    }
                    return false;
                }
            }
            ScoopGitAction::WingetRequired => {
                if !self.stopped() {
                    self.log(
                        LogLevel::Error,
                        "Scoop 引导需要 Git；未发现 Git 且 WinGet 不可用，已停止该提供程序。",
                    );
                }
                return false;
            }
        }

        let scoop_root = match crate::platform::scoop_root() {
            Ok(path) => path,
            Err(error) => {
                self.log(LogLevel::Error, format!("Scoop 根目录不可用：{error}"));
                return false;
            }
        };
        if let Err(error) = crate::utils::reject_links(&scoop_root) {
            self.log(LogLevel::Error, format!("Scoop 根目录包含链接：{error}"));
            return false;
        }
        let current_dir = scoop_root.join("apps").join("scoop").join("current");
        let scoop_script = current_dir.join("bin").join("scoop.ps1");

        if !scoop_script.exists() {
            if current_dir.exists() {
                self.log(
                    LogLevel::Error,
                    format!(
                        "Scoop 目录不完整，已保留现场而未覆盖：{}",
                        current_dir.display()
                    ),
                );
                return false;
            }
            if let Some(parent) = current_dir.parent()
                && let Err(error) = std::fs::create_dir_all(parent)
            {
                self.log(LogLevel::Error, format!("无法创建 Scoop 目录：{error}"));
                return false;
            }
            let current_text = current_dir.to_string_lossy().into_owned();
            if !self.execute(
                "下载 Scoop 官方仓库",
                "git",
                &[
                    "clone",
                    "--depth",
                    "1",
                    "https://github.com/ScoopInstaller/Scoop.git",
                    &current_text,
                ],
                300,
            ) {
                return false;
            }
        }

        let shims = scoop_root.join("shims");
        if let Err(error) = std::fs::create_dir_all(&shims) {
            self.log(LogLevel::Error, format!("无法创建 Scoop shims：{error}"));
            return false;
        }
        let shim = shims.join("scoop.cmd");
        let shim_content = r#"@echo off
where pwsh.exe >nul 2>&1
if %errorlevel%==0 (
  pwsh.exe -NoLogo -NoProfile -File "%~dp0..\apps\scoop\current\bin\scoop.ps1" %*
) else (
  powershell.exe -NoLogo -NoProfile -File "%~dp0..\apps\scoop\current\bin\scoop.ps1" %*
)
"#;
        if let Err(error) = crate::utils::write_preserving(&shim, shim_content.as_bytes()) {
            self.log(
                LogLevel::Error,
                format!("无法写入 Scoop shim {}：{error}", shim.display()),
            );
            return false;
        }
        if let Err(error) = set_user_environment("SCOOP", &scoop_root.to_string_lossy()) {
            self.log(LogLevel::Error, format!("无法保存 SCOOP 环境变量：{error}"));
            return false;
        }
        if let Err(error) = prepend_user_path(&shims) {
            self.log(LogLevel::Error, format!("无法更新用户 PATH：{error}"));
            return false;
        }

        let verified = self.command("scoop.cmd", &["--version"], 45);
        if !verified.succeeded() {
            self.log(
                LogLevel::Error,
                format!("Scoop shim 验证失败：{}", verified.diagnostic()),
            );
            return false;
        }
        if !self.configure_scoop_buckets() {
            return false;
        }
        self.log(LogLevel::Ok, "Scoop 官方仓库与 shim 已由 Rust 配置完成。");
        true
    }

    fn configure_scoop_buckets(&self) -> bool {
        let listed = self.command("scoop.cmd", &["bucket", "list"], 60);
        let output = if listed.succeeded() {
            listed.output
        } else {
            String::new()
        };
        let mut ready = true;
        for bucket in ["main", "extras", "versions"] {
            if scoop_bucket_is_listed(&output, bucket) {
                continue;
            }
            let result = self.command("scoop.cmd", &["bucket", "add", bucket], 300);
            if result.succeeded() {
                self.log(LogLevel::Ok, format!("Scoop bucket：{bucket}"));
            } else if !result.cancelled() {
                ready = false;
                self.log(
                    LogLevel::Error,
                    format!("Scoop bucket {bucket} 添加失败：{}", result.diagnostic()),
                );
            }
        }
        ready && !self.stopped()
    }

    fn step_environment_mirrors(&self) {
        let mirrors = &self.config.profile.environment_mirrors;
        self.log(LogLevel::Start, "配置 Cargo、Pip 与 NPM 镜像");
        let home = user_home();

        let cargo_file = home.join(".cargo").join("config.toml");
        let cargo_body = format!(
            "[source.crates-io]\nreplace-with = 'ltsc-tools-mirror'\n\n[source.ltsc-tools-mirror]\nregistry = {:?}",
            mirrors.cargo_sparse_index
        );
        let existing = match crate::utils::read_config(&cargo_file) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => {
                self.log(LogLevel::Error, format!("读取 Cargo 配置失败：{error}"));
                return;
            }
        };
        if existing.contains("[source.crates-io]") && !existing.contains(MANAGED_START) {
            self.log(
                LogLevel::Warn,
                format!(
                    "已保留现有 Cargo source 配置，避免产生重复 TOML 表：{}",
                    cargo_file.display()
                ),
            );
        } else {
            self.write_managed(&cargo_file, &cargo_body, "Cargo 镜像");
        }

        let appdata = app_data(&home);
        let pip_file = appdata.join("pip").join("pip.ini");
        match upsert_ini_values(
            &pip_file,
            "global",
            &[("index-url", mirrors.pip_index_url.as_str())],
        ) {
            Ok(true) => self.log(
                LogLevel::Ok,
                format!("Pip 镜像已更新：{}", pip_file.display()),
            ),
            Ok(false) => self.log(
                LogLevel::Ok,
                format!("Pip 镜像无需更改：{}", pip_file.display()),
            ),
            Err(error) => self.log(LogLevel::Error, format!("Pip 镜像写入失败：{error}")),
        }
    }

    fn step_uwp_apps(&self, winget_available: bool, start: f32, end: f32) {
        if !winget_available {
            self.log(LogLevel::Warn, "WinGet 不可用，已跳过 UWP 应用恢复。");
            self.progress(end);
            return;
        }
        self.log(LogLevel::Start, "恢复 LTSC 常用 UWP 应用");
        let apps = [
            ("9WZDNCRFHVN5", "Windows Calculator"),
            ("9WZDNCRFJBH4", "Microsoft Photos"),
            ("9PCFS5B6T72H", "Microsoft Paint"),
            ("9MZ95KL8MR0L", "Snipping Tool"),
            ("9N0DX20HK701", "Windows Terminal"),
            ("9NT1R1C2HH7J", "ChatGPT"),
        ];
        for (index, (id, name)) in apps.iter().enumerate() {
            if self.stopped() {
                break;
            }
            self.install_store_app(id, name);
            self.progress(item_progress(start, end, index, apps.len()));
        }
    }

    fn step_windows_features(&self) {
        let features = &self.config.profile.windows_features;
        self.log(LogLevel::Start, "检测并启用 Windows 可选功能");

        if features.enable_wsl2 {
            for feature in [
                "VirtualMachinePlatform",
                "Microsoft-Windows-Subsystem-Linux",
            ] {
                if !self.enable_windows_feature(feature, true) {
                    return;
                }
            }
        }
        if features.enable_netfx3 {
            self.enable_windows_feature("NetFx3", false);
        }
        if features.enable_windows_sandbox {
            self.enable_windows_feature("Containers-DisposableClientVM", false);
        }
        if features.enable_hyper_v {
            self.enable_windows_feature("Microsoft-Hyper-V", false);
        }

        if features.enable_wsl2 && !self.stopped() {
            let result = self.command("wsl.exe", &["--set-default-version", "2"], 90);
            if result.succeeded() {
                self.log(LogLevel::Ok, "WSL 默认版本已设置为 2");
            } else if !result.cancelled() {
                self.log(
                    LogLevel::Warn,
                    format!(
                        "WSL 可能需要重启后才能完成默认版本设置：{}",
                        result.diagnostic()
                    ),
                );
            }
        }
    }

    fn enable_windows_feature(&self, feature: &str, required: bool) -> bool {
        if self.stopped() {
            return false;
        }
        let feature_arg = format!("/FeatureName:{feature}");
        let result = self.command(
            "dism.exe",
            &[
                "/Online",
                "/Enable-Feature",
                &feature_arg,
                "/All",
                "/NoRestart",
            ],
            900,
        );
        if result.succeeded_or_reboot_required() {
            self.log(LogLevel::Ok, format!("Windows 功能已启用：{feature}"));
            true
        } else if result.cancelled() {
            self.stopped();
            false
        } else if required {
            self.log(
                LogLevel::Error,
                format!(
                    "必要 Windows 功能 {feature} 启用失败：{}",
                    result.diagnostic()
                ),
            );
            false
        } else {
            self.log(
                LogLevel::Warn,
                format!(
                    "当前 LTSC 版本或硬件不支持可选功能 {feature}：{}",
                    result.diagnostic()
                ),
            );
            true
        }
    }

    fn step_agent_skills(&self) {
        self.log(
            LogLevel::Error,
            "旧内置 Agent payload 未迁移为受信资源，未写入任何插件或 MCP 配置。",
        );
    }

    fn step_git_shell_configs(&self) {
        let git = &self.config.profile.git_config;
        let shell = &self.config.profile.powershell_profile;
        self.log(LogLevel::Start, "同步 Git 与 PowerShell 7 Profile");

        let ignore_file = user_home().join(".config").join("git").join("ignore");
        self.write_managed(
            &ignore_file,
            &git.global_gitignore_rules.join("\n"),
            "Git 全局忽略规则",
        );

        if !git.user_name.is_empty() {
            self.git_config("user.name", &git.user_name);
        }
        if !git.user_email.is_empty() {
            self.git_config("user.email", &git.user_email);
        }
        self.git_config("http.postBuffer", &git.post_buffer_bytes.to_string());
        if git.safe_directory.trim().is_empty() {
            let config_path = user_home().join(".gitconfig");
            if crate::utils::reject_links(&config_path).is_err() {
                self.log(LogLevel::Error, "Git 配置路径包含链接");
                return;
            }
            let config_path = config_path.to_string_lossy();
            let result = run_native_cmd(
                "git",
                &[
                    "config",
                    "--file",
                    &config_path,
                    "--unset-all",
                    "safe.directory",
                    "^\\*$",
                ],
                &self.cancellation,
            );
            if result.succeeded() || result.exit_code == Some(5) {
                self.log(LogLevel::Ok, "Git safe.directory 未使用全局通配符");
            }
        } else {
            self.git_config("safe.directory", &git.safe_directory);
        }
        self.git_config(
            "core.longpaths",
            if git.enable_long_paths {
                "true"
            } else {
                "false"
            },
        );
        self.git_config(
            "core.autocrlf",
            if git.enable_autocrlf { "true" } else { "false" },
        );
        self.git_config("core.excludesfile", "~/.config/git/ignore");
        self.git_config("filter.lfs.required", "true");

        let mut body = String::new();
        if shell.enable_utf8_encoding {
            body.push_str("[Console]::OutputEncoding = [System.Text.Encoding]::UTF8\n");
        }
        if shell.init_starship {
            body.push_str(
                "if (Get-Command starship -ErrorAction SilentlyContinue) { Invoke-Expression (&starship init powershell) }\n",
            );
        }
        if shell.init_zoxide {
            body.push_str(
                "if (Get-Command zoxide -ErrorAction SilentlyContinue) { Invoke-Expression (& { (zoxide init powershell | Out-String) }) }\n",
            );
        }
        for (name, value) in &shell.aliases {
            body.push_str(&format!(
                "Set-Alias -Name '{}' -Value '{}' -ErrorAction SilentlyContinue\n",
                ps_escape(name),
                ps_escape(value)
            ));
        }
        let profile = user_home()
            .join("Documents")
            .join("PowerShell")
            .join("Microsoft.PowerShell_profile.ps1");
        self.write_managed(&profile, &body, "PowerShell 7 Profile");
    }

    fn git_config(&self, key: &str, value: &str) {
        let path = user_home().join(".gitconfig");
        if let Err(error) = crate::utils::reject_links(&path) {
            self.log(LogLevel::Error, format!("Git 配置路径不可用：{error}"));
            return;
        }
        let Some(path) = path.to_str() else {
            self.log(LogLevel::Error, "Git 配置路径不是有效 Unicode");
            return;
        };
        let result = run_native_cmd(
            "git",
            &["config", "--file", path, key, value],
            &self.cancellation,
        );
        if result.succeeded() {
            self.log(
                LogLevel::Ok,
                format!("git config {key} 已写入（原文件保留在本次历史）"),
            );
        } else if !result.cancelled() {
            self.log(
                LogLevel::Error,
                format!("git config {key} 失败：{}", result.diagnostic()),
            );
        }
    }

    fn step_vscode_and_tools_config(&self, start: f32, end: f32) {
        let profile = &self.config.profile;
        self.log(LogLevel::Start, "合并 IDE 设置并同步扩展");
        let home = user_home();

        if self.config.include_npmrc_config {
            let npmrc = home.join(".npmrc");
            let body = format!(
                "registry={}\nignore-scripts=true",
                profile.environment_mirrors.npm_registry
            );
            self.write_managed(&npmrc, &body, "NPM 配置");
        }
        if !self.config.include_vscode_extensions {
            return;
        }

        let appdata = app_data(&home);
        for (label, directory) in [
            ("VS Code", appdata.join("Code").join("User")),
            ("Cursor", appdata.join("Cursor").join("User")),
        ] {
            let path = directory.join("settings.json");
            match merge_json_file(&path, &profile.vscode_config.user_settings) {
                Ok(()) => self.log(
                    LogLevel::Ok,
                    format!("{label} 设置已合并：{}", path.display()),
                ),
                Err(error) => self.log(LogLevel::Warn, format!("{label} 设置未覆盖：{error}")),
            }
        }

        let code_available = self.command("code.cmd", &["--version"], 20).succeeded();
        let cursor_available = self.command("cursor.cmd", &["--version"], 20).succeeded();
        if !code_available && !cursor_available {
            self.log(
                LogLevel::Error,
                "未发现 code 或 cursor 命令；IDE 扩展同步已停止。",
            );
            self.progress(end);
            return;
        }

        let extensions = &profile.vscode_config.extensions;
        for (index, extension) in extensions.iter().enumerate() {
            if self.stopped() {
                break;
            }
            let mut installed = false;
            if code_available {
                installed |= self
                    .command(
                        "code.cmd",
                        &["--install-extension", extension, "--force"],
                        120,
                    )
                    .succeeded();
            }
            if cursor_available {
                installed |= self
                    .command(
                        "cursor.cmd",
                        &["--install-extension", extension, "--force"],
                        120,
                    )
                    .succeeded();
            }
            if installed {
                self.log(LogLevel::Ok, format!("IDE 扩展：{extension}"));
            } else if !self.stopped() {
                self.log(LogLevel::Error, format!("IDE 扩展安装失败：{extension}"));
            }
            self.progress(item_progress(start, end, index, extensions.len()));
        }
    }

    fn step_core_apps(&self, winget_available: bool, start: f32, end: f32) {
        if !winget_available {
            self.log(LogLevel::Warn, "WinGet 不可用，已跳过桌面软件。");
            self.progress(end);
            return;
        }
        let apps = &self.config.profile.packages.winget_core;
        self.log(
            LogLevel::Start,
            format!("安装 {} 款核心桌面软件", apps.len()),
        );
        for (index, app) in apps.iter().enumerate() {
            if self.stopped() {
                break;
            }
            self.install_winget_app(&app.id, &app.name);
            self.progress(item_progress(start, end, index, apps.len()));
        }
    }

    fn step_dev_suite(&self, managers: ManagerAvailability, start: f32, end: f32) {
        let packages = &self.config.profile.packages;
        self.log(LogLevel::Start, "同步开发工具矩阵");
        let span = end - start;
        let winget_end = start + span * 0.20;
        let scoop_end = start + span * 0.52;
        let cargo_end = start + span * 0.72;
        let npm_end = start + span * 0.86;
        let pip_end = start + span * 0.96;

        if managers.winget {
            for (index, app) in packages.winget_dev.iter().enumerate() {
                if self.stopped() {
                    return;
                }
                self.install_winget_app(&app.id, &app.name);
                self.progress(item_progress(
                    start,
                    winget_end,
                    index,
                    packages.winget_dev.len(),
                ));
            }
        } else {
            self.log(LogLevel::Warn, "WinGet 不可用，已跳过开发桌面应用。");
            self.progress(winget_end);
        }

        if managers.scoop {
            let listed = self.command("scoop.cmd", &["list"], 90).output;
            for (index, tool) in packages.scoop_tools.iter().enumerate() {
                if self.stopped() {
                    return;
                }
                if package_is_listed(&listed, tool) {
                    self.log(LogLevel::Ok, format!("Scoop：{tool} 已安装"));
                } else {
                    self.install_provider_package(
                        "Scoop",
                        "scoop.cmd",
                        &["install", tool],
                        tool,
                        900,
                    );
                }
                self.progress(item_progress(
                    winget_end,
                    scoop_end,
                    index,
                    packages.scoop_tools.len(),
                ));
            }
        } else {
            self.log(LogLevel::Warn, "Scoop 不可用，已跳过便携 CLI 工具。");
            self.progress(scoop_end);
        }

        let cargo_available = self.command("cargo", &["--version"], 30).succeeded();
        if cargo_available {
            let listed = self.command("cargo", &["install", "--list"], 90).output;
            for (index, package) in packages.cargo_packages.iter().enumerate() {
                if self.stopped() {
                    return;
                }
                if package_is_listed(&listed, package) {
                    self.log(LogLevel::Ok, format!("Cargo：{package} 已安装"));
                } else {
                    self.install_provider_package(
                        "Cargo",
                        "cargo",
                        &["install", package, "--locked"],
                        package,
                        1_800,
                    );
                }
                self.progress(item_progress(
                    scoop_end,
                    cargo_end,
                    index,
                    packages.cargo_packages.len(),
                ));
            }
        } else if !packages.cargo_packages.is_empty() {
            self.log(
                LogLevel::Error,
                "cargo 不可用，已跳过 Rust CLI；请确认 Rustlang.Rustup 安装成功。",
            );
            self.progress(cargo_end);
        }

        let npm_available = self.command("npm.cmd", &["--version"], 30).succeeded();
        if npm_available {
            for (index, package) in packages.npm_globals.iter().enumerate() {
                if self.stopped() {
                    return;
                }
                let installed = self
                    .command("npm.cmd", &["list", "-g", package, "--depth=0"], 60)
                    .succeeded();
                if installed {
                    self.log(LogLevel::Ok, format!("NPM：{package} 已安装"));
                } else {
                    self.install_provider_package(
                        "NPM",
                        "npm.cmd",
                        &["install", "-g", package, "--loglevel=error"],
                        package,
                        900,
                    );
                }
                self.progress(item_progress(
                    cargo_end,
                    npm_end,
                    index,
                    packages.npm_globals.len(),
                ));
            }
        } else if !packages.npm_globals.is_empty() {
            self.log(LogLevel::Error, "npm 不可用，已跳过全局 NPM 工具。");
            self.progress(npm_end);
        }

        let python_available = self.command("python", &["--version"], 30).succeeded();
        if python_available {
            let listed = self
                .command("python", &["-m", "pip", "list", "--format=freeze"], 90)
                .output;
            for (index, package) in packages.pip_packages.iter().enumerate() {
                if self.stopped() {
                    return;
                }
                if python_package_is_listed(&listed, package) {
                    self.log(LogLevel::Ok, format!("Pip：{package} 已安装"));
                } else {
                    self.install_provider_package(
                        "Pip",
                        "python",
                        &["-m", "pip", "install", package, "--quiet"],
                        package,
                        1_200,
                    );
                }
                self.progress(item_progress(
                    npm_end,
                    pip_end,
                    index,
                    packages.pip_packages.len(),
                ));
            }
        } else if !packages.pip_packages.is_empty() {
            self.log(LogLevel::Error, "python 不可用，已跳过 Pip 包。");
            self.progress(pip_end);
        }

        let uv_available = self.command("uv", &["--version"], 30).succeeded();
        if uv_available {
            let listed = self.command("uv", &["tool", "list"], 60).output;
            for (index, tool) in packages.uv_tools.iter().enumerate() {
                if self.stopped() {
                    return;
                }
                if package_is_listed(&listed, tool) {
                    self.log(LogLevel::Ok, format!("UV：{tool} 已安装"));
                } else {
                    self.install_provider_package(
                        "UV",
                        "uv",
                        &["tool", "install", tool],
                        tool,
                        900,
                    );
                }
                self.progress(item_progress(pip_end, end, index, packages.uv_tools.len()));
            }
        } else if !packages.uv_tools.is_empty() {
            self.log(LogLevel::Warn, "uv 不可用，已跳过 UV 工具。");
            self.progress(end);
        }
    }

    fn step_ollama_models(&self) {
        if !self.command("ollama", &["--version"], 30).succeeded() {
            self.log(LogLevel::Error, "ollama 不可用，无法拉取本地模型。");
            return;
        }
        for model in &self.config.profile.ollama_models {
            if self.stopped() {
                return;
            }
            self.log(
                LogLevel::Info,
                format!("拉取 Ollama 模型 {model}；可随时点击取消。"),
            );
            let result = self.command("ollama", &["pull", model], 1_800);
            if result.succeeded() {
                self.log(LogLevel::Ok, format!("Ollama 模型：{model}"));
            } else if !result.cancelled() {
                self.log(
                    LogLevel::Error,
                    format!("Ollama 模型 {model} 拉取失败：{}", result.diagnostic()),
                );
            }
        }
    }

    fn step_storage(&self) {
        let tweaks = &self.config.profile.system_tweaks;
        self.log(
            LogLevel::Start,
            "存储优化 · Windows 按介质类型选择 TRIM/碎片整理策略",
        );
        if tweaks.enable_trim {
            let query = self.command(
                "fsutil.exe",
                &["behavior", "query", "DisableDeleteNotify"],
                45,
            );
            let Some(states) = query
                .succeeded()
                .then(|| parse_trim_states(&query.output))
                .flatten()
            else {
                self.log(
                    LogLevel::Error,
                    "无法分别确认 NTFS/ReFS 删除通知原值，已停止 TRIM 写入。",
                );
                return;
            };
            for (filesystem, previous) in states {
                if !self.record_rollback(RollbackAction::Command {
                    label: format!("恢复 {filesystem} TRIM 通知设置"),
                    program: "fsutil.exe".into(),
                    args: vec![
                        "behavior".into(),
                        "set".into(),
                        "DisableDeleteNotify".into(),
                        filesystem.into(),
                        previous.to_string(),
                    ],
                }) {
                    return;
                }
            }
            if self.stopped() {
                return;
            }
            for filesystem in ["NTFS", "ReFS"] {
                if !self.execute(
                    "启用删除通知（TRIM/UNMAP）",
                    "fsutil.exe",
                    &["behavior", "set", "DisableDeleteNotify", filesystem, "0"],
                    45,
                ) {
                    return;
                }
            }
            let verified = self.command(
                "fsutil.exe",
                &["behavior", "query", "DisableDeleteNotify"],
                45,
            );
            if !verified.succeeded()
                || parse_trim_states(&verified.output) != Some([("NTFS", 0), ("ReFS", 0)])
            {
                self.log(
                    LogLevel::Error,
                    "TRIM 写后校验失败；保留旧账本与原值，未标记成功。",
                );
                return;
            }
        }
        if tweaks.optimize_storage && !self.stopped() {
            self.execute(
                "按 SSD/HDD 类型优化所有固定卷",
                "defrag.exe",
                &["/C", "/O", "/U"],
                2_700,
            );
        }
    }

    fn step_deep_win_tweaks(&self) {
        let tweaks = &self.config.profile.system_tweaks;
        self.log(LogLevel::Start, "应用 Windows 原生注册表与电源设置");

        if tweaks.activate_ultimate_performance {
            self.activate_ultimate_performance();
        }
        if tweaks.disable_hibernation && !self.stopped() {
            match get_registry_dword(
                RegistryHive::LocalMachine,
                r"SYSTEM\CurrentControlSet\Control\Power",
                "HibernateEnabled",
            ) {
                Ok(Some(0)) => self.log(LogLevel::Ok, "休眠原本已关闭"),
                Ok(Some(_)) => {
                    if self.record_rollback(RollbackAction::Command {
                        label: "重新启用休眠".into(),
                        program: "powercfg.exe".into(),
                        args: vec!["/hibernate".into(), "on".into()],
                    }) {
                        self.execute(
                            "关闭休眠（同时关闭依赖休眠文件的快速启动）",
                            "powercfg.exe",
                            &["/hibernate", "off"],
                            45,
                        );
                    }
                }
                Ok(None) => self.log(
                    LogLevel::Warn,
                    "无法确认休眠原始状态，为避免错误回滚已跳过关闭休眠",
                ),
                Err(error) => self.log(
                    LogLevel::Warn,
                    format!("读取休眠状态失败，已跳过关闭休眠：{error}"),
                ),
            }
        }
        if tweaks.disable_telemetry {
            self.write_registry(
                RegistryHive::LocalMachine,
                r"SOFTWARE\Policies\Microsoft\Windows\DataCollection",
                "AllowTelemetry",
                0,
            );
        }
        if tweaks.disable_bing_search {
            self.write_registry(
                RegistryHive::CurrentUser,
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\Search",
                "BingSearchEnabled",
                0,
            );
            self.write_registry(
                RegistryHive::CurrentUser,
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\Search",
                "DisableSearchBoxSuggestions",
                1,
            );
        }
        if tweaks.disable_consumer_features {
            self.write_registry(
                RegistryHive::LocalMachine,
                r"SOFTWARE\Policies\Microsoft\Windows\CloudContent",
                "DisableWindowsConsumerFeatures",
                1,
            );
        }
        if tweaks.disable_advertising_id {
            self.write_registry(
                RegistryHive::CurrentUser,
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\AdvertisingInfo",
                "Enabled",
                0,
            );
        }
        if tweaks.disable_activity_history {
            for (name, value) in [
                ("EnableActivityFeed", 0),
                ("PublishUserActivities", 0),
                ("UploadUserActivities", 0),
            ] {
                self.write_registry(
                    RegistryHive::LocalMachine,
                    r"SOFTWARE\Policies\Microsoft\Windows\System",
                    name,
                    value,
                );
            }
        }
        if tweaks.disable_feedback_prompts {
            self.write_registry(
                RegistryHive::CurrentUser,
                r"SOFTWARE\Microsoft\Siuf\Rules",
                "NumberOfSIUFInPeriod",
                0,
            );
        }
        if tweaks.explorer_open_to_this_pc {
            self.write_registry(
                RegistryHive::CurrentUser,
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\Advanced",
                "LaunchTo",
                1,
            );
        }
        if tweaks.explorer_show_file_extensions {
            self.write_registry(
                RegistryHive::CurrentUser,
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\Advanced",
                "HideFileExt",
                0,
            );
        }
        if tweaks.explorer_show_hidden_files {
            self.write_registry(
                RegistryHive::CurrentUser,
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\Advanced",
                "Hidden",
                1,
            );
        }
        if tweaks.enable_ntfs_long_paths {
            self.write_registry(
                RegistryHive::LocalMachine,
                r"SYSTEM\CurrentControlSet\Control\FileSystem",
                "LongPathsEnabled",
                1,
            );
        }
        if tweaks.enable_developer_mode {
            self.write_registry(
                RegistryHive::LocalMachine,
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock",
                "AllowDevelopmentWithoutDevLicense",
                1,
            );
        }
        self.write_registry(
            RegistryHive::LocalMachine,
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Multimedia\SystemProfile",
            "SystemResponsiveness",
            tweaks.system_responsiveness,
        );
        self.write_registry(
            RegistryHive::LocalMachine,
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Multimedia\SystemProfile",
            "NetworkThrottlingIndex",
            tweaks.network_throttling_index as u32,
        );
    }

    fn activate_ultimate_performance(&self) {
        const TEMPLATE: &str = "e9a42b02-d5df-448d-aa00-03f14749eb61";
        const MANAGED_NAME: &str = "LTSC Workspace Ultimate";
        let previous = self.command("powercfg.exe", &["/getactivescheme"], 30);
        let previous_guid = extract_guid(&previous.output);
        let listed = self.command("powercfg.exe", &["/list"], 30);
        let managed_guid = listed
            .output
            .lines()
            .find(|line| line.contains(MANAGED_NAME))
            .and_then(extract_guid);
        let duplicate = managed_guid
            .is_none()
            .then(|| self.command("powercfg.exe", &["/duplicatescheme", TEMPLATE], 60));
        let created_guid = duplicate
            .as_ref()
            .filter(|result| result.succeeded())
            .and_then(|result| extract_guid(&result.output));
        let guid = managed_guid.or_else(|| created_guid.clone());
        let Some(guid) = guid else {
            self.log(
                LogLevel::Warn,
                format!(
                    "未能创建卓越性能电源计划：{}",
                    duplicate
                        .as_ref()
                        .map_or("无命令结果", CommandResult::diagnostic)
                ),
            );
            return;
        };

        if created_guid.is_some() {
            let renamed = self.command("powercfg.exe", &["/changename", &guid, MANAGED_NAME], 45);
            if !renamed.succeeded() {
                self.log(
                    LogLevel::Warn,
                    format!("电源计划命名失败：{}", renamed.diagnostic()),
                );
            }
            if !self.record_rollback(RollbackAction::Command {
                label: "删除本次创建的卓越性能计划".into(),
                program: "powercfg.exe".into(),
                args: vec!["/delete".into(), guid.clone()],
            }) {
                return;
            }
        }
        if let Some(previous_guid) = previous_guid.filter(|previous| previous != &guid)
            && !self.record_rollback(RollbackAction::Command {
                label: "恢复原电源计划".into(),
                program: "powercfg.exe".into(),
                args: vec!["/setactive".into(), previous_guid],
            })
        {
            return;
        }

        if self.config.profile.system_tweaks.extreme_ac_power_settings {
            self.configure_extreme_ac_power(&guid);
        }
        if self.stopped() {
            return;
        }
        let result = self.command("powercfg.exe", &["-setactive", &guid], 45);
        if result.succeeded() {
            self.log(LogLevel::Ok, format!("卓越性能电源计划：{guid}"));
        } else if !result.cancelled() {
            self.log(
                LogLevel::Warn,
                format!("卓越性能电源计划未启用：{}", result.diagnostic()),
            );
        }
    }

    fn configure_extreme_ac_power(&self, scheme: &str) {
        const PROCESSOR: &str = "54533251-82be-4824-96c1-47b60b740d00";
        const PROCESSOR_MIN: &str = "893dee8e-2bef-41e0-89c6-b55d0929964c";
        const PROCESSOR_MAX: &str = "bc5038f7-23e0-4960-96da-33abaf5935ec";
        const DISK: &str = "0012ee47-9041-4b5d-9b77-535fba8b1442";
        const DISK_IDLE: &str = "6738e2c4-e8a5-4a42-b16a-e040e769756e";
        const SLEEP: &str = "238c9fa8-0aad-41ed-83f4-97be242c8f20";
        const STANDBY: &str = "29f6c1db-86da-48c5-9fdb-f2b67b1f44da";
        const HIBERNATE: &str = "9d7815a6-7ee4-497e-8888-515a05f02364";
        const USB: &str = "2a737441-1930-4402-8d77-b2bebba308a3";
        const USB_SELECTIVE: &str = "48e6b7a6-50f5-4782-a5d4-53bb8f07e226";
        const PCI: &str = "501a4d13-42af-4429-9fd1-a8218c268e20";
        const PCI_ASPM: &str = "ee12f906-d277-404b-b6da-e5fa1a576df5";

        self.log(
            LogLevel::Info,
            "极限电源仅修改接通电源（AC）参数，不改电池（DC）策略",
        );
        for (label, subgroup, setting, value) in [
            ("AC 处理器最低状态 100%", PROCESSOR, PROCESSOR_MIN, "100"),
            ("AC 处理器最高状态 100%", PROCESSOR, PROCESSOR_MAX, "100"),
            ("AC 磁盘不自动断电", DISK, DISK_IDLE, "0"),
            ("AC 不自动睡眠", SLEEP, STANDBY, "0"),
            ("AC 不自动休眠", SLEEP, HIBERNATE, "0"),
            ("AC 关闭 USB 选择性暂停", USB, USB_SELECTIVE, "0"),
            ("AC 关闭 PCIe 链路节能", PCI, PCI_ASPM, "0"),
        ] {
            if self.stopped() {
                return;
            }
            let result = self.command(
                "powercfg.exe",
                &["/setacvalueindex", scheme, subgroup, setting, value],
                45,
            );
            if result.succeeded() {
                self.log(LogLevel::Ok, label);
            } else if !result.cancelled() {
                self.log(
                    LogLevel::Warn,
                    format!("{label} 不受当前硬件支持：{}", result.diagnostic()),
                );
            }
        }
    }

    fn write_registry(&self, hive: RegistryHive, path: &str, name: &str, value: u32) {
        if self.stopped() {
            return;
        }
        match get_registry_dword(hive, path, name) {
            Ok(previous) => {
                if !self.record_rollback(RollbackAction::RegistryDword {
                    hive,
                    path: path.to_string(),
                    name: name.to_string(),
                    previous,
                }) {
                    return;
                }
            }
            Err(error) => {
                self.log(
                    LogLevel::Error,
                    format!("[REG] 无法备份 {hive:?}\\{path}\\{name}：{error}"),
                );
                self.cancellation.cancel();
                return;
            }
        }
        match set_registry_dword(hive, path, name, value) {
            Ok(()) => self.log(
                LogLevel::Ok,
                format!("[REG] {hive:?}\\{path}\\{name} = {value}"),
            ),
            Err(error) => self.log(
                LogLevel::Error,
                format!("[REG] {hive:?}\\{path}\\{name} 写入失败：{error}"),
            ),
        }
    }

    fn install_winget_app(&self, id: &str, name: &str) -> bool {
        let listed = self.command(
            "winget.exe",
            &["list", "--id", id, "-e", "--disable-interactivity"],
            90,
        );
        if listed.succeeded()
            && listed
                .output
                .to_ascii_lowercase()
                .contains(&id.to_ascii_lowercase())
        {
            self.log(LogLevel::Ok, format!("WinGet：{name} 已安装"));
            return true;
        }

        self.install_provider_package(
            "WinGet",
            "winget.exe",
            &[
                "install",
                "--id",
                id,
                "-e",
                "--source",
                "winget",
                "--silent",
                "--disable-interactivity",
            ],
            name,
            1_200,
        )
    }

    fn install_store_app(&self, product_id: &str, name: &str) -> bool {
        let listed = self.command(
            "winget.exe",
            &["list", "--id", product_id, "-e", "--disable-interactivity"],
            90,
        );
        if listed.succeeded()
            && listed
                .output
                .to_ascii_lowercase()
                .contains(&product_id.to_ascii_lowercase())
        {
            self.log(LogLevel::Ok, format!("Microsoft Store：{name} 已安装"));
            return true;
        }

        self.install_provider_package(
            "Microsoft Store",
            "winget.exe",
            &[
                "install",
                "--id",
                product_id,
                "-e",
                "--source",
                "msstore",
                "--silent",
                "--disable-interactivity",
            ],
            name,
            1_200,
        )
    }

    fn install_provider_package(
        &self,
        provider: &str,
        program: &str,
        args: &[&str],
        name: &str,
        timeout_secs: u64,
    ) -> bool {
        self.log(LogLevel::Info, format!("{provider} 安装：{name}"));
        let first = self.command(program, args, timeout_secs);
        if first.succeeded() {
            self.log(LogLevel::Ok, format!("{provider}：{name}"));
            return true;
        }
        if first.cancelled() {
            self.stopped();
            return false;
        }
        if first.state != CommandState::Exited {
            self.log(
                LogLevel::Error,
                format!("{provider} 安装 {name} 未正常退出：{}", first.diagnostic()),
            );
            return false;
        }

        self.log(
            LogLevel::Warn,
            format!(
                "{provider} 首次安装 {name} 失败，自动重试一次：{}",
                first.diagnostic()
            ),
        );
        let second = self.command(program, args, timeout_secs);
        if second.succeeded() {
            self.log(LogLevel::Ok, format!("{provider}：{name}（重试成功）"));
            true
        } else if second.cancelled() {
            self.stopped();
            false
        } else {
            self.log(
                LogLevel::Error,
                format!("{provider} 安装 {name} 失败：{}", second.diagnostic()),
            );
            false
        }
    }

    fn write_managed(&self, path: &Path, body: &str, label: &str) {
        match update_managed_block(path, MANAGED_START, MANAGED_END, body) {
            Ok(true) => self.log(LogLevel::Ok, format!("{label} 已更新：{}", path.display())),
            Ok(false) => self.log(
                LogLevel::Ok,
                format!("{label} 无需更改：{}", path.display()),
            ),
            Err(error) => self.log(
                LogLevel::Error,
                format!("{label} 写入失败 {}：{error}", path.display()),
            ),
        }
    }
}

fn user_home() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn app_data(home: &Path) -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("AppData").join("Roaming"))
}

fn item_progress(start: f32, end: f32, index: usize, total: usize) -> f32 {
    if total == 0 {
        end
    } else {
        start + (end - start) * ((index + 1) as f32 / total as f32)
    }
}

fn scoop_bucket_is_listed(output: &str, bucket: &str) -> bool {
    output
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .any(|name| name.eq_ignore_ascii_case(bucket))
}

fn package_is_listed(output: &str, name: &str) -> bool {
    let needle = name.to_ascii_lowercase();
    output.lines().any(|line| {
        let normalized = line
            .trim()
            .trim_start_matches(['├', '└', '─', '│', '+', '*', ' '])
            .to_ascii_lowercase();
        normalized.strip_prefix(&needle).is_some_and(|rest| {
            rest.is_empty()
                || rest
                    .chars()
                    .next()
                    .is_some_and(|character| character.is_whitespace() || "@:v".contains(character))
        })
    })
}

fn python_package_is_listed(output: &str, name: &str) -> bool {
    let normalized_name = name.replace('_', "-").to_ascii_lowercase();
    output.lines().any(|line| {
        line.split_once("==")
            .map(|(package, _)| package.replace('_', "-").to_ascii_lowercase())
            .is_some_and(|package| package == normalized_name)
    })
}

fn ps_escape(value: &str) -> String {
    value.replace('\'', "''")
}

fn panic_payload_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "未知 panic".to_string()
    }
}

fn extract_guid(text: &str) -> Option<String> {
    text.split(|character: char| !character.is_ascii_hexdigit() && character != '-')
        .find(|candidate| {
            candidate.len() == 36
                && candidate.chars().enumerate().all(|(index, character)| {
                    matches!(index, 8 | 13 | 18 | 23) == (character == '-')
                        && (character == '-' || character.is_ascii_hexdigit())
                })
        })
        .map(str::to_ascii_lowercase)
}

#[cfg(test)]
fn parse_disable_delete_notify(text: &str) -> Option<u32> {
    text.lines().find_map(|line| {
        if !line.to_ascii_lowercase().contains("disabledeletenotify") {
            return None;
        }
        line.split(|character: char| !character.is_ascii_digit())
            .rfind(|part| !part.is_empty())
            .and_then(|value| value.parse().ok())
    })
}

fn parse_trim_states(text: &str) -> Option<[(&'static str, u32); 2]> {
    let mut ntfs = None;
    let mut refs = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let state = value.split_whitespace().next()?.parse::<u32>().ok()?;
        if state > 1 {
            return None;
        }
        let slot = if key.trim().eq_ignore_ascii_case("NTFS DisableDeleteNotify") {
            &mut ntfs
        } else if key.trim().eq_ignore_ascii_case("ReFS DisableDeleteNotify") {
            &mut refs
        } else {
            continue;
        };
        if slot.replace(state).is_some() {
            return None;
        }
    }
    Some([("NTFS", ntfs?), ("ReFS", refs?)])
}

fn merge_json_file(path: &Path, managed: &serde_json::Value) -> anyhow::Result<()> {
    let mut existing = match crate::utils::read_config(path) {
        Ok(content) if content.trim().is_empty() => serde_json::json!({}),
        Ok(content) => serde_json::from_str(&content).map_err(|error| {
            anyhow::anyhow!(
                "{} 不是严格 JSON（可能含 JSONC 注释）；为避免破坏用户配置，已跳过：{}",
                path.display(),
                error
            )
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(error) => return Err(error.into()),
    };
    merge_json_value(&mut existing, managed);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::utils::write_preserving(path, serde_json::to_string_pretty(&existing)?.as_bytes())?;
    Ok(())
}

fn merge_json_value(target: &mut serde_json::Value, managed: &serde_json::Value) {
    match (target, managed) {
        (serde_json::Value::Object(target), serde_json::Value::Object(managed)) => {
            for (key, value) in managed {
                merge_json_value(target.entry(key).or_insert(serde_json::Value::Null), value);
            }
        }
        (target, managed) => *target = managed.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ScoopGitAction, SetupEvent, SetupOutcome, extract_guid, item_progress, merge_json_value,
        package_is_listed, parse_disable_delete_notify, python_package_is_listed, run_setup_worker,
        scoop_bucket_is_listed, scoop_git_action,
    };
    use crate::config::SetupConfig;
    use crate::utils::CancellationToken;

    #[test]
    fn package_list_match_is_exact_across_provider_formats() {
        let output = "git-filter-repo 2.47\ngit 2.53\nkimi-cli v1.49:\n├── @scope/tool@2.0";
        assert!(package_is_listed(output, "git"));
        assert!(package_is_listed(output, "kimi-cli"));
        assert!(package_is_listed(output, "@scope/tool"));
        assert!(!package_is_listed(output, "kimi"));
    }

    #[test]
    fn scoop_requires_main_bucket_on_fresh_install() {
        assert!(!scoop_bucket_is_listed("Name Source\n---- ------", "main"));
        assert!(scoop_bucket_is_listed(
            "Name Source\nmain https://github.com/ScoopInstaller/Main",
            "main"
        ));
    }

    #[test]
    fn scoop_uses_existing_git_without_winget_and_only_requires_winget_to_install_git() {
        assert_eq!(scoop_git_action(true, false), ScoopGitAction::UseExisting);
        assert_eq!(
            scoop_git_action(false, true),
            ScoopGitAction::InstallWithWinget
        );
        assert_eq!(
            scoop_git_action(false, false),
            ScoopGitAction::WingetRequired
        );
    }

    #[test]
    fn pip_names_compare_with_normalized_separators() {
        let output = "opencv-python==4.12\nPyWavelets==1.8";
        assert!(python_package_is_listed(output, "opencv_python"));
        assert!(python_package_is_listed(output, "pywavelets"));
        assert!(!python_package_is_listed(output, "opencv"));
    }

    #[test]
    fn progress_handles_empty_and_non_empty_ranges() {
        assert_eq!(item_progress(0.2, 0.4, 0, 0), 0.4);
        assert_eq!(item_progress(0.2, 0.4, 1, 2), 0.4);
    }

    #[test]
    fn extracts_power_plan_guid() {
        let output =
            "Power Scheme GUID: 01234567-89AB-CDEF-0123-456789ABCDEF  (Ultimate Performance)";
        assert_eq!(
            extract_guid(output).as_deref(),
            Some("01234567-89ab-cdef-0123-456789abcdef")
        );
    }

    #[test]
    fn parses_trim_delete_notification_state() {
        assert_eq!(
            parse_disable_delete_notify("NTFS DisableDeleteNotify = 0 (Disabled)"),
            Some(0)
        );
        assert_eq!(
            parse_disable_delete_notify("ReFS DisableDeleteNotify = 1"),
            Some(1)
        );
    }

    #[test]
    fn trim_capture_requires_both_explicit_states_without_duplicates() {
        assert_eq!(
            super::parse_trim_states("NTFS DisableDeleteNotify = 1\nReFS DisableDeleteNotify = 0"),
            Some([("NTFS", 1), ("ReFS", 0)])
        );
        for output in [
            "NTFS DisableDeleteNotify = 0",
            "NTFS DisableDeleteNotify = 0\nNTFS DisableDeleteNotify = 1\nReFS DisableDeleteNotify = 0",
            "NTFS DisableDeleteNotify = 0\nReFS DisableDeleteNotify = (Not Currently Set)",
        ] {
            assert!(super::parse_trim_states(output).is_none());
        }
    }

    #[test]
    fn json_merge_preserves_unmanaged_keys() {
        let mut current = serde_json::json!({
            "editor.fontSize": 16,
            "nested": {"personal": true, "managed": false}
        });
        let managed = serde_json::json!({
            "editor.formatOnSave": true,
            "nested": {"managed": true}
        });

        merge_json_value(&mut current, &managed);

        assert_eq!(current["editor.fontSize"], 16);
        assert_eq!(current["editor.formatOnSave"], true);
        assert_eq!(current["nested"]["personal"], true);
        assert_eq!(current["nested"]["managed"], true);
    }

    #[test]
    fn validation_failure_still_emits_finished_event() {
        let mut config = SetupConfig::default();
        config.profile.profile_version.clear();
        let (sender, receiver) = std::sync::mpsc::sync_channel(64);

        run_setup_worker(sender, config, CancellationToken::default());

        let events: Vec<_> = receiver.try_iter().collect();
        let outcome = events.into_iter().find_map(|event| match event {
            SetupEvent::Finished(summary) => Some(summary.outcome),
            _ => None,
        });
        assert_eq!(outcome, Some(SetupOutcome::CompletedWithErrors));
    }
}
