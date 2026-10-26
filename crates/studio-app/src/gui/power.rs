//! Activity-scheme AC parameters have their own inspection and recovery domain.
use super::*;
use studio_platform::power::{PowerChoice, PowerPlan};

pub(super) struct PowerUiState {
    pub catalog: Result<Vec<PowerChoice>, String>,
    pub states: BTreeMap<String, (String, String)>,
    pub recovery: Result<Vec<RecoveryPoint>, String>,
}

impl Default for PowerUiState {
    fn default() -> Self {
        Self {
            catalog: studio_platform::power::catalog().map_err(|error| format!("{error:#}")),
            states: BTreeMap::new(),
            recovery: Ok(Vec::new()),
        }
    }
}

impl Studio {
    pub(super) fn refresh_power_recovery(&mut self) {
        self.power.recovery =
            studio_platform::power::recovery_points().map_err(|error| format!("{error:#}"));
    }

    pub(super) fn start_power_apply(&mut self) {
        if !self.mutation_available() {
            self.notice = Some(Notice {
                text: tx(
                    "当前主机或目标不允许应用电源参数。",
                    "This host or target cannot apply power parameters.",
                )
                .into(),
                error: true,
            });
            return;
        }
        let plan = match studio_platform::power::build_plan(self.target, &self.power_selected) {
            Ok(plan) if !plan.choices.is_empty() => plan,
            Ok(_) => return,
            Err(error) => {
                self.notice = Some(Notice {
                    text: format!("{error:#}"),
                    error: true,
                });
                return;
            }
        };
        self.start_worker("正在应用电源参数", move |cancel| {
            studio_platform::power::apply(plan, cancel).map(WorkerResult::Power)
        });
        self.power.states.clear();
    }

    pub(super) fn start_power_restore(&mut self, path: PathBuf) {
        if !self.mutation_available() {
            self.notice = Some(Notice {
                text: tx(
                    "当前主机或目标不允许恢复电源参数。",
                    "This host or target cannot restore power parameters.",
                )
                .into(),
                error: true,
            });
            return;
        }
        self.start_worker("正在恢复电源参数", move |cancel| {
            studio_platform::power::restore(path, cancel).map(WorkerResult::Power)
        });
        self.power.states.clear();
    }

