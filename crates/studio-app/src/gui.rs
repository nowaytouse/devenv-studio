//! Native desktop workbench. Every mutation goes through the platform journal engine.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use eframe::egui::{self, Color32, FontId, RichText, Stroke, Vec2};
use image::ImageEncoder;
use studio_core::{Catalog, Hive, Target};
use studio_platform::{HostInfo, RecoveryPoint, RunReport};

use crate::workspace_io;

const CANVAS: Color32 = Color32::from_rgb(243, 246, 250);
const INK: Color32 = Color32::from_rgb(32, 45, 72);
const BLUE: Color32 = Color32::from_rgb(59, 102, 232);
const WASH: Color32 = Color32::from_rgb(234, 240, 255);
const MUTED: Color32 = Color32::from_rgb(107, 120, 144);
const LINE: Color32 = Color32::from_rgb(224, 231, 240);
const RED: Color32 = Color32::from_rgb(173, 58, 67);

pub fn run() -> eframe::Result<()> {
    run_with_options(None)
}

/// Screenshot mode is an explicit development aid, never enabled in normal sessions.
pub fn run_with_options(screenshot: Option<PathBuf>) -> eframe::Result<()> {
    eframe::run_native(
        "LTSC Studio",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1240.0, 860.0])
                .with_min_inner_size([980.0, 680.0]),
            ..Default::default()
        },
        Box::new(move |cc| {
            configure(&cc.egui_ctx);
            Ok(Box::new(Studio::new(screenshot)))
        }),
    )
}

fn configure(ctx: &egui::Context) {
    let mut style = (*ctx.style_of(egui::Theme::Light)).clone();
    style.visuals = egui::Visuals::light();
    style.visuals.panel_fill = CANVAS;
    style.visuals.override_text_color = Some(INK);
    style.visuals.widgets.inactive.bg_fill = Color32::WHITE;
    style.visuals.widgets.inactive.weak_bg_fill = Color32::WHITE;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, LINE);
    style.visuals.widgets.hovered.bg_fill = WASH;
    style.visuals.widgets.hovered.weak_bg_fill = WASH;
    style.visuals.widgets.active.bg_fill = WASH;
    style.visuals.selection.bg_fill = BLUE;
    style.visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    style.visuals.hyperlink_color = BLUE;
    style.spacing.item_spacing = Vec2::new(12.0, 10.0);
    style.spacing.button_padding = Vec2::new(16.0, 10.0);
    style.spacing.interact_size.y = 36.0;
    style
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(15.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, FontId::proportional(15.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, FontId::proportional(12.0));
    style
        .text_styles
        .insert(egui::TextStyle::Heading, FontId::proportional(25.0));
    ctx.set_global_style(style);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Workbench,
    Software,
    Features,
    Maintenance,
    Plan,
    Recovery,
    Sources,
}

#[derive(Clone, Copy)]
enum FileOperation {
    Export,
    Save,
    Load,
}

impl FileOperation {
    fn title(self) -> &'static str {
        match self {
            Self::Export => "导出变更方案",
            Self::Save => "保存配置档案",
            Self::Load => "载入配置档案",
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
    Registry(RunReport),
    Deployment(studio_deployment::DeploymentReport),
    Inspection {
        settings: Vec<studio_platform::SettingState>,
        deployment: Vec<studio_deployment::Detection>,
        cancelled: bool,
    },
}

struct Notice {
    text: String,
    error: bool,
}

struct Studio {
    catalog: Result<Catalog, String>,
    deployment_catalog: Result<studio_deployment::Catalog, String>,
    winget: studio_deployment::Capability,
    host: HostInfo,
    target: Target,
    selected: BTreeSet<String>,
    deployment_selected: BTreeSet<String>,
    setting_states: BTreeMap<String, (String, String)>,
    deployment_states: BTreeMap<String, (String, String)>,
    last_event: Option<String>,
    deployment_history: Vec<PathBuf>,
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
        let mut app = Self {
            catalog: studio_core::load_catalog().map_err(|e| format!("{e:#}")),
            deployment_catalog: studio_deployment::catalog().map_err(|e| format!("{e:#}")),
            winget: studio_deployment::winget_capability(),
            host,
            target,
            selected: BTreeSet::new(),
            deployment_selected: BTreeSet::new(),
            setting_states: BTreeMap::new(),
            deployment_states: BTreeMap::new(),
            last_event: None,
            deployment_history: Vec::new(),
            page: Page::Workbench,
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
        app.refresh_recovery();
        app
    }

