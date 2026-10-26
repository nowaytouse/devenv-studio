//! Native desktop workbench. Every mutation goes through the platform journal engine.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use eframe::egui::{self, Color32, RichText, Stroke, Vec2};
use image::ImageEncoder;
use studio_core::{Catalog, Hive, Target};
use studio_platform::{HostInfo, RecoveryPoint, RunReport};

use crate::prefs::{Accent, Appearance, Density, Language, Preferences};
use crate::workspace_io;
mod i18n;
use i18n::{lf, t, tx};
mod app_pages;
mod backup;
mod cargo_tools;
mod compatibility;
mod environment;
mod git_config;
mod hybrid;
mod mise_tools;
mod network;
mod npm_tools;
mod power;
mod python_tools;
mod rust_toolchains;
mod scoop_tools;
mod storage;
mod theme;
mod vscode_tools;

use theme::palette;

pub fn run() -> eframe::Result<()> {
    run_with_options(None)
}

/// Screenshot mode is an explicit development aid, never enabled in normal sessions.
pub fn run_with_options(screenshot: Option<PathBuf>) -> eframe::Result<()> {
    eframe::run_native(
        "DevEnv Studio",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1240.0, 860.0])
                .with_min_inner_size([980.0, 680.0]),
            ..Default::default()
        },
        Box::new(move |cc| {
            let app = Studio::new(screenshot);
            theme::apply(&cc.egui_ctx, &app.preferences);
            Ok(Box::new(app))
        }),
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Compatibility,
    Recipes,
    Backup,
    Environment,
    Network,
    Migration,
    Developer,
    Workbench,
    Power,
    Storage,
    Software,
    Features,
    Maintenance,
    Plan,
    Recovery,
    Sources,
    Status,
    Settings,
    Help,
    About,
}

#[derive(Clone, Copy)]
enum FileOperation {
    Export,
    Save,
    Load,
    SaveBackup,
    LoadBackup,
    ExportBrewfile,
    ExportWinget,
    SaveBundleReport,
    SaveRustRequest,
    LoadRustRequest,
    SaveCompatibilityRequest,
    LoadCompatibilityRequest,
    ExportRustToolchain,
    ExportMise,
    SaveRustReport,
    SavePythonRequest,
    LoadPythonRequest,
    SavePythonReport,
    SaveCargoRequest,
    LoadCargoRequest,
    SaveCargoReport,
    SaveVscodeRequest,
    LoadVscodeRequest,
    SaveVscodeReport,
    SaveVscodeSettingsProfile,
    LoadVscodeSettingsProfile,
    SaveMiseRuntimeRequest,
    LoadMiseRuntimeRequest,
    ExportMiseRuntimeManifest,
    ExportMiseRuntimeLock,
    SaveMiseRuntimeReport,
    NetworkSave,
    NetworkLoad,
    NetworkExportConfig,
    SaveGitProfile,
    LoadGitProfile,
    SaveGitReport,
}

impl FileOperation {
    fn title(self) -> &'static str {
        match self {
            Self::Export => t("导出变更方案"),
            Self::Save => t("保存配置档案"),
            Self::Load => t("载入配置档案"),
            Self::SaveBackup => tx("保存开发环境备份", "Save environment backup"),
            Self::LoadBackup => tx("载入开发环境备份", "Load environment backup"),
            Self::ExportBrewfile => tx("导出 Brewfile", "Export Brewfile"),
            Self::ExportWinget => tx("导出 WinGet 清单", "Export WinGet inventory"),
            Self::SaveBundleReport => tx("保存 Mac 配置结果", "Save Mac configuration result"),
            Self::SaveRustRequest => tx("保存 Rust 重部署请求", "Save Rust redeployment request"),
            Self::LoadRustRequest => tx("载入 Rust 重部署请求", "Load Rust redeployment request"),
            Self::SaveCompatibilityRequest => {
                tx("保存扩展部署请求", "Save extended deployment request")
            }
            Self::LoadCompatibilityRequest => {
                tx("载入扩展部署请求", "Load extended deployment request")
            }
            Self::ExportRustToolchain => {
                tx("导出 rust-toolchain.toml", "Export rust-toolchain.toml")
            }
            Self::ExportMise => tx("导出 mise.toml", "Export mise.toml"),
            Self::SaveRustReport => tx("保存 Rust 重部署结果", "Save Rust redeployment result"),
            Self::SavePythonRequest => tx("保存 Python 工具请求", "Save Python tools request"),
            Self::LoadPythonRequest => tx("载入 Python 工具请求", "Load Python tools request"),
            Self::SavePythonReport => tx("保存 Python 重部署结果", "Save Python deployment result"),
            Self::SaveCargoRequest => tx("保存 Cargo 工具请求", "Save Cargo tools request"),
            Self::LoadCargoRequest => tx("载入 Cargo 工具请求", "Load Cargo tools request"),
            Self::SaveCargoReport => tx("保存 Cargo 重部署结果", "Save Cargo deployment result"),
            Self::SaveVscodeRequest => {
                tx("保存 VS Code 扩展请求", "Save VS Code extensions request")
            }
            Self::LoadVscodeRequest => {
                tx("载入 VS Code 扩展请求", "Load VS Code extensions request")
            }
            Self::SaveVscodeReport => tx("保存 VS Code 扩展结果", "Save VS Code extensions result"),
            Self::SaveVscodeSettingsProfile => {
                tx("保存 VS Code 设置档案", "Save VS Code settings profile")
            }
            Self::LoadVscodeSettingsProfile => {
                tx("载入 VS Code 设置档案", "Load VS Code settings profile")
            }
            Self::SaveMiseRuntimeRequest => tx("保存 mise 运行时请求", "Save mise runtime request"),
            Self::LoadMiseRuntimeRequest => tx("载入 mise 运行时请求", "Load mise runtime request"),
            Self::ExportMiseRuntimeManifest => {
                tx("导出运行时 mise.toml", "Export runtime mise.toml")
            }
            Self::ExportMiseRuntimeLock => tx("导出运行时 mise.lock", "Export runtime mise.lock"),
            Self::SaveMiseRuntimeReport => tx("保存 mise 运行时结果", "Save mise runtime report"),
            Self::SaveGitProfile => tx("保存 Git 配置档案", "Save Git profile"),
            Self::LoadGitProfile => tx("载入 Git 配置档案", "Load Git profile"),
            Self::SaveGitReport => tx("保存 Git 配置结果", "Save Git configuration result"),
            Self::NetworkSave => tx("保存网络方案", "Save network profile"),
            Self::NetworkLoad => tx("载入网络方案", "Load network profile"),
            Self::NetworkExportConfig => {
                tx("导出工具网络配置", "Export tool network configuration")
            }
        }
    }
}

enum Dialog {
    File {
        operation: FileOperation,
        path: String,
        error: Option<String>,
    },
    Apply,
    Deploy,
    ApplyPower,
    RestorePower(PathBuf),
    Preset(String),
    Restore(PathBuf),
}

struct Worker {
    cancel: Arc<AtomicBool>,
    receiver: mpsc::Receiver<Result<WorkerResult, String>>,
    events: Option<mpsc::Receiver<studio_deployment::DeploymentEvent>>,
    thread: Option<JoinHandle<()>>,
    label: &'static str,
}

enum WorkerResult {
    Compatibility(studio_legacy::Report),
    Registry(RunReport),
    Power(RunReport),
    Deployment(studio_deployment::DeploymentReport),
    StorageStatus(Result<studio_deployment::StorageStatus, String>),
    Backup(studio_deployment::backup::Snapshot),
    Bundle(studio_deployment::environment::BundleReport),
    Rust(studio_deployment::toolchains::RustReport),
    RustInventory(Vec<String>),
    PythonTools(studio_deployment::python_tools::Report),
    PythonToolsInventory(Vec<studio_deployment::backup::Package>),
    CargoTools(studio_deployment::cargo_tools::Report),
    VSCodeTools(studio_deployment::vscode_tools::Report),
    VSCodeSettingsCapture(studio_deployment::vscode_settings::SettingsSnapshot),
    VSCodeSettingsApply(studio_deployment::vscode_settings::SettingsApplyOutcome),
    NpmTools(studio_deployment::npm_tools::Report),
    ScoopTools(studio_deployment::scoop_tools::Report),
    MiseRuntime(studio_deployment::mise_tools::Report),
    MiseRuntimeInventory(Vec<studio_deployment::mise_tools::InstalledRuntime>),
    GitConfigCapture(studio_deployment::git_config::Profile),
    GitConfigRefresh(studio_deployment::git_config::Profile),
    GitConfigApply(studio_deployment::git_config::ApplyReport),
    Inspection {
        settings: Vec<studio_platform::SettingState>,
        power: Vec<studio_platform::SettingState>,
        deployment: Vec<studio_deployment::Detection>,
        cancelled: bool,
    },
}

#[derive(Clone)]
struct Notice {
    text: String,
    error: bool,
}

struct Studio {
    compatibility: compatibility::UiState,
    hybrid: hybrid::HybridState,
    backup: backup::BackupUiState,
    network: network::NetworkUiState,
    power: power::PowerUiState,
    storage_status: Option<Result<studio_deployment::StorageStatus, String>>,
    preferences: Preferences,
    saved_preferences: Preferences,
    preferences_error: Option<String>,
    last_operation: Option<Notice>,
    history: Result<Vec<studio_deployment::HistoryRecord>, String>,
    catalog: Result<Catalog, String>,
    deployment_catalog: Result<studio_deployment::Catalog, String>,
    winget: studio_deployment::Capability,
    host: HostInfo,
    target: Target,
    selected: BTreeSet<String>,
    deployment_selected: BTreeSet<String>,
    power_selected: BTreeSet<String>,
    setting_states: BTreeMap<String, (String, String)>,
    deployment_states: BTreeMap<String, (String, String)>,
    last_event: Option<String>,
    page: Page,
    category: String,
    query: String,
    dialog: Option<Dialog>,
    notice: Option<Notice>,
    recovery: Vec<RecoveryPoint>,
    recovery_error: Option<String>,
    worker: Option<Worker>,
    screenshot: Option<PathBuf>,
    frames: usize,
}

impl Studio {
    fn new(screenshot: Option<PathBuf>) -> Self {
        let host = studio_platform::host();
        let target = host.target.unwrap_or(Target::Win11Ltsc2024);
        let (preferences, preferences_error) = match crate::prefs::load() {
            Ok(prefs) => (prefs, None),
            Err(error) => (Preferences::default(), Some(format!("{error:#}"))),
        };
        let mut app = Self {
            compatibility: compatibility::UiState::default(),
            hybrid: hybrid::HybridState::default(),
            backup: backup::BackupUiState::default(),
            network: network::NetworkUiState::default(),
            power: power::PowerUiState::default(),
            storage_status: None,
            saved_preferences: preferences.clone(),
            preferences,
            preferences_error,
            last_operation: None,
            history: studio_deployment::history().map_err(|error| format!("{error:#}")),
            catalog: studio_core::load_catalog().map_err(|e| format!("{e:#}")),
            deployment_catalog: studio_deployment::catalog().map_err(|e| format!("{e:#}")),
            winget: studio_deployment::winget_capability(),
            host,
            target,
            selected: BTreeSet::new(),
            deployment_selected: BTreeSet::new(),
            power_selected: BTreeSet::new(),
            setting_states: BTreeMap::new(),
            deployment_states: BTreeMap::new(),
            last_event: None,
            page: Page::Environment,
            category: String::new(),
            query: String::new(),
            dialog: None,
            notice: None,
            recovery: Vec::new(),
            recovery_error: None,
            worker: None,
            screenshot,
            frames: 0,
        };
        app.network = network::NetworkUiState::load();
        app.refresh_recovery();
        app
    }

    fn refresh_recovery(&mut self) {
        self.history = studio_deployment::history().map_err(|error| format!("{error:#}"));
        self.refresh_power_recovery();
        match studio_platform::recovery_points() {
            Ok(points) => {
                self.recovery = points;
                self.recovery_error = None;
            }
            Err(error) => self.recovery_error = Some(format!("{error:#}")),
        }
    }

    fn mutation_available(&self) -> bool {
        self.host.can_apply && self.host.target == Some(self.target) && self.worker.is_none()
    }

    fn open_file(&mut self, operation: FileOperation) {
        self.dialog = Some(Dialog::File {
            operation,
            path: String::new(),
            error: None,
        });
    }

