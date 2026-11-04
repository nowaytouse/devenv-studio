use super::*;
use maintenance::brew_inventory;

mod brew;

pub(super) type History = Result<Vec<maintenance::Report>, String>;
pub(super) type BrewInventory = Result<brew_inventory::Inventory, String>;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Section {
    #[default]
    Tasks,
    Homebrew,
    History,
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
                    text: check.detail.clone(),
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
        if !cfg!(target_os = "macos") {
            ui.label(tx(
                "更新中心仅在 macOS 本机可用。",
                "The update center is available on macOS.",
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
                Section::Homebrew,
                tx("Homebrew 已安装", "Homebrew installed"),
            );
            ui.selectable_value(
                &mut self.updates.section,
                Section::History,
                tx("执行记录", "Run history"),
            );
        });
        ui.add_space(8.0);
        match self.updates.section {
            Section::Tasks => self.update_tasks_view(ui, busy),
            Section::Homebrew => {
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

    fn update_tasks_view(&mut self, ui: &mut egui::Ui, busy: bool) {
        ui.heading(tx("更新本机开发工具", "Update local developer tools"));
        ui.label(tx(
            "检查本机 Topgrade，再选择、审核和确认更新。检查只读取本地状态。",
            "Check the local Topgrade, then select, review, and confirm updates. The check reads local state.",
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
            if check.available {
                ui.label(tx(
                    "已核验本机 Topgrade 引擎和配置。",
                    "Local Topgrade engine and profile verified.",
                ));
            } else {
                ui.colored_label(palette().danger, &check.detail);
            }
            if !check.steps.is_empty() {
                ui.heading(tx("可在 Studio 执行", "Run in Studio"));
            }
            for step in check.steps.iter().filter(|step| !step.terminal) {
                let mut selected = self.updates.selected.contains(&step.id);
                let label = step_label(&step.id, &step.label);
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(
                            !busy && check.available && step.available && !step.terminal,
                            egui::Checkbox::new(&mut selected, label),
                        )
                        .changed()
                    {
                        selection_change = Some((step.id.clone(), selected));
                    }
                    if !step.available {
                        ui.label(tx("不可用", "Unavailable"));
                    }
                });
                ui.label(RichText::new(step_detail(step)).color(palette().muted));
            }
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

        if self
            .updates
            .check
            .as_ref()
            .is_some_and(|check| !check.steps.is_empty())
        {
            ui.separator();
            if let Some(check) = &self.updates.check {
                terminal_catalog(ui, &check.steps, &mut self.updates.catalog_query, busy);
            }
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
        }
    }
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
                    egui::CollapsingHeader::new(format!(
                        "{} · {}",
                        run_status_label(&report.status),
                        report.run_id
                    ))
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

fn report_details(ui: &mut egui::Ui, report: &maintenance::Report) {
    let colors = palette();
    let color = if report.status == maintenance::RunStatus::Verified {
        colors.accent
    } else {
        colors.danger
    };
    ui.colored_label(color, run_status_label(&report.status));
    ui.label(&report.detail);
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
        maintenance::RunStatus::Verified => tx("执行结果已核验", "Run result verified"),
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
            "刷新本机 TLDR 手册缓存。",
            "Refresh the local TLDR command reference cache.",
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
                "Homebrew Formulae (HEAD-synced)", "Mac App Store (verified)", "gcloud (verified)",
                "SQLite Python trunk", "SQLite trunk", "TourBox Beta Update", "Xcode Beta Update",
                "BetterDisplay Internal Update", "Preview macOS Apps", "Conda frozen base",
                "Conda default", "Gradle Nightly Toolchain", "Rolling Project Dependencies",
                "npm Bundled Dependency Security", "Topgrade Skills Coverage Gate",
            ] {
                ui.push_id(name, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(name);
                        if ui.add_enabled(!busy, egui::Button::new(tx("复制预演命令", "Copy preview command"))).clicked() {
                            ui.ctx().copy_text(format!(
                                "{} --dry-run --no-self-update --no-tmux --show-skipped --only custom_commands --custom-commands '{name}'",
                                maintenance::TERMINAL_COMMAND
                            ));
                        }
                    });
                });
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

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
