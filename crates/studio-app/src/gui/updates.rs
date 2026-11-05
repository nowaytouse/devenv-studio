use super::*;
use maintenance::{brew_inventory, windows_inventory};

mod brew;
mod windows;

pub(super) type History = Result<Vec<maintenance::Report>, String>;
pub(super) type BrewInventory = Result<brew_inventory::Inventory, String>;
pub(super) type WindowsInventory = Result<windows_inventory::Inventory, String>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Tasks,
    Installed,
    Terminal,
    History,
}

impl Default for Section {
    fn default() -> Self {
        if cfg!(windows) {
            Self::Installed
        } else {
            Self::Tasks
        }
    }
}

#[derive(Default)]
pub(super) struct UiState {
    section: Section,
    check: Option<maintenance::Check>,
    selected: BTreeSet<String>,
    catalog_query: String,
    review: Option<maintenance::Review>,
    confirmed: bool,
    latest: Option<maintenance::Report>,
    history: Option<History>,
    focus_result: bool,
    brew: brew::View,
    windows: windows::View,
}

impl UiState {
    pub(super) fn invalidate_check(&mut self) {
        self.check = None;
        self.selected.clear();
        self.clear_review();
    }

    fn clear_review(&mut self) {
        self.review = None;
        self.confirmed = false;
    }

    fn select_step(&mut self, id: &str, selected: bool) {
        if !self.check.as_ref().is_some_and(|check| {
            check.available
                && check
                    .steps
                    .iter()
                    .any(|step| step.id == id && step.available && !step.terminal)
        }) {
            return;
        }
        let changed = if selected {
            self.selected.insert(id.into())
        } else {
            self.selected.remove(id)
        };
        if changed {
            self.clear_review();
        }
    }

    fn can_review(&self, busy: bool) -> bool {
        !busy
            && !self.selected.is_empty()
            && self.check.as_ref().is_some_and(|check| check.available)
    }

    fn matches_review(&self, steps: &[maintenance::Step]) -> bool {
        steps.len() == self.selected.len()
            && steps.iter().all(|step| step.available && !step.terminal)
            && steps
                .iter()
                .map(|step| step.id.as_str())
                .collect::<BTreeSet<_>>()
                == self.selected.iter().map(String::as_str).collect()
    }

    fn can_run(&self, busy: bool) -> bool {
        self.can_review(busy)
            && self.confirmed
            && self
                .review
                .as_ref()
                .is_some_and(|review| self.matches_review(&review.steps))
    }

    pub(super) fn accept_check(
        &mut self,
        check: Result<maintenance::Check, String>,
        history: History,
        cancelled: bool,
    ) -> Notice {
        self.invalidate_check();
        if cancelled {
            return cancelled_notice();
        }
        self.history = Some(history);
        match check {
            Ok(check) => {
                let notice = Notice {
                    text: if check.available {
                        tx("更新入口已核验", "Update capability verified").into()
                    } else {
                        check.detail.clone()
                    },
                    error: !check.available,
                };
                self.check = Some(check);
                notice
            }
            Err(error) => Notice {
                text: error,
                error: true,
            },
        }
    }

    pub(super) fn accept_review(
        &mut self,
        review: Result<maintenance::Review, String>,
        cancelled: bool,
    ) -> Notice {
        self.clear_review();
        if cancelled {
            return cancelled_notice();
        }
        match review {
            Ok(review)
                if self.check.as_ref().and_then(|check| check.engine.as_ref())
                    != Some(&review.engine) =>
            {
                self.invalidate_check();
                Notice {
                    text: tx(
                        "引擎在检查后已切换。请重新检查并选择步骤。",
                        "The engine changed after the check. Check again and select steps.",
                    )
                    .into(),
                    error: true,
                }
            }
            Ok(review) if self.can_review(false) && self.matches_review(&review.steps) => {
                self.review = Some(review);
                Notice {
                    text: tx(
                        "已生成审核范围。核对步骤后，请单独确认执行。",
                        "Review is ready. Check the steps and confirm execution separately.",
                    )
                    .into(),
                    error: false,
                }
            }
            Ok(_) => Notice {
                text: tx(
                    "审核返回的步骤与当前选择不一致。请重新检查并选择。",
                    "Reviewed steps differ from the current selection. Check and select again.",
                )
                .into(),
                error: true,
            },
            Err(error) => Notice {
                text: error,
                error: true,
            },
        }
    }

    pub(super) fn accept_run(
        &mut self,
        report: Result<maintenance::Report, String>,
        history: History,
    ) -> Notice {
        self.history = Some(history);
        match report {
            Ok(report) => {
                self.section = Section::History;
                self.focus_result = true;
                let notice = Notice {
                    text: run_status_label(&report.status).into(),
                    error: !matches!(report.status, maintenance::RunStatus::Verified),
                };
                self.latest = Some(report);
                notice
            }
            Err(error) => Notice {
                text: error,
                error: true,
            },
        }
    }

    pub(super) fn accept_history(&mut self, history: History, cancelled: bool) -> Notice {
        if cancelled {
            return cancelled_notice();
        }
        let notice = match &history {
            Ok(_) => Notice {
                text: tx("已读取本地执行记录。", "Local run history loaded.").into(),
                error: false,
            },
            Err(error) => Notice {
                text: error.clone(),
                error: true,
            },
        };
        self.history = Some(history);
        notice
    }

    pub(super) fn accept_brew(&mut self, inventory: BrewInventory, cancelled: bool) -> Notice {
        if cancelled {
            return cancelled_notice();
        }
        self.brew.accept(inventory)
    }