    fn poll_worker(&mut self, ctx: &egui::Context) {
        if let Some(events) = self
            .worker
            .as_ref()
            .and_then(|worker| worker.events.as_ref())
        {
            for event in events.try_iter() {
                self.last_event = Some(lf!(
                    "{} / {} 项已完成 · {}：{}",
                    "{} / {} completed · {}: {}",
                    event.completed,
                    event.total,
                    event.task_id,
                    event.message
                ));
            }
        }
        let result = self
            .worker
            .as_ref()
            .and_then(|worker| match worker.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(t(
                    "后台任务异常结束，未收到执行报告。请查看恢复记录后再继续。",
                )
                .into())),
                Err(mpsc::TryRecvError::Empty) => None,
            });
        if let Some(mut result) = result {
            let cancelled = self
                .worker
                .as_ref()
                .is_some_and(|worker| worker.cancel.load(Ordering::Acquire));
            if let Some(mut worker) = self.worker.take()
                && let Some(thread) = worker.thread.take()
                && thread.join().is_err()
            {
                result = Err(t("后台任务发生异常；请检查已有恢复记录。").into());
            }
            if cancelled
                && matches!(
                    &result,
                    Ok(WorkerResult::Backup(_)
                        | WorkerResult::RustInventory(_)
                        | WorkerResult::PythonToolsInventory(_)
                        | WorkerResult::MiseRuntimeInventory(_)
                        | WorkerResult::GitConfigCapture(_)
                        | WorkerResult::GitConfigRefresh(_)
                        | WorkerResult::VSCodeSettingsCapture(_))
                )
            {
                result = Err(tx(
                    "开发环境采集已取消，保留上一次清单与保存路径。",
                    "Environment collection cancelled; the previous inventory and saved path were preserved.",
                ).into());
            }
            let quiet_inventory_success = matches!(
                &result,
                Ok(WorkerResult::RustInventory(_)
                    | WorkerResult::PythonToolsInventory(_)
                    | WorkerResult::MiseRuntimeInventory(_))
            );
            let notice = match result {
                Ok(WorkerResult::Registry(report) | WorkerResult::Power(report)) => Notice {
                    text: lf!(
                        "{}{}\n已变更 {} 项；原值已符合 {} 项。恢复日志：{}",
                        "{}{}\nChanged: {}; already matched: {}. Recovery journal: {}",
                        if report.cancelled {
                            t("任务已取消。")
                        } else {
                            ""
                        },
                        report.summary,
                        report.changed,
                        report.already_set,
                        report.journal.display()
                    ),
                    error: report.cancelled,
                },
                Ok(WorkerResult::Compatibility(report)) => {
                    let notice = Notice {
                        text: report.summary.clone(),
                        error: !report.succeeded,
                    };
                    self.compatibility.report = Some(report);
                    notice
                }
                Ok(WorkerResult::Backup(snapshot)) => self.accept_backup(snapshot),
                Ok(WorkerResult::Rust(report)) => {
                    let notice = Notice {
                        text: report.detail.clone(),
                        error: !report.verified,
                    };
                    self.backup.rust.report = Some(report);
                    notice
                }
                Ok(WorkerResult::RustInventory(installed)) => {
                    let count = installed.len();
                    self.backup.rust.installed = Some(installed);
                    Notice {
                        text: lf!(
                            "已读取 {count} 项 Rust 工具链；未安装或更新。",
                            "Read {count} Rust toolchains; no installation or update performed.",
                            count = count
                        ),
                        error: false,
                    }
                }
                Ok(WorkerResult::PythonTools(report)) => {
                    let notice = Notice {
                        text: report.detail.clone(),
                        error: !report.verified,
                    };
                    self.backup.python_tools.report = Some(report);
                    notice
                }
                Ok(WorkerResult::PythonToolsInventory(installed)) => {
                    let count = installed.len();
                    self.backup.python_tools.inventory = Some(installed);
                    self.backup.python_tools.confirmed = false;
                    Notice {
                        text: lf!(
                            "已读取 {count} 项 uv 工具；未安装或更新。",
                            "Read {count} uv tools; no installation or update performed.",
                            count = count
                        ),
                        error: false,
                    }
                }
                Ok(WorkerResult::CargoTools(report)) => {
                    let notice = Notice {
                        text: report.detail.clone(),
                        error: !report.verified,
                    };
                    self.backup.cargo_tools.report = Some(report);
                    notice
                }
                Ok(WorkerResult::VSCodeSettingsCapture(snapshot)) => {
                    let count = snapshot.settings.len();
                    let state = &mut self.backup.vscode_tools.settings;
                    if state.profile.is_none() {
                        let profile =
                            studio_deployment::vscode_settings::profile_from_snapshot(&snapshot);
                        state.selected.clear();
                        state.profile = Some(profile);
                    }
                    state.snapshot = Some(snapshot);
                    state.confirmed = false;
                    state.report = None;
                    Notice {
                        text: lf!(
                            "已读取 {count} 项 VS Code 白名单设置；未修改配置。",
                            "Read {count} allowlisted VS Code settings; configuration was not changed.",
                            count = count
                        ),
                        error: false,
                    }
                }
                Ok(WorkerResult::VSCodeSettingsApply(outcome)) => {
                    let report = outcome.report;
                    let notice = Notice {
                        text: report.detail.clone(),
                        error: !report.verified || !report.history_complete || report.cancelled,
                    };
                    if let Some(after) = outcome.after {
                        self.backup.vscode_tools.settings.snapshot = Some(after);
                    }
                    self.backup.vscode_tools.settings.report = Some(report);
                    self.backup.vscode_tools.settings.confirmed = false;
                    notice
                }
                Ok(WorkerResult::VSCodeTools(report)) => {
                    let notice = Notice {
                        text: report.detail.clone(),
                        error: !report.verified,
                    };
                    self.backup.vscode_tools.report = Some(report);
                    notice
                }
                Ok(WorkerResult::NpmTools(report)) => {
                    let notice = Notice {
                        text: report.detail.clone(),
                        error: !report.verified,
                    };
                    self.backup.npm_tools.report = Some(report);
                    notice
                }
                Ok(WorkerResult::ScoopTools(report)) => {
                    let notice = Notice {
                        text: report.detail.clone(),
                        error: !report.verified,
                    };
                    self.backup.scoop_tools.report = Some(report);
                    notice
                }
                Ok(WorkerResult::MiseRuntime(report)) => {
                    let notice = Notice {
                        text: report.detail.clone(),
                        error: !report.verified,
                    };
                    self.backup.mise_tools.report = Some(report);
                    notice
                }
                Ok(WorkerResult::MiseRuntimeInventory(installed)) => {
                    let count = installed.len();
                    self.backup.mise_tools.inventory = Some(installed);
                    self.backup.mise_tools.confirmed = false;
                    Notice {
                        text: lf!(
                            "已读取 {count} 项 mise 核心运行时；未安装或更新。",
                            "Read {count} mise core runtimes; no installation or update performed.",
                            count = count
                        ),
                        error: false,
                    }
                }
                Ok(WorkerResult::Bundle(report)) => {
                    let notice = Notice {
                        text: report.detail.clone(),
                        error: !report.verified,
                    };
                    self.backup.bundle_report = Some(report);
                    notice
                }
                Ok(WorkerResult::StorageStatus(result)) => {
                    let notice = match &result {
                        Ok(status) => Notice {
                            text: if status.preview {
                                tx(
                                    "存储状态为预览，未读取 Windows。",
                                    "Storage status is a preview; Windows was not inspected.",
                                )
                                .into()
                            } else {
                                tx("已读取删除通知策略；这不验证硬件 TRIM 效果。", "Delete-notification policy read; hardware TRIM operation was not verified.").into()
                            },
                            error: false,
                        },
                        Err(error) => Notice {
                            text: error.clone(),
                            error: true,
                        },
                    };
                    self.storage_status = Some(result);
                    notice
                }
                Ok(WorkerResult::Deployment(report)) => Notice {
                    text: lf!(
                        "{}\n完成 {} 项；失败 {} 项；{}{}执行历史：{}",
                        "{}\nCompleted: {}; failed: {}; {}{}History: {}",
                        report.summary,
                        report.completed,
                        report.failed,
                        if report.cancelled {
                            t("已取消。")
                        } else {
                            ""
                        },
                        if report.restart_required {
                            t("需要重启。")
                        } else {
                            ""
                        },
                        report.history.display()
                    ),
                    error: report.cancelled || report.failed > 0,
                },
                Ok(WorkerResult::GitConfigCapture(profile)) => {
                    let count = profile.settings.len();
                    self.backup.git_config.profile = Some(profile.clone());
                    self.backup.git_config.current = Some(profile);
                    self.backup.git_config.selected.clear();
                    self.backup.git_config.confirmed = false;
                    self.backup.git_config.report = None;
                    Notice {
                        text: lf!(
                            "已读取 {count} 项受支持的 Git 设置；未修改配置。",
                            "Read {count} supported Git settings; configuration was not changed.",
                            count = count
                        ),
                        error: false,
                    }
                }
                Ok(WorkerResult::GitConfigRefresh(profile)) => {
                    let count = profile.settings.len();
                    self.backup.git_config.current = Some(profile);
                    self.backup.git_config.confirmed = false;
                    self.backup.git_config.report = None;
                    Notice {
                        text: lf!(
                            "已刷新 {count} 项 Git 当前值；未修改配置。",
                            "Refreshed {count} current Git values; configuration was not changed.",
                            count = count
                        ),
                        error: false,
                    }
                }
                Ok(WorkerResult::GitConfigApply(report)) => {
                    let notice = Notice {
                        text: report.detail.clone(),
                        error: !report.verified || report.cancelled,
                    };
                    self.backup.git_config.report = Some(report);
                    self.backup.git_config.confirmed = false;
                    notice
                }
                Ok(WorkerResult::Inspection {
                    settings,
                    power,
                    deployment,
                    cancelled,
                }) => {
                    let count = settings.len() + deployment.len() + power.len();
                    for state in power {
                        self.power
                            .states
                            .insert(state.id, (state.state, state.detail));
                    }
                    for state in settings {
                        self.setting_states
                            .insert(state.id, (state.state, state.detail));
                    }
                    for state in deployment {
                        self.deployment_states
                            .insert(state.id, (state.state, state.detail));
                    }
                    Notice {
                        text: lf!(
                            "{}已返回 {count} 项检查状态。预览状态不代表读取了 Windows；检查不修改系统。",
                            "{}Returned {count} inspection states. Preview states do not inspect Windows; inspection makes no changes.",
                            if cancelled {
                                t("检查已取消，仅显示已返回结果。")
                            } else {
                                t("检查结束。")
                            },
                            count = count
                        ),
                        error: cancelled,
                    }
                }
                Err(error) => Notice {
                    text: error,
                    error: true,
                },
            };
            self.notice = (!quiet_inventory_success).then_some(notice);
            self.last_operation = self.notice.clone();
            self.refresh_recovery();
        }
        if self.worker.is_some() {
            ctx.request_repaint_after(Duration::from_millis(120));
        }
    }

    fn start_apply(&mut self) {
        if !self.mutation_available() {
            self.notice = Some(Notice {
                text: t("当前主机或目标不允许执行，请核对实际 Windows 版本。").into(),
                error: true,
            });
            return;
        }
        let plan = match self
            .catalog
            .as_ref()
            .map_err(String::as_str)
            .and_then(|catalog| {
                studio_core::build_plan(catalog, self.target, &self.selected)
                    .map_err(|_| t("方案校验失败，请重新检查所选项目。"))
            }) {
            Ok(plan) => plan,
            Err(error) => {
                self.notice = Some(Notice {
                    text: error.into(),
                    error: true,
                });
                return;
            }
        };
        self.start_worker("正在应用变更", move |cancel| {
            studio_platform::apply(plan, cancel).map(WorkerResult::Registry)
        });
        self.setting_states.clear();
    }

    fn start_restore(&mut self, path: PathBuf) {
        if !self.mutation_available() {
            self.notice = Some(Notice {
                text: t("当前主机不允许恢复 Windows 配置。").into(),
                error: true,
            });
            return;
        }
        self.start_worker("正在恢复配置", move |cancel| {
            studio_platform::restore(path, cancel).map(WorkerResult::Registry)
        });
        self.setting_states.clear();
    }

    fn network_summary(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(tx("开发网络方案…", "Developer network profile…"))
                .clicked()
            {
                self.page = Page::Network;
            }
            match &self.network.active {
                Ok(profile) => {
                    let mode = match profile.mode {
                        studio_deployment::network::ProxyMode::Inherit => {
                            tx("继承进程代理", "Inherit process proxy")
                        }
                        studio_deployment::network::ProxyMode::Direct => {
                            tx("HTTP 直连", "Direct HTTP")
                        }
                        studio_deployment::network::ProxyMode::Manual => profile.proxy_url.as_str(),
                    };
                    ui.label(lf!("本次部署将使用：{}", "Next deployment uses: {}", mode));
                }
                Err(_) => {
                    ui.colored_label(
                        palette().danger,
                        tx(
                            "网络方案不可用，部署将停止。",
                            "Network profile unavailable; deployment will stop.",
                        ),
                    );
                }
            }
        });
        ui.label(RichText::new(tx("仅影响本应用启动的受支持开发工具；系统代理、DNS、WinGet、DISM、VS Code 与 SSH 不在此范围。", "Applies only to supported developer tools launched by this app; system proxy, DNS, WinGet, DISM, VS Code and SSH are outside this scope.")).size(12.0).color(palette().muted));
    }

    fn start_network_worker<F>(&mut self, label: &'static str, operation: F)
    where
        F: FnOnce(Arc<AtomicBool>) -> anyhow::Result<WorkerResult> + Send + 'static,
    {
        let profile = match &self.network.active {
            Ok(profile) => profile.clone(),
            Err(error) => {
                self.notice = Some(Notice {
                    text: lf!(
                        "网络方案不可用：{error}",
                        "Network profile unavailable: {error}",
                        error = error
                    ),
                    error: true,
                });
                return;
            }
        };
        self.start_worker(label, move |cancel| {
            studio_deployment::network::with_profile(&profile, || operation(cancel))
        });
    }

    fn start_worker<F>(&mut self, label: &'static str, operation: F)
    where
        F: FnOnce(Arc<AtomicBool>) -> anyhow::Result<WorkerResult> + Send + 'static,
    {
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (sender, receiver) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let result = operation(worker_cancel).map_err(|error| format!("{error:#}"));
            // A closed UI receiver does not change the durable operation's result.
            let _ = sender.send(result);
        });
        self.worker = Some(Worker {
            cancel,
            receiver,
            events: None,
            thread: Some(thread),
            label,
        });
        self.notice = None;
        self.last_event = None;
    }

    fn deployment_limit_exceeded(&self) -> bool {
        self.deployment_selected.len() > studio_deployment::MAX_SESSION_TASKS
    }

    fn deployment_limit_message(&self) -> String {
        lf!(
            "已选 {} 项部署任务，单次检查或执行最多 {} 项。可保存或导出完整档案，执行前请拆分选择。",
            "{} deployment tasks selected; a session can inspect or execute at most {}. Save or export the complete profile, then split the selection before execution.",
            self.deployment_selected.len(),
            studio_deployment::MAX_SESSION_TASKS
        )
    }

    fn start_inspect(&mut self) {
        if self.deployment_limit_exceeded() {
            self.notice = Some(Notice {
                text: self.deployment_limit_message(),
                error: true,
            });
            return;
        }
        if self.worker.is_some() {
            return;
        }
        let plan = match self
            .catalog
            .as_ref()
            .map_err(|error| anyhow::anyhow!(error.clone()))
            .and_then(|catalog| {
                let settings = studio_core::build_plan(catalog, self.target, &self.selected)?;
                let power = studio_platform::power::build_plan(self.target, &self.power_selected)?;
                Ok((settings, power))
            }) {
            Ok(plan) => plan,
            Err(error) => {
                self.notice = Some(Notice {
                    text: format!("{error:#}"),
                    error: true,
                });
                return;
            }
        };
        let target = self.target;
        let selected = self.deployment_selected.clone();
        self.setting_states.clear();
        self.deployment_states.clear();
        self.power.states.clear();
        self.start_worker("正在只读检查所选项目", move |cancel| {
            let settings = studio_platform::inspect(plan.0, Arc::clone(&cancel))?;
            let power = if cancel.load(Ordering::Relaxed) {
                Vec::new()
            } else {
                studio_platform::power::inspect(plan.1, Arc::clone(&cancel))?
            };
            let deployment = if cancel.load(Ordering::Relaxed) {
                Vec::new()
            } else {
                studio_deployment::inspect(target, &selected, Arc::clone(&cancel))?
            };
            Ok(WorkerResult::Inspection {
                settings,
                power,
                deployment,
                cancelled: cancel.load(Ordering::Relaxed),
            })
        });
    }

    fn start_deploy(&mut self) {
        if self.deployment_limit_exceeded() {
            self.notice = Some(Notice {
                text: self.deployment_limit_message(),
                error: true,
            });
            return;
        }
        if !self.mutation_available() {
            return;
        }
        let plan = match studio_deployment::build_plan(self.target, &self.deployment_selected) {
            Ok(plan) => plan,
            Err(error) => {
                self.notice = Some(Notice {
                    text: format!("{error:#}"),
                    error: true,
                });
                return;
            }
        };
        let (sender, receiver) = mpsc::channel();
        self.start_network_worker("正在执行部署 / 功能 / 维护", move |cancel| {
            studio_deployment::execute(plan, cancel, Some(sender)).map(WorkerResult::Deployment)
        });
        if let Some(worker) = &mut self.worker {
            worker.events = Some(receiver);
        }
        self.deployment_states.clear();
    }

    fn navigate_to(&mut self, page: Page) {
        self.page = page;
        self.query.clear();
        self.hybrid.visible_tasks = 60;
    }

    fn rail(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("navigation")
            .exact_size(224.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(palette().sidebar)
                    .inner_margin(14.0)
                    .stroke(Stroke::new(1.0, palette().line)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    logo(ui, 29.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new("DevEnv Studio").size(17.0).strong());
                        ui.label(
                            RichText::new(format!(
                                "{} · {}",
                                tx("开发预览", "Development"),
                                env!("CARGO_PKG_VERSION")
                            ))
                            .size(11.0)
                            .color(palette().muted),
                        );
                    });
                });
                ui.add_space(18.0);
                egui::Panel::bottom("navigation-tools")
                    .resizable(false)
                    .frame(egui::Frame::new())
                    .show(ui, |ui| {
                        ui.separator();
                        ui.add_space(4.0);
                        let width = (ui.available_width() - 8.0) / 2.0;
                        egui::Grid::new("navigation-tool-grid")
                            .spacing([8.0, 5.0])
                            .show(ui, |ui| {
                                for (index, (page, title)) in [
                                    (Page::Settings, tx("设置", "Settings")),
                                    (Page::Help, tx("帮助", "Help")),
                                    (Page::About, tx("关于", "About")),
                                    (Page::Sources, tx("项目来源", "Sources")),
                                ]
                                .into_iter()
                                .enumerate()
                                {
                                    if navigation_button(ui, title, self.page == page, width)
                                        .clicked()
                                    {
                                        self.navigate_to(page);
                                    }
                                    if index % 2 == 1 {
                                        ui.end_row();
                                    }
                                }
                            });
                        if ui
                            .add_sized(
                                [ui.available_width(), 28.0],
                                egui::Button::new(
                                    RichText::new(tx("退出 DevEnv Studio", "Quit DevEnv Studio"))
                                        .size(12.0)
                                        .color(palette().muted),
                                )
                                .frame(false),
                            )
                            .clicked()
                        {
                            self.request_close(ui.ctx());
                        }
                    });
                egui::ScrollArea::vertical()
                    .id_salt("navigation-scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        for (group, pages) in [
                            (
                                tx("跨平台环境", "Environments"),
                                vec![
                                    (
                                        Page::Environment,
                                        tx("环境配置", "Environment configuration"),
                                    ),
                                    (Page::Backup, tx("环境备份", "Environment backup")),
                                    (Page::Network, tx("网络环境", "Network environment")),
                                ],
                            ),
                            (
                                tx("Windows 配置", "Windows configuration"),
                                vec![
                                    (Page::Recipes, tx("方案工作区", "Workspace recipes")),
                                    (Page::Developer, tx("开发工具", "Developer tools")),
                                    (Page::Migration, tx("迁移与映射", "Migration & mapping")),
                                    (Page::Compatibility, tx("扩展部署", "Extended deployment")),
                                    (Page::Workbench, t("配置工作台")),
                                    (Page::Power, tx("电源与节能", "Power & energy")),
                                    (Page::Storage, tx("存储与磁盘", "Storage & disks")),
                                    (Page::Software, t("软件部署")),
                                    (Page::Features, t("Windows 功能")),
                                    (Page::Maintenance, t("系统维护")),
                                ],
                            ),
                            (
                                tx("执行与记录", "Execution"),
                                vec![
                                    (Page::Plan, t("变更方案")),
                                    (Page::Recovery, t("恢复记录")),
                                    (Page::Status, tx("状态", "Status")),
                                ],
                            ),
                        ] {
                            ui.label(RichText::new(group).size(11.0).color(palette().muted));
                            ui.add_space(4.0);
                            for (page, title) in pages {
                                if navigation_button(
                                    ui,
                                    title,
                                    self.page == page,
                                    ui.available_width(),
                                )
                                .clicked()
                                {
                                    self.navigate_to(page);
                                }
                            }
                            ui.add_space(12.0);
                        }
                    });
            });
    }

    fn tray(&mut self, ui: &mut egui::Ui) {
        if matches!(
            self.page,
            Page::Compatibility
                | Page::Environment
                | Page::Backup
                | Page::Network
                | Page::Settings
                | Page::Help
                | Page::About
                | Page::Sources
                | Page::Status
        ) {
            return;
        }
        egui::Panel::bottom("selection-tray")
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(palette().surface)
                    .stroke(Stroke::new(1.0, palette().line))
                    .inner_margin(egui::Margin::symmetric(24, 12)),
            )
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(lf!(
                            "已选择 {} 项",
                            "{} items selected",
                            self.selection_count()
                        ))
                        .strong()
                        .size(15.0),
                    );
                    ui.label(
                        RichText::new(lf!(
                            "设置 {} / 电源 {} / 其他 {}",
                            "Settings {} / Power {} / Other {}",
                            self.selected.len(),
                            self.power_selected.len(),
                            self.deployment_selected.len()
                        ))
                        .size(12.0)
                        .color(palette().muted),
                    );
                });
                ui.horizontal_wrapped(|ui| {
                    if primary(
                        ui,
                        t("检查变更方案"),
                        self.selection_count() > 0 && self.worker.is_none(),
                    )
                    .clicked()
                    {
                        self.page = Page::Plan;
                    }
                    if ui
                        .add_enabled(
                            self.selection_count() > 0 && self.worker.is_none(),
                            egui::Button::new(t("清空选择")).frame(false),
                        )
                        .clicked()
                    {
                        self.clear_selection();
                    }
                });
            });
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(match self.page {
                    Page::Recipes => tx("方案工作区", "Workspace recipes"),
                    Page::Backup => tx("环境备份", "Environment backup"),
                    Page::Environment => tx("环境配置", "Environment configuration"),
                    Page::Network => tx("网络环境", "Network environment"),
                    Page::Migration => tx("迁移与映射", "Migration & mapping"),
                    Page::Compatibility => tx("扩展部署", "Extended deployment"),
                    Page::Developer => tx("开发环境", "Developer tools"),
                    Page::Workbench => t("配置工作台"),
                    Page::Power => tx("电源与节能", "Power & energy"),
                    Page::Storage => tx("存储与磁盘", "Storage & disks"),
                    Page::Software => t("软件部署"),
                    Page::Features => t("Windows 功能"),
                    Page::Maintenance => t("系统维护"),
                    Page::Plan => t("变更方案"),
                    Page::Recovery => t("恢复记录"),
                    Page::Sources => t("项目来源"),
                    Page::Status => t("状态"),
                    Page::Settings => t("设置"),
                    Page::Help => t("帮助"),
                    Page::About => t("关于"),
                })
                .size(14.0)
                .color(palette().muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                badge(
                    ui,
                    &format!(
                        "{} · {}",
                        if self.host.can_apply {
                            self.host.name.as_str()
                        } else {
                            preview_host_name()
                        },
                        if matches!(self.page, Page::Backup | Page::Environment | Page::Network)
                            && cfg!(any(target_os = "macos", windows))
                        {
                            tx("本机开发环境", "Local developer environment")
                        } else if self.host.can_apply {
                            t("本机执行")
                        } else {
                            t("交互预览")
                        }
                    ),
                    palette().wash,
                    palette().accent,
                );
                if self.page != Page::Help
                    && ui
                        .small_button(tx("❓ 帮助", "❓ Help"))
                        .on_hover_text(tx("打开分步使用指南", "Open the step-by-step guide"))
                        .clicked()
                {
                    self.page = Page::Help;
                }
            });
        });
        ui.add_space(14.0);
        if let Some(worker) = &self.worker {
            egui::Frame::new()
                .fill(palette().wash)
                .inner_margin(14.0)
                .corner_radius(8)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(t(worker.label));
                        ui.label(
                            RichText::new(
                                self.last_event
                                    .as_deref()
                                    .unwrap_or(t("等待当前操作返回，完成后显示真实结果")),
                            )
                            .size(12.0)
                            .color(palette().muted),
                        );
                        let cancelling = worker.cancel.load(Ordering::Relaxed);
                        if ui
                            .add_enabled(
                                !cancelling,
                                egui::Button::new(if cancelling {
                                    t("正在取消…")
                                } else {
                                    t("取消任务")
                                }),
                            )
                            .clicked()
                        {
                            worker.cancel.store(true, Ordering::Relaxed);
                        }
                    });
                });
            ui.add_space(12.0);
        }
        if self.preferences_error.is_some() {
            ui.colored_label(
                palette().danger,
                tx(
                    "界面设置文件存在错误；请前往设置查看。",
                    "Preferences file error; open Settings for details.",
                ),
            );
        }
        if matches!(self.preferences.language, Language::EnUs)
            && crate::catalog_locale::validate().is_err()
        {
            ui.colored_label(palette().danger, "Content translations unavailable. Original catalog text is shown; see Status for details.");
        }
        if let Some(notice) = &self.notice {
            let text = notice.text.clone();
            let error = notice.error;
            let mut dismiss = false;
            egui::Frame::new()
                .fill(palette().wash)
                .stroke(Stroke::new(
                    1.0,
                    if error {
                        palette().danger
                    } else {
                        palette().line
                    },
                ))
                .inner_margin(14.0)
                .corner_radius(8)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(text).size(13.0).color(if error {
                            palette().danger
                        } else {
                            palette().ink
                        }));
                        dismiss = ui.small_button(t("关闭提示")).clicked();
                    });
                });
            if dismiss {
                self.notice = None;
            }
            ui.add_space(12.0);
        }
    }

    fn target_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(t("方案目标")).strong());
            egui::ComboBox::from_id_salt("target-system")
                .selected_text(short_target(self.target))
                .width(230.0)
                .show_ui(ui, |ui| {
                    for target in Target::ALL {
                        if ui
                            .add_enabled(
                                self.worker.is_none(),
                                egui::Button::selectable(
                                    self.target == target,
                                    short_target(target),
                                ),
                            )
                            .clicked()
                        {
                            self.target = target;
                            self.setting_states.clear();
                            self.deployment_states.clear();
                            self.power.states.clear();
                        }
                    }
                });
            if ui
                .add_enabled(self.worker.is_none(), egui::Button::new(t("载入档案")))
                .clicked()
            {
                self.open_file(FileOperation::Load);
            }
            if ui
                .add_enabled(self.worker.is_none(), egui::Button::new(t("保存档案")))
                .clicked()
            {
                self.open_file(FileOperation::Save);
            }
            if ui
                .add_enabled(
                    self.worker.is_none() && self.selection_count() > 0,
                    egui::Button::new(t("检查所选项目状态")),
                )
                .clicked()
            {
                self.start_inspect();
            }
        });
        ui.label(
            RichText::new(
                if self.host.can_apply && self.host.target == Some(self.target) {
                    t("已识别本机 Windows；变更前仍需检查方案并确认。")
                } else {
                    preview_explanation()
                },
            )
            .size(12.0)
            .color(palette().muted),
        );
        ui.add_space(12.0);
    }

    fn selection_count(&self) -> usize {
        self.selected.len() + self.deployment_selected.len() + self.power_selected.len()
    }

    fn clear_selection(&mut self) {
        self.selected.clear();
        self.deployment_selected.clear();
        self.power_selected.clear();
    }

    fn request_close(&mut self, ctx: &egui::Context) {
        if let Some(worker) = &self.worker {
            worker.cancel.store(true, Ordering::Relaxed);
            self.notice = Some(Notice { text: tx("已请求取消。请等待当前操作返回并查看报告，然后关闭窗口。", "Cancellation requested. Wait for the current operation and review its report before closing.").into(), error: false });
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn workbench(&mut self, ui: &mut egui::Ui) {
        ui.heading(t("配置工作台"));
        ui.label(
            RichText::new(t("所有项目默认不选；互斥选项自动替换。"))
                .size(12.0)
                .color(palette().muted),
        );
        ui.add_space(10.0);
        self.target_controls(ui);
        let catalog = match &self.catalog {
            Ok(catalog) => catalog,
            Err(error) => {
                empty(ui, t("无法载入配置目录"), error);
                return;
            }
        };
        let display_catalog = localized_catalog(catalog);
        let catalog = &display_catalog;
        egui::CollapsingHeader::new(lf!("配置组合 · {} 个可选起点", "Presets · {} optional starting points", catalog.presets.len())).id_salt("presets").show(ui, |ui| {
            ui.label(RichText::new(t("选用组合会替换当前系统设置选择，软件 / 功能 / 维护选择保留。选用后仍可逐项调整。")).size(12.0).color(palette().muted));
            for preset in &catalog.presets {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&preset.title).strong());
                    ui.label(RichText::new(lf!("{} 项 · {}", "{} items · {}", preset.actions.len(), preset.description)).size(12.0).color(palette().muted));
                    if ui.add_enabled(self.worker.is_none(), egui::Button::new(t("检查并选用…")).small()).clicked() { self.dialog = Some(Dialog::Preset(preset.id.clone())); }
                });
            }
        });
        ui.add_space(8.0);
        let categories: BTreeSet<_> = catalog
            .actions
            .iter()
            .map(|action| action.category.as_str())
            .collect();
        let mut categories: Vec<_> = categories.into_iter().collect();
        categories.sort_by_key(|category| category_order(category));
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("setting-category")
                .selected_text(if self.category.is_empty() {
                    t("全部分类")
                } else {
                    category_label(&self.category)
                })
                .width(160.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.category, String::new(), t("全部分类"));
                    for category in &categories {
                        let count = catalog
                            .actions
                            .iter()
                            .filter(|action| action.category == *category)
                            .count();
                        ui.selectable_value(
                            &mut self.category,
                            (*category).into(),
                            format!("{} ({count})", category_label(category)),
                        );
                    }
                });
            ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text(t("搜索设置、用途或 ID…"))
                    .desired_width(330.0),
            );
            ui.label(
                RichText::new(lf!("{} 项设置", "{} settings", catalog.actions.len()))
                    .size(12.0)
                    .color(palette().muted),
            );
        });
        ui.add_space(8.0);
        let query = self.query.trim().to_lowercase();
        let mut changed: Option<(String, bool)> = None;
        let mut visible = 0;
        for category in &categories {
            if !self.category.is_empty() && self.category != *category {
                continue;
            }
            let actions: Vec<_> = catalog
                .actions
                .iter()
                .filter(|action| {
                    action.category == *category
                        && (query.is_empty()
                            || format!("{} {} {}", action.title, action.description, action.id)
                                .to_lowercase()
                                .contains(&query))
                })
                .collect();
            if actions.is_empty() {
                continue;
            }
            visible += actions.len();
            let selected_count = actions
                .iter()
                .filter(|action| self.selected.contains(&action.id))
                .count();
            let header = egui::CollapsingHeader::new(
                RichText::new(lf!(
                    "{}  {} 项 / 已选 {}",
                    "{}  {} items / {} selected",
                    category_label(category),
                    actions.len(),
                    selected_count
                ))
                .strong()
                .size(15.0),
            )
            .id_salt(("category", category))
            .default_open(*category == "privacy" || !self.category.is_empty());
            let header = if !query.is_empty() {
                header.open(Some(true))
            } else {
                header
            };
            header.show(ui, |ui| {
                egui::Frame::new()
                    .fill(palette().surface)
                    .stroke(Stroke::new(1.0, palette().line))
                    .corner_radius(8)
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        for (index, action) in actions.iter().enumerate() {
                            if index > 0 {
                                ui.separator();
                            }
                            let supported = action.targets.contains(&self.target);
                            let mut selected = self.selected.contains(&action.id);
                            ui.horizontal_wrapped(|ui| {
                                if ui
                                    .add_enabled(
                                        supported && self.worker.is_none(),
                                        egui::Checkbox::new(
                                            &mut selected,
                                            RichText::new(&action.title).strong(),
                                        ),
                                    )
                                    .changed()
                                {
                                    changed = Some((action.id.clone(), selected));
                                }
                                if !supported {
                                    ui.label(RichText::new(t("目标不适用")).size(11.0).color(palette().muted));
                                }
                                if let Some((state, detail)) = self.setting_states.get(&action.id) {
                                    state_label(ui, state, detail);
                                }
                            });
                            ui.label(RichText::new(&action.description).size(12.0).color(palette().muted));
                            egui::CollapsingHeader::new(
                                RichText::new(t("风险、来源与确切变更")).size(11.0).color(palette().accent),
                            )
                            .id_salt(("action-details", &action.id))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(lf!(
                                        "风险：{}；生效：{}；ID：{}", "Risk: {}; takes effect: {}; ID: {}",
                                        action.risk, action.restart, action.id
                                    ))
                                    .size(12.0),
                                );
                                if let Some(group) = &action.choice_group {
                                    ui.label(
                                        RichText::new(lf!(
                                            "互斥设置组：{group}。选择此项会移除组内其他已选项。", "Choice group: {group}. Selecting this item removes other selected alternatives."
                                        , group = group))
                                        .size(12.0)
                                        .color(palette().muted),
                                    );
                                }
                                registry_details(ui, action);
                                for (index, url) in action.evidence.iter().enumerate() {
                                    ui.hyperlink_to(
                                        RichText::new(lf!("固定版本原始配置 {}", "Pinned upstream evidence {}", index + 1))
                                            .size(12.0),
                                        url,
                                    );
                                }
                            });
                        }
                    });
            });
        }
        if visible == 0 {
            empty(ui, t("没有匹配的设置"), t("调整关键词或分类继续查找。"));
        }
        if let Some((id, value)) = changed {
            select_setting(catalog, &mut self.selected, &id, value);
        }
    }

    fn deployment_page(&mut self, ui: &mut egui::Ui) {
        let (category, title, description) = match self.page {
            Page::Software => (
                "software",
                t("软件部署"),
                t("精确的软件包 ID，由 WinGet 执行安装；安装器可能显示额外的许可与交互提示。"),
            ),
            Page::Developer => (
                "developer",
                tx("开发环境", "Developer tools"),
                tx(
                    "通过 Cargo、uv、Pip、NPM 与 VS Code 安装工具、Python 包及扩展，固定包标识并记录执行结果。",
                    "Install tools, Python packages and extensions with Cargo, uv, Pip, NPM and VS Code, using exact identities and recorded outcomes.",
                ),
            ),
            Page::Storage => (
                "storage",
                tx("存储与磁盘", "Storage & disks"),
                tx(
                    "先检查删除通知策略，再明确选择磁盘分析或优化。系统卷与所有卷分别选择，Windows 决定适当的介质优化方式。",
                    "Inspect delete-notification policy, then explicitly choose disk analysis or optimization. Choose the Windows volume or all volumes; Windows selects the appropriate media operation.",
                ),
            ),
            Page::Features => (
                "features",
                t("Windows 功能"),
                t("通过 Windows 自身的可选功能机制启用组件，部分功能需要源文件与重启。"),
            ),
            _ => (
                "maintenance",
                t("系统维护"),
                t(
                    "逐项选择 Windows 内置诊断与维护命令。维护可能耗时，取消需等待当前系统操作返回。",
                ),
            ),
        };
        ui.heading(title);
        ui.label(RichText::new(description).size(13.0).color(palette().muted));
        ui.label(
            RichText::new(t(
                "这些操作不属于注册表恢复范围。执行历史是操作证据，不提供通用回滚。",
            ))
            .size(12.0)
            .color(palette().danger),
        );
        ui.add_space(12.0);
        self.target_controls(ui);
        if self.page == Page::Developer {
            self.providers_panel(ui);
        }
        if self.page == Page::Storage {
            self.storage_panel(ui);
        }
        if self.page == Page::Software {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(if self.winget.available {
                        t("WinGet 已识别；源、权限与许可要求在执行时再次检查。")
                    } else if cfg!(windows) {
                        t("WinGet 尚不可用：需要微软应用安装程序及其执行别名。")
                    } else {
                        t("WinGet 仅在 Windows 执行；这里可选择并导出软件部署方案。")
                    })
                    .size(12.0)
                    .color(if self.winget.available {
                        palette().accent
                    } else {
                        palette().muted
                    }),
                )
                .on_hover_text(&self.winget.detail);
                if ui
                    .add_enabled(
                        self.worker.is_none(),
                        egui::Button::new(t("刷新检测")).small(),
                    )
                    .clicked()
                {
                    self.winget = studio_deployment::winget_capability();
                }
            });
            egui::CollapsingHeader::new(RichText::new(t("查看 WinGet 检测详情")).size(11.0))
                .id_salt("winget-details")
                .show(ui, |ui| {
                    ui.label(RichText::new(&self.winget.detail).size(12.0));
                });
        }
        if ui
            .add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text(t("搜索名称、软件包 ID 或功能名…"))
                    .desired_width(450.0),
            )
            .changed()
        {
            self.hybrid.visible_tasks = 60;
        }
        ui.add_space(8.0);
        let catalog = match &self.deployment_catalog {
            Ok(catalog) => catalog,
            Err(error) => {
                empty(ui, t("无法载入部署目录"), error);
                return;
            }
        };
        let display_catalog = studio_deployment::Catalog {
            revision: catalog.revision.clone(),
            tasks: catalog.tasks.iter().map(localized_task).collect(),
        };
        let catalog = &display_catalog;
        let query = self.query.trim().to_lowercase();
        let tasks: Vec<_> = catalog
            .tasks
            .iter()
            .filter(|task| {
                task.category == category
                    && (self.page != Page::Developer
                        || self.hybrid.developer_filter.is_empty()
                        || task.kind == self.hybrid.developer_filter)
                    && (query.is_empty()
                        || format!(
                            "{} {} {} {:?} {:?}",
                            task.title,
                            task.description,
                            task.id,
                            task.package_id,
                            task.feature_name
                        )
                        .to_lowercase()
                        .contains(&query))
            })
            .collect();
        ui.label(lf!(
            "匹配 {} 项；当前显示 {} 项",
            "{} matches; showing {}",
            tasks.len(),
            tasks.len().min(self.hybrid.visible_tasks)
        ));
        if tasks.is_empty() {
            empty(ui, t("没有匹配的项目"), t("调整关键词继续查找。"));
            return;
        }
        egui::Frame::new()
            .fill(palette().surface)
            .stroke(Stroke::new(1.0, palette().line))
            .corner_radius(8)
            .inner_margin(14.0)
            .show(ui, |ui| {
                for (index, task) in tasks.iter().take(self.hybrid.visible_tasks).enumerate() {
                    if index > 0 {
                        ui.separator();
                    }
                    let supported = task.targets.contains(&self.target);
                    let mut selected = self.deployment_selected.contains(&task.id);
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled(
                                supported && self.worker.is_none(),
                                egui::Checkbox::new(
                                    &mut selected,
                                    RichText::new(&task.title).strong(),
                                ),
                            )
                            .changed()
                        {
                            if selected {
                                self.deployment_selected.insert(task.id.clone());
                            } else {
                                self.deployment_selected.remove(&task.id);
                            }
                        }
                        if !supported {
                            ui.label(
                                RichText::new(t("目标不适用"))
                                    .size(11.0)
                                    .color(palette().muted),
                            );
                        }
                        if let Some((state, detail)) = self.deployment_states.get(&task.id) {
                            state_label(ui, state, detail);
                        }
                    });
                    ui.label(
                        RichText::new(&task.description)
                            .size(12.0)
                            .color(palette().muted),
                    );
                    ui.label(
                        RichText::new(task_identity(task))
                            .size(12.0)
                            .color(palette().accent),
                    );
                    egui::CollapsingHeader::new(
                        RichText::new(t("权限、限制与来源"))
                            .size(11.0)
                            .color(palette().accent),
                    )
                    .id_salt(("task-details", &task.id))
                    .show(ui, |ui| {
                        deployment_details(ui, task);
                    });
                }
            });
        if tasks.len() > self.hybrid.visible_tasks
            && ui.button(tx("再显示 60 项", "Show 60 more")).clicked()
        {
            self.hybrid.visible_tasks += 60;
        }
    }

    fn plan(&mut self, ui: &mut egui::Ui) {
        self.network_summary(ui);
        ui.heading(t("检查完整工作方案"));
        ui.label(
            RichText::new(lf!(
                "目标：{}。导出不会修改本机。",
                "Target: {}. Exporting makes no changes to this computer.",
                self.target.label()
            ))
            .color(palette().muted),
        );
        ui.add_space(12.0);
        if self.selection_count() == 0 {
            empty(
                ui,
                t("还没有选择项目"),
                tx(
                    "从系统设置、电源与节能、存储与磁盘、软件部署、Windows 功能或系统维护中逐项选择。",
                    "Choose individual settings, power values, storage operations, software, Windows features or maintenance tasks.",
                ),
            );
            return;
        }
        let catalog = match &self.catalog {
            Ok(catalog) => catalog,
            Err(error) => {
                empty(ui, t("设置目录不可用"), error);
                return;
            }
        };
        let settings_plan = studio_core::build_plan(catalog, self.target, &self.selected);
        let deployment_plan = studio_deployment::build_plan(self.target, &self.deployment_selected);
        let power_plan = studio_platform::power::build_plan(self.target, &self.power_selected);
        if let Err(error) = &settings_plan {
            ui.colored_label(
                palette().danger,
                lf!(
                    "系统设置校验失败：{error:#}",
                    "Settings validation failed: {error:#}",
                    error = error
                ),
            );
        }
        if let Err(error) = &deployment_plan {
            ui.colored_label(
                palette().danger,
                lf!(
                    "部署项目校验失败：{error:#}",
                    "Deployment validation failed: {error:#}",
                    error = error
                ),
            );
        }
        if let Err(error) = &power_plan {
            ui.colored_label(
                palette().danger,
                lf!(
                    "电源项目校验失败：{error:#}",
                    "Power validation failed: {error:#}",
                    error = error
                ),
            );
        }
        let valid = settings_plan.is_ok() && deployment_plan.is_ok() && power_plan.is_ok();
        if self.deployment_limit_exceeded() {
            ui.colored_label(palette().danger, self.deployment_limit_message());
        }
        ui.horizontal_wrapped(|ui| {
            if primary(ui, t("导出完整 JSON 方案"), valid && self.worker.is_none()).clicked()
            {
                self.open_file(FileOperation::Export);
            }
            if ui
                .add_enabled(
                    self.worker.is_none(),
                    egui::Button::new(t("检查所选项目状态")),
                )
                .clicked()
            {
                self.start_inspect();
            }
            if ui
                .add_enabled(
                    self.mutation_available() && settings_plan.is_ok() && !self.selected.is_empty(),
                    egui::Button::new(t("应用系统设置…")),
                )
                .clicked()
            {
                self.dialog = Some(Dialog::Apply);
            }
            if ui
                .add_enabled(
                    self.mutation_available()
                        && deployment_plan.is_ok()
                        && !self.deployment_limit_exceeded()
                        && !self.deployment_selected.is_empty(),
                    egui::Button::new(t("执行部署 / 功能 / 维护…")),
                )
                .clicked()
            {
                self.dialog = Some(Dialog::Deploy);
            }
            if ui
                .add_enabled(
                    self.mutation_available()
                        && power_plan.is_ok()
                        && !self.power_selected.is_empty(),
                    egui::Button::new(tx("应用电源参数…", "Apply power values…")),
                )
                .clicked()
            {
                self.dialog = Some(Dialog::ApplyPower);
            }
        });
        ui.add_space(14.0);
        egui::CollapsingHeader::new(lf!(
            "系统设置 · {} 项 · 原值留档、按日志恢复",
            "System settings · {} items · Original values journaled for recovery",
            self.selected.len()
        ))
        .default_open(true)
        .id_salt("plan-settings")
        .show(ui, |ui| {
            if let Ok(settings) = &settings_plan {
                for action in &settings.actions {
                    let action = localized_action(action);
                    ui.label(
                        RichText::new(crate::catalog_locale::text(
                            i18n::language(),
                            "actions",
                            &action.id,
                            "title",
                            &action.title,
                        ))
                        .strong(),
                    );
                    registry_details(ui, &action);
                    ui.label(
                        RichText::new(lf!(
                            "风险：{}；生效：{}",
                            "Risk: {}; takes effect: {}",
                            action.risk,
                            action.restart
                        ))
                        .size(12.0)
                        .color(palette().muted),
                    );
                    ui.separator();
                }
            }
        });
        egui::CollapsingHeader::new(lf!(
            "部署 / 功能 / 维护 · {} 项 · 不属于注册表恢复范围",
            "Deployment / features / maintenance · {} items · Outside registry recovery",
            self.deployment_selected.len()
        ))
        .default_open(true)
        .id_salt("plan-deployment")
        .show(ui, |ui| {
            if let Ok(deployment) = &deployment_plan {
                for task in &deployment.tasks {
                    let task = localized_task(task);
                    ui.label(
                        RichText::new(crate::catalog_locale::text(
                            i18n::language(),
                            "tasks",
                            &task.id,
                            "title",
                            &task.title,
                        ))
                        .strong(),
                    );
                    deployment_details(ui, &task);
                    ui.separator();
                }
            }
        });
        egui::CollapsingHeader::new(lf!(
            "电源与节能 · {} 项 · 独立原值日志",
            "Power & energy · {} items · Separate original-value journal",
            self.power_selected.len()
        ))
        .default_open(true)
        .id_salt("plan-power")
        .show(ui, |ui| {
            if let Ok(plan) = &power_plan {
                power::plan_preview(ui, plan);
            }
        });
        ui.add_space(12.0);
        ui.label(RichText::new(tx("系统设置、电源参数与部署任务分别确认。注册表与电源各有独立恢复日志；软件安装、功能启用及维护没有通用回滚。不会自动接受外部安装器的许可协议。", "Settings, power values and deployment tasks are confirmed separately. Registry and power have separate recovery journals; software, features and maintenance have no general rollback. External installer licenses are not automatically accepted.")).size(12.0).color(palette().muted));
    }

    fn recovery(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading(t("恢复记录"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(self.worker.is_none(), egui::Button::new(t("刷新记录")))
                    .clicked()
                {
                    self.refresh_recovery();
                }
            });
        });
        ui.label(
            RichText::new(t(
                "这里只显示实际生成的恢复日志。恢复会检查原值与当前值的冲突。",
            ))
            .color(palette().muted),
        );
        if let Ok(directory) = studio_platform::data_directory() {
            ui.label(
                RichText::new(lf!(
                    "日志目录：{}",
                    "Journal directory: {}",
                    directory.display()
                ))
                .size(12.0)
                .color(palette().muted),
            );
        }
        ui.add_space(24.0);
        self.history_view(ui);
        ui.add_space(14.0);
        ui.label(
            RichText::new(tx("注册表恢复记录", "Registry recovery journals"))
                .size(18.0)
                .strong(),
        );
        if let Some(error) = &self.recovery_error {
            empty(ui, t("无法读取恢复记录"), error);
            return;
        }
        if self.recovery.is_empty() {
            empty(
                ui,
                t("还没有恢复记录"),
                t("在受支持的 Windows 主机应用配置后，真实的变更日志会出现在这里。"),
            );
            return;
        }
        let enabled = self.mutation_available();
        for point in &self.recovery {
            egui::Frame::new()
                .fill(palette().surface)
                .stroke(Stroke::new(1.0, palette().line))
                .inner_margin(20.0)
                .corner_radius(8)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&point.id).strong());
                        badge(
                            ui,
                            app_pages::history_status(&point.status),
                            palette().wash,
                            palette().accent,
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add_enabled(enabled, egui::Button::new(t("检查并恢复…")))
                                .clicked()
                            {
                                self.dialog = Some(Dialog::Restore(point.path.clone()));
                            }
                        });
                    });
                    ui.label(
                        RichText::new(lf!(
                            "{} / {} 项配置",
                            "{} / {} settings",
                            point.started_at,
                            point.actions
                        ))
                        .color(palette().muted),
                    );
                    ui.label(
                        RichText::new(point.path.display().to_string())
                            .size(12.0)
                            .color(palette().muted),
                    );
                });
            ui.add_space(12.0);
        }
    }

    fn sources(&self, ui: &mut egui::Ui) {
        ui.heading(t("项目来源"));
        ui.label(
            RichText::new(t(
                "配置由 Rust 目录逐项映射与校验；不会下载或执行上游脚本。",
            ))
            .color(palette().muted),
        );
        ui.add_space(24.0);
        let catalog = match &self.catalog {
            Ok(catalog) => catalog,
            Err(error) => {
                empty(ui, t("目录不可用"), error);
                return;
            }
        };
        for source in &catalog.sources {
            egui::Frame::new()
                .fill(palette().surface)
                .stroke(Stroke::new(1.0, palette().line))
                .inner_margin(22.0)
                .corner_radius(8)
                .show(ui, |ui| {
                    ui.hyperlink_to(RichText::new(&source.name).strong().size(18.0), &source.url);
                    ui.label(
                        RichText::new(if source.name == "LTSC_Tools_Backup" { tx("旧版部署逻辑已合并到扩展部署；恢复日志与未审查资源保留独立边界。", "The retained deployment engine is integrated into Extended deployment, with separate boundaries for old journals and unaudited resources.") } else { t(&source.description) })
                            .size(14.0)
                            .color(palette().muted),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(lf!("固定版本：{}", "Pinned revision: {}", source.revision))
                            .size(12.0),
                    );
                    ui.label(
                        RichText::new(lf!(
                            "许可证 / 使用范围：{}",
                            "License / scope: {}",
                            if source.name == "LTSC_Tools_Backup" { tx("所有者授权的 Rust 后端整合；旧项目退役", "Owner-authorized Rust engine integration; predecessor retired") } else { t(&source.license) }
                        ))
                        .size(12.0)
                        .color(palette().muted),
                    );
                });
            ui.add_space(14.0);
        }
        ui.label(
            RichText::new(lf!(
                "配置目录版本：{}",
                "Settings catalog revision: {}",
                catalog.revision
            ))
            .size(12.0)
            .color(palette().muted),
        );
        if let Ok(deployment) = &self.deployment_catalog {
            ui.label(
                RichText::new(lf!(
                    "部署目录版本：{} / {} 项任务",
                    "Deployment catalog revision: {} / {} tasks",
                    deployment.revision,
                    deployment.tasks.len()
                ))
                .size(12.0)
                .color(palette().muted),
            );
        }
        ui.add_space(12.0);
        ui.label(
            RichText::new(
                t("系统设置映射固定版本来源；部署项目逐项显示软件包、功能或内置命令及限制。软件本身遵循各自许可证，安装器许可需由你决定。"),
            )
            .size(13.0)
            .color(palette().muted),
        );
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.take() else {
            return;
        };
        let mut close = false;
        let mut apply = false;
        let mut deploy = false;
        let mut power_apply = false;
        let mut power_restore = None;
        let mut preset = None;
        let mut restore = None;
        let response = egui::Modal::new(egui::Id::unique("studio-dialog"))
            .frame(egui::Frame::new().fill(palette().surface).stroke(Stroke::new(1.0, palette().line)).corner_radius(14).inner_margin(26.0))
            .show(ctx, |ui| {
                ui.set_width(520.0);
                match &mut dialog {
                    Dialog::File { operation, path, error } => {
                        ui.heading(operation.title());
                        ui.label(RichText::new(if matches!(operation, FileOperation::LoadBackup) { tx("输入开发环境 JSON 备份完整路径。载入不会改变 Windows 方案选择。", "Enter the full environment backup JSON path. Loading does not change Windows selections.") } else if matches!(operation, FileOperation::LoadRustRequest | FileOperation::LoadCompatibilityRequest | FileOperation::LoadVscodeRequest | FileOperation::LoadVscodeSettingsProfile | FileOperation::NetworkLoad | FileOperation::LoadGitProfile) { tx("输入现有 JSON 请求或网络档案的完整路径。载入后仍需审核确认，不会自动执行或启用。", "Enter an existing JSON request or network profile path. Loading still requires review and confirmation; it does not execute or activate automatically.") } else if matches!(operation, FileOperation::Load) { t("输入现有 JSON 档案的完整路径。载入会替换当前选择。") } else { t("输入新文件的完整路径。为保护现有文件，已存在的路径不会被覆盖。") }).color(palette().muted));
                        ui.add_space(12.0);
                        ui.add(egui::TextEdit::singleline(path).desired_width(f32::INFINITY).hint_text(t("完整文件路径，例如 /Users/you/Desktop/lt-sc.json")));
                        if let Some(error) = error.as_ref() { ui.colored_label(palette().danger, error); }
                        ui.add_space(14.0);
                        ui.horizontal(|ui| {
                            if primary(ui, operation.title(), !path.trim().is_empty()).clicked() {
                                match self.file_operation(*operation, PathBuf::from(path.trim())) {
                                    Ok(message) => { self.notice = Some(Notice { text: message, error: false }); close = true; }
                                    Err(message) => *error = Some(message),
                                }
                            }
                            if ui.button(t("取消")).clicked() { close = true; }
                        });
                    }
                    Dialog::Apply => {
                        ui.heading(t("确认在这台 Windows 上应用"));
                        ui.label(lf!("实际主机：{}", "Actual host: {}", self.host.name));
                        ui.label(lf!("方案目标：{}", "Plan target: {}", self.target.label()));
                        ui.label(lf!("共 {} 项配置，以下是将写入的确切值：", "{} settings. These are the exact values to write:", self.selected.len()));
                        egui::ScrollArea::vertical().id_salt("confirmation-plan").max_height(230.0).show(ui, |ui| {
                            if let Ok(catalog) = &self.catalog {
                                for action in catalog.actions.iter().filter(|action| self.selected.contains(&action.id)) {
                                    ui.label(RichText::new(crate::catalog_locale::text(i18n::language(), "actions", &action.id, "title", &action.title)).strong());
                                    for change in &action.changes {
                                        let hive = match change.hive { Hive::CurrentUser => "HKCU", Hive::LocalMachine => "HKLM" };
                                        ui.label(RichText::new(format!("{hive}\\{}\\{} = {} (DWORD)", change.path, change.name, change.value)).size(12.0));
                                    }
                                    ui.separator();
                                }
                            }
                        });
                        ui.add_space(10.0);
                        ui.label(RichText::new(t("每项修改前保存原始值，修改后核对。失败或取消可能留下部分已完成的变更，请按真实报告与恢复日志处理。")).color(palette().muted));
                        ui.add_space(16.0);
                        ui.horizontal(|ui| {
                            if primary(ui, t("确认应用所选配置"), self.mutation_available() && !self.selected.is_empty()).clicked() { apply = true; close = true; }
                            if ui.button(t("返回检查")).clicked() { close = true; }
                        });
                    }
                    Dialog::Deploy => {
                        ui.heading(t("确认执行部署 / 功能 / 维护"));
                        ui.label(lf!("实际主机：{}", "Actual host: {}", self.host.name));
                        ui.label(lf!("方案目标：{}", "Plan target: {}", self.target.label()));
                        let plan = studio_deployment::build_plan(self.target, &self.deployment_selected);
                        let valid = plan.is_ok() && !self.deployment_limit_exceeded();
                        if self.deployment_limit_exceeded() { ui.colored_label(palette().danger, self.deployment_limit_message()); }
                        egui::ScrollArea::vertical().id_salt("confirm-deployment").max_height(260.0).show(ui, |ui| {
                            match &plan {
                                Ok(plan) => for task in &plan.tasks { ui.label(RichText::new(crate::catalog_locale::text(i18n::language(), "tasks", &task.id, "title", &task.title)).strong()); deployment_details(ui, task); ui.separator(); },
                                Err(error) => { ui.colored_label(palette().danger, format!("{error:#}")); }
                            }
                        });
                        ui.label(RichText::new(t("这些操作不能由注册表恢复日志撤销。可能需要管理员权限、网络、系统源文件或重启；取消只能在系统允许的边界生效，已经完成的操作仍会保留。")).color(palette().danger));
                        ui.label(RichText::new(t("不会自动接受外部安装器的许可协议。软件安装器可能显示交互提示，请自行检查并决定。")).size(12.0).color(palette().muted));
                        ui.horizontal(|ui| {
                            if primary(ui, t("确认执行以上任务"), self.mutation_available() && valid && !self.deployment_selected.is_empty()).clicked() { deploy = true; close = true; }
                            if ui.button(t("返回检查")).clicked() { close = true; }
                        });
                    }
                    Dialog::ApplyPower => {
                        if self.power_confirmation(ui) { power_apply = true; close = true; }
                        if ui.button(t("返回检查")).clicked() { close = true; }
                    }
                    Dialog::RestorePower(path) => {
                        ui.heading(tx("确认恢复这条电源记录", "Confirm this power recovery"));
                        ui.label(lf!("实际主机：{}", "Actual host: {}", self.host.name));
                        ui.label(path.display().to_string());
                        ui.label(tx("只恢复这条日志的电源方案与 AC 原值。执行前检查当前活动方案与参数冲突，遇到外部变更停止；不会切换电源方案或修改 DC。", "Restore only this journal's scheme and original AC values. The active scheme and parameter conflicts are checked before execution; external changes stop recovery. No scheme is switched and DC values are untouched."));
                        if primary(ui, tx("确认检查并恢复电源", "Review and restore power values"), self.mutation_available()).clicked() { power_restore = Some(path.clone()); close = true; }
                        if ui.button(t("取消")).clicked() { close = true; }
                    }
                    Dialog::Preset(id) => {
                        ui.heading(t("选用配置组合"));
                        if let Ok(catalog) = &self.catalog && let Some(item) = catalog.presets.iter().find(|item| &item.id == id) {
                            ui.label(RichText::new(crate::catalog_locale::text(i18n::language(), "presets", &item.id, "title", &item.title)).strong());
                            ui.label(crate::catalog_locale::text(i18n::language(), "presets", &item.id, "description", &item.description));
                            let valid = studio_core::build_plan(catalog, self.target, &item.actions);
                            egui::ScrollArea::vertical().id_salt("preset-review").max_height(230.0).show(ui, |ui| {
                                for action in catalog.actions.iter().filter(|action| item.actions.contains(&action.id)) { ui.label(crate::catalog_locale::text(i18n::language(), "actions", &action.id, "title", &action.title)); }
                            });
                            ui.label(RichText::new(lf!("将以这 {} 项替换当前 {} 项系统设置选择；部署、功能及维护选择保留。不会立即修改系统。", "Replace {} items in this preset for the current {} settings selections. Deployment, feature and maintenance selections remain. No system changes are made now.", item.actions.len(), self.selected.len())).color(palette().muted));
                            if let Err(error) = &valid { ui.colored_label(palette().danger, lf!("此组合不适合当前目标：{error:#}", "This preset does not fit the selected target: {error:#}", error = error)); }
                            if primary(ui, t("确认替换系统设置选择"), valid.is_ok() && self.worker.is_none()).clicked() { preset = Some(item.actions.clone()); close = true; }
                        } else { ui.colored_label(palette().danger, t("配置组合已不可用。")); }
                        if ui.button(t("取消")).clicked() { close = true; }
                    }
                    Dialog::Restore(path) => {
                        ui.heading(t("确认恢复这条记录"));
                        ui.label(lf!("实际主机：{}", "Actual host: {}", self.host.name));
                        ui.label(path.display().to_string());
                        ui.label(RichText::new(t("恢复只处理这条日志记录的注册表值。若当前值在执行后被其他程序修改，冲突检查会阻止覆盖。")).color(palette().muted));
                        ui.add_space(16.0);
                        ui.horizontal(|ui| {
                            if primary(ui, t("确认检查并恢复"), self.mutation_available()).clicked() { restore = Some(path.clone()); close = true; }
                            if ui.button(t("取消")).clicked() { close = true; }
                        });
                    }
                }
            });
        if !close && !response.should_close() {
            self.dialog = Some(dialog);
        }
        if apply {
            self.start_apply();
        }
        if deploy {
            self.start_deploy();
        }
        if power_apply {
            self.start_power_apply();
        }
        if let Some(path) = power_restore {
            self.start_power_restore(path);
        }
        if let Some(selection) = preset {
            self.selected = selection;
        }
        if let Some(path) = restore {
            self.start_restore(path);
        }
    }

    fn file_operation(
        &mut self,
        operation: FileOperation,
        path: PathBuf,
    ) -> Result<String, String> {
        if !path.is_absolute() {
            return Err(t("请输入完整的绝对路径。").into());
        }
        if matches!(
            operation,
            FileOperation::SaveBackup
                | FileOperation::LoadBackup
                | FileOperation::ExportBrewfile
                | FileOperation::ExportWinget
                | FileOperation::SaveBundleReport
        ) {
            return self.backup_file_operation(operation, path);
        }
        if matches!(
            operation,
            FileOperation::SaveRustRequest
                | FileOperation::LoadRustRequest
                | FileOperation::ExportRustToolchain
                | FileOperation::ExportMise
                | FileOperation::SaveRustReport
        ) {
            return self.rust_file_operation(operation, &path);
        }
        if matches!(
            operation,
            FileOperation::SaveCargoRequest
                | FileOperation::LoadCargoRequest
                | FileOperation::SaveCargoReport
        ) {
            return self.cargo_tools_file_operation(operation, &path);
        }
        if matches!(
            operation,
            FileOperation::SaveVscodeRequest
                | FileOperation::LoadVscodeRequest
                | FileOperation::SaveVscodeReport
                | FileOperation::SaveVscodeSettingsProfile
                | FileOperation::LoadVscodeSettingsProfile
        ) {
            return self.vscode_tools_file_operation(operation, &path);
        }
        if matches!(
            operation,
            FileOperation::SavePythonRequest
                | FileOperation::LoadPythonRequest
                | FileOperation::SavePythonReport
        ) {
            return self.python_tools_file_operation(operation, &path);
        }
        if matches!(
            operation,
            FileOperation::SaveMiseRuntimeRequest
                | FileOperation::LoadMiseRuntimeRequest
                | FileOperation::ExportMiseRuntimeManifest
                | FileOperation::ExportMiseRuntimeLock
                | FileOperation::SaveMiseRuntimeReport
        ) {
            return self.mise_file_operation(operation, &path);
        }
        if matches!(
            operation,
            FileOperation::NetworkSave
                | FileOperation::NetworkLoad
                | FileOperation::NetworkExportConfig
        ) {
            return self.network_file_operation(operation, &path);
        }
        if matches!(
            operation,
            FileOperation::SaveGitProfile
                | FileOperation::LoadGitProfile
                | FileOperation::SaveGitReport
        ) {
            return self.git_config_file_operation(operation, &path);
        }
        if matches!(
            operation,
            FileOperation::SaveCompatibilityRequest | FileOperation::LoadCompatibilityRequest
        ) {
            if self.worker.is_some() {
                return Err(tx("请等待当前任务结束后再保存或载入扩展部署请求。", "Wait for the current task before saving or loading an extended deployment request.").into());
            }
            self.compatibility_file_operation(operation, &path)
                .map_err(|error| format!("{error:#}"))?;
            return Ok(lf!(
                "扩展部署请求已处理：{}",
                "Extended deployment request processed: {}",
                path.display()
            ));
        }
        let catalog = self.catalog.as_ref().map_err(Clone::clone)?;
        match operation {
            FileOperation::SaveCompatibilityRequest | FileOperation::LoadCompatibilityRequest => {
                Err("Extended deployment request was not dispatched".into())
            }
            FileOperation::NetworkSave
            | FileOperation::NetworkLoad
            | FileOperation::NetworkExportConfig => {
                Err("Network operation was not dispatched".into())
            }
            FileOperation::SaveGitProfile
            | FileOperation::LoadGitProfile
            | FileOperation::SaveGitReport => {
                Err("Git configuration operation was not dispatched".into())
            }
            FileOperation::SaveRustRequest
            | FileOperation::LoadRustRequest
            | FileOperation::ExportRustToolchain
            | FileOperation::ExportMise
            | FileOperation::SaveRustReport => Err("Rust operation was not dispatched".into()),
            FileOperation::SavePythonRequest
            | FileOperation::LoadPythonRequest
            | FileOperation::SavePythonReport => {
                Err("Python tools operation was not dispatched".into())
            }
            FileOperation::SaveCargoRequest
            | FileOperation::LoadCargoRequest
            | FileOperation::SaveCargoReport => {
                Err("Cargo tools operation was not dispatched".into())
            }
            FileOperation::SaveVscodeRequest
            | FileOperation::LoadVscodeRequest
            | FileOperation::SaveVscodeReport
            | FileOperation::SaveVscodeSettingsProfile
            | FileOperation::LoadVscodeSettingsProfile => {
                Err("VS Code extensions operation was not dispatched".into())
            }
            FileOperation::SaveMiseRuntimeRequest
            | FileOperation::LoadMiseRuntimeRequest
            | FileOperation::ExportMiseRuntimeManifest
            | FileOperation::ExportMiseRuntimeLock
            | FileOperation::SaveMiseRuntimeReport => {
                Err("mise runtime operation was not dispatched".into())
            }
            FileOperation::SaveBackup
            | FileOperation::LoadBackup
            | FileOperation::ExportBrewfile
            | FileOperation::ExportWinget
            | FileOperation::SaveBundleReport => Err("Backup operation was not dispatched".into()),
            FileOperation::Export => {
                let plan = workspace_io::plan(
                    catalog,
                    self.target,
                    &self.selected,
                    &self.deployment_selected,
                    &self.power_selected,
                )
                .map_err(|e| format!("{e:#}"))?;
                studio_core::export_document(&plan, &path).map_err(|e| format!("{e:#}"))?;
                Ok(lf!("方案已导出：{}", "Plan exported: {}", path.display()))
            }
            FileOperation::Save => {
                workspace_io::save(
                    catalog,
                    self.target,
                    &self.selected,
                    &self.deployment_selected,
                    &self.power_selected,
                    &path,
                )
                .map_err(|e| format!("{e:#}"))?;
                Ok(lf!(
                    "配置档案已保存：{}",
                    "Profile saved: {}",
                    path.display()
                ))
            }
            FileOperation::Load => {
                let profile = workspace_io::load(&path, catalog).map_err(|e| format!("{e:#}"))?;
                self.target = profile.settings.target;
                self.selected = profile.settings.selected;
                self.deployment_selected = profile.deployment;
                self.power_selected = profile.power;
                self.power.states.clear();
                self.setting_states.clear();
                self.deployment_states.clear();
                Ok(lf!(
                    "已载入 {} 项配置：{}",
                    "Loaded {} items: {}",
                    self.selection_count(),
                    path.display()
                ))
            }
        }
    }
}