    fn refresh_recovery(&mut self) {
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
                self.last_event = Some(format!(
                    "{} / {} 项已完成 · {}：{}",
                    event.completed, event.total, event.task_id, event.message
                ));
            }
        }
        let result = self
            .worker
            .as_ref()
            .and_then(|worker| match worker.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(
                    "后台任务异常结束，未收到执行报告。请查看恢复记录后再继续。".into(),
                )),
                Err(mpsc::TryRecvError::Empty) => None,
            });
        if let Some(mut result) = result {
            if let Some(mut worker) = self.worker.take()
                && let Some(thread) = worker.thread.take()
                && thread.join().is_err()
            {
                result = Err("后台任务发生异常；请检查已有恢复记录。".into());
            }
            self.notice = Some(match result {
                Ok(WorkerResult::Registry(report)) => Notice {
                    text: format!(
                        "{}{}\n已变更 {} 项；原值已符合 {} 项。恢复日志：{}",
                        if report.cancelled {
                            "任务已取消。"
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
                Ok(WorkerResult::Deployment(report)) => {
                    self.deployment_history.push(report.history.clone());
                    Notice {
                        text: format!(
                            "{}\n完成 {} 项；失败 {} 项；{}{}执行历史：{}",
                            report.summary,
                            report.completed,
                            report.failed,
                            if report.cancelled { "已取消。" } else { "" },
                            if report.restart_required {
                                "需要重启。"
                            } else {
                                ""
                            },
                            report.history.display()
                        ),
                        error: report.cancelled || report.failed > 0,
                    }
                }
                Ok(WorkerResult::Inspection {
                    settings,
                    deployment,
                    cancelled,
                }) => {
                    let count = settings.len() + deployment.len();
                    for state in settings {
                        self.setting_states
                            .insert(state.id, (state.state, state.detail));
                    }
                    for state in deployment {
                        self.deployment_states
                            .insert(state.id, (state.state, state.detail));
                    }
                    Notice {
                        text: format!(
                            "{}已返回 {count} 项检查状态。预览状态不代表读取了 Windows；检查不修改系统。",
                            if cancelled {
                                "检查已取消，仅显示已返回结果。"
                            } else {
                                "检查结束。"
                            }
                        ),
                        error: cancelled,
                    }
                }
                Err(error) => Notice {
                    text: error,
                    error: true,
                },
            });
            self.refresh_recovery();
        }
        if self.worker.is_some() {
            ctx.request_repaint_after(Duration::from_millis(120));
        }
    }

    fn start_apply(&mut self) {
        if !self.mutation_available() {
            self.notice = Some(Notice {
                text: "当前主机或目标不允许执行，请核对实际 Windows 版本。".into(),
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
                    .map_err(|_| "方案校验失败，请重新检查所选项目。")
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
                text: "当前主机不允许恢复 Windows 配置。".into(),
                error: true,
            });
            return;
        }
        self.start_worker("正在恢复配置", move |cancel| {
            studio_platform::restore(path, cancel).map(WorkerResult::Registry)
        });
        self.setting_states.clear();
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

    fn start_inspect(&mut self) {
        if self.worker.is_some() {
            return;
        }
        let plan = match self
            .catalog
            .as_ref()
            .map_err(|error| anyhow::anyhow!(error.clone()))
            .and_then(|catalog| studio_core::build_plan(catalog, self.target, &self.selected))
        {
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
        self.start_worker("正在只读检查所选项目", move |cancel| {
            let settings = studio_platform::inspect(plan, Arc::clone(&cancel))?;
            let deployment = if cancel.load(Ordering::Relaxed) {
                Vec::new()
            } else {
                studio_deployment::inspect(target, &selected, Arc::clone(&cancel))?
            };
            Ok(WorkerResult::Inspection {
                settings,
                deployment,
                cancelled: cancel.load(Ordering::Relaxed),
            })
        });
    }

    fn start_deploy(&mut self) {
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
        self.start_worker("正在执行部署 / 功能 / 维护", move |cancel| {
            studio_deployment::execute(plan, cancel, Some(sender)).map(WorkerResult::Deployment)
        });
        if let Some(worker) = &mut self.worker {
            worker.events = Some(receiver);
        }
        self.deployment_states.clear();
    }

    fn rail(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("navigation")
            .exact_size(212.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(Color32::WHITE)
                    .inner_margin(22.0)
                    .stroke(Stroke::new(1.0, LINE)),
            )
            .show(ui, |ui| {
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    logo(ui, 30.0);
                    ui.label(RichText::new("LTSC Studio").size(19.0).strong());
                });
                ui.add_space(8.0);
                ui.label(
                    RichText::new("让系统，回到你的习惯")
                        .size(12.0)
                        .color(MUTED),
                );
                ui.add_space(24.0);
                for (page, title, subtitle) in [
                    (Page::Workbench, "配置工作台", "按需选择系统设置"),
                    (Page::Software, "软件部署", "精确包 ID，按需安装"),
                    (Page::Features, "Windows 功能", "启用可选系统组件"),
                    (Page::Maintenance, "系统维护", "内置检查与维护命令"),
                    (Page::Plan, "变更方案", "检查每一处修改"),
                    (Page::Recovery, "恢复记录", "查看真实执行日志"),
                    (Page::Sources, "项目来源", "透明、可追溯的配置"),
                ] {
                    let active = self.page == page;
                    let response = ui.add_sized(
                        [168.0, 36.0],
                        egui::Button::new(RichText::new(title).strong().color(if active {
                            BLUE
                        } else {
                            INK
                        }))
                        .fill(if active { WASH } else { Color32::WHITE })
                        .stroke(Stroke::NONE)
                        .corner_radius(8),
                    );
                    if response.clicked() {
                        self.page = page;
                        self.query.clear();
                    }
                    ui.label(RichText::new(subtitle).size(11.0).color(MUTED));
                    ui.add_space(4.0);
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.label(RichText::new("离线方案 · 逐项选择").size(11.0).color(MUTED));
                    ui.label(RichText::new("每项变更，由你决定").size(12.0).color(INK));
                    ui.separator();
                });
            });
    }

    fn tray(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("selection-tray")
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(Color32::WHITE)
                    .stroke(Stroke::new(1.0, LINE))
                    .inner_margin(egui::Margin::symmetric(28, 16)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("已选择 {} 项", self.selection_count()))
                            .strong()
                            .size(17.0),
                    );
                    ui.label(
                        RichText::new(format!(
                            "设置 {} / 部署、功能、维护 {}",
                            self.selected.len(),
                            self.deployment_selected.len()
                        ))
                        .size(13.0)
                        .color(MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if primary(
                            ui,
                            "检查变更方案",
                            self.selection_count() > 0 && self.worker.is_none(),
                        )
                        .clicked()
                        {
                            self.page = Page::Plan;
                        }
                        if ui
                            .add_enabled(
                                self.selection_count() > 0 && self.worker.is_none(),
                                egui::Button::new("清空选择").frame(false),
                            )
                            .clicked()
                        {
                            self.selected.clear();
                            self.deployment_selected.clear();
                        }
                    });
                });
            });
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(match self.page {
                    Page::Workbench => "配置工作台",
                    Page::Software => "软件部署",
                    Page::Features => "Windows 功能",
                    Page::Maintenance => "系统维护",
                    Page::Plan => "变更方案",
                    Page::Recovery => "恢复记录",
                    Page::Sources => "项目来源",
                })
                .size(14.0)
                .color(MUTED),
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
                        if self.host.can_apply {
                            "本机执行"
                        } else {
                            "交互预览"
                        }
                    ),
                    WASH,
                    BLUE,
                );
            });
        });
        ui.add_space(14.0);
        if let Some(worker) = &self.worker {
            egui::Frame::new()
                .fill(WASH)
                .inner_margin(14.0)
                .corner_radius(8)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(worker.label);
                        ui.label(
                            RichText::new(
                                self.last_event
                                    .as_deref()
                                    .unwrap_or("等待当前操作返回，完成后显示真实结果"),
                            )
                            .size(12.0)
                            .color(MUTED),
                        );
                        let cancelling = worker.cancel.load(Ordering::Relaxed);
                        if ui
                            .add_enabled(
                                !cancelling,
                                egui::Button::new(if cancelling {
                                    "正在取消…"
                                } else {
                                    "取消任务"
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
        if let Some(notice) = &self.notice {
            let text = notice.text.clone();
            let error = notice.error;
            let mut dismiss = false;
            egui::Frame::new()
                .fill(if error {
                    Color32::from_rgb(255, 240, 241)
                } else {
                    WASH
                })
                .inner_margin(14.0)
                .corner_radius(8)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(text).size(13.0).color(if error {
                            RED
                        } else {
                            INK
                        }));
                        dismiss = ui.small_button("关闭提示").clicked();
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
            ui.label(RichText::new("方案目标").strong());
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
                        }
                    }
                });
            if ui
                .add_enabled(self.worker.is_none(), egui::Button::new("载入档案"))
                .clicked()
            {
                self.open_file(FileOperation::Load);
            }
            if ui
                .add_enabled(self.worker.is_none(), egui::Button::new("保存档案"))
                .clicked()
            {
                self.open_file(FileOperation::Save);
            }
            if ui
                .add_enabled(
                    self.worker.is_none() && self.selection_count() > 0,
                    egui::Button::new("检查所选项目状态"),
                )
                .clicked()
            {
                self.start_inspect();
            }
        });
        ui.label(
            RichText::new(
                if self.host.can_apply && self.host.target == Some(self.target) {
                    "已识别本机 Windows；变更前仍需检查方案并确认。"
                } else {
                    preview_explanation()
                },
            )
            .size(12.0)
            .color(MUTED),
        );
        ui.add_space(12.0);
    }

    fn selection_count(&self) -> usize {
        self.selected.len() + self.deployment_selected.len()
    }

    fn workbench(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(WASH)
            .corner_radius(10)
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width((ui.available_width() - 170.0).max(330.0));
                        ui.label(
                            RichText::new("系统设置，按你的习惯组合。")
                                .size(24.0)
                                .strong(),
                        );
                        ui.label(
                            RichText::new("分类查找、预设起步，逐项检查实际注册表变更。")
                                .size(13.0)
                                .color(MUTED),
                        );
                        ui.label(
                            RichText::new("所有项目默认不选；互斥选项自动替换。")
                                .size(12.0)
                                .color(BLUE),
                        );
                    });
                    device_illustration(ui);
                });
            });
        ui.add_space(14.0);
        self.target_controls(ui);
        let catalog = match &self.catalog {
            Ok(catalog) => catalog,
            Err(error) => {
                empty(ui, "无法载入配置目录", error);
                return;
            }
        };
        egui::CollapsingHeader::new(format!("配置组合 · {} 个可选起点", catalog.presets.len())).id_salt("presets").show(ui, |ui| {
            ui.label(RichText::new("选用组合会替换当前系统设置选择，软件 / 功能 / 维护选择保留。选用后仍可逐项调整。").size(12.0).color(MUTED));
            for preset in &catalog.presets {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&preset.title).strong());
                    ui.label(RichText::new(format!("{} 项 · {}", preset.actions.len(), preset.description)).size(12.0).color(MUTED));
                    if ui.add_enabled(self.worker.is_none(), egui::Button::new("检查并选用…").small()).clicked() { self.dialog = Some(Dialog::Preset(preset.id.clone())); }
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
                    "全部分类"
                } else {
                    category_label(&self.category)
                })
                .width(160.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.category, String::new(), "全部分类");
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
                    .hint_text("搜索设置、用途或 ID…")
                    .desired_width(330.0),
            );
            ui.label(
                RichText::new(format!("{} 项设置", catalog.actions.len()))
                    .size(12.0)
                    .color(MUTED),
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
                RichText::new(format!(
                    "{}  {} 项 / 已选 {}",
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
                    .fill(Color32::WHITE)
                    .stroke(Stroke::new(1.0, LINE))
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
                                    ui.label(RichText::new("目标不适用").size(11.0).color(MUTED));
                                }
                                if let Some((state, detail)) = self.setting_states.get(&action.id) {
                                    state_label(ui, state, detail);
                                }
                            });
                            ui.label(RichText::new(&action.description).size(12.0).color(MUTED));
                            egui::CollapsingHeader::new(
                                RichText::new("风险、来源与确切变更").size(11.0).color(BLUE),
                            )
                            .id_salt(("action-details", &action.id))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(format!(
                                        "风险：{}；生效：{}；ID：{}",
                                        action.risk, action.restart, action.id
                                    ))
                                    .size(12.0),
                                );
                                if let Some(group) = &action.choice_group {
                                    ui.label(
                                        RichText::new(format!(
                                            "互斥设置组：{group}。选择此项会移除组内其他已选项。"
                                        ))
                                        .size(12.0)
                                        .color(MUTED),
                                    );
                                }
                                registry_details(ui, action);
                                for (index, url) in action.evidence.iter().enumerate() {
                                    ui.hyperlink_to(
                                        RichText::new(format!("固定版本原始配置 {}", index + 1))
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
            empty(ui, "没有匹配的设置", "调整关键词或分类继续查找。");
        }
        if let Some((id, value)) = changed {
            select_setting(catalog, &mut self.selected, &id, value);
        }
    }

    fn deployment_page(&mut self, ui: &mut egui::Ui) {
        let (category, title, description) = match self.page {
            Page::Software => (
                "software",
                "软件部署",
                "精确的软件包 ID，由 WinGet 执行安装；安装器可能显示额外的许可与交互提示。",
            ),
            Page::Features => (
                "features",
                "Windows 功能",
                "通过 Windows 自身的可选功能机制启用组件，部分功能需要源文件与重启。",
            ),
            _ => (
                "maintenance",
                "系统维护",
                "逐项选择 Windows 内置诊断与维护命令。维护可能耗时，取消需等待当前系统操作返回。",
            ),
        };
        ui.heading(title);
        ui.label(RichText::new(description).size(13.0).color(MUTED));
        ui.label(
            RichText::new("这些操作不属于注册表恢复范围。执行历史是操作证据，不提供通用回滚。")
                .size(12.0)
                .color(RED),
        );
        ui.add_space(12.0);
        self.target_controls(ui);
        if self.page == Page::Software {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(if self.winget.available {
                        "WinGet 已识别；源、权限与许可要求在执行时再次检查。"
                    } else if cfg!(windows) {
                        "WinGet 尚不可用：需要微软应用安装程序及其执行别名。"
                    } else {
                        "WinGet 仅在 Windows 执行；这里可选择并导出软件部署方案。"
                    })
                    .size(12.0)
                    .color(if self.winget.available { BLUE } else { MUTED }),
                )
                .on_hover_text(&self.winget.detail);
                if ui
                    .add_enabled(self.worker.is_none(), egui::Button::new("刷新检测").small())
                    .clicked()
                {
                    self.winget = studio_deployment::winget_capability();
                }
            });
            egui::CollapsingHeader::new(RichText::new("查看 WinGet 检测详情").size(11.0))
                .id_salt("winget-details")
                .show(ui, |ui| {
                    ui.label(RichText::new(&self.winget.detail).size(12.0));
                });
        }
        ui.add(
            egui::TextEdit::singleline(&mut self.query)
                .hint_text("搜索名称、软件包 ID 或功能名…")
                .desired_width(450.0),
        );
        ui.add_space(8.0);
        let catalog = match &self.deployment_catalog {
            Ok(catalog) => catalog,
            Err(error) => {
                empty(ui, "无法载入部署目录", error);
                return;
            }
        };
        let query = self.query.trim().to_lowercase();
        let tasks: Vec<_> = catalog
            .tasks
            .iter()
            .filter(|task| {
                task.category == category
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
        if tasks.is_empty() {
            empty(ui, "没有匹配的项目", "调整关键词继续查找。");
            return;
        }
        egui::Frame::new()
            .fill(Color32::WHITE)
            .stroke(Stroke::new(1.0, LINE))
            .corner_radius(8)
            .inner_margin(14.0)
            .show(ui, |ui| {
                for (index, task) in tasks.iter().enumerate() {
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
                            ui.label(RichText::new("目标不适用").size(11.0).color(MUTED));
                        }
                        if let Some((state, detail)) = self.deployment_states.get(&task.id) {
                            state_label(ui, state, detail);
                        }
                    });
                    ui.label(RichText::new(&task.description).size(12.0).color(MUTED));
                    ui.label(RichText::new(task_identity(task)).size(12.0).color(BLUE));
                    egui::CollapsingHeader::new(
                        RichText::new("权限、限制与来源").size(11.0).color(BLUE),
                    )
                    .id_salt(("task-details", &task.id))
                    .show(ui, |ui| {
                        deployment_details(ui, task);
                    });
                }
            });
    }

    fn plan(&mut self, ui: &mut egui::Ui) {
        ui.heading("检查完整工作方案");
        ui.label(
            RichText::new(format!("目标：{}。导出不会修改本机。", self.target.label()))
                .color(MUTED),
        );
        ui.add_space(12.0);
        if self.selection_count() == 0 {
            empty(
                ui,
                "还没有选择项目",
                "从系统设置、软件部署、Windows 功能或系统维护中逐项选择。",
            );
            return;
        }
        let catalog = match &self.catalog {
            Ok(catalog) => catalog,
            Err(error) => {
                empty(ui, "设置目录不可用", error);
                return;
            }
        };
        let settings_plan = studio_core::build_plan(catalog, self.target, &self.selected);
        let deployment_plan = studio_deployment::build_plan(self.target, &self.deployment_selected);
        if let Err(error) = &settings_plan {
            ui.colored_label(RED, format!("系统设置校验失败：{error:#}"));
        }
        if let Err(error) = &deployment_plan {
            ui.colored_label(RED, format!("部署项目校验失败：{error:#}"));
        }
        let valid = settings_plan.is_ok() && deployment_plan.is_ok();
        ui.horizontal_wrapped(|ui| {
            if primary(ui, "导出完整 JSON 方案", valid && self.worker.is_none()).clicked() {
                self.open_file(FileOperation::Export);
            }
            if ui
                .add_enabled(self.worker.is_none(), egui::Button::new("检查所选项目状态"))
                .clicked()
            {
                self.start_inspect();
            }
            if ui
                .add_enabled(
                    self.mutation_available() && settings_plan.is_ok() && !self.selected.is_empty(),
                    egui::Button::new("应用系统设置…"),
                )
                .clicked()
            {
                self.dialog = Some(Dialog::Apply);
            }
            if ui
                .add_enabled(
                    self.mutation_available()
                        && deployment_plan.is_ok()
                        && !self.deployment_selected.is_empty(),
                    egui::Button::new("执行部署 / 功能 / 维护…"),
                )
                .clicked()
            {
                self.dialog = Some(Dialog::Deploy);
            }
        });
        ui.add_space(14.0);
        egui::CollapsingHeader::new(format!(
            "系统设置 · {} 项 · 原值留档、按日志恢复",
            self.selected.len()
        ))
        .default_open(true)
        .id_salt("plan-settings")
        .show(ui, |ui| {
            if let Ok(settings) = &settings_plan {
                for action in &settings.actions {
                    ui.label(RichText::new(&action.title).strong());
                    registry_details(ui, action);
                    ui.label(
                        RichText::new(format!("风险：{}；生效：{}", action.risk, action.restart))
                            .size(12.0)
                            .color(MUTED),
                    );
                    ui.separator();
                }
            }
        });
        egui::CollapsingHeader::new(format!(
            "部署 / 功能 / 维护 · {} 项 · 不属于注册表恢复范围",
            self.deployment_selected.len()
        ))
        .default_open(true)
        .id_salt("plan-deployment")
        .show(ui, |ui| {
            if let Ok(deployment) = &deployment_plan {
                for task in &deployment.tasks {
                    ui.label(RichText::new(&task.title).strong());
                    deployment_details(ui, task);
                    ui.separator();
                }
            }
        });
        ui.add_space(12.0);
        ui.label(RichText::new("两类执行分别确认。注册表恢复只覆盖日志中的原值；软件安装、功能启用及维护没有通用回滚。不会自动接受外部安装器的许可协议。").size(12.0).color(MUTED));
    }

    fn recovery(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("给每次变更，留一条回程路");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(self.worker.is_none(), egui::Button::new("刷新记录"))
                    .clicked()
                {
                    self.refresh_recovery();
                }
            });
        });
        ui.label(
            RichText::new("这里只显示实际生成的恢复日志。恢复会检查原值与当前值的冲突。")
                .color(MUTED),
        );
        if let Ok(directory) = studio_platform::data_directory() {
            ui.label(
                RichText::new(format!("日志目录：{}", directory.display()))
                    .size(12.0)
                    .color(MUTED),
            );
        }
        ui.add_space(24.0);
        if !self.deployment_history.is_empty() {
            ui.label(RichText::new("本次会话的部署执行历史").strong());
            ui.label(
                RichText::new("这些日志记录执行结果，不是可恢复的系统设置备份。")
                    .size(12.0)
                    .color(MUTED),
            );
            for path in &self.deployment_history {
                ui.label(RichText::new(path.display().to_string()).size(12.0));
            }
            ui.separator();
        }
        if let Some(error) = &self.recovery_error {
            empty(ui, "无法读取恢复记录", error);
            return;
        }
        if self.recovery.is_empty() {
            empty(
                ui,
                "还没有恢复记录",
                "在受支持的 Windows 主机应用配置后，真实的变更日志会出现在这里。",
            );
            return;
        }
        let enabled = self.mutation_available();
        for point in &self.recovery {
            egui::Frame::new()
                .fill(Color32::WHITE)
                .stroke(Stroke::new(1.0, LINE))
                .inner_margin(20.0)
                .corner_radius(8)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&point.id).strong());
                        badge(ui, &point.status, WASH, BLUE);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add_enabled(enabled, egui::Button::new("检查并恢复…"))
                                .clicked()
                            {
                                self.dialog = Some(Dialog::Restore(point.path.clone()));
                            }
                        });
                    });
                    ui.label(
                        RichText::new(format!("{} / {} 项配置", point.started_at, point.actions))
                            .color(MUTED),
                    );
                    ui.label(
                        RichText::new(point.path.display().to_string())
                            .size(12.0)
                            .color(MUTED),
                    );
                });
            ui.add_space(12.0);
        }
    }

    fn sources(&self, ui: &mut egui::Ui) {
        ui.heading("好的配置，也应说清来处");
        ui.label(
            RichText::new("配置由 Rust 目录逐项映射与校验；不会下载或执行上游脚本。").color(MUTED),
        );
        ui.add_space(24.0);
        let catalog = match &self.catalog {
            Ok(catalog) => catalog,
            Err(error) => {
                empty(ui, "目录不可用", error);
                return;
            }
        };
        for source in &catalog.sources {
            egui::Frame::new()
                .fill(Color32::WHITE)
                .stroke(Stroke::new(1.0, LINE))
                .inner_margin(22.0)
                .corner_radius(8)
                .show(ui, |ui| {
                    ui.hyperlink_to(RichText::new(&source.name).strong().size(18.0), &source.url);
                    ui.label(RichText::new(&source.description).size(14.0).color(MUTED));
                    ui.add_space(8.0);
                    ui.label(RichText::new(format!("固定版本：{}", source.revision)).size(12.0));
                    ui.label(
                        RichText::new(format!("许可证 / 使用范围：{}", source.license))
                            .size(12.0)
                            .color(MUTED),
                    );
                });
            ui.add_space(14.0);
        }
        ui.label(
            RichText::new(format!("配置目录版本：{}", catalog.revision))
                .size(12.0)
                .color(MUTED),
        );
        if let Ok(deployment) = &self.deployment_catalog {
            ui.label(
                RichText::new(format!(
                    "部署目录版本：{} / {} 项任务",
                    deployment.revision,
                    deployment.tasks.len()
                ))
                .size(12.0)
                .color(MUTED),
            );
        }
        ui.add_space(12.0);
        ui.label(
            RichText::new(
                "系统设置映射固定版本来源；部署项目逐项显示软件包、功能或内置命令及限制。软件本身遵循各自许可证，安装器许可需由你决定。",
            )
            .size(13.0)
            .color(MUTED),
        );
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.take() else {
            return;
        };
        let mut close = false;
        let mut apply = false;
        let mut deploy = false;
        let mut preset = None;
        let mut restore = None;
        let response = egui::Modal::new(egui::Id::unique("studio-dialog"))
            .frame(egui::Frame::new().fill(Color32::WHITE).stroke(Stroke::new(1.0, LINE)).corner_radius(14).inner_margin(26.0))
            .show(ctx, |ui| {
                ui.set_width(520.0);
                match &mut dialog {
                    Dialog::File { operation, path, error } => {
                        ui.heading(operation.title());
                        ui.label(RichText::new(if matches!(operation, FileOperation::Load) { "输入现有 JSON 档案的完整路径。载入会替换当前选择。" } else { "输入新文件的完整路径。为保护现有文件，已存在的路径不会被覆盖。" }).color(MUTED));
                        ui.add_space(12.0);
                        ui.add(egui::TextEdit::singleline(path).desired_width(f32::INFINITY).hint_text("完整文件路径，例如 /Users/you/Desktop/lt-sc.json"));
                        if let Some(error) = error.as_ref() { ui.colored_label(RED, error); }
                        ui.add_space(14.0);
                        ui.horizontal(|ui| {
                            if primary(ui, operation.title(), !path.trim().is_empty()).clicked() {
                                match self.file_operation(*operation, PathBuf::from(path.trim())) {
                                    Ok(message) => { self.notice = Some(Notice { text: message, error: false }); close = true; }
                                    Err(message) => *error = Some(message),
                                }
                            }
                            if ui.button("取消").clicked() { close = true; }
                        });
                    }
                    Dialog::Apply => {
                        ui.heading("确认在这台 Windows 上应用");
                        ui.label(format!("实际主机：{}", self.host.name));
                        ui.label(format!("方案目标：{}", self.target.label()));
                        ui.label(format!("共 {} 项配置，以下是将写入的确切值：", self.selected.len()));
                        egui::ScrollArea::vertical().id_salt("confirmation-plan").max_height(230.0).show(ui, |ui| {
                            if let Ok(catalog) = &self.catalog {
                                for action in catalog.actions.iter().filter(|action| self.selected.contains(&action.id)) {
                                    ui.label(RichText::new(&action.title).strong());
                                    for change in &action.changes {
                                        let hive = match change.hive { Hive::CurrentUser => "HKCU", Hive::LocalMachine => "HKLM" };
                                        ui.label(RichText::new(format!("{hive}\\{}\\{} = {} (DWORD)", change.path, change.name, change.value)).size(12.0));
                                    }
                                    ui.separator();
                                }
                            }
                        });
                        ui.add_space(10.0);
                        ui.label(RichText::new("每项修改前保存原始值，修改后核对。失败或取消可能留下部分已完成的变更，请按真实报告与恢复日志处理。").color(MUTED));
                        ui.add_space(16.0);
                        ui.horizontal(|ui| {
                            if primary(ui, "确认应用所选配置", self.mutation_available() && !self.selected.is_empty()).clicked() { apply = true; close = true; }
                            if ui.button("返回检查").clicked() { close = true; }
                        });
                    }
                    Dialog::Deploy => {
                        ui.heading("确认执行部署 / 功能 / 维护");
                        ui.label(format!("实际主机：{}", self.host.name));
                        ui.label(format!("方案目标：{}", self.target.label()));
                        let plan = studio_deployment::build_plan(self.target, &self.deployment_selected);
                        let valid = plan.is_ok();
                        egui::ScrollArea::vertical().id_salt("confirm-deployment").max_height(260.0).show(ui, |ui| {
                            match &plan {
                                Ok(plan) => for task in &plan.tasks { ui.label(RichText::new(&task.title).strong()); deployment_details(ui, task); ui.separator(); },
                                Err(error) => { ui.colored_label(RED, format!("{error:#}")); }
                            }
                        });
                        ui.label(RichText::new("这些操作不能由注册表恢复日志撤销。可能需要管理员权限、网络、系统源文件或重启；取消只能在系统允许的边界生效，已经完成的操作仍会保留。").color(RED));
                        ui.label(RichText::new("不会自动接受外部安装器的许可协议。软件安装器可能显示交互提示，请自行检查并决定。").size(12.0).color(MUTED));
                        ui.horizontal(|ui| {
                            if primary(ui, "确认执行以上任务", self.mutation_available() && valid && !self.deployment_selected.is_empty()).clicked() { deploy = true; close = true; }
                            if ui.button("返回检查").clicked() { close = true; }
                        });
                    }
                    Dialog::Preset(id) => {
                        ui.heading("选用配置组合");
                        if let Ok(catalog) = &self.catalog && let Some(item) = catalog.presets.iter().find(|item| &item.id == id) {
                            ui.label(RichText::new(&item.title).strong());
                            ui.label(&item.description);
                            let valid = studio_core::build_plan(catalog, self.target, &item.actions);
                            egui::ScrollArea::vertical().id_salt("preset-review").max_height(230.0).show(ui, |ui| {
                                for action in catalog.actions.iter().filter(|action| item.actions.contains(&action.id)) { ui.label(&action.title); }
                            });
                            ui.label(RichText::new(format!("将以这 {} 项替换当前 {} 项系统设置选择；部署、功能及维护选择保留。不会立即修改系统。", item.actions.len(), self.selected.len())).color(MUTED));
                            if let Err(error) = &valid { ui.colored_label(RED, format!("此组合不适合当前目标：{error:#}")); }
                            if primary(ui, "确认替换系统设置选择", valid.is_ok() && self.worker.is_none()).clicked() { preset = Some(item.actions.clone()); close = true; }
                        } else { ui.colored_label(RED, "配置组合已不可用。"); }
                        if ui.button("取消").clicked() { close = true; }
                    }
                    Dialog::Restore(path) => {
                        ui.heading("确认恢复这条记录");
                        ui.label(format!("实际主机：{}", self.host.name));
                        ui.label(path.display().to_string());
                        ui.label(RichText::new("恢复只处理这条日志记录的注册表值。若当前值在执行后被其他程序修改，冲突检查会阻止覆盖。").color(MUTED));
                        ui.add_space(16.0);
                        ui.horizontal(|ui| {
                            if primary(ui, "确认检查并恢复", self.mutation_available()).clicked() { restore = Some(path.clone()); close = true; }
                            if ui.button("取消").clicked() { close = true; }
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
            return Err("请输入完整的绝对路径。".into());
        }
        let catalog = self.catalog.as_ref().map_err(Clone::clone)?;
        match operation {
            FileOperation::Export => {
                let plan = workspace_io::plan(
                    catalog,
                    self.target,
                    &self.selected,
                    &self.deployment_selected,
                )
                .map_err(|e| format!("{e:#}"))?;
                studio_core::export_document(&plan, &path).map_err(|e| format!("{e:#}"))?;
                Ok(format!("方案已导出：{}", path.display()))
            }
            FileOperation::Save => {
                workspace_io::save(
                    catalog,
                    self.target,
                    &self.selected,
                    &self.deployment_selected,
                    &path,
                )
                .map_err(|e| format!("{e:#}"))?;
                Ok(format!("配置档案已保存：{}", path.display()))
            }
            FileOperation::Load => {
                let profile = workspace_io::load(&path, catalog).map_err(|e| format!("{e:#}"))?;
                self.target = profile.settings.target;
                self.selected = profile.settings.selected;
                self.deployment_selected = profile.deployment;
                self.setting_states.clear();
                self.deployment_states.clear();
                Ok(format!(
                    "已载入 {} 项配置：{}",
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
        self.poll_worker(&ctx);
        if ctx.input(|input| input.viewport().close_requested()) && self.worker.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if let Some(worker) = &self.worker {
                worker.cancel.store(true, Ordering::Relaxed);
            }
            self.notice = Some(Notice {
                text: "已请求取消。请等待当前操作返回并查看报告，然后关闭窗口。".into(),
                error: false,
            });
        }
        self.rail(ui);
        self.tray(ui);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(CANVAS).inner_margin(28.0))
            .show(ui, |ui| {
                self.header(ui);
                egui::ScrollArea::vertical()
                    .id_salt("main-content")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        match self.page {
                            Page::Workbench => self.workbench(ui),
                            Page::Software | Page::Features | Page::Maintenance => {
                                self.deployment_page(ui)
                            }
                            Page::Plan => self.plan(ui),
                            Page::Recovery => self.recovery(ui),
                            Page::Sources => self.sources(ui),
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
                    "LTSC Studio worker panicked during shutdown; inspect recovery journals."
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
        "privacy" => "隐私",
        "explorer" => "资源管理器",
        "experience" => "桌面体验",
        "start" => "开始菜单",
        "taskbar" => "任务栏",
        "personalization" => "个性化",
        "gaming" => "游戏",
        "accessibility" => "辅助功能",
        "system" => "系统",
        "updates" => "更新偏好",
        other => other,
    }
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
        "aligned" => "符合方案",
        "different" => "与方案不同",
        "unavailable" => "无法检查",
        "preview" => "预览，未检查本机 Windows",
        "installed" => "已安装",
        "not_installed" => "未安装",
        "enabled" => "已启用",
        "disabled" => "未启用",
        "enable_pending" => "等待重启启用",
        "ready" => "可执行",
        "error" => "检查失败",
        other => other,
    };
    ui.label(
        RichText::new(format!("上次检查：{label}"))
            .size(11.0)
            .color(if matches!(state, "error" | "unavailable") {
                RED
            } else {
                BLUE
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
                    "(默认)"
                } else {
                    &change.name
                },
                change.value
            ))
            .size(12.0)
            .color(BLUE),
        );
    }
}

fn task_identity(task: &studio_deployment::Task) -> String {
    if let Some(id) = &task.package_id {
        format!("WinGet 软件包：{id}")
    } else if let Some(name) = &task.feature_name {
        format!("Windows 功能：{name}")
    } else {
        format!("内置维护命令：{} ({})", task.id, task.kind)
    }
}

fn deployment_details(ui: &mut egui::Ui, task: &studio_deployment::Task) {
    ui.label(RichText::new(&task.description).size(12.0).color(MUTED));
    ui.label(RichText::new(task_identity(task)).size(12.0).color(BLUE));
    ui.label(
        RichText::new(format!(
            "{}；生效：{}",
            if task.requires_admin {
                "需要管理员权限"
            } else {
                "以当前用户执行，安装器可能要求提升权限"
            },
            task.restart
        ))
        .size(12.0),
    );
    ui.label(RichText::new(&task.limitations).size(12.0).color(MUTED));
    for (index, evidence) in task.evidence.iter().enumerate() {
        ui.hyperlink_to(
            RichText::new(format!("来源与执行依据 {}", index + 1)).size(12.0),
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
        "当前系统"
    }
}

fn preview_explanation() -> &'static str {
    if cfg!(target_os = "windows") {
        "当前 Windows 版本不支持执行。可以选择、检查并导出方案。"
    } else {
        "当前为预览模式：可选择、检查并导出方案；应用与恢复需在受支持的 Windows 上进行。"
    }
}

fn primary(ui: &mut egui::Ui, title: &str, enabled: bool) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(title).color(Color32::WHITE).strong())
            .fill(BLUE)
            .stroke(Stroke::NONE)
            .corner_radius(7),
    )
}

fn badge(ui: &mut egui::Ui, text: &str, fill: Color32, ink: Color32) {
    egui::Frame::new()
        .fill(fill)
        .corner_radius(5)
        .inner_margin(egui::Margin::symmetric(8, 5))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.0).color(ink));
        });
}