    pub(super) fn accept_windows(
        &mut self,
        inventory: WindowsInventory,
        cancelled: bool,
    ) -> Notice {
        if cancelled {
            let notice = cancelled_notice();
            self.windows.accept(Err(notice.text.clone()));
            return notice;
        }
        self.windows.accept(inventory)
    }
}

impl Studio {
    fn start_update_check(&mut self) {
        if !cfg!(target_os = "macos") || self.worker.is_some() {
            return;
        }
        self.updates.invalidate_check();
        self.start_worker(
            tx("正在检查更新入口", "Checking update capability"),
            |cancel| {
                let check = maintenance::check(cancel).map_err(|error| format!("{error:#}"));
                Ok(WorkerResult::UpdateCheck(check, read_history()))
            },
        );
    }

    fn start_update_review(&mut self) {
        if !cfg!(target_os = "macos") || !self.updates.can_review(self.worker.is_some()) {
            return;
        }
        self.updates.clear_review();
        let selected = self.updates.selected.clone();
        self.start_worker(
            tx("正在核对所选更新", "Reviewing selected updates"),
            move |cancel| {
                let review =
                    maintenance::review(&selected, cancel).map_err(|error| format!("{error:#}"));
                Ok(WorkerResult::UpdateReview(review))
            },
        );
    }

    fn start_update_run(&mut self) {
        if !cfg!(target_os = "macos") || !self.updates.can_run(self.worker.is_some()) {
            return;
        }
        let Some(review) = self.updates.review.take() else {
            return;
        };
        self.updates.invalidate_check();
        self.updates.latest = None;
        let (sender, receiver) = mpsc::channel();
        self.start_worker(
            tx("正在更新所选开发工具", "Updating selected developer tools"),
            move |cancel| {
                let report = maintenance::run(review, cancel, Some(sender))
                    .map_err(|error| format!("{error:#}"));
                Ok(WorkerResult::UpdateRun(report, read_history()))
            },
        );
        if let Some(worker) = &mut self.worker {
            worker.events = Some(receiver);
        }
    }