impl eframe::App for Studio {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        i18n::set_language(self.preferences.language);
        theme::apply(&ctx, &self.preferences);
        if ctx.input_mut(|input| {
            input.consume_key(egui::Modifiers::COMMAND, egui::Key::Q)
                || input.consume_key(egui::Modifiers::COMMAND, egui::Key::W)
        }) {
            self.request_close(&ctx);
        }
        self.poll_worker(&ctx);
        if ctx.input(|input| input.viewport().close_requested()) && self.worker.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if let Some(worker) = &self.worker {
                worker.cancel.store(true, Ordering::Relaxed);
            }
            self.notice = Some(Notice {
                text: t("已请求取消。请等待当前操作返回并查看报告，然后关闭窗口。").into(),
                error: false,
            });
        }
        self.rail(ui);
        self.tray(ui);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(palette().canvas).inner_margin(26.0))
            .show(ui, |ui| {
                self.header(ui);
                egui::ScrollArea::vertical()
                    .id_salt(("main-content", self.page as u8))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        match self.page {
                            Page::Recipes => self.recipes_page(ui),
                            Page::Backup => self.backup_page(ui),
                            Page::Environment => self.environment_page(ui),
                            Page::Network => self.network_page(ui),
                            Page::Migration => self.migration_page(ui),
                            Page::Compatibility => self.compatibility_page(ui),
                            Page::Workbench => self.workbench(ui),
                            Page::Power => self.power_page(ui),
                            Page::Software
                            | Page::Developer
                            | Page::Features
                            | Page::Storage
                            | Page::Maintenance => self.deployment_page(ui),
                            Page::Plan => self.plan(ui),
                            Page::Recovery => self.recovery(ui),
                            Page::Sources => self.sources(ui),
                            Page::Status => self.status_page(ui),
                            Page::Settings => self.settings_page(ui),
                            Page::Help => self.help_page(ui),
                            Page::About => self.about_page(ui),
                        }
                        ui.add_space(20.0);
                    });
            });
        self.dialogs(&ctx);
        self.frames += 1;
        if self.frames == 4
            && let Some(path) = self.screenshot.take()
        {
            let screenshot_ctx = ctx.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(
                egui::ScreenshotCallback::new(move |image| {
                    let bytes: Vec<u8> = image
                        .pixels
                        .iter()
                        .flat_map(|pixel| pixel.to_array())
                        .collect();
                    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
                        let file = std::fs::OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .open(&path)?;
                        image::codecs::png::PngEncoder::new(file).write_image(
                            &bytes,
                            image.width() as u32,
                            image.height() as u32,
                            image::ExtendedColorType::Rgba8,
                        )?;
                        Ok(())
                    })();
                    if let Err(error) = result {
                        eprintln!("Screenshot failed ({}): {error}", path.display());
                    }
                    screenshot_ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    screenshot_ctx.request_repaint();
                }),
            ));
        }
        if self.screenshot.is_some() {
            ctx.request_repaint();
        }
    }
}