fn empty(ui: &mut egui::Ui, title: &str, description: &str) {
    egui::Frame::new()
        .fill(Color32::WHITE)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(10)
        .inner_margin(30.0)
        .show(ui, |ui| {
            ui.add_space(10.0);
            ui.label(RichText::new(title).size(21.0).strong());
            ui.add_space(8.0);
            ui.label(RichText::new(description).size(14.0).color(MUTED));
            ui.add_space(10.0);
        });
}

fn logo(ui: &mut egui::Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    let gap = 3.0;
    let pane = (size - gap) / 2.0;
    for (x, y, color) in [
        (0.0, 0.0, BLUE),
        (pane + gap, 0.0, Color32::from_rgb(101, 139, 245)),
        (0.0, pane + gap, Color32::from_rgb(101, 139, 245)),
        (pane + gap, pane + gap, BLUE),
    ] {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(rect.min + Vec2::new(x, y), Vec2::splat(pane)),
            2.0,
            color,
        );
    }
}

fn device_illustration(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(142.0, 86.0), egui::Sense::hover());
    let p = ui.painter();
    let monitor = egui::Rect::from_min_size(rect.min, Vec2::new(126.0, 68.0));
    p.rect_filled(
        monitor.translate(Vec2::new(5.0, 5.0)),
        7.0,
        Color32::from_rgb(212, 224, 252),
    );
    p.rect_filled(monitor, 7.0, Color32::from_rgb(36, 57, 103));
    p.rect_filled(monitor.shrink(5.0), 3.0, Color32::WHITE);
    p.rect_filled(
        egui::Rect::from_min_size(rect.min + Vec2::new(5.0, 5.0), Vec2::new(26.0, 58.0)),
        2.0,
        Color32::from_rgb(222, 232, 255),
    );
    for (y, width) in [(18.0, 60.0), (34.0, 48.0), (50.0, 54.0)] {
        p.rect_filled(
            egui::Rect::from_min_size(rect.min + Vec2::new(40.0, y), Vec2::new(width, 5.0)),
            2.0,
            if y == 18.0 { BLUE } else { LINE },
        );
    }
    p.rect_filled(
        egui::Rect::from_min_size(rect.min + Vec2::new(55.0, 68.0), Vec2::new(16.0, 10.0)),
        1.0,
        Color32::from_rgb(139, 161, 207),
    );
    p.line_segment(
        [
            rect.min + Vec2::new(41.0, 79.0),
            rect.min + Vec2::new(84.0, 79.0),
        ],
        Stroke::new(4.0, Color32::from_rgb(139, 161, 207)),
    );
    p.circle_filled(rect.min + Vec2::new(121.0, 64.0), 16.0, BLUE);
    p.line_segment(
        [
            rect.min + Vec2::new(114.0, 64.0),
            rect.min + Vec2::new(120.0, 70.0),
        ],
        Stroke::new(2.5, Color32::WHITE),
    );
    p.line_segment(
        [
            rect.min + Vec2::new(120.0, 70.0),
            rect.min + Vec2::new(130.0, 58.0),
        ],
        Stroke::new(2.5, Color32::WHITE),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preview() -> Studio {
        Studio {
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
            setting_states: BTreeMap::new(),
            deployment_states: BTreeMap::new(),
            last_event: None,
            deployment_history: Vec::new(),
            page: Page::Workbench,
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
        assert!(!app.mutation_available());
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
    fn relative_export_path_is_rejected_before_any_write() {
        let mut app = preview();
        let result = app.file_operation(FileOperation::Export, PathBuf::from("relative.json"));
        assert!(result.unwrap_err().contains("绝对路径"));
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
}