    pub(super) fn updates_page(&mut self, ui: &mut egui::Ui) {
        if !cfg!(any(target_os = "macos", windows)) {
            ui.label(tx(
                "更新中心在 macOS 或 Windows 本机可用。",
                "The update center is available on macOS and Windows.",
            ));
            return;
        }

        let busy = self.worker.is_some();
        if std::mem::take(&mut self.updates.focus_result) {
            ui.scroll_to_cursor(Some(egui::Align::Min));
        }
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(
                &mut self.updates.section,
                Section::Tasks,
                tx("更新任务", "Update tasks"),
            );
            ui.selectable_value(
                &mut self.updates.section,
                Section::Installed,
                if cfg!(windows) {
                    tx("Windows 已安装", "Windows installed")
                } else {
                    tx("Homebrew 已安装", "Homebrew installed")
                },
            );
            ui.selectable_value(
                &mut self.updates.section,
                Section::History,
                tx("执行记录", "Run history"),
            );
            if cfg!(target_os = "macos") {
                ui.selectable_value(
                    &mut self.updates.section,
                    Section::Terminal,
                    tx("终端维护", "Terminal maintenance"),
                );
            }
        });
        ui.add_space(8.0);
        match self.updates.section {
            Section::Tasks if cfg!(windows) => self.windows_tasks_view(ui, busy),
            Section::Tasks => self.update_tasks_view(ui, busy),
            Section::Terminal => self.terminal_tasks_view(ui, busy),
            Section::Installed if cfg!(windows) => {
                if self.updates.windows.show(ui, busy) {
                    self.start_worker(
                        tx("正在读取 WinGet 清单", "Reading WinGet inventory"),
                        |cancel| {
                            let result = windows_inventory::read(cancel)
                                .map_err(|error| format!("{error:#}"));
                            Ok(WorkerResult::WindowsInventory(result))
                        },
                    );
                }
            }
            Section::Installed => {
                if self.updates.brew.show(ui, busy) {
                    self.start_worker(
                        tx("正在读取 Homebrew 清单", "Reading Homebrew inventory"),
                        |cancel| {
                            let result =
                                brew_inventory::read(cancel).map_err(|error| format!("{error:#}"));
                            Ok(WorkerResult::BrewInventory(result))
                        },
                    );
                }
            }
            Section::History if cfg!(windows) => {
                if ui
                    .add_enabled(!busy, egui::Button::new(tx("刷新记录", "Refresh history")))
                    .clicked()
                {
                    self.refresh_recovery();
                }
                self.history_view(ui);
            }
            Section::History => {
                if ui
                    .add_enabled(!busy, egui::Button::new(tx("刷新记录", "Refresh history")))
                    .clicked()
                {
                    self.start_worker(
                        tx("正在读取执行记录", "Reading run history"),
                        |_| Ok(WorkerResult::UpdateHistory(read_history())),
                    );
                }
                self.updates.history_view(ui);
            }
        }
    }

    fn windows_tasks_view(&mut self, ui: &mut egui::Ui, busy: bool) {
        ui.heading(tx("配置与维护 Windows", "Configure and maintain Windows"));
        ui.label(tx(
            "从已安装清单核对软件版本及单包升级命令。系统设置、软件部署和维护使用各自的审核方案。",
            "Review installed versions and per-package update commands in the inventory. Settings, software deployment, and maintenance use their own reviewed plans.",
        ));
        ui.add_space(8.0);
        surface_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(lf!("方案目标 · {}", "Plan target · {}", self.target.label()));
            ui.label(if self.host.can_apply && self.host.target == Some(self.target) {
                tx(
                    "本机与方案目标匹配。应用前仍需分别审核及确认。",
                    "This host matches the plan target. Review and confirm changes before applying.",
                )
            } else {
                tx(
                    "当前主机与方案目标不匹配或不支持执行。仍可预览和导出。",
                    "The selected target is unsupported or does not match this host. Preview and export remain available.",
                )
            });
        });
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            for (page, label) in [
                (Page::Software, tx("软件部署", "Software deployment")),
                (Page::Workbench, tx("配置工作台", "Configuration workbench")),
                (Page::Maintenance, tx("系统维护", "System maintenance")),
            ] {
                if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                    self.navigate_to(page);
                }
            }
        });
    }

    fn update_tasks_view(&mut self, ui: &mut egui::Ui, busy: bool) {
        ui.heading(tx("软件与工具更新", "Software and tool updates"));
        ui.label(tx(
            "先检查本机可用任务，再按需选择。检查只读取本地状态。",
            "Check which tasks are available on this Mac, then choose what to review. This check reads local state.",
        ));
        if primary(
            ui,
            tx("检查更新能力与历史", "Check capability and history"),
            !busy,
        )
        .clicked()
        {
            self.start_update_check();
        }
        let mut selection_change = None;
        if let Some(check) = &self.updates.check {
            if !check.available {
                ui.colored_label(palette().danger, &check.detail);
            }
            let title = check.engine.as_ref().map_or_else(
                || tx("引擎与回退", "Engine and rollback").to_owned(),
                |engine| {
                    lf!(
                        "引擎与回退 · Topgrade {} · {}",
                        "Engine and rollback · Topgrade {} · {}",
                        engine.upstream_version,
                        short_id(&engine.binary_sha256)
                    )
                },
            );
            egui::CollapsingHeader::new(title)
                .id_salt("update-engine")
                .show(ui, |ui| engine_card(ui, check, busy));
            ui.add_space(12.0);
            if check.steps.iter().any(|step| !step.terminal) {
                ui.horizontal_wrapped(|ui| {
                    ui.heading(tx("可在 Studio 执行", "Run in Studio"));
                    let count = check
                        .steps
                        .iter()
                        .filter(|step| check.available && step.available && !step.terminal)
                        .count();
                    badge(
                        ui,
                        &lf!("{} 项可审核", "{} available to review", count),
                        palette().wash,
                        palette().ink,
                    );
                });
                ui.label(tx(
                    "选择后审核。任务可用性检查不查询上游更新。",
                    "Select tasks for review. The availability check does not query upstream updates.",
                ));
                for (group, diagnostics) in [
                    (tx("工具与缓存", "Tools and caches"), false),
                    (tx("只读诊断", "Read-only diagnostics"), true),
                ] {
                    let steps = check
                        .steps
                        .iter()
                        .filter(|step| !step.terminal && diagnostic_step(&step.id) == diagnostics);
                    if steps.clone().next().is_none() {
                        continue;
                    }
                    surface_frame().inner_margin(12.0).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.strong(group);
                        for (index, step) in steps.enumerate() {
                            if index > 0 {
                                ui.separator();
                            }
                            ui.push_id(&step.id, |ui| {
                                let mut selected = self.updates.selected.contains(&step.id);
                                ui.horizontal(|ui| {
                                    task_mark(ui, &step.id);
                                    ui.vertical(|ui| {
                                        ui.horizontal_wrapped(|ui| {
                                            if ui
                                                .add_enabled(
                                                    !busy && check.available && step.available,
                                                    egui::Checkbox::new(
                                                        &mut selected,
                                                        step_label(&step.id, &step.label),
                                                    ),
                                                )
                                                .changed()
                                            {
                                                selection_change =
                                                    Some((step.id.clone(), selected));
                                            }
                                            if !check.available || !step.available {
                                                badge(
                                                    ui,
                                                    tx("不可用", "Unavailable"),
                                                    palette().wash,
                                                    palette().ink,
                                                );
                                            }
                                        });
                                        ui.label(
                                            RichText::new(step_detail(step)).color(palette().muted),
                                        );
                                    });
                                });
                            });
                        }
                    });
                    ui.add_space(6.0);
                }
            }
        }
        if self.updates.check.is_none() && !busy {
            ui.label(
                RichText::new(tx(
                    "检查后可看到当前引擎、可用任务和上一次运行。",
                    "Check to see the current engine, available tasks, and the last run.",
                ))
                .color(palette().muted),
            );
        }
        if let Some((id, selected)) = selection_change {
            self.updates.select_step(&id, selected);
        }

        let review_title = if self.updates.selected.is_empty() {
            tx("审核所选更新", "Review selected updates").to_owned()
        } else {
            lf!(
                "审核所选 {} 项",
                "Review {} selected",
                self.updates.selected.len()
            )
        };
        if primary(ui, &review_title, self.updates.can_review(busy)).clicked() {
            self.start_update_review();
        }
        let mut run = false;
        if let Some(review) = &self.updates.review {
            let eligible =
                self.updates.can_review(busy) && self.updates.matches_review(&review.steps);
            surface_frame().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.heading(tx("即将执行的步骤", "Steps to run"));
                engine_summary(ui, &review.engine);
                for step in &review.steps {
                    ui.strong(step_label(&step.id, &step.label));
                    ui.label(RichText::new(step_detail(step)).color(palette().muted));
                    ui.label(
                        RichText::new(operation_detail(&step.id))
                            .monospace()
                            .color(palette().muted),
                    );
                    ui.add_space(4.0);
                }
                ui.label(tx(
                    "取消后，已完成的变更会保留。",
                    "Completed changes remain after cancellation.",
                ));
                ui.add_enabled(
                    !busy,
                    egui::Checkbox::new(
                        &mut self.updates.confirmed,
                        tx(
                            "我已核对以上步骤，确认执行。",
                            "I reviewed these steps and confirm execution.",
                        ),
                    ),
                );
                run = primary(
                    ui,
                    tx("执行已审核的更新", "Run reviewed updates"),
                    eligible && self.updates.confirmed,
                )
                .clicked();
            });
        }
        if run {
            self.start_update_run();
        }
    }

    fn terminal_tasks_view(&mut self, ui: &mut egui::Ui, busy: bool) {
        if let Some(check) = &self.updates.check
            && !check.steps.is_empty()
        {
            terminal_catalog(ui, &check.steps, &mut self.updates.catalog_query, busy);
            ui.separator();
            ui.heading(tx("在终端执行完整更新", "Run the full update in Terminal"));
            ui.label(tx(
                "命令使用当前 Topgrade 配置，范围与上方选择独立。请在 Studio 空闲时运行，在终端核对范围并处理提示或管理员授权。",
                "The command uses the current Topgrade configuration independently of the selection above. Run it when Studio is idle. Review the scope and handle prompts or administrator authorization in Terminal.",
            ));
            if ui
                .add_enabled(
                    !busy,
                    egui::Button::new(tx("复制终端命令", "Copy Terminal command")),
                )
                .clicked()
            {
                ui.ctx().copy_text(maintenance::TERMINAL_COMMAND.to_owned());
            }
        } else {
            ui.heading(tx("终端维护", "Terminal maintenance"));
            ui.label(tx(
                "先检查本机，再查看常用任务及其预演命令。",
                "Check this Mac to view featured tasks and their preview commands.",
            ));
            if primary(ui, tx("检查终端入口", "Check Terminal options"), !busy).clicked() {
                self.start_update_check();
            }
        }
    }
}