impl Drop for Studio {
    fn drop(&mut self) {
        // Never detach a mutating worker: cancellation is cooperative and the current
        // native operation must return before the process can safely finish.
        if let Some(mut worker) = self.worker.take() {
            worker.cancel.store(true, Ordering::Relaxed);
            if let Some(thread) = worker.thread.take()
                && thread.join().is_err()
            {
                eprintln!(
                    "DevEnv Studio worker panicked during shutdown; inspect recovery journals."
                );
            }
        }
    }
}

fn short_target(target: Target) -> &'static str {
    match target {
        Target::Win10Ltsc2021 => "Windows 10 LTSC 2021",
        Target::Win11Ltsc2024 => "Windows 11 LTSC 2024",
    }
}

fn category_label(category: &str) -> &str {
    match category {
        "privacy" => t("隐私"),
        "explorer" => t("资源管理器"),
        "experience" => t("桌面体验"),
        "start" => t("开始菜单"),
        "taskbar" => t("任务栏"),
        "personalization" => t("个性化"),
        "gaming" => t("游戏"),
        "accessibility" => t("辅助功能"),
        "system" => t("系统"),
        "updates" => t("更新偏好"),
        other => other,
    }
}

fn localized_catalog(catalog: &Catalog) -> Catalog {
    let mut display = catalog.clone();
    display.actions = display.actions.iter().map(localized_action).collect();
    for preset in &mut display.presets {
        preset.title = crate::catalog_locale::text(
            i18n::language(),
            "presets",
            &preset.id,
            "title",
            &preset.title,
        );
        preset.description = crate::catalog_locale::text(
            i18n::language(),
            "presets",
            &preset.id,
            "description",
            &preset.description,
        );
    }
    display
}