    pub(super) fn power_page(&mut self, ui: &mut egui::Ui) {
        ui.heading(tx("电源与节能", "Power & energy"));
        ui.label(RichText::new(scope()).color(palette().muted));
        ui.add_space(10.0);
        self.target_controls(ui);
        let index = usize::from(i18n::english());
        let mut changed = None;
        match &self.power.catalog {
            Ok(catalog) => {
                for group in ["display", "sleep", "disk", "usb", "pcie"] {
                    let choices: Vec<_> = catalog
                        .iter()
                        .filter(|choice| choice.group == group)
                        .collect();
                    if choices.is_empty() {
                        continue;
                    }
                    egui::CollapsingHeader::new(RichText::new(group_title(group)).strong())
                        .id_salt(("power-group", group))
                        .default_open(true)
                        .show(ui, |ui| {
                            for choice in choices {
                                let mut selected = self.power_selected.contains(&choice.id);
                                ui.horizontal_wrapped(|ui| {
                                    if ui
                                        .add_enabled(
                                            self.worker.is_none(),
                                            egui::Checkbox::new(
                                                &mut selected,
                                                &choice.title[index],
                                            ),
                                        )
                                        .changed()
                                    {
                                        changed = Some((choice.id.clone(), selected));
                                    }
                                    if let Some((state, detail)) = self.power.states.get(&choice.id)
                                    {
                                        state_label(ui, state, detail);
                                    }
                                });
                                ui.label(
                                    RichText::new(&choice.description[index])
                                        .size(12.0)
                                        .color(palette().muted),
                                );
                                egui::CollapsingHeader::new(tx(
                                    "确切 AC 参数",
                                    "Exact AC parameter",
                                ))
                                .id_salt(("power-detail", &choice.id))
                                .show(ui, |ui| details(ui, choice));
                                ui.add_space(5.0);
                            }
                        });
                }
                if let Some((id, selected)) = changed {
                    select_power(catalog, &mut self.power_selected, &id, selected);
                }
            }
            Err(error) => empty(ui, tx("电源目录不可用", "Power catalog unavailable"), error),
        }
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(tx("检查完整工作方案", "Review complete workspace"))
                .clicked()
            {
                self.navigate_to(Page::Plan);
            }
            if ui
                .add_enabled(
                    self.mutation_available() && !self.power_selected.is_empty(),
                    egui::Button::new(tx("应用电源参数…", "Apply power values…")),
                )
                .clicked()
            {
                self.dialog = Some(Dialog::ApplyPower);
            }
        });
        ui.hyperlink_to(tx("微软电源配置文档", "Microsoft power configuration documentation"),
            "https://learn.microsoft.com/windows-hardware/design/device-experiences/powercfg-command-line-options");
        ui.add_space(16.0);
        self.power_recovery(ui);
    }

    fn power_recovery(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.heading(tx("电源恢复记录", "Power recovery journals"));
            if ui
                .add_enabled(
                    self.worker.is_none(),
                    egui::Button::new(tx("刷新电源记录", "Refresh power records")),
                )
                .clicked()
            {
                self.refresh_power_recovery();
            }
        });
        ui.label(RichText::new(tx(
            "只恢复这条电源日志记录的方案与 AC 原值；不替换注册表恢复记录，不切换到其他活动方案。",
            "Restore only this power journal's scheme and original AC values. Registry recovery is separate, and no other scheme is activated.",
        )).size(12.0).color(palette().muted));
        let enabled = self.mutation_available();
        match &self.power.recovery {
            Err(error) => empty(
                ui,
                tx(
                    "无法读取电源恢复记录",
                    "Cannot read power recovery journals",
                ),
                error,
            ),
            Ok(points) if points.is_empty() => empty(
                ui,
                tx("还没有电源恢复记录", "No power recovery journals yet"),
                tx(
                    "在受支持的 Windows 上明确应用电源参数后，真实日志才会出现在这里。",
                    "Actual journals appear after explicitly applying power parameters on a supported Windows host.",
                ),
            ),
            Ok(points) => {
                for point in points {
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(&point.id).strong());
                        ui.label(app_pages::history_status(&point.status));
                        if ui
                            .add_enabled(
                                enabled,
                                egui::Button::new(tx(
                                    "检查并恢复电源…",
                                    "Review and restore power…",
                                )),
                            )
                            .clicked()
                        {
                            self.dialog = Some(Dialog::RestorePower(point.path.clone()));
                        }
                    });
                    ui.label(lf!(
                        "{} / {} 项参数",
                        "{} / {} values",
                        point.started_at,
                        point.actions
                    ));
                    ui.label(
                        RichText::new(point.path.display().to_string())
                            .size(12.0)
                            .color(palette().muted),
                    );
                }
            }
        }
    }

    pub(super) fn power_confirmation(&self, ui: &mut egui::Ui) -> bool {
        ui.heading(tx("确认应用电源参数", "Confirm power changes"));
        ui.label(lf!("实际主机：{}", "Actual host: {}", self.host.name));
        ui.label(lf!("方案目标：{}", "Plan target: {}", self.target.label()));
        ui.label(RichText::new(scope()).color(palette().muted));
        let plan = studio_platform::power::build_plan(self.target, &self.power_selected);
        egui::ScrollArea::vertical()
            .id_salt("confirm-power")
            .max_height(230.0)
            .show(ui, |ui| match &plan {
                Ok(plan) => plan_preview(ui, plan),
                Err(error) => {
                    ui.colored_label(palette().danger, format!("{error:#}"));
                }
            });
        ui.label(tx("每项写入前记录原值并核对回读；失败或取消可能留下部分变更，请查看电源日志。", "Original values are journaled before verified writes. Failure or cancellation may leave partial changes; review the power journal."));
        primary(
            ui,
            tx("确认应用以上电源参数", "Apply these power values"),
            self.mutation_available() && plan.as_ref().is_ok_and(|plan| !plan.choices.is_empty()),
        )
        .clicked()
    }
}

pub(super) fn select_power(
    catalog: &[PowerChoice],
    selected: &mut BTreeSet<String>,
    id: &str,
    value: bool,
) {
    let Some(choice) = catalog.iter().find(|choice| choice.id == id) else {
        return;
    };
    if value {
        selected.retain(|id| {
            !catalog
                .iter()
                .any(|other| other.id == *id && other.group == choice.group)
        });
        selected.insert(id.into());
    } else {
        selected.remove(id);
    }
}

pub(super) fn plan_preview(ui: &mut egui::Ui, plan: &PowerPlan) {
    ui.label(RichText::new(scope()).size(12.0).color(palette().muted));
    for choice in &plan.choices {
        ui.label(RichText::new(&choice.title[usize::from(i18n::english())]).strong());
        details(ui, choice);
        ui.separator();
    }
}

fn details(ui: &mut egui::Ui, choice: &PowerChoice) {
    ui.label(
        RichText::new(format!("AC · {} = {}", choice.setting, choice.value))
            .monospace()
            .size(12.0),
    );
    ui.label(
        RichText::new(format!("{} · {}", choice.group, choice.subgroup))
            .size(11.0)
            .color(palette().muted),
    );
    ui.label(RichText::new(&choice.id).size(11.0).color(palette().muted));
}

fn scope() -> &'static str {
    tx(
        "只修改执行时当前活动电源方案的插电（AC）参数；每组最多选一项。不修改电池（DC）参数，不创建卓越性能方案，也不保证硬件支持睡眠或休眠。",
        "Only changes plugged-in (AC) values of the active scheme at execution time; one choice per group. Battery (DC) values are untouched, no Ultimate Performance scheme is created, and sleep or hibernation support is not guaranteed.",
    )
}

fn group_title(group: &str) -> &str {
    match group {
        "display" => tx("显示器关闭时间", "Display timeout"),
        "sleep" => tx("睡眠等待时间", "Sleep timeout"),
        "disk" => tx("磁盘空闲时间", "Disk idle timeout"),
        "usb" => tx("USB 选择性暂停", "USB selective suspend"),
        "pcie" => tx("PCIe 链路状态电源管理", "PCIe link-state power management"),
        _ => group,
    }
}