fn diagnostic_step(id: &str) -> bool {
    matches!(id, "brew_linkage_audit" | "cli_versions")
}

fn task_mark(ui: &mut egui::Ui, id: &str) {
    let mark = match id {
        "brew_linkage_audit" => "HB",
        "cli_versions" => "CLI",
        "github_cli_extensions" | "github_cli_skills" => "GH",
        "helm" => "H",
        "tldr" => "TL",
        _ => "TG",
    };
    let (rect, _) = ui.allocate_exact_size(egui::vec2(36.0, 36.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 8, palette().wash);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        mark,
        egui::FontId::monospace(12.0),
        palette().ink,
    );
}

impl UiState {
    fn history_view(&self, ui: &mut egui::Ui) {
        ui.separator();
        if let Some(report) = &self.latest {
            ui.heading(tx("本次执行结果", "Current run result"));
            report_details(ui, report);
        }
        ui.heading(tx("更新执行记录", "Update run history"));
        match &self.history {
            None => {
                ui.label(tx(
                    "刷新后显示本地执行记录。",
                    "Refresh to read local run history.",
                ));
            }
            Some(Err(error)) => {
                ui.colored_label(palette().danger, error);
            }
            Some(Ok(reports)) if reports.is_empty() => {
                ui.label(tx("尚无本地更新记录。", "No local update runs recorded."));
            }
            Some(Ok(reports)) => {
                for report in reports {
                    if self
                        .latest
                        .as_ref()
                        .is_some_and(|latest| latest.run_id == report.run_id)
                    {
                        continue;
                    }
                    let status = run_status_label(&report.status);
                    let title = match &report.engine {
                        Some(engine) => format!(
                            "{status} · Topgrade {} / {} · {}",
                            engine.upstream_version,
                            short_id(&engine.binary_sha256),
                            report.run_id
                        ),
                        None => format!("{status} · {}", report.run_id),
                    };
                    egui::CollapsingHeader::new(title)
                        .id_salt(("update-history", &report.run_id))
                        .show(ui, |ui| report_details(ui, report));
                }
            }
        }
    }
}

fn read_history() -> History {
    maintenance::history().map_err(|error| format!("{error:#}"))
}

fn cancelled_notice() -> Notice {
    Notice {
        text: tx(
            "检查或审核已取消，请重新检查或审核。",
            "Check or review cancelled. Check or review again.",
        )
        .into(),
        error: true,
    }
}

fn engine_card(ui: &mut egui::Ui, check: &maintenance::Check, busy: bool) {
    surface_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.heading(tx("当前更新引擎", "Current update engine"));
        let Some(engine) = &check.engine else {
            ui.label(tx("当前引擎身份未核验。", "The current engine identity is unverified."));
            return;
        };
        engine_summary(ui, engine);
        ui.label(if check.available {
            tx(
                "DevEnv Studio 受管构建 · 本次已核验",
                "DevEnv Studio managed build · Verified in this check",
            )
        } else {
            tx("查看上方检查结果，确认当前配置状态。", "See the check result above for the current profile status.")
        });
        engine_details(ui, tx("当前引擎详情", "Current engine details"), engine);
        ui.separator();
        ui.strong(tx("上一版回退候选", "Previous rollback candidate"));
        if let Some(previous) = &check.rollback {
            engine_summary(ui, previous);
            engine_details(ui, tx("回退候选详情", "Rollback candidate details"), previous);
            ui.horizontal_wrapped(|ui| {
                if let Ok(exe) = std::env::current_exe()
                    && let Some(command) =
                        rollback_command(&exe, &engine.component_tree, &previous.component_tree)
                {
                    if ui.add_enabled(!busy, egui::Button::new(tx(
                        "复制终端回退命令",
                        "Copy Terminal rollback command",
                    ))).clicked() {
                        ui.ctx().copy_text(command);
                    }
                    ui.label(tx(
                        "终端会核对两版引擎及配置，并验证恢复结果。",
                        "Terminal rechecks both engines and the profile, then verifies what was restored.",
                    ));
                } else {
                    ui.label(tx("无法生成本机回退命令。", "The local rollback command is unavailable."));
                }
            });
        } else {
            ui.label(tx("暂无已核验的回退候选。", "No verified rollback candidate."));
        }
        if let Some(detail) = &check.rollback_detail {
            ui.label(detail);
        }
    });
}