fn localized_action(action: &studio_core::Action) -> studio_core::Action {
    let mut display = action.clone();
    display.title = crate::catalog_locale::text(
        i18n::language(),
        "actions",
        &action.id,
        "title",
        &action.title,
    );
    display.description = crate::catalog_locale::text(
        i18n::language(),
        "actions",
        &action.id,
        "description",
        &action.description,
    );
    display.restart = crate::catalog_locale::text(
        i18n::language(),
        "actions",
        &action.id,
        "restart",
        &action.restart,
    );
    display.risk = crate::catalog_locale::text(
        i18n::language(),
        "actions",
        &action.id,
        "risk",
        &action.risk,
    );
    display
}

fn localized_task(task: &studio_deployment::Task) -> studio_deployment::Task {
    let mut display = task.clone();
    display.title =
        crate::catalog_locale::text(i18n::language(), "tasks", &task.id, "title", &task.title);
    display.description = crate::catalog_locale::text(
        i18n::language(),
        "tasks",
        &task.id,
        "description",
        &task.description,
    );
    display.restart = crate::catalog_locale::text(
        i18n::language(),
        "tasks",
        &task.id,
        "restart",
        &task.restart,
    );
    display.limitations = crate::catalog_locale::text(
        i18n::language(),
        "tasks",
        &task.id,
        "limitations",
        &task.limitations,
    );
    display
}

