use super::*;

pub(super) type History = Result<Vec<maintenance::Report>, String>;

#[derive(Default)]
pub(super) struct UiState {
    check: Option<maintenance::Check>,
    selected: BTreeSet<String>,
    review: Option<maintenance::Review>,
    confirmed: bool,
    latest: Option<maintenance::Report>,
    history: Option<History>,
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
        ui.heading(tx("更新本机开发工具", "Update local developer tools"));
        if !cfg!(target_os = "macos") {
            ui.label(tx(
                "更新中心仅在 macOS 本机可用。",
                "The update center is available on macOS.",
            ));
            return;
        }

        let busy = self.worker.is_some();
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
            ui.label(&check.detail);
            for step in &check.steps {
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
                    if step.terminal {
                        ui.label(tx("终端执行", "Terminal"));
                    } else if !step.available {
                        ui.label(tx("不可用", "Unavailable"));
                    }
                });
                ui.label(RichText::new(&step.detail).color(palette().muted));
            }
        }
        if let Some((id, selected)) = selection_change {
            self.updates.select_step(&id, selected);
        }

        if primary(
            ui,
            tx("审核所选更新", "Review selected updates"),
            self.updates.can_review(busy),
        )
        .clicked()
        {
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
                    ui.label(step_label(&step.id, &step.label));
                    ui.label(RichText::new(&step.detail).color(palette().muted));
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
            .is_some_and(|check| check.available)
        {
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
        }
        self.updates.history_view(ui);
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
                    "检查后显示本地执行记录。",
                    "Check to read local run history.",
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
        "github_cli_skills" => tx("GitHub CLI 技能", "GitHub CLI skills"),
        "helm" => tx("Helm 仓库", "Helm repositories"),
        _ => fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