fn engine_summary(ui: &mut egui::Ui, engine: &maintenance::EngineIdentity) {
    ui.horizontal_wrapped(|ui| {
        ui.strong(lf!("Topgrade {}", "Topgrade {}", engine.upstream_version));
        ui.label(lf!(
            "组件 {}",
            "Component {}",
            short_id(&engine.component_tree)
        ));
        ui.label(lf!("产物 {}", "Binary {}", short_id(&engine.binary_sha256)));
        ui.label(lf!(
            "配置清单 v{} · {}",
            "Profile manifest v{} · {}",
            engine.profile_version,
            short_id(&engine.profile_sha256)
        ));
    });
}

fn short_id(id: &str) -> &str {
    id.get(..12).unwrap_or(id)
}

fn engine_details(ui: &mut egui::Ui, title: &str, engine: &maintenance::EngineIdentity) {
    egui::CollapsingHeader::new(title)
        .id_salt(&engine.component_tree)
        .show(ui, |ui| {
            let details = [
                tx(
                    "来源 · DevEnv Studio 受管构建",
                    "Source · DevEnv Studio managed build",
                )
                .to_owned(),
                lf!("Topgrade {}", "Topgrade {}", engine.upstream_version),
                lf!(
                    "Studio 构建提交 · {}",
                    "Studio build commit · {}",
                    engine.studio_commit
                ),
                lf!("组件树 · {}", "Component tree · {}", engine.component_tree),
                lf!("二进制 · {}", "Binary · {}", engine.binary.display()),
                lf!(
                    "二进制 SHA-256 · {}",
                    "Binary SHA-256 · {}",
                    engine.binary_sha256
                ),
                lf!(
                    "配置清单 v{} · SHA-256 {}",
                    "Profile manifest v{} · SHA-256 {}",
                    engine.profile_version,
                    engine.profile_sha256
                ),
            ]
            .join("\n");
            ui.label(RichText::new(&details).monospace());
            if ui
                .button(tx("复制引擎身份", "Copy engine identity"))
                .clicked()
            {
                ui.ctx().copy_text(details);
            }
        });
}

fn rollback_command(exe: &std::path::Path, active: &str, previous: &str) -> Option<String> {
    let valid_tree = |tree: &str| {
        tree.len() == 40
            && tree
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    };
    let path = exe.to_str()?;
    if !cfg!(target_os = "macos")
        || !exe.is_absolute()
        || path.chars().any(char::is_control)
        || active == previous
        || !valid_tree(active)
        || !valid_tree(previous)
    {
        return None;
    }
    Some(format!(
        "'{}' topgrade-engine rollback-current {active} {previous}",
        path.replace('\'', "'\"'\"'")
    ))
}

fn report_details(ui: &mut egui::Ui, report: &maintenance::Report) {
    let colors = palette();
    let color = if report.status == maintenance::RunStatus::Verified {
        colors.accent
    } else {
        colors.danger
    };
    ui.colored_label(color, run_status_label(&report.status));
    if report.status == maintenance::RunStatus::Verified {
        ui.label(tx(
            "所选步骤均返回成功。是否实际更新，请核对步骤输出及更新后的状态。",
            "All selected steps returned success. Check the output and resulting state to confirm what changed.",
        ));
    } else {
        ui.label(&report.detail);
    }
    if let Some(engine) = &report.engine {
        engine_summary(ui, engine);
        engine_details(ui, tx("执行引擎详情", "Run engine details"), engine);
    } else if let Some(tree) = &report.engine_tree {
        ui.label(lf!(
            "旧记录 · 组件树 {}",
            "Earlier record · Component tree {}",
            tree
        ));
    }
    if let Some(previous) = &report.rollback {
        ui.label(tx("当时的回退候选", "Rollback candidate at the time"));
        engine_summary(ui, previous);
        engine_details(
            ui,
            tx("历史回退候选详情", "Historical rollback candidate details"),
            previous,
        );
    }
    for step in &report.steps {
        let status = match step.status {
            maintenance::StepStatus::Success => tx("完成", "Completed"),
            maintenance::StepStatus::Failed => tx("失败", "Failed"),
            maintenance::StepStatus::Skipped => tx("已跳过", "Skipped"),
            maintenance::StepStatus::TimedOut => tx("超时", "Timed out"),
        };
        ui.label(format!("{} · {status}", step_label(&step.id, &step.id)));
    }
    ui.label(lf!(
        "本地记录：{}",
        "Local record: {}",
        report.path.display()
    ));
    let output_id = ui.scope_id().with(("topgrade-run-output", &report.run_id));
    let output = ui
        .ctx()
        .data(|data| data.get_temp::<Result<String, String>>(output_id));
    if let Some(output) = output {
        if ui.button(tx("收起执行输出", "Hide run output")).clicked() {
            ui.ctx()
                .data_mut(|data| data.remove::<Result<String, String>>(output_id));
        } else {
            match output {
                Ok(output) if output.is_empty() => {
                    ui.label(tx("此记录没有输出。", "This record has no output."));
                }
                Ok(output) => {
                    egui::ScrollArea::vertical()
                        .id_salt(output_id)
                        .max_height(220.0)
                        .show(ui, |ui| {
                            ui.label(RichText::new(output).monospace());
                        });
                }
                Err(error) => {
                    ui.colored_label(colors.danger, error);
                }
            }
        }
    } else if ui.button(tx("查看执行输出", "Show run output")).clicked() {
        let output = maintenance::output_excerpt(report).map_err(|error| format!("{error:#}"));
        ui.ctx()
            .data_mut(|data| data.insert_temp(output_id, output));
    }
}