fn category_order(category: &str) -> usize {
    [
        "privacy",
        "explorer",
        "experience",
        "start",
        "taskbar",
        "personalization",
        "gaming",
        "accessibility",
        "system",
        "updates",
    ]
    .iter()
    .position(|known| *known == category)
    .unwrap_or(usize::MAX)
}

fn select_setting(catalog: &Catalog, selected: &mut BTreeSet<String>, id: &str, value: bool) {
    if !value {
        selected.remove(id);
        return;
    }
    if let Some(action) = catalog.actions.iter().find(|action| action.id == id) {
        if let Some(group) = &action.choice_group {
            for alternative in catalog
                .actions
                .iter()
                .filter(|item| item.choice_group.as_ref() == Some(group))
            {
                selected.remove(&alternative.id);
            }
        }
        selected.insert(action.id.clone());
    }
}

fn state_label(ui: &mut egui::Ui, state: &str, detail: &str) {
    let label = match state {
        "aligned" => t("符合方案"),
        "different" => t("与方案不同"),
        "unavailable" => t("无法检查"),
        "preview" => t("预览，未检查本机 Windows"),
        "installed" => t("已安装"),
        "not_installed" => t("未安装"),
        "enabled" => t("已启用"),
        "disabled" => t("未启用"),
        "enable_pending" => t("等待重启启用"),
        "ready" => t("可执行"),
        "error" => t("检查失败"),
        other => other,
    };
    ui.label(
        RichText::new(lf!(
            "上次检查：{label}",
            "Last inspection: {label}",
            label = label
        ))
        .size(11.0)
        .color(if matches!(state, "error" | "unavailable") {
            palette().danger
        } else {
            palette().accent
        }),
    )
    .on_hover_text(detail);
}

fn registry_details(ui: &mut egui::Ui, action: &studio_core::Action) {
    for change in &action.changes {
        let hive = match change.hive {
            Hive::CurrentUser => "HKEY_CURRENT_USER",
            Hive::LocalMachine => "HKEY_LOCAL_MACHINE",
        };
        ui.label(
            RichText::new(format!("{hive}\\{}", change.path))
                .monospace()
                .size(12.0),
        );
        ui.label(
            RichText::new(format!(
                "{} = {} (REG_DWORD)",
                if change.name.is_empty() {
                    t("(默认)")
                } else {
                    &change.name
                },
                change.value
            ))
            .size(12.0)
            .color(palette().accent),
        );
    }
}

fn task_identity(task: &studio_deployment::Task) -> String {
    if task.category == "storage" {
        return storage::task_command(&task.id).unwrap_or_else(|| task.id.clone());
    }
    if let Some(id) = &task.package_id {
        if task.kind == "package" {
            lf!("WinGet 软件包：{id}", "WinGet package: {id}", id = id)
        } else {
            format!("{}: {id}", task.kind)
        }
    } else if let Some(name) = &task.feature_name {
        lf!(
            "Windows 功能：{name}",
            "Windows feature: {name}",
            name = name
        )
    } else {
        lf!(
            "内置维护命令：{} ({})",
            "Built-in maintenance: {} ({})",
            task.id,
            task.kind
        )
    }
}

fn deployment_details(ui: &mut egui::Ui, task: &studio_deployment::Task) {
    let localized = localized_task(task);
    let task = &localized;
    ui.label(
        RichText::new(&task.description)
            .size(12.0)
            .color(palette().muted),
    );
    ui.label(
        RichText::new(task_identity(task))
            .size(12.0)
            .color(palette().accent),
    );
    ui.label(
        RichText::new(lf!(
            "{}；生效：{}",
            "{}; takes effect: {}",
            if task.requires_admin {
                t("需要管理员权限")
            } else {
                t("以当前用户执行，安装器可能要求提升权限")
            },
            task.restart
        ))
        .size(12.0),
    );
    ui.label(
        RichText::new(&task.limitations)
            .size(12.0)
            .color(palette().muted),
    );
    for (index, evidence) in task.evidence.iter().enumerate() {
        ui.hyperlink_to(
            RichText::new(lf!(
                "来源与执行依据 {}",
                "Source and execution reference {}",
                index + 1
            ))
            .size(12.0),
            evidence,
        );
    }
}

fn preview_host_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "windows") {
        "Windows"
    } else if cfg!(target_os = "linux") {
        "Linux"
    } else {
        t("当前系统")
    }
}

fn preview_explanation() -> &'static str {
    if cfg!(target_os = "windows") {
        t("当前 Windows 版本不支持执行。可以选择、检查并导出方案。")
    } else {
        t("当前为预览模式：可选择、检查并导出方案；应用与恢复需在受支持的 Windows 上进行。")
    }
}

fn surface_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(palette().surface)
        .stroke(Stroke::new(1.0, palette().line))
        .corner_radius(12)
        .inner_margin(18.0)
}

fn navigation_button(ui: &mut egui::Ui, title: &str, active: bool, width: f32) -> egui::Response {
    let response = ui.add_sized(
        [width, 32.0],
        egui::Button::new((
            RichText::new(title).size(13.0).color(if active {
                palette().ink
            } else {
                palette().muted
            }),
            egui::Atom::grow(),
        ))
        .selected(active)
        .fill(if active {
            palette().wash
        } else {
            Color32::TRANSPARENT
        })
        .stroke(Stroke::NONE)
        .corner_radius(7),
    );
    if active {
        let x = response.rect.left() + 1.0;
        ui.painter().line_segment(
            [
                egui::pos2(x, response.rect.top() + 9.0),
                egui::pos2(x, response.rect.bottom() - 9.0),
            ],
            Stroke::new(3.0, palette().accent),
        );
    }
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect,
            7,
            Stroke::new(1.5, palette().accent),
            egui::StrokeKind::Inside,
        );
    }
    response
}

fn primary(ui: &mut egui::Ui, title: &str, enabled: bool) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(title).color(palette().on_accent).strong())
            .fill(palette().accent)
            .stroke(Stroke::NONE)
            .corner_radius(7),
    )
}

fn badge(ui: &mut egui::Ui, text: &str, fill: Color32, ink: Color32) {
    egui::Frame::new()
        .fill(fill)
        .corner_radius(255)
        .inner_margin(egui::Margin::symmetric(10, 5))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.0).color(ink));
        });
}

fn empty(ui: &mut egui::Ui, title: &str, description: &str) {
    egui::Frame::new()
        .fill(palette().surface)
        .stroke(Stroke::new(1.0, palette().line))
        .corner_radius(10)
        .inner_margin(30.0)
        .show(ui, |ui| {
            ui.add_space(10.0);
            ui.label(RichText::new(title).size(21.0).strong());
            ui.add_space(8.0);
            ui.label(RichText::new(description).size(14.0).color(palette().muted));
            ui.add_space(10.0);
        });
}

