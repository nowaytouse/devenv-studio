//! Native desktop workbench. Every mutation goes through the platform journal engine.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use eframe::egui::{self, Color32, FontId, RichText, Stroke, Vec2};
use image::ImageEncoder;
use studio_core::{Catalog, Hive, Profile, Target};
use studio_platform::{HostInfo, RecoveryPoint, RunReport};

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
    Restore(PathBuf),
}

struct Worker {
    cancel: Arc<AtomicBool>,
    receiver: mpsc::Receiver<Result<RunReport, String>>,
    thread: Option<JoinHandle<()>>,
    label: &'static str,
}

struct Notice {
    text: String,
    error: bool,
}

struct Studio {
    catalog: Result<Catalog, String>,
    host: HostInfo,
    target: Target,
    selected: BTreeSet<String>,
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
            host,
            target,
            selected: BTreeSet::new(),
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
                Ok(report) => Notice {
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
            studio_platform::apply(plan, cancel)
        });
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
            studio_platform::restore(path, cancel)
        });
    }

    fn start_worker<F>(&mut self, label: &'static str, operation: F)
    where
        F: FnOnce(Arc<AtomicBool>) -> anyhow::Result<RunReport> + Send + 'static,
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
            thread: Some(thread),
            label,
        });
        self.notice = None;
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
                ui.add_space(42.0);
                for (page, title, subtitle) in [
                    (Page::Workbench, "配置工作台", "按需选择系统设置"),
                    (Page::Plan, "变更方案", "检查每一处修改"),
                    (Page::Recovery, "恢复记录", "查看真实执行日志"),
                    (Page::Sources, "项目来源", "透明、可追溯的配置"),
                ] {
                    let active = self.page == page;
                    let response = ui.add_sized(
                        [168.0, 45.0],
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
                    }
                    ui.label(RichText::new(subtitle).size(11.0).color(MUTED));
                    ui.add_space(12.0);
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
                        RichText::new(format!("已选择 {} 项", self.selected.len()))
                            .strong()
                            .size(17.0),
                    );
                    ui.label(
                        RichText::new(if self.selected.is_empty() {
                            "从一项适合你的设置开始"
                        } else {
                            "选择保留至本次会话结束，可保存为配置档案"
                        })
                        .size(13.0)
                        .color(MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if primary(
                            ui,
                            "检查变更方案",
                            !self.selected.is_empty() && self.worker.is_none(),
                        )
                        .clicked()
                        {
                            self.page = Page::Plan;
                        }
                        if ui
                            .add_enabled(
                                !self.selected.is_empty() && self.worker.is_none(),
                                egui::Button::new("清空选择").frame(false),
                            )
                            .clicked()
                        {
                            self.selected.clear();
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
                            RichText::new("等待当前操作返回，完成后显示真实结果")
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

    fn workbench(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(WASH)
            .corner_radius(14)
            .inner_margin(20.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width((ui.available_width() - 220.0).max(340.0));
                        ui.label(
                            RichText::new("一个顺手的 Windows，\n从你的选择开始。")
                                .size(26.0)
                                .strong(),
                        );
                        ui.add_space(5.0);
                        ui.label(
                            RichText::new("专注隐私、资源管理器与桌面体验，先看清变更再应用。")
                                .size(14.0)
                                .color(MUTED),
                        );
                        ui.add_space(7.0);
                        ui.horizontal(|ui| {
                            badge(ui, "默认零勾选", Color32::WHITE, BLUE);
                            badge(ui, "原值留档 · 可恢复", Color32::WHITE, BLUE);
                        });
                    });
                    device_illustration(ui);
                });
            });
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("目标系统").strong());
            egui::ComboBox::from_id_salt("target-system")
                .selected_text(short_target(self.target))
                .width(260.0)
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
                        }
                    }
                });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
            });
        });
        if !self.host.can_apply {
            ui.label(RichText::new(preview_explanation()).size(12.0).color(MUTED));
        } else if self.host.target != Some(self.target) {
            ui.label(
                RichText::new("所选目标与当前 Windows 不一致，仅可检查和导出方案。")
                    .size(12.0)
                    .color(RED),
            );
        }
        ui.add_space(16.0);
        let catalog = match &self.catalog {
            Ok(catalog) => catalog,
            Err(error) => {
                empty(ui, "无法载入配置目录", error);
                return;
            }
        };
        ui.horizontal(|ui| {
            for (id, title) in [
                ("", "全部配置"),
                ("privacy", "隐私"),
                ("explorer", "资源管理器"),
                ("experience", "桌面体验"),
            ] {
                let count = catalog
                    .actions
                    .iter()
                    .filter(|action| id.is_empty() || action.category == id)
                    .count();
                if ui
                    .selectable_label(
                        self.category == id,
                        RichText::new(format!("{title}  {count}")).color(if self.category == id {
                            Color32::WHITE
                        } else {
                            INK
                        }),
                    )
                    .clicked()
                {
                    self.category = id.into();
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text("搜索配置、用途…")
                        .desired_width(220.0),
                );
            });
        });
        ui.add_space(14.0);
        let query = self.query.trim().to_lowercase();
        let actions: Vec<_> = catalog
            .actions
            .iter()
            .filter(|action| {
                (self.category.is_empty() || action.category == self.category)
                    && (query.is_empty()
                        || format!("{} {} {}", action.title, action.description, action.id)
                            .to_lowercase()
                            .contains(&query))
            })
            .collect();
        if actions.is_empty() {
            empty(ui, "没有匹配的配置", "试试其他关键词，或切换到全部配置。");
            return;
        }
        egui::Frame::new()
            .fill(Color32::WHITE)
            .stroke(Stroke::new(1.0, LINE))
            .corner_radius(10)
            .inner_margin(egui::Margin::symmetric(20, 4))
            .show(ui, |ui| {
                for (index, action) in actions.iter().enumerate() {
                    if index > 0 {
                        ui.separator();
                    }
                    ui.add_space(11.0);
                    let supported = action.targets.contains(&self.target);
                    let mut selected = self.selected.contains(&action.id);
                    ui.horizontal(|ui| {
                        let changed = ui
                            .add_enabled(
                                supported && self.worker.is_none(),
                                egui::Checkbox::new(
                                    &mut selected,
                                    RichText::new(&action.title).strong().size(16.0),
                                ),
                            )
                            .changed();
                        if changed {
                            if selected {
                                self.selected.insert(action.id.clone());
                            } else {
                                self.selected.remove(&action.id);
                            }
                        }
                        if !supported {
                            badge(ui, "目标系统不适用", CANVAS, MUTED);
                        } else if selected {
                            badge(ui, "已加入方案", WASH, BLUE);
                        }
                    });
                    ui.indent(&action.id, |ui| {
                        ui.label(RichText::new(&action.description).size(13.0).color(MUTED));
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new(format!("风险：{}", action.risk))
                                    .size(11.0)
                                    .color(MUTED),
                            );
                            ui.label(
                                RichText::new(format!("生效：{}", action.restart))
                                    .size(11.0)
                                    .color(MUTED),
                            );
                            for source_id in &action.sources {
                                if let Some(source) = catalog
                                    .sources
                                    .iter()
                                    .find(|source| &source.id == source_id)
                                {
                                    ui.hyperlink_to(
                                        RichText::new(&source.name).size(11.0),
                                        &source.url,
                                    )
                                    .on_hover_text(format!(
                                        "固定版本：{}\n许可证：{}",
                                        source.revision, source.license
                                    ));
                                }
                            }
                            for (index, evidence) in action.evidence.iter().enumerate() {
                                ui.hyperlink_to(
                                    RichText::new(format!("原始配置 {}", index + 1)).size(11.0),
                                    evidence,
                                );
                            }
                        });
                    });
                    ui.add_space(11.0);
                }
            });
    }

    fn plan(&mut self, ui: &mut egui::Ui) {
        ui.heading("每一处变更，都有据可查");
        ui.label(
            RichText::new(format!(
                "目标：{}。导出方案不会修改本机设置。",
                self.target.label()
            ))
            .color(MUTED),
        );
        ui.add_space(18.0);
        let catalog = match &self.catalog {
            Ok(catalog) => catalog,
            Err(error) => {
                empty(ui, "目录不可用", error);
                return;
            }
        };
        if self.selected.is_empty() {
            empty(
                ui,
                "还没有选择配置",
                "返回配置工作台，选择你需要的项目。所有设置默认关闭。",
            );
            if primary(ui, "前往配置工作台", true).clicked() {
                self.page = Page::Workbench;
            }
            return;
        }
        let plan_result = studio_core::build_plan(catalog, self.target, &self.selected);
        if let Err(error) = &plan_result {
            ui.colored_label(RED, format!("方案校验失败：{error:#}"));
        }
        for action in catalog
            .actions
            .iter()
            .filter(|action| self.selected.contains(&action.id))
        {
            egui::Frame::new()
                .fill(Color32::WHITE)
                .stroke(Stroke::new(1.0, LINE))
                .inner_margin(18.0)
                .corner_radius(8)
                .show(ui, |ui| {
                    ui.label(RichText::new(&action.title).strong().size(16.0));
                    ui.label(RichText::new(&action.description).size(13.0).color(MUTED));
                    ui.add_space(5.0);
                    for change in &action.changes {
                        let hive = match change.hive {
                            Hive::CurrentUser => "HKEY_CURRENT_USER",
                            Hive::LocalMachine => "HKEY_LOCAL_MACHINE",
                        };
                        ui.label(
                            RichText::new(format!("{hive}\\{}", change.path))
                                .family(egui::FontFamily::Monospace)
                                .size(12.0),
                        );
                        ui.label(
                            RichText::new(format!(
                                "{} = {}  (REG_DWORD)",
                                if change.name.is_empty() {
                                    "(默认)"
                                } else {
                                    &change.name
                                },
                                change.value
                            ))
                            .size(13.0)
                            .color(BLUE),
                        );
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "风险：{} / 生效：{}",
                                action.risk, action.restart
                            ))
                            .size(12.0)
                            .color(MUTED),
                        );
                        for source in catalog
                            .sources
                            .iter()
                            .filter(|source| action.sources.contains(&source.id))
                        {
                            ui.hyperlink_to(RichText::new(&source.name).size(12.0), &source.url);
                        }
                        for (index, evidence) in action.evidence.iter().enumerate() {
                            ui.hyperlink_to(
                                RichText::new(format!("上游原始配置 {}", index + 1)).size(12.0),
                                evidence,
                            );
                        }
                    });
                });
            ui.add_space(10.0);
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if primary(
                ui,
                "导出 JSON 方案",
                plan_result.is_ok() && self.worker.is_none(),
            )
            .clicked()
            {
                self.open_file(FileOperation::Export);
            }
            if ui
                .add_enabled(
                    self.mutation_available() && plan_result.is_ok(),
                    egui::Button::new("在本机应用…"),
                )
                .clicked()
            {
                self.dialog = Some(Dialog::Apply);
            }
        });
        ui.add_space(10.0);
        ui.label(
            RichText::new(if self.host.can_apply {
                "应用前会再次确认。原始值先写入恢复日志，写入后核对结果。"
            } else {
                preview_explanation()
            })
            .size(12.0)
            .color(MUTED),
        );
        ui.label(
            RichText::new(
                "恢复记录仅覆盖已记录的注册表值，不是完整系统备份；恢复时会检查后续外部修改。",
            )
            .size(12.0)
            .color(MUTED),
        );
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
        ui.add_space(12.0);
        ui.label(
            RichText::new(
                "本版专注可恢复的系统设置。软件部署、驱动更新、商店修复与深度精简尚未集成。",
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
                let plan = studio_core::build_plan(catalog, self.target, &self.selected)
                    .map_err(|e| format!("{e:#}"))?;
                studio_core::export_plan(&plan, &path).map_err(|e| format!("{e:#}"))?;
                Ok(format!("方案已导出：{}", path.display()))
            }
            FileOperation::Save => {
                let profile = Profile {
                    schema_version: 1,
                    target: self.target,
                    selected: self.selected.clone(),
                };
                studio_core::save_profile(&profile, &path).map_err(|e| format!("{e:#}"))?;
                Ok(format!("配置档案已保存：{}", path.display()))
            }
            FileOperation::Load => {
                let profile =
                    studio_core::load_profile(&path, catalog).map_err(|e| format!("{e:#}"))?;
                self.target = profile.target;
                self.selected = profile.selected;
                Ok(format!(
                    "已载入 {} 项配置：{}",
                    self.selected.len(),
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
    let (rect, _) = ui.allocate_exact_size(Vec2::new(190.0, 150.0), egui::Sense::hover());
    let p = ui.painter();
    let monitor =
        egui::Rect::from_min_size(rect.min + Vec2::new(4.0, 5.0), Vec2::new(170.0, 112.0));
    p.rect_filled(
        monitor.translate(Vec2::new(6.0, 8.0)),
        10.0,
        Color32::from_rgb(212, 224, 252),
    );
    p.rect_filled(monitor, 9.0, Color32::from_rgb(36, 57, 103));
    let screen = monitor.shrink(6.0);
    p.rect_filled(screen, 5.0, Color32::from_rgb(247, 250, 255));
    let sidebar = egui::Rect::from_min_size(screen.min, Vec2::new(37.0, screen.height()));
    p.rect_filled(sidebar, 3.0, Color32::from_rgb(222, 232, 255));
    for y in [25.0, 42.0, 59.0] {
        p.line_segment(
            [
                screen.min + Vec2::new(9.0, y),
                screen.min + Vec2::new(28.0, y),
            ],
            Stroke::new(3.0, Color32::from_rgb(145, 172, 239)),
        );
    }
    for (y, width) in [(20.0, 81.0), (42.0, 65.0), (64.0, 75.0)] {
        p.rect_filled(
            egui::Rect::from_min_size(screen.min + Vec2::new(49.0, y), Vec2::new(width, 7.0)),
            3.0,
            if y == 20.0 {
                BLUE
            } else {
                Color32::from_rgb(207, 219, 247)
            },
        );
        p.circle_filled(
            screen.min + Vec2::new(141.0, y + 3.0),
            5.0,
            if y == 20.0 {
                BLUE
            } else {
                Color32::from_rgb(207, 219, 247)
            },
        );
    }
    p.rect_filled(
        egui::Rect::from_min_size(rect.min + Vec2::new(77.0, 117.0), Vec2::new(25.0, 15.0)),
        2.0,
        Color32::from_rgb(139, 161, 207),
    );
    p.rect_filled(
        egui::Rect::from_min_size(rect.min + Vec2::new(54.0, 130.0), Vec2::new(72.0, 5.0)),
        3.0,
        Color32::from_rgb(115, 143, 201),
    );
    p.circle_filled(rect.min + Vec2::new(169.0, 110.0), 20.0, BLUE);
    p.line_segment(
        [
            rect.min + Vec2::new(160.0, 110.0),
            rect.min + Vec2::new(167.0, 117.0),
        ],
        Stroke::new(3.0, Color32::WHITE),
    );
    p.line_segment(
        [
            rect.min + Vec2::new(167.0, 117.0),
            rect.min + Vec2::new(179.0, 103.0),
        ],
        Stroke::new(3.0, Color32::WHITE),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preview() -> Studio {
        Studio {
            catalog: Err("test catalog unavailable".into()),
            host: HostInfo {
                name: "Test preview".into(),
                target: None,
                can_apply: false,
                explanation: "Preview only".into(),
            },
            target: Target::Win11Ltsc2024,
            selected: BTreeSet::new(),
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
        assert!(!app.mutation_available());
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
            label: "test",
        });
        drop(app);
        assert!(cancel.load(Ordering::Acquire));
        assert!(finished.load(Ordering::Acquire));
    }
}