fn run_status_label(status: &maintenance::RunStatus) -> &'static str {
    match status {
        maintenance::RunStatus::Verified => tx("步骤成功 · 退出 0", "Steps succeeded · Exit 0"),
        maintenance::RunStatus::Failed => tx("执行失败", "Run failed"),
        maintenance::RunStatus::Cancelled => tx("执行已取消", "Run cancelled"),
        maintenance::RunStatus::TimedOut => tx("执行超时", "Run timed out"),
        maintenance::RunStatus::Incomplete => tx("执行结果未完整核验", "Run result incomplete"),
    }
}

fn step_label<'a>(id: &str, fallback: &'a str) -> &'a str {
    match id {
        "brew_linkage_audit" => tx("Homebrew 链接审计", "Homebrew linkage audit"),
        "cli_versions" => tx("命令行工具版本", "CLI tool versions"),
        "github_cli_extensions" => tx("GitHub CLI 扩展", "GitHub CLI extensions"),
        "github_cli_skills" => tx("GitHub CLI 技能", "GitHub CLI skills"),
        "helm" => tx("Helm 仓库", "Helm repositories"),
        "tldr" => tx("TLDR 命令手册", "TLDR command pages"),
        _ => fallback,
    }
}

fn step_detail(step: &maintenance::Step) -> &str {
    if !step.detail.is_empty() {
        return &step.detail;
    }
    match step.id.as_str() {
        "brew_linkage_audit" => tx(
            "检查缺失动态库和损坏的依赖关系。",
            "Check for missing libraries and broken dependencies.",
        ),
        "cli_versions" => tx(
            "读取实际命令版本和 Homebrew 当前链接的版本。",
            "Read executable versions and the active Homebrew kegs.",
        ),
        "github_cli_extensions" => tx(
            "更新已安装的 GitHub CLI 扩展。",
            "Update installed GitHub CLI extensions.",
        ),
        "github_cli_skills" => tx(
            "核对来源及锁记录后更新由 GitHub CLI 管理的技能。",
            "Verify origins and lock records before updating GitHub CLI skills.",
        ),
        "helm" => tx(
            "核验或刷新现有 Helm 仓库索引。",
            "Verify or refresh existing Helm repository indexes.",
        ),
        "tldr" => tx(
            "刷新 TLDR 手册缓存。使用多语言缓存时，请先在 tealdeer 中明确配置下载语言。",
            "Refresh the TLDR cache. If you keep multiple languages, configure tealdeer's download languages first.",
        ),
        _ => "",
    }
}

fn operation_detail(id: &str) -> &'static str {
    match id {
        "brew_linkage_audit" => "brew linkage",
        "cli_versions" => tx(
            "读取工具的版本和 Homebrew 活跃 keg",
            "Probe tool versions and active Homebrew kegs",
        ),
        "github_cli_extensions" => "gh extension upgrade --all",
        "github_cli_skills" => tx(
            "gh skill list · gh api · 有可安全应用的更新时运行 gh skill update",
            "gh skill list · gh api · gh skill update when safe updates are available",
        ),
        "helm" => tx(
            "helm repo list · 校验缓存 · 必要时运行 helm repo update",
            "helm repo list · verify cache · helm repo update when needed",
        ),
        "tldr" => "tldr --update",
        _ => "",
    }
}

fn terminal_preview(id: &str) -> Option<String> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return None;
    }
    Some(format!(
        "{} --dry-run --no-self-update --no-tmux --show-skipped --only {id}",
        maintenance::TERMINAL_COMMAND
    ))
}