fn logo(ui: &mut egui::Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    let stroke = Stroke::new((size / 14.0).max(1.5), palette().accent);
    ui.painter().rect_filled(rect, 7, palette().wash);
    let at = |x: f32, y: f32| rect.min + Vec2::new(size * x, size * y);
    ui.painter()
        .line_segment([at(0.26, 0.31), at(0.45, 0.5)], stroke);
    ui.painter()
        .line_segment([at(0.45, 0.5), at(0.26, 0.69)], stroke);
    ui.painter()
        .line_segment([at(0.54, 0.69), at(0.76, 0.69)], stroke);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preview() -> Studio {
        Studio {
            compatibility: compatibility::UiState::default(),
            hybrid: hybrid::HybridState::default(),
            backup: backup::BackupUiState::default(),
            network: network::NetworkUiState::default(),
            power: power::PowerUiState::default(),
            storage_status: None,
            preferences: Preferences::default(),
            saved_preferences: Preferences::default(),
            preferences_error: None,
            last_operation: None,
            history: Ok(Vec::new()),
            catalog: Err("test catalog unavailable".into()),
            deployment_catalog: Err("test deployment unavailable".into()),
            winget: studio_deployment::Capability {
                available: false,
                detail: "Test preview".into(),
            },
            host: HostInfo {
                name: "Test preview".into(),
                target: None,
                can_apply: false,
                explanation: "Preview only".into(),
            },
            target: Target::Win11Ltsc2024,
            selected: BTreeSet::new(),
            deployment_selected: BTreeSet::new(),
            power_selected: BTreeSet::new(),
            setting_states: BTreeMap::new(),
            deployment_states: BTreeMap::new(),
            last_event: None,
            page: Page::Recipes,
            category: String::new(),
            query: String::new(),
            dialog: None,
            notice: None,
            recovery: Vec::new(),
            recovery_error: None,
            worker: None,
            screenshot: None,
            frames: 0,
        }
    }

    #[test]
    fn preview_starts_opt_in_and_cannot_mutate() {
        let app = preview();
        assert!(app.selected.is_empty());
        assert!(app.deployment_selected.is_empty());
        assert!(app.power_selected.is_empty());
        assert!(!app.mutation_available());
    }

    #[test]
    fn merged_workflow_cannot_start_from_a_preview_host() {
        let mut app = preview();
        app.compatibility.options.dev_tools = true;
        let entry = studio_legacy::request::candidates(&app.compatibility.profile_json)
            .unwrap()
            .into_iter()
            .find(|entry| entry.group == studio_legacy::request::Group::Cargo)
            .unwrap();
        app.compatibility.selected.insert(entry.key);
        app.compatibility.confirmed = true;
        let effective = studio_legacy::request::selected_profile(
            &app.compatibility.profile_json,
            &app.compatibility.selected,
        )
        .unwrap();
        assert!(
            studio_legacy::review(
                &effective,
                app.compatibility.target,
                app.compatibility.mode,
                &app.compatibility.options
            )
            .is_ok()
        );
        app.start_compatibility();
        assert!(app.worker.is_none());
        assert_eq!(app.selection_count(), 0);
        assert!(app.compatibility.report.is_none());
    }

    #[test]
    fn extended_request_io_is_blocked_while_a_task_is_running() {
        let mut app = preview();
        let (_sender, receiver) = mpsc::channel();
        app.worker = Some(Worker {
            cancel: Arc::new(AtomicBool::new(false)),
            receiver,
            thread: None,
            events: None,
            label: "test",
        });
        let before = app.compatibility.profile_json.clone();
        for operation in [
            FileOperation::SaveCompatibilityRequest,
            FileOperation::LoadCompatibilityRequest,
        ] {
            let error = app
                .file_operation(
                    operation,
                    std::env::temp_dir().join("devenv-busy-request.json"),
                )
                .unwrap_err();
            assert_eq!(
                error,
                tx(
                    "请等待当前任务结束后再保存或载入扩展部署请求。",
                    "Wait for the current task before saving or loading an extended deployment request."
                )
            );
            assert_eq!(app.compatibility.profile_json, before);
            assert!(app.worker.is_some());
        }
    }

    #[test]
    fn choice_groups_replace_only_the_alternative_selection() {
        let catalog = studio_core::load_catalog().unwrap();
        let first = catalog
            .actions
            .iter()
            .find(|action| action.choice_group.is_some())
            .expect("catalog has deliberate alternative choices");
        let second = catalog
            .actions
            .iter()
            .find(|action| action.id != first.id && action.choice_group == first.choice_group)
            .expect("choice group has alternatives");
        let unrelated = catalog
            .actions
            .iter()
            .find(|action| action.choice_group != first.choice_group)
            .unwrap();
        let mut selected = BTreeSet::from([unrelated.id.clone()]);
        select_setting(&catalog, &mut selected, &first.id, true);
        select_setting(&catalog, &mut selected, &second.id, true);
        assert!(!selected.contains(&first.id));
        assert!(selected.contains(&second.id));
        assert!(selected.contains(&unrelated.id));
        select_setting(&catalog, &mut selected, &second.id, false);
        assert_eq!(selected, BTreeSet::from([unrelated.id.clone()]));
    }

    #[test]
    fn presets_have_an_exact_valid_target_without_silent_filtering() {
        let catalog = studio_core::load_catalog().unwrap();
        assert!(!catalog.presets.is_empty());
        for preset in &catalog.presets {
            let plan = Target::ALL
                .into_iter()
                .find_map(|target| studio_core::build_plan(&catalog, target, &preset.actions).ok())
                .expect("preset supports a complete target");
            assert_eq!(
                plan.actions
                    .into_iter()
                    .map(|action| action.id)
                    .collect::<BTreeSet<_>>(),
                preset.actions
            );
        }
    }

    #[test]
    fn mutation_requires_actual_host_target_and_idle_worker() {
        let mut app = preview();
        app.host.can_apply = true;
        app.host.target = Some(Target::Win10Ltsc2021);
        assert!(!app.mutation_available());
        app.target = Target::Win10Ltsc2021;
        assert!(app.mutation_available());
        let (_sender, receiver) = mpsc::channel();
        app.worker = Some(Worker {
            cancel: Arc::new(AtomicBool::new(false)),
            receiver,
            thread: None,
            events: None,
            label: "test",
        });
        assert!(!app.mutation_available());
    }

    #[test]
    fn network_profile_roundtrip_preserves_active_state_and_exports_without_clobbering()
    -> anyhow::Result<()> {
        use studio_deployment::network::{Format, NetworkProfile, ProxyMode};
        let dir = std::env::temp_dir().join(format!(
            "studio-network-gui-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        std::fs::create_dir(&dir)?;
        let mut app = preview();
        let original = app.network.active.clone().unwrap();
        let profile = NetworkProfile {
            mode: ProxyMode::Manual,
            proxy_url: "http://127.0.0.1:7890".into(),
            ..NetworkProfile::default()
        };
        app.network.draft = profile.clone();
        let path = dir.join("network.json");
        app.file_operation(FileOperation::NetworkSave, path.clone())
            .map_err(anyhow::Error::msg)?;
        assert!(
            app.file_operation(FileOperation::NetworkSave, path.clone())
                .is_err()
        );
        app.network.draft = NetworkProfile::default();
        app.network.confirmed = true;
        app.file_operation(FileOperation::NetworkLoad, path)
            .map_err(anyhow::Error::msg)?;
        assert_eq!(app.network.draft, profile);
        assert_eq!(app.network.active.as_ref().ok(), Some(&original));
        assert!(!app.network.confirmed);
        for (format, name) in [
            (Format::Git, "gitconfig"),
            (Format::Cargo, "config.toml"),
            (Format::Npm, "npmrc"),
            (Format::Pip, "pip.conf"),
            (Format::Uv, "uv.toml"),
        ] {
            app.network.format = format;
            let path = dir.join(name);
            app.file_operation(FileOperation::NetworkExportConfig, path.clone())
                .map_err(anyhow::Error::msg)?;
            let bytes = std::fs::read(&path)?;
            assert!(
                app.file_operation(FileOperation::NetworkExportConfig, path.clone())
                    .is_err()
            );
            assert_eq!(std::fs::read(path)?, bytes);
        }
        let bad = dir.join("invalid.json");
        std::fs::write(&bad, br#"{"schema_version":1,"proxy":"private"}"#)?;
        assert!(app.file_operation(FileOperation::NetworkLoad, bad).is_err());
        assert_eq!(app.network.draft, profile);
        assert!(
            app.worker.is_none() && app.selected.is_empty() && app.deployment_selected.is_empty()
        );
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn network_workers_pin_the_reviewed_profile_and_invalid_active_state_prevents_start()
    -> anyhow::Result<()> {
        use studio_deployment::network::{NetworkProfile, ProxyMode};
        let mut app = preview();
        app.network.active = Err("Synthetic unreadable profile".into());
        app.start_network_worker("synthetic network", |_| {
            panic!("Invalid active state must not run")
        });
        assert!(app.worker.is_none());
        let profile = NetworkProfile {
            mode: ProxyMode::Manual,
            proxy_url: "http://localhost:7890".into(),
            ..NetworkProfile::default()
        };
        app.network.active = Ok(profile.clone());
        let dir = std::env::temp_dir().join(format!(
            "studio-network-scope-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        std::fs::create_dir(&dir)?;
        let path = dir.join("network.json");
        let output = path.clone();
        let (release, wait) = mpsc::channel();
        app.start_network_worker("synthetic network", move |_| {
            wait.recv()?;
            studio_deployment::network::record_current(&output)?;
            Ok(WorkerResult::RustInventory(vec![]))
        });
        app.network.active = Ok(NetworkProfile::default());
        release.send(())?;
        app.worker
            .as_mut()
            .unwrap()
            .thread
            .take()
            .unwrap()
            .join()
            .unwrap();
        app.poll_worker(&egui::Context::default());
        assert_eq!(studio_deployment::network::load(&path)?, profile);
        assert!(app.worker.is_none());
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn relative_export_path_is_rejected_before_any_write() {
        let mut app = preview();
        let result = app.file_operation(FileOperation::Export, PathBuf::from("relative.json"));
        assert!(result.unwrap_err().contains(t("绝对路径")));
    }

    #[test]
    fn shutdown_requests_cancellation_and_joins_worker() {
        let cancel = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let worker_finished = Arc::clone(&finished);
        let (_sender, receiver) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            while !worker_cancel.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            worker_finished.store(true, Ordering::Release);
        });
        let mut app = preview();
        app.worker = Some(Worker {
            cancel: Arc::clone(&cancel),
            receiver,
            thread: Some(thread),
            events: None,
            label: "test",
        });
        drop(app);
        assert!(cancel.load(Ordering::Acquire));
        assert!(finished.load(Ordering::Acquire));
    }

    #[test]
    fn localized_catalog_views_preserve_trusted_selection_and_execution_payloads() {
        let catalog = studio_core::load_catalog().unwrap();
        let deployment = studio_deployment::catalog().unwrap();
        for language in [Language::ZhCn, Language::EnUs] {
            i18n::set_language(language);
            let mut display = localized_catalog(&catalog);
            if language == Language::EnUs {
                assert!(
                    display
                        .actions
                        .iter()
                        .zip(&catalog.actions)
                        .any(|(shown, original)| shown.title != original.title)
                );
                assert!(
                    display
                        .presets
                        .iter()
                        .zip(&catalog.presets)
                        .any(|(shown, original)| shown.title != original.title)
                );
            }
            for (shown, original) in display.actions.iter_mut().zip(&catalog.actions) {
                shown.title.clone_from(&original.title);
                shown.description.clone_from(&original.description);
                shown.restart.clone_from(&original.restart);
                shown.risk.clone_from(&original.risk);
                assert_eq!(shown, original);
                for target in &original.targets {
                    let plan = studio_core::build_plan(
                        &catalog,
                        *target,
                        &BTreeSet::from([shown.id.clone()]),
                    )
                    .unwrap();
                    assert_eq!(plan.actions, vec![original.clone()]);
                    studio_core::validate_plan(&plan).unwrap();
                }
            }
            for (shown, original) in display.presets.iter_mut().zip(&catalog.presets) {
                shown.title.clone_from(&original.title);
                shown.description.clone_from(&original.description);
                assert_eq!(shown, original);
                let plan = Target::ALL
                    .into_iter()
                    .find_map(|target| {
                        studio_core::build_plan(&catalog, target, &shown.actions).ok()
                    })
                    .unwrap();
                assert_eq!(
                    plan.actions
                        .iter()
                        .map(|action| action.id.clone())
                        .collect::<BTreeSet<_>>(),
                    original.actions
                );
                studio_core::validate_plan(&plan).unwrap();
            }
            assert_eq!(display, catalog);
            for original in &deployment.tasks {
                let mut shown = localized_task(original);
                shown.title.clone_from(&original.title);
                shown.description.clone_from(&original.description);
                shown.restart.clone_from(&original.restart);
                shown.limitations.clone_from(&original.limitations);
                assert_eq!(shown, *original);
                for target in &shown.targets {
                    let plan =
                        studio_deployment::build_plan(*target, &BTreeSet::from([shown.id.clone()]))
                            .unwrap();
                    assert_eq!(plan.tasks, vec![original.clone()]);
                }
            }
        }
        i18n::set_language(Language::ZhCn);
    }

    #[test]
    fn quit_closes_idle_viewport_but_cancels_and_keeps_active_worker_open() {
        let context = egui::Context::default();
        let mut idle = preview();
        let mut idle_output = context.run_ui(Default::default(), |ui| idle.request_close(ui.ctx()));
        idle_output.textures_delta.clear();
        assert!(
            idle_output
                .viewport_output
                .values()
                .flat_map(|output| &output.commands)
                .any(|command| matches!(command, egui::ViewportCommand::Close))
        );

        let context = egui::Context::default();
        let cancel = Arc::new(AtomicBool::new(false));
        let (_sender, receiver) = mpsc::channel();
        let mut active = preview();
        active.worker = Some(Worker {
            cancel: Arc::clone(&cancel),
            receiver,
            thread: None,
            events: None,
            label: "test",
        });
        let mut active_output =
            context.run_ui(Default::default(), |ui| active.request_close(ui.ctx()));
        active_output.textures_delta.clear();
        assert!(cancel.load(Ordering::Acquire));
        assert!(active.worker.is_some());
        assert!(active.notice.as_ref().is_some_and(|notice| !notice.error));
        assert!(
            !active_output
                .viewport_output
                .values()
                .flat_map(|output| &output.commands)
                .any(|command| matches!(command, egui::ViewportCommand::Close))
        );
    }

    #[test]
    fn application_tools_remain_visible_at_minimum_size_and_large_scale() {
        for language in [Language::ZhCn, Language::EnUs] {
            for scale in [100, 125] {
                let mut app = preview();
                app.page = Page::Environment;
                app.preferences.language = language;
                app.preferences.scale_percent = scale;
                i18n::set_language(language);
                let context = egui::Context::default();
                context.enable_accesskit();
                theme::apply(&context, &app.preferences);
                let mut nodes = BTreeMap::new();
                for _ in 0..2 {
                    let mut output = context.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(980.0, 680.0),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            app.rail(ui);
                            app.tray(ui);
                            egui::CentralPanel::default().show(ui, |ui| {
                                ui.label("content");
                            });
                        },
                    );
                    output.textures_delta.clear();
                    nodes.extend(
                        output
                            .platform_output
                            .accesskit_update
                            .expect("accessibility update")
                            .nodes,
                    );
                }
                for title in [
                    tx("设置", "Settings"),
                    tx("帮助", "Help"),
                    tx("关于", "About"),
                    tx("项目来源", "Sources"),
                ] {
                    let bounds = nodes
                        .values()
                        .find(|node| node.label() == Some(title))
                        .and_then(|node| node.bounds())
                        .expect("visible application tool");
                    assert!(
                        bounds.x0 >= 0.0
                            && bounds.y0 >= 0.0
                            && bounds.x1 <= 980.0
                            && bounds.y1 <= 680.0,
                        "{title}: {bounds:?}"
                    );
                }
                assert!(app.worker.is_none());
                assert_eq!(app.selection_count(), 0);
            }
        }
        i18n::set_language(Language::ZhCn);
    }

    #[test]
    fn every_page_renders_bilingually_at_minimum_size_without_enabling_preview_mutation() {
        let pages = [
            Page::Compatibility,
            Page::Recipes,
            Page::Backup,
            Page::Environment,
            Page::Network,
            Page::Migration,
            Page::Developer,
            Page::Workbench,
            Page::Power,
            Page::Storage,
            Page::Software,
            Page::Features,
            Page::Maintenance,
            Page::Plan,
            Page::Recovery,
            Page::Sources,
            Page::Status,
            Page::Settings,
            Page::Help,
            Page::About,
        ];
        for (language, appearance) in [
            (Language::ZhCn, Appearance::Light),
            (Language::EnUs, Appearance::Dark),
        ] {
            for scale in [100, 125] {
                let mut app = preview();
                app.preferences.language = language;
                app.preferences.appearance = appearance;
                app.preferences.scale_percent = scale;
                app.saved_preferences = app.preferences.clone();
                app.catalog = Ok(studio_core::load_catalog().unwrap());
                app.deployment_catalog = Ok(studio_deployment::catalog().unwrap());
                let action = app
                    .catalog
                    .as_ref()
                    .unwrap()
                    .actions
                    .iter()
                    .find(|action| action.targets.contains(&app.target))
                    .unwrap();
                app.selected.insert(action.id.clone());
                let task = app
                    .deployment_catalog
                    .as_ref()
                    .unwrap()
                    .tasks
                    .iter()
                    .find(|task| task.targets.contains(&app.target))
                    .unwrap();
                app.backup.snapshot = Some(
                    serde_json::from_slice(include_bytes!(
                        "../../../examples/mac-development-backup.json"
                    ))
                    .unwrap(),
                );
                app.deployment_selected.insert(task.id.clone());
                app.deployment_selected
                    .insert("storage-analyze-system".into());
                app.power_selected
                    .insert(studio_platform::power::catalog().unwrap()[0].id.clone());
                let selection = (
                    app.selected.clone(),
                    app.deployment_selected.clone(),
                    app.power_selected.clone(),
                );
                for page in pages {
                    app.page = page;
                    let context = egui::Context::default();
                    i18n::set_language(language);
                    theme::apply(&context, &app.preferences);
                    let mut output = context.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(980.0, 680.0),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            app.rail(ui);
                            app.tray(ui);
                            egui::CentralPanel::default().show(ui, |ui| {
                                app.header(ui);
                                egui::ScrollArea::vertical().show(ui, |ui| match page {
                                    Page::Recipes => app.recipes_page(ui),
                                    Page::Backup => app.backup_page(ui),
                                    Page::Environment => app.environment_page(ui),
                                    Page::Network => app.network_page(ui),
                                    Page::Migration => app.migration_page(ui),
                                    Page::Compatibility => app.compatibility_page(ui),
                                    Page::Workbench => app.workbench(ui),
                                    Page::Power => app.power_page(ui),
                                    Page::Software
                                    | Page::Developer
                                    | Page::Features
                                    | Page::Storage
                                    | Page::Maintenance => app.deployment_page(ui),
                                    Page::Plan => app.plan(ui),
                                    Page::Recovery => app.recovery(ui),
                                    Page::Sources => app.sources(ui),
                                    Page::Status => app.status_page(ui),
                                    Page::Settings => app.settings_page(ui),
                                    Page::Help => app.help_page(ui),
                                    Page::About => app.about_page(ui),
                                });
                            });
                        },
                    );
                    output.textures_delta.clear();
                    assert!(!output.shapes.is_empty());
                    assert!(!app.mutation_available());
                    assert!(app.host.target.is_none());
                    assert!(app.worker.is_none());
                    assert_eq!(
                        (&app.selected, &app.deployment_selected, &app.power_selected),
                        (&selection.0, &selection.1, &selection.2)
                    );
                    assert!(app.dialog.is_none());
                }
                app.start_apply();
                assert!(app.worker.is_none());
                assert!(app.notice.as_ref().is_some_and(|notice| notice.error));
                app.start_deploy();
                assert!(app.worker.is_none());
                app.start_power_apply();
                assert!(app.worker.is_none());
                app.start_power_restore(PathBuf::from("/synthetic/power-journal.json"));
                assert!(app.worker.is_none());
                assert!(app.notice.as_ref().is_some_and(|notice| notice.error));
            }
        }
        i18n::set_language(Language::ZhCn);
    }

    #[test]
    fn reviewed_migration_preserves_workspace_and_developer_tasks_through_profile()
    -> anyhow::Result<()> {
        let mut app = preview();
        app.catalog = Ok(studio_core::load_catalog()?);
        app.deployment_catalog = Ok(studio_deployment::catalog()?);
        let report = crate::migration::analyze(
            include_bytes!("../../../examples/migration-profile.json"),
            app.target,
        )?;
        assert!(report.items.iter().any(|item| item.status == "mapped"));
        app.hybrid.report = Some(report);
        app.hybrid.report_target = Some(app.target);
        assert!(app.hybrid.selected_rows.is_empty());
        assert!(app.selected.is_empty());
        assert!(app.deployment_selected.is_empty());
        assert!(app.worker.is_none());

        let preserved = "experience-taskview".to_owned();
        app.selected.insert(preserved.clone());
        let report = app.hybrid.report.as_ref().unwrap();
        app.hybrid.selected_rows = report
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.status == "mapped")
            .map(|(index, _)| index)
            .collect();
        let mut added_settings = BTreeSet::new();
        let mut added_tasks = BTreeSet::new();
        for index in &app.hybrid.selected_rows {
            let item = &report.items[*index];
            added_settings.extend(item.settings.iter().cloned());
            added_tasks.extend(item.deployment.iter().cloned());
        }
        let catalog = app.catalog.as_ref().unwrap();
        let (settings, tasks) = crate::solutions::merge(
            catalog,
            app.target,
            &app.selected,
            &app.deployment_selected,
            &added_settings,
            &added_tasks,
        )?;
        assert!(settings.contains(&preserved));
        let deployment = app.deployment_catalog.as_ref().unwrap();
        for kind in ["cargo", "uv", "vscode"] {
            assert!(
                deployment
                    .tasks
                    .iter()
                    .any(|task| task.kind == kind && tasks.contains(&task.id)),
                "missing developer provider {kind}"
            );
        }

        struct TestDirectory(PathBuf);
        impl Drop for TestDirectory {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let path = std::env::temp_dir().join(format!(
            "ltsc-migration-profile-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        std::fs::create_dir(&path)?;
        let directory = TestDirectory(path);
        let profile_path = directory.0.join("reviewed-profile.json");
        workspace_io::save(
            catalog,
            app.target,
            &settings,
            &tasks,
            &app.power_selected,
            &profile_path,
        )?;
        let loaded = workspace_io::load(&profile_path, catalog)?;
        assert_eq!(loaded.schema_version, 3);
        assert_eq!(loaded.settings.selected, settings);
        assert_eq!(loaded.deployment, tasks);
        assert!(app.worker.is_none());
        assert!(app.deployment_selected.is_empty());
        Ok(())
    }

    #[test]
    fn oversized_deployment_is_reviewable_but_never_starts_a_worker() {
        let mut app = preview();
        app.catalog = Ok(studio_core::load_catalog().unwrap());
        app.deployment_selected = studio_deployment::catalog()
            .unwrap()
            .tasks
            .into_iter()
            .take(studio_deployment::MAX_SESSION_TASKS + 1)
            .map(|task| task.id)
            .collect();
        assert!(app.deployment_limit_exceeded());
        assert!(
            workspace_io::plan(
                app.catalog.as_ref().unwrap(),
                app.target,
                &app.selected,
                &app.deployment_selected,
                &app.power_selected,
            )
            .is_ok()
        );
        app.start_inspect();
        assert!(app.worker.is_none());
        assert!(app.notice.as_ref().is_some_and(|notice| notice.error));
        app.host.can_apply = true;
        app.host.target = Some(app.target);
        app.start_deploy();
        assert!(app.worker.is_none());
        assert!(app.notice.as_ref().unwrap().text.contains("128"));
    }

    #[test]
    fn power_groups_replace_only_alternatives_and_clear_all_domains() {
        let mut app = preview();
        let catalog = studio_platform::power::catalog().unwrap();
        let first = &catalog[0];
        let second = catalog
            .iter()
            .find(|choice| choice.id != first.id && choice.group == first.group)
            .unwrap();
        let unrelated = catalog
            .iter()
            .find(|choice| choice.group != first.group)
            .unwrap();
        app.power_selected.insert(unrelated.id.clone());
        power::select_power(&catalog, &mut app.power_selected, &first.id, true);
        power::select_power(&catalog, &mut app.power_selected, &second.id, true);
        assert!(!app.power_selected.contains(&first.id));
        assert!(app.power_selected.contains(&second.id));
        assert!(app.power_selected.contains(&unrelated.id));
        power::select_power(&catalog, &mut app.power_selected, &second.id, false);
        assert_eq!(app.power_selected, BTreeSet::from([unrelated.id.clone()]));
        app.selected.insert("synthetic-setting".into());
        app.deployment_selected.insert("synthetic-task".into());
        assert_eq!(app.selection_count(), 3);
        app.clear_selection();
        assert_eq!(app.selection_count(), 0);
        assert!(app.power_selected.is_empty());
    }

    #[test]
    fn power_and_storage_gui_profile_roundtrip_export_and_legacy_load_are_exact()
    -> anyhow::Result<()> {
        struct TestDirectory(PathBuf);
        impl Drop for TestDirectory {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let path = std::env::temp_dir().join(format!(
            "ltsc-power-gui-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        std::fs::create_dir(&path)?;
        let directory = TestDirectory(path);
        let mut app = preview();
        app.catalog = Ok(studio_core::load_catalog()?);
        let power_id = studio_platform::power::catalog()?[0].id.clone();
        app.power_selected.insert(power_id.clone());
        app.deployment_selected
            .insert("storage-analyze-system".into());
        let settings = studio_core::Profile {
            schema_version: studio_core::SCHEMA_VERSION,
            target: app.target,
            selected: BTreeSet::new(),
        };
        let profile_path = directory.0.join("profile.json");
        let export_path = directory.0.join("plan.json");
        app.file_operation(FileOperation::Save, profile_path.clone())
            .map_err(anyhow::Error::msg)?;
        app.file_operation(FileOperation::Export, export_path.clone())
            .map_err(anyhow::Error::msg)?;
        let document: serde_json::Value = serde_json::from_slice(&std::fs::read(export_path)?)?;
        assert_eq!(document["schema_version"], 3);
        assert_eq!(document["power"]["choices"][0]["id"], power_id);
        assert_eq!(
            document["deployment"]["tasks"][0]["id"],
            "storage-analyze-system"
        );
        app.clear_selection();
        app.file_operation(FileOperation::Load, profile_path)
            .map_err(anyhow::Error::msg)?;
        assert_eq!(app.power_selected, BTreeSet::from([power_id.clone()]));
        assert_eq!(
            app.deployment_selected,
            BTreeSet::from(["storage-analyze-system".into()])
        );
        assert_eq!(app.selection_count(), 2);
        for (name, legacy) in [
            ("v1.json", serde_json::to_value(&settings)?),
            (
                "v2.json",
                serde_json::json!({"schema_version": 2, "settings": settings, "deployment": []}),
            ),
        ] {
            let path = directory.0.join(name);
            studio_core::export_document(&legacy, &path)?;
            app.power_selected.insert(power_id.clone());
            app.file_operation(FileOperation::Load, path)
                .map_err(anyhow::Error::msg)?;
            assert!(app.power_selected.is_empty());
            assert!(app.deployment_selected.is_empty());
        }
        assert!(app.worker.is_none());
        Ok(())
    }

    #[cfg(not(windows))]
    #[test]
    fn storage_inspection_preview_is_retained_after_worker_completion() {
        let mut app = preview();
        app.start_storage_status();
        app.worker
            .as_mut()
            .unwrap()
            .thread
            .take()
            .unwrap()
            .join()
            .unwrap();
        app.poll_worker(&egui::Context::default());
        assert!(app.worker.is_none());
        assert!(
            app.storage_status
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .preview
        );
        assert!(app.selected.is_empty());
        assert!(app.deployment_selected.is_empty());
        assert!(!app.mutation_available());
        app.notice = None;
        assert!(app.storage_status.is_some());
    }

    #[test]
    fn mac_backup_save_load_and_mapping_keep_windows_selection_opt_in() -> anyhow::Result<()> {
        struct TestDir(PathBuf);
        impl Drop for TestDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let directory = TestDir(std::env::temp_dir().join(format!(
                "studio-backup-gui-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos()
            )));
        std::fs::create_dir(&directory.0)?;
        let mut app = preview();
        let snapshot: studio_deployment::backup::Snapshot = serde_json::from_slice(
            include_bytes!("../../../examples/mac-development-backup.json"),
        )?;
        let previous = snapshot.clone();
        app.accept_backup(snapshot);
        let path = directory.0.join("environment.json");
        app.file_operation(FileOperation::SaveBackup, path.clone())
            .map_err(anyhow::Error::msg)?;
        assert!(
            app.file_operation(FileOperation::SaveBackup, path.clone())
                .is_err()
        );
        app.backup.snapshot = None;
        app.file_operation(FileOperation::LoadBackup, path.clone())
            .map_err(anyhow::Error::msg)?;
        assert_eq!(app.backup.snapshot.as_ref(), Some(&previous));
        let invalid = directory.0.join("invalid.json");
        std::fs::write(&invalid, b"{}")?;
        assert!(
            app.file_operation(FileOperation::LoadBackup, invalid)
                .is_err()
        );
        assert_eq!(app.backup.snapshot.as_ref(), Some(&previous));
        assert_eq!(app.backup.path, Some(path));
        app.hybrid.review_page = 99;
        app.hybrid.review_filter = "other".into();
        app.hybrid.selected_rows.insert(0);
        app.review_backup()?;
        assert_eq!(app.hybrid.review_page, 0);
        assert!(app.hybrid.review_filter.is_empty());
        assert!(app.page == Page::Migration);
        assert!(app.migration_current());
        assert!(
            app.hybrid
                .report
                .as_ref()
                .unwrap()
                .items
                .iter()
                .any(|item| item.status == "mapped")
        );
        assert!(app.hybrid.selected_rows.is_empty());
        assert!(
            app.selected.is_empty()
                && app.deployment_selected.is_empty()
                && app.power_selected.is_empty()
        );
        assert!(app.worker.is_none());
        Ok(())
    }

    #[test]
    fn cancelled_backup_discards_queued_success_and_preserves_previous_saved_inventory()
    -> anyhow::Result<()> {
        let mut app = preview();
        let previous: studio_deployment::backup::Snapshot = serde_json::from_slice(
            include_bytes!("../../../examples/mac-development-backup.json"),
        )?;
        let path = PathBuf::from("/synthetic/previous-environment.json");
        app.backup.snapshot = Some(previous.clone());
        app.backup.path = Some(path.clone());
        let mut late = previous.clone();
        late.captured_at = "1792800000".into();
        let (sender, receiver) = mpsc::channel();
        sender.send(Ok(WorkerResult::Backup(late)))?;
        app.worker = Some(Worker {
            cancel: Arc::new(AtomicBool::new(true)),
            receiver,
            thread: None,
            events: None,
            label: "test",
        });
        app.poll_worker(&egui::Context::default());
        assert_eq!(app.backup.snapshot.as_ref(), Some(&previous));
        assert_eq!(app.backup.path.as_ref(), Some(&path));
        assert!(app.worker.is_none());
        assert!(app.notice.as_ref().is_some_and(|notice| notice.error));
        assert!(
            app.selected.is_empty()
                && app.deployment_selected.is_empty()
                && app.power_selected.is_empty()
        );
        Ok(())
    }

    #[test]
    fn large_backup_keeps_controls_accessible_and_searches_all_virtual_rows() -> anyhow::Result<()>
    {
        fn render(
            app: &mut Studio,
            context: &egui::Context,
            expand: bool,
        ) -> Vec<(egui::accesskit::NodeId, String)> {
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(980.0, 680.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("backup-test-main")
                            .show(ui, |ui| {
                                if expand {
                                    app.backup.query = "synthetic-".into();
                                }
                                app.backup_page(ui);
                            });
                    });
                },
            );
            output.textures_delta.clear();
            let tree = output
                .platform_output
                .accesskit_update
                .expect("accessibility update");
            let nodes: std::collections::BTreeMap<_, _> = tree.nodes.into_iter().collect();
            let mut pending = vec![egui::accesskit_root_id().accesskit_id()];
            let mut seen = BTreeSet::new();
            let mut labels = Vec::new();
            while let Some(id) = pending.pop() {
                if !seen.insert(id) {
                    continue;
                }
                let node = nodes.get(&id).expect("reachable node");
                pending.extend_from_slice(node.children());
                if let Some(label) = node.label().or_else(|| node.value()) {
                    labels.push((id, label.to_owned()));
                }
            }
            labels
        }
        let mut app = preview();
        app.preferences.language = Language::EnUs;
        i18n::set_language(Language::EnUs);
        let mut snapshot: studio_deployment::backup::Snapshot = serde_json::from_slice(
            include_bytes!("../../../examples/mac-development-backup.json"),
        )?;
        snapshot
            .providers
            .iter_mut()
            .find(|provider| provider.kind == "brew_formula")
            .unwrap()
            .packages = (0..447)
            .map(|index| studio_deployment::backup::Package {
                name: format!("synthetic-{index:04}"),
                version: "1.2.3".into(),
            })
            .collect();
        studio_deployment::backup::validate(&snapshot)?;
        app.backup.snapshot = Some(snapshot.clone());
        app.backup.path = Some(PathBuf::from("/synthetic/environment.json"));
        let context = egui::Context::default();
        context.all_styles_mut(|style| style.animation_time = 0.0);
        context.enable_accesskit();
        let collapsed = render(&mut app, &context, false);
        for control in ["Save backup…", "Review Windows mappings"] {
            assert!(
                collapsed.iter().any(|(_, label)| label == control),
                "{control} is reachable"
            );
        }
        assert!(!collapsed.iter().any(|(_, label)| label == "synthetic-0000"));
        render(&mut app, &context, true);
        let expanded = render(&mut app, &context, true);
        assert!(
            expanded.iter().any(|(_, label)| label == "synthetic-0000"),
            "Expanded AX labels: {expanded:?}"
        );
        assert!(
            expanded
                .iter()
                .filter(|(_, label)| label.starts_with("synthetic-"))
                .count()
                < 25
        );
        assert!(!expanded.iter().any(|(_, label)| label == "synthetic-0446"));
        let first_id = expanded
            .iter()
            .find(|(_, label)| label == "synthetic-0000")
            .unwrap()
            .0;
        app.backup.query = "synthetic-0000".into();
        render(&mut app, &context, false);
        let filtered_first = render(&mut app, &context, false);
        assert_eq!(
            filtered_first
                .iter()
                .find(|(_, label)| label == "synthetic-0000")
                .unwrap()
                .0,
            first_id
        );
        app.backup.query = "synthetic-0446".into();
        render(&mut app, &context, false);
        let tail = render(&mut app, &context, false);
        assert!(tail.iter().any(|(_, label)| label == "synthetic-0446"));
        assert_eq!(app.backup.snapshot.as_ref(), Some(&snapshot));
        let report = crate::backup_migration::analyze(&snapshot, app.target)?;
        assert_eq!(
            report
                .items
                .iter()
                .filter(|item| item.origin == "homebrew_formulae")
                .count(),
            447
        );
        assert!(
            app.selected.is_empty()
                && app.deployment_selected.is_empty()
                && app.power_selected.is_empty()
        );
        Ok(())
    }

    #[test]
    fn migration_pagination_keeps_accessible_review_and_full_report_checked_requests()
    -> anyhow::Result<()> {
        fn render(app: &mut Studio, context: &egui::Context) -> Vec<String> {
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(980.0, 680.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("migration-test-main")
                            .show(ui, |ui| app.migration_page(ui));
                    });
                },
            );
            output.textures_delta.clear();
            let nodes: std::collections::BTreeMap<_, _> = output
                .platform_output
                .accesskit_update
                .expect("AX update")
                .nodes
                .into_iter()
                .collect();
            let mut pending = vec![egui::accesskit_root_id().accesskit_id()];
            let mut seen = BTreeSet::new();
            let mut labels = Vec::new();
            while let Some(id) = pending.pop() {
                if !seen.insert(id) {
                    continue;
                }
                let node = nodes.get(&id).expect("reachable AX node");
                pending.extend_from_slice(node.children());
                if let Some(label) = node.label().or_else(|| node.value()) {
                    labels.push(label.to_owned());
                }
            }
            labels
        }
        let mut app = preview();
        app.preferences.language = Language::EnUs;
        i18n::set_language(Language::EnUs);
        app.catalog = Ok(studio_core::load_catalog()?);
        app.deployment_catalog = Ok(studio_deployment::catalog()?);
        let mut report = crate::migration::analyze(
            include_bytes!("../../../examples/migration-profile.json"),
            app.target,
        )?;
        let first = report
            .items
            .iter()
            .find(|item| item.status == "mapped" && !item.settings.is_empty())
            .unwrap()
            .clone();
        let last = report
            .items
            .iter()
            .find(|item| item.status == "mapped" && !item.deployment.is_empty())
            .unwrap()
            .clone();
        report.items = vec![first.clone()];
        report.items.extend((0..447).map(|index| crate::migration::MigrationItem {
            origin: "synthetic_notes".into(), requested: format!("synthetic-note-{index:04}"),
            status: "manual".into(), detail: "Source package has no reviewed Windows equivalent. Captured version remains source metadata only; no installation request was selected.".into(),
            settings: Vec::new(), deployment: Vec::new(),
        }));
        report.items.push(last.clone());
        let last_index = report.items.len() - 1;
        app.hybrid.report = Some(report);
        app.hybrid.path = "/synthetic/migration.json".into();
        app.hybrid.report_path = Some(app.hybrid.path.clone());
        app.hybrid.report_target = Some(app.target);
        let context = egui::Context::default();
        context.enable_accesskit();
        let initial = render(&mut app, &context);
        for control in [
            "All",
            "Mapped",
            "Other",
            "Previous",
            "Next",
            "Merge checked items into workspace",
        ] {
            assert!(
                initial.iter().any(|label| label == control),
                "{control} is reachable"
            );
        }
        assert!(
            initial
                .iter()
                .filter(|label| label.starts_with("synthetic-note-"))
                .count()
                < 20
        );
        assert!(app.hybrid.selected_rows.is_empty());
        assert!(
            app.hybrid.checked_requests().0.is_empty()
                && app.hybrid.checked_requests().1.is_empty()
        );
        app.hybrid.review_page = usize::MAX;
        let tail = render(&mut app, &context);
        assert_eq!(app.hybrid.review_page, 22);
        assert!(tail.iter().any(|label| label == "synthetic-note-0446"));
        app.hybrid.selected_rows.extend([0, last_index]);
        app.hybrid.review_filter = "other".into();
        app.hybrid.review_page = 1;
        render(&mut app, &context);
        assert_eq!(app.hybrid.selected_rows, BTreeSet::from([0, last_index]));
        let manual = &mut app.hybrid.report.as_mut().unwrap().items[1];
        manual.settings.push("manual-setting-must-not-merge".into());
        manual.deployment.push("manual-task-must-not-merge".into());
        app.hybrid.selected_rows.extend([1, usize::MAX]);
        let (settings, tasks) = app.hybrid.checked_requests();
        assert_eq!(settings, first.settings.iter().cloned().collect());
        assert_eq!(tasks, last.deployment.iter().cloned().collect());
        app.hybrid.selected_rows.remove(&1);
        app.hybrid.selected_rows.remove(&usize::MAX);
        assert!(app.migration_current());
        let (merged_settings, merged_tasks) = crate::solutions::merge(
            app.catalog.as_ref().unwrap(),
            app.target,
            &app.selected,
            &app.deployment_selected,
            &settings,
            &tasks,
        )?;
        assert!(settings.is_subset(&merged_settings) && tasks.is_subset(&merged_tasks));
        app.hybrid.review_filter = "mapped".into();
        let mapped = render(&mut app, &context);
        assert_eq!(app.hybrid.review_page, 0);
        assert!(
            !mapped
                .iter()
                .any(|label| label.starts_with("synthetic-note-"))
        );
        assert_eq!(app.hybrid.selected_rows, BTreeSet::from([0, last_index]));
        app.hybrid.reset_review();
        assert_eq!(app.hybrid.review_page, 0);
        assert!(app.hybrid.review_filter.is_empty() && app.hybrid.selected_rows.is_empty());
        Ok(())
    }

    #[test]
    fn migration_merge_requires_report_and_unchanged_analyzed_path_and_target() -> anyhow::Result<()>
    {
        let mut app = preview();
        app.hybrid.path = "  /synthetic/profile.json  ".into();
        app.hybrid.report_path = Some("/synthetic/profile.json".into());
        app.hybrid.report_target = Some(app.target);
        assert!(!app.migration_current());
        app.hybrid.report = Some(crate::migration::analyze(
            include_bytes!("../../../examples/migration-profile.json"),
            app.target,
        )?);
        assert!(app.migration_current());
        app.hybrid.path = "/synthetic/different.json".into();
        assert!(!app.migration_current());
        app.hybrid.path = "/synthetic/profile.json".into();
        app.target = Target::Win10Ltsc2021;
        assert!(!app.migration_current());
        app.target = Target::Win11Ltsc2024;
        assert!(app.migration_current());
        app.hybrid.report = None;
        assert!(!app.migration_current());
        assert!(app.selected.is_empty());
        assert!(app.deployment_selected.is_empty());
        Ok(())
    }
}