fn terminal_catalog(
    ui: &mut egui::Ui,
    steps: &[maintenance::Step],
    query: &mut String,
    busy: bool,
) {
    let count = steps.iter().filter(|step| step.terminal).count();
    ui.heading(tx("终端维护", "Terminal maintenance"));
    ui.label(tx(
        "复制命令前先核对范围。预演会使用本机配置及其前后置命令。",
        "Review the scope before copying a command. Previews use the local configuration and its pre/post commands.",
    ));
    surface_frame().inner_margin(12.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.strong(tx("常用维护入口", "Featured maintenance"));
        for (name, description) in [
            (
                "BetterDisplay Internal Update",
                tx(
                    "应用更新 · 先核对版本、签名与运行状态。",
                    "App update · Check the version, signature, and running state first.",
                ),
            ),
            (
                "Homebrew Formulae (HEAD-synced)",
                tx(
                    "公式更新 · 核对 LLVM 固定版本、HEAD 构建范围及恢复状态。",
                    "Formula updates · Check the LLVM pin, HEAD build scope, and recovery state.",
                ),
            ),
        ] {
            ui.push_id(name, |ui| {
                ui.separator();
                ui.strong(name);
                ui.label(RichText::new(description).color(palette().muted));
                let label = lf!("复制 {} 预演命令", "Copy {} preview command", name);
                copy_profile_preview(ui, name, &label, busy);
            });
        }
    });
    ui.add_space(8.0);
    egui::CollapsingHeader::new(format!("{} ({count})", tx("Topgrade 步骤目录", "Topgrade step catalog")))
        .id_salt("topgrade-step-catalog")
        .show(ui, |ui| {
            ui.label(tx(
                "这里列出引擎提供的其余步骤。终端预演使用当前配置，可能包括前后置命令。是否适用于本机以终端结果为准。",
                "These are the engine's remaining steps. A terminal preview uses the current configuration, including any pre/post commands. Check applicability in Terminal.",
            ));
            ui.add(egui::TextEdit::singleline(query).hint_text(tx("搜索步骤，例如 brew、rustup、mas", "Search steps, e.g. brew, rustup, mas")));
            let query = query.trim().to_ascii_lowercase();
            egui::ScrollArea::vertical()
                .id_salt("topgrade-terminal-steps")
                .max_height(320.0)
                .show(ui, |ui| {
                    for step in steps.iter().filter(|step| {
                        step.terminal && (step.id.contains(&query) || step.label.to_ascii_lowercase().contains(&query))
                    }) {
                        ui.push_id(&step.id, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(&step.label);
                                ui.label(RichText::new(&step.id).monospace().color(palette().muted));
                                if let Some(command) = terminal_preview(&step.id)
                                    && ui.add_enabled(!busy, egui::Button::new(tx("复制预演命令", "Copy preview command"))).clicked()
                                {
                                    ui.ctx().copy_text(command);
                                }
                            });
                        });
                    }
                });
        });

    egui::CollapsingHeader::new(tx("本机 profile 的托管任务", "Managed local profile tasks"))
        .id_salt("topgrade-managed-profile")
        .show(ui, |ui| {
            ui.label(tx(
                "以下任务沿用本机 profile 的前置检查和恢复机制。可在终端查看各项预演。",
                "These tasks use the local profile's checks and recovery. Preview each task in Terminal.",
            ));
            for name in [
                "Mac App Store (verified)", "gcloud (verified)",
                "SQLite Python trunk", "SQLite trunk", "TourBox Beta Update", "Xcode Beta Update",
                "Preview macOS Apps", "Conda frozen base",
                "Conda default", "Gradle Nightly Toolchain", "Rolling Project Dependencies",
                "npm Bundled Dependency Security", "Topgrade Skills Coverage Gate",
            ] {
                ui.push_id(name, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(name);
                        let label = lf!("复制 {} 预演命令", "Copy {} preview command", name);
                        copy_profile_preview(ui, name, &label, busy);
                    });
                });
            }
        });
}

fn copy_profile_preview(ui: &mut egui::Ui, name: &str, label: &str, busy: bool) {
    if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
        ui.ctx().copy_text(format!(
            "{} --dry-run --no-self-update --no-tmux --show-skipped --only custom_commands --custom-commands '{name}'",
            maintenance::TERMINAL_COMMAND
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn copied_rollback_pins_both_trees_and_quotes_the_app_path() {
        let executable = std::path::Path::new("/Applications/Studio's App.app/bin/studio");
        let active = "a".repeat(40);
        let previous = "b".repeat(40);
        let command = rollback_command(executable, &active, &previous).unwrap();
        assert_eq!(
            command,
            format!(
                "'/Applications/Studio'\"'\"'s App.app/bin/studio' topgrade-engine rollback-current {active} {previous}"
            )
        );
        assert!(rollback_command(executable, &active, &active).is_none());
        assert!(rollback_command(executable, "a; touch /tmp/example", &previous).is_none());
        assert!(rollback_command(std::path::Path::new("studio"), &active, &previous).is_none());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn copied_rollback_is_unavailable_on_unsupported_hosts() {
        let executable = std::env::current_exe().unwrap();
        assert!(rollback_command(&executable, &"a".repeat(40), &"b".repeat(40)).is_none());
    }

    #[test]
    fn featured_terminal_tasks_only_offer_preview_and_respect_busy_state() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(650.0, 520.0),
                )),
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    terminal_catalog(ui, &[], &mut String::new(), true);
                });
            },
        );
        output.textures_delta.clear();
        let nodes = output.platform_output.accesskit_update.unwrap().nodes;
        for name in [
            "BetterDisplay Internal Update",
            "Homebrew Formulae (HEAD-synced)",
        ] {
            let button = nodes
                .iter()
                .find(|(_, node)| {
                    node.role() == egui::accesskit::Role::Button
                        && node.label().is_some_and(|label| {
                            label.contains(name)
                                && (label.contains("预演命令") || label.contains("preview command"))
                        })
                })
                .expect("named Terminal preview button");
            assert!(button.1.is_disabled());
        }
    }

    #[test]
    fn same_version_engines_have_distinct_accessible_ids_and_busy_rollback_is_disabled() {
        let identity = |hex: &str| maintenance::EngineIdentity {
            binary: PathBuf::from("/test/bin/topgrade"),
            studio_commit: hex.repeat(40),
            component_tree: hex.repeat(40),
            binary_sha256: hex.repeat(64),
            profile_sha256: hex.repeat(64),
            profile_version: 1,
            upstream_version: "17.15.0".into(),
        };
        let check = maintenance::Check {
            available: true,
            detail: String::new(),
            steps: Vec::new(),
            engine: Some(identity("a")),
            rollback: Some(identity("b")),
            rollback_detail: None,
        };
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(680.0, 520.0),
                )),
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| engine_card(ui, &check, true));
            },
        );
        output.textures_delta.clear();
        let nodes = output.platform_output.accesskit_update.unwrap().nodes;
        for (zh, en) in [
            ("组件 aaaaaaaaaaaa", "Component aaaaaaaaaaaa"),
            ("组件 bbbbbbbbbbbb", "Component bbbbbbbbbbbb"),
            ("产物 aaaaaaaaaaaa", "Binary aaaaaaaaaaaa"),
            ("产物 bbbbbbbbbbbb", "Binary bbbbbbbbbbbb"),
        ] {
            assert!(
                nodes.iter().any(|(_, node)| {
                    [node.label(), node.value()]
                        .into_iter()
                        .flatten()
                        .any(|text| text == zh || text == en)
                }),
                "Missing identity {en} in {nodes:?}"
            );
        }
        let button = nodes.iter().find(|(_, node)| {
            matches!(
                node.label(),
                Some("复制终端回退命令" | "Copy Terminal rollback command")
            )
        });
        if cfg!(target_os = "macos") {
            assert!(button.expect("named rollback control").1.is_disabled());
        } else {
            assert!(button.is_none(), "unsupported host offers rollback");
            assert!(nodes.iter().any(|(_, node)| matches!(
                node.label(),
                Some("无法生成本机回退命令。" | "The local rollback command is unavailable.")
            )));
        }
    }

    #[test]
    fn copied_step_preview_uses_a_fixed_launcher_and_rejects_shell_tokens() {
        let command = terminal_preview("brew_formula").unwrap();
        assert!(command.starts_with(maintenance::TERMINAL_COMMAND));
        assert!(command.contains("--dry-run"));
        assert!(command.ends_with("--only brew_formula"));
        assert!(terminal_preview("helm; touch /tmp/example").is_none());
    }

    fn step(id: &str, available: bool, terminal: bool) -> maintenance::Step {
        maintenance::Step {
            id: id.into(),
            label: id.into(),
            detail: String::new(),
            terminal,
            available,
        }
    }

    fn checked() -> UiState {
        let mut state = UiState::default();
        state.accept_check(
            Ok(maintenance::Check {
                available: true,
                detail: String::new(),
                engine: None,
                rollback: None,
                rollback_detail: None,
                steps: vec![
                    step("helm", true, false),
                    step("github_cli_skills", true, false),
                    step("mas", true, true),
                    step("unavailable", false, false),
                ],
            }),
            Ok(Vec::new()),
            false,
        );
        state
    }

    #[test]
    fn only_available_gui_steps_can_be_selected_and_changes_clear_confirmation() {
        let mut state = checked();
        assert!(state.selected.is_empty() && !state.can_review(false) && !state.can_run(false));
        for id in ["mas", "unavailable", "unknown"] {
            state.select_step(id, true);
        }
        assert!(state.selected.is_empty());
        state.confirmed = true;
        state.select_step("helm", true);
        assert_eq!(state.selected, BTreeSet::from(["helm".into()]));
        assert!(!state.confirmed && !state.can_run(false));
        assert!(state.can_review(false) && !state.can_review(true));
        state.select_step("helm", false);
        assert!(!state.can_review(false));
    }

    #[test]
    fn review_requires_the_exact_eligible_selection_and_check_invalidation_consumes_it() {
        let mut state = checked();
        state.select_step("helm", true);
        state.select_step("github_cli_skills", true);
        assert!(state.matches_review(&[
            step("helm", true, false),
            step("github_cli_skills", true, false)
        ]));
        assert!(!state.matches_review(&[step("helm", true, false), step("helm", true, false)]));
        assert!(!state.matches_review(&[
            step("helm", false, false),
            step("github_cli_skills", true, false)
        ]));
        state.confirmed = true;
        state.invalidate_check();
        assert!(state.check.is_none() && state.selected.is_empty() && !state.confirmed);
        assert!(!state.can_run(false));
    }

    #[test]
    fn cancelled_or_failed_check_cannot_enable_the_previous_selection() {
        let mut state = checked();
        state.select_step("helm", true);
        let check = state.check.take().unwrap();
        let notice = state.accept_check(Ok(check), Ok(Vec::new()), true);
        assert!(notice.error && state.check.is_none() && state.selected.is_empty());
        let notice = state.accept_check(Err("check failed".into()), Ok(Vec::new()), false);
        assert!(notice.error && !state.can_review(false));
    }

    #[test]
    fn only_a_verified_result_is_presented_as_success() {
        use maintenance::RunStatus::*;
        let mut state = UiState::default();
        for status in [Verified, Failed, Cancelled, TimedOut, Incomplete] {
            let notice = state.accept_run(
                Ok(maintenance::Report {
                    path: PathBuf::from("synthetic-receipt.json"),
                    run_id: "test-run".into(),
                    status,
                    steps: Vec::new(),
                    detail: "test result".into(),
                    engine: None,
                    engine_tree: None,
                    rollback: None,
                }),
                Ok(Vec::new()),
            );
            assert_eq!(notice.error, status != Verified);
            assert_eq!(
                state.latest.as_ref().map(|report| report.status),
                Some(status)
            );
        }
        assert!(
            state
                .accept_run(Err("no receipt".into()), Ok(Vec::new()))
                .error
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_pre_cancelled_mutating_worker_does_not_run_its_operation() {
        let worker = with_macos_mutation_lock(|_| panic!("Cancelled work must not run"));
        let result = worker(Arc::new(AtomicBool::new(true)));
        assert!(result.is_err());
    }

    #[test]
    fn forged_confirmation_without_a_review_does_not_spawn_a_worker() {
        let mut app = super::super::tests::preview();
        app.updates = checked();
        app.updates.select_step("helm", true);
        app.updates.confirmed = true;
        app.start_update_run();
        assert!(app.worker.is_none());
    }
}
