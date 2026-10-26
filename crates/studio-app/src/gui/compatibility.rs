//! Reviewed access to the retained Windows deployment engine in the Studio window.
use super::*;
use studio_legacy::request::{self, Entry, Group, Request};
use studio_legacy::{ExecutionTarget, NetworkMode, Options, Report};

pub(super) struct UiState {
    pub profile_json: String,
    pub target: ExecutionTarget,
    pub mode: NetworkMode,
    pub options: Options,
    pub confirmed: bool,
    pub report: Option<Report>,
    pub selected: BTreeSet<String>,
    query: String,
    group: Option<Group>,
    profile_error: Option<String>,
}

impl Default for UiState {
    fn default() -> Self {
        let (profile_json, profile_error) = match studio_legacy::default_profile_json() {
            Ok(text) => (text, None),
            Err(error) => (String::new(), Some(format!("{error:#}"))),
        };
        Self {
            profile_json,
            target: ExecutionTarget::DevToolsOnly,
            mode: NetworkMode::Basic,
            options: Options::default(),
            confirmed: false,
            report: None,
            selected: BTreeSet::new(),
            query: String::new(),
            group: None,
            profile_error,
        }
    }
}

impl UiState {
    pub(super) fn effective_profile(&self) -> anyhow::Result<String> {
        request::selected_profile(&self.profile_json, &self.selected)
    }

    pub(super) fn draft_request(&self) -> anyhow::Result<Request> {
        let request = Request {
            schema_version: 1,
            profile: serde_json::from_str(&self.profile_json)?,
            target: self.target,
            network_mode: self.mode,
            options: self.options.clone(),
            selection: self.selected.clone(),
        };
        request::validate(&request)?;
        Ok(request)
    }

    fn restore_request(&mut self, request: Request) -> anyhow::Result<()> {
        request::validate(&request)?;
        let profile_json = serde_json::to_string_pretty(&request.profile)?;
        self.profile_json = profile_json;
        self.target = request.target;
        self.mode = request.network_mode;
        self.options = request.options;
        self.selected = request.selection;
        self.confirmed = false;
        self.report = None;
        self.profile_error = None;
        self.query.clear();
        self.group = None;
        Ok(())
    }

    fn profile_edited(&mut self) {
        self.selected.clear();
        self.confirmed = false;
        self.profile_error = None;
    }

    fn select(&mut self, key: &str, selected: bool) {
        if selected {
            self.selected.insert(key.to_owned());
        } else {
            self.selected.remove(key);
        }
        self.confirmed = false;
    }

    fn visible<'a>(&self, candidates: &'a [Entry]) -> Vec<&'a Entry> {
        let query = self.query.trim().to_lowercase();
        candidates
            .iter()
            .filter(|entry| {
                self.group.is_none_or(|group| entry.group == group)
                    && (query.is_empty()
                        || format!("{} {} {}", entry.id, entry.name, group_label(entry.group))
                            .to_lowercase()
                            .contains(&query))
            })
            .collect()
    }
}

impl Studio {
    pub(super) fn compatibility_file_operation(
        &mut self,
        operation: FileOperation,
        path: &std::path::Path,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.worker.is_none(),
            "{}",
            tx(
                "请等待当前任务结束后再操作部署草案。",
                "Wait for the current task before changing deployment drafts."
            )
        );
        match operation {
            FileOperation::SaveCompatibilityRequest => {
                request::save(&self.compatibility.draft_request()?, path)
            }
            FileOperation::LoadCompatibilityRequest => {
                let request = request::load(path)?;
                self.compatibility.restore_request(request)
            }
            _ => anyhow::bail!("Unsupported deployment request operation"),
        }
    }

    pub(super) fn start_compatibility(&mut self) {
        if !self.host.can_apply
            || self.worker.is_some()
            || !self.compatibility.confirmed
            || !allowed_target(self.compatibility.target)
        {
            return;
        }
        self.compatibility.confirmed = false;
        let review = match self.compatibility.effective_profile().and_then(|profile| {
            studio_legacy::review(
                &profile,
                self.compatibility.target,
                self.compatibility.mode,
                &self.compatibility.options,
            )
        }) {
            Ok(review) => review,
            Err(error) => {
                self.notice = Some(Notice {
                    text: format!("{error:#}"),
                    error: true,
                });
                return;
            }
        };
        self.compatibility.report = None;
        self.start_worker(
            tx("正在执行扩展部署", "Applying extended deployment"),
            move |cancel| studio_legacy::execute(review, cancel).map(WorkerResult::Compatibility),
        );
    }

    fn compatibility_package_picker(&mut self, ui: &mut egui::Ui, editable: bool) {
        surface_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.heading(tx("选择软件、扩展与模型", "Choose packages, extensions and models"));
            ui.label(tx(
                "只安装勾选项目，且仅在对应领域启用时执行。关闭领域会保留选择；搜索与筛选不会改变已选项目。Store 项目也需要逐项勾选。",
                "Only checked items in enabled capabilities are installed. Disabling a capability keeps its selections; search and filters preserve them. Store products also require individual selection.",
            ));
            let candidates = match request::candidates(&self.compatibility.profile_json) {
                Ok(candidates) => candidates,
                Err(error) => {
                    ui.colored_label(palette().danger, format!("{error:#}"));
                    return;
                }
            };
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.compatibility.query)
                        .hint_text(tx("搜索名称、ID 或包管理器…", "Search name, ID or provider…"))
                        .desired_width(300.0),
                );
                egui::ComboBox::from_id_salt("compatibility-package-group")
                    .selected_text(self.compatibility.group.map_or(tx("全部来源", "All providers"), group_label))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.compatibility.group, None, tx("全部来源", "All providers"));
                        for group in Group::ALL {
                            let total = candidates.iter().filter(|entry| entry.group == group).count();
                            let selected = candidates.iter().filter(|entry| {
                                entry.group == group && self.compatibility.selected.contains(&entry.key)
                            }).count();
                            ui.selectable_value(
                                &mut self.compatibility.group,
                                Some(group),
                                format!("{} · {selected}/{total}", group_label(group)),
                            );
                        }
                    });
            });
            let visible = self.compatibility.visible(&candidates);
            let selected_visible = visible.iter().filter(|entry| self.compatibility.selected.contains(&entry.key)).count();
            ui.add_space(6.0);
            ui.label(
                RichText::new(lf!(
                    "已选 {} 项 · 当前显示 {} / {} 项 · 其中已选 {} 项",
                    "{} selected · Showing {} / {} items · {} selected here",
                    self.compatibility.selected.len(), visible.len(), candidates.len(), selected_visible
                )).color(palette().muted),
            );
            ui.add_enabled_ui(editable, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(!visible.is_empty(), egui::Button::new(tx("选择当前显示", "Select visible"))).clicked() {
                        self.compatibility.selected.extend(visible.iter().map(|entry| entry.key.clone()));
                        self.compatibility.confirmed = false;
                    }
                    let clear_label = if self.compatibility.group.is_some() {
                        tx("清空当前来源", "Clear this provider")
                    } else {
                        tx("清空全部选择", "Clear all selections")
                    };
                    if ui.add_enabled(!self.compatibility.selected.is_empty(), egui::Button::new(clear_label)).clicked() {
                        let group = self.compatibility.group;
                        for entry in &candidates {
                            if group.is_none_or(|group| entry.group == group) {
                                self.compatibility.selected.remove(&entry.key);
                            }
                        }
                        self.compatibility.confirmed = false;
                    }
                });
            });
            ui.add_space(8.0);
            if visible.is_empty() {
                ui.label(tx("没有匹配项目，请调整搜索或来源。", "No matching items. Change the search or provider."));
            }
            let row_height = ui.spacing().interact_size.y
                .max(ui.text_style_height(&egui::TextStyle::Body));
            egui::ScrollArea::vertical()
                .id_salt("compatibility-package-rows")
                .max_height(280.0)
                .auto_shrink([false, true])
                .show_rows(ui, row_height, visible.len(), |ui, range| {
                    for entry in &visible[range] {
                        let mut checked = self.compatibility.selected.contains(&entry.key);
                        ui.push_id(&entry.key, |ui| {
                            ui.horizontal(|ui| {
                                let width = ui.available_width();
                                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                                let response = ui.add_enabled_ui(editable, |ui| {
                                    ui.add_sized(
                                        [width * 0.40, row_height],
                                        egui::Checkbox::new(&mut checked, &entry.name),
                                    )
                                }).inner;
                                if response.changed() {
                                    self.compatibility.select(&entry.key, checked);
                                }
                                response.on_hover_text(&entry.name);
                                ui.add_sized(
                                    [width * 0.35, row_height],
                                    egui::Label::new(RichText::new(&entry.id).monospace().color(palette().muted)).truncate(),
                                ).on_hover_text(&entry.id);
                                ui.add_sized(
                                    [ui.available_width(), row_height],
                                    egui::Label::new(RichText::new(group_label(entry.group)).small().color(palette().muted)).truncate(),
                                );
                            });
                        });
                    }
                });
        });
    }

    pub(super) fn compatibility_page(&mut self, ui: &mut egui::Ui) {
        ui.heading(tx("扩展部署", "Extended deployment"));
        ui.label(
            RichText::new(tx(
                "按需组合 Windows 部署能力与单个软件。默认不选择任何软件，先选领域，再勾选所需项目。",
                "Compose Windows deployment capabilities and individual packages. Nothing is selected by default: enable capabilities, then pick the items you need.",
            ))
            .color(palette().muted),
        );
        ui.add_space(14.0);
        let editable = self.worker.is_none();
        ui.add_enabled_ui(editable, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button(tx("保存部署草案…", "Save deployment draft…"))
                    .clicked()
                {
                    self.open_file(FileOperation::SaveCompatibilityRequest);
                }
                if ui
                    .button(tx("载入部署草案…", "Load deployment draft…"))
                    .clicked()
                {
                    self.open_file(FileOperation::LoadCompatibilityRequest);
                }
                if ui
                    .button(tx("重置为默认候选清单", "Reset default candidate catalog"))
                    .clicked()
                {
                    self.compatibility = UiState::default();
                }
            });
        });
        ui.add_space(10.0);
        let old_target = self.compatibility.target;
        let old_mode = self.compatibility.mode;
        let old_options = self.compatibility.options.clone();
        surface_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.heading(tx("选择执行范围", "Choose the deployment scope"));
            ui.label(tx(
                "所有功能域默认关闭。选择目标和需要启用的领域，再审核生成的执行范围。",
                "Every capability is off by default. Choose a target and capabilities, then review the resulting scope.",
            ));
            ui.add_space(8.0);
            ui.add_enabled_ui(editable, |ui| {
                ui.horizontal_wrapped(|ui| {
                    egui::ComboBox::from_id_salt("compatibility-target")
                        .selected_text(target_label(self.compatibility.target))
                        .show_ui(ui, |ui| {
                            for target in [
                                ExecutionTarget::DevToolsOnly,
                                ExecutionTarget::FullSetup,
                                ExecutionTarget::NetworkOnly,
                                ExecutionTarget::StorageOnly,
                                ExecutionTarget::WindowsFeaturesOnly,
                                ExecutionTarget::VSCodeExtensionsOnly,
                                ExecutionTarget::SystemTweaksOnly,
                            ] {
                                ui.selectable_value(
                                    &mut self.compatibility.target,
                                    target,
                                    target_label(target),
                                );
                            }
                        });
                    egui::ComboBox::from_id_salt("compatibility-network-mode")
                        .selected_text(mode_label(self.compatibility.mode))
                        .show_ui(ui, |ui| {
                            for mode in [
                                NetworkMode::Basic,
                                NetworkMode::Optimized,
                                NetworkMode::Extreme,
                            ] {
                                ui.selectable_value(
                                    &mut self.compatibility.mode,
                                    mode,
                                    mode_label(mode),
                                );
                            }
                        });
                });
                ui.add_space(8.0);
                ui.columns(2, |columns| {
                    for (index, (selected, title)) in [
                    (
                        &mut self.compatibility.options.extra_apps,
                        tx("基础与附加软件", "Core and additional applications"),
                    ),
                    (
                        &mut self.compatibility.options.dev_tools,
                        tx("开发工具与包矩阵", "Developer tools and package matrix"),
                    ),
                    (
                        &mut self.compatibility.options.git_shell,
                        tx("Git 与 PowerShell 配置", "Git and PowerShell configuration"),
                    ),
                    (
                        &mut self.compatibility.options.editor,
                        tx("VS Code / Cursor 设置与扩展", "VS Code / Cursor settings and extensions"),
                    ),
                    (
                        &mut self.compatibility.options.npm_config,
                        tx("全局软件源配置（Cargo/Pip/NPM）", "Global package sources (Cargo/Pip/NPM)"),
                    ),
                    (
                        &mut self.compatibility.options.ollama,
                        tx("Ollama 模型", "Ollama models"),
                    ),
                    (
                        &mut self.compatibility.options.windows_features,
                        tx("Windows 可选功能", "Windows optional features"),
                    ),
                    (
                        &mut self.compatibility.options.storage,
                        tx("存储优化", "Storage optimization"),
                    ),
                    (
                        &mut self.compatibility.options.tweaks,
                        tx("系统性能与隐私配置", "System performance and privacy configuration"),
                    ),
                ].into_iter().enumerate() {
                    columns[index % 2].checkbox(selected, title);
                    }
                });
            });
            ui.add_space(8.0);
            egui::CollapsingHeader::new(tx(
                "完整 Profile · 包矩阵与高级配置",
                "Complete Profile · Package matrix and advanced configuration",
            ))
            .id_salt("compatibility-profile")
            .show(ui, |ui| {
                ui.label(tx(
                    "这是候选清单与高级设置，不会全部安装。编辑 JSON 会清空所有单项选择和执行确认；Git 身份等字段应改为自己的值。保存草案会包含这些字段，请妥善保管。",
                    "This is the candidate catalog and advanced settings, not a bulk install list. Editing JSON clears every item selection and execution confirmation. Review Git identity and personal fields; saved drafts include these values.",
                ));
                if let Some(error) = &self.compatibility.profile_error {
                    ui.colored_label(palette().danger, error);
                }
                egui::ScrollArea::vertical()
                    .id_salt("compatibility-profile-editor")
                    .max_height(300.0)
                    .show(ui, |ui| {
                        if ui
                            .add_enabled(
                                editable,
                                egui::TextEdit::multiline(&mut self.compatibility.profile_json)
                                    .font(egui::TextStyle::Monospace)
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(14),
                            )
                            .changed()
                        {
                            self.compatibility.profile_edited();
                        }
                    });
            });
        });
        if old_target != self.compatibility.target
            || old_mode != self.compatibility.mode
            || old_options != self.compatibility.options
        {
            self.compatibility.confirmed = false;
        }
        ui.add_space(16.0);
        self.compatibility_package_picker(ui, editable);
        ui.add_space(16.0);
        surface_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.heading(tx("审核与执行", "Review and apply"));
            ui.label(tx(
                "此工作流的网络设置独立于应用的现代网络方案。开启全局软件源配置会写入 Cargo/Pip/NPM 原生配置；网络配置目标即使在 Basic 模式下也会刷新 DNS。完整部署的实际范围以以下审核结果为准。",
                "This workflow uses network settings independently of the application's modern network profile. Global package sources write Cargo/Pip/NPM native configuration; the network configuration target flushes DNS even in Basic mode. The review below defines the actual scope of complete deployment.",
            ));
            ui.add_space(8.0);
            let effective = self.compatibility.effective_profile();
            let review = effective.as_ref().map_err(|error| anyhow::anyhow!("{error:#}")).and_then(|profile| {
                studio_legacy::review(
                    profile,
                    self.compatibility.target,
                    self.compatibility.mode,
                    &self.compatibility.options,
                )
            });
            match &review {
                Ok(review) => {
                    ui.strong(lf!(
                        "{} · 已选 {} 个单项",
                        "{} · {} individual items selected",
                        target_label(self.compatibility.target),
                        self.compatibility.selected.len()
                    ));
                    ui.label(&review.summary);
                }
                Err(_) if effective.is_ok()
                    && self.compatibility.selected.is_empty()
                    && self.compatibility.options == Options::default() =>
                {
                    ui.label(RichText::new(tx(
                        "先启用要配置的领域；安装软件时，再从候选清单勾选单项。完成后将在这里显示实际计划供你审核。",
                        "Enable the capabilities you want to configure, then select individual items for software installation. The resulting plan will appear here for review.",
                    )).color(palette().muted));
                }
                Err(error) => {
                    let detail = format!("{error:#}");
                    let message = if detail == "Choose the operation's corresponding scope before reviewing" {
                        tx(
                            "请先选择与当前目标对应的操作范围，再审核。",
                            "Choose the operation's corresponding scope before reviewing",
                        )
                    } else {
                        detail.as_str()
                    };
                    ui.colored_label(palette().danger, message);
                    ui.label(tx(
                        "请检查当前目标允许的领域、该领域的单项选择，以及高级 Profile 配置。",
                        "Check which capabilities this target supports, the items selected for those capabilities, and the advanced Profile settings.",
                    ));
                }
            }
            if let Ok(profile) = &effective {
                egui::CollapsingHeader::new(tx("实际执行 Profile · 仅含勾选项目", "Effective Profile · Selected items only"))
                    .id_salt("compatibility-effective-profile")
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("compatibility-effective-profile-content")
                            .max_height(240.0)
                            .show(ui, |ui| {
                                ui.add(egui::Label::new(RichText::new(profile).monospace()).wrap());
                            });
                    });
            }
            ui.add_space(8.0);
            ui.label(
                RichText::new(tx(
                    "安装可能下载软件、运行安装器并修改系统或用户配置。取消不等于撤销；本页不提供自动回滚。",
                    "Installation may download software, run installers and change system or user configuration. Cancellation does not undo changes; automatic rollback is unavailable on this page.",
                ))
                .color(palette().danger),
            );
            if !self.host.can_apply {
                ui.label(
                    RichText::new(tx(
                        "当前主机仅供审核预览，不能执行 Windows 兼容部署。",
                        "This host can review the plan but cannot apply Windows compatibility deployment.",
                    ))
                    .color(palette().muted),
                );
            }
            let valid = allowed_target(self.compatibility.target) && review.is_ok();
            ui.add_enabled(
                editable && valid,
                egui::Checkbox::new(
                    &mut self.compatibility.confirmed,
                    tx(
                        "我已审核实际执行 Profile、勾选项目、领域及上述配置变更",
                        "I reviewed the effective Profile, selected items, capabilities and configuration changes above",
                    ),
                ),
            );
            ui.horizontal_wrapped(|ui| {
                if primary(
                    ui,
                    tx("在本机执行已审核计划", "Apply the reviewed plan on this machine"),
                    self.host.can_apply
                        && editable
                        && valid
                        && self.compatibility.confirmed,
                )
                .clicked()
                {
                    self.start_compatibility();
                }
            });
        });
        ui.add_space(16.0);
        surface_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.heading(tx("兼容范围", "Compatibility boundaries"));
            ui.label(tx(
                "Agent Skills / Plugins 内置资源尚未完成审查，旧版未认证回滚日志也未接入安全执行，因此这两个旧目标尚未开放。其余领域以当前审核结果为准。",
                "Embedded Agent Skills / Plugins have not completed review, and unauthenticated legacy rollback journals are not enabled for execution. These two legacy targets remain unavailable. The current review defines the other capabilities.",
            ));
            ui.label(tx(
                "开发环境迁移页面继续可用；本页审核手动勾选项目与高级设置，不自动采集个人工具清单。部署草案包含候选 Profile 和选择，可保存后在其他机器重新审核。",
                "The developer environment migration page remains available. This page reviews manually selected items and advanced settings without collecting a personal inventory. A saved draft includes its candidate Profile and selections for review on another machine.",
            ));
        });
        if let Some(report) = &self.compatibility.report {
            ui.add_space(16.0);
            surface_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
                ui.heading(tx("最近执行结果", "Latest execution result"));
                let (label, color) = if report.cancelled {
                    (tx("已取消", "Cancelled"), palette().danger)
                } else if report.succeeded {
                    (tx("后端报告成功", "Backend reported success"), palette().accent)
                } else {
                    (tx("未成功完成", "Did not complete successfully"), palette().danger)
                };
                ui.colored_label(color, label);
                ui.label(&report.summary);
                egui::CollapsingHeader::new(tx("历史记录与执行日志", "History and execution log"))
                    .id_salt("compatibility-report")
                    .show(ui, |ui| {
                        ui.label(lf!("历史记录：{}", "History: {}", report.history.display()));
                        ui.label(lf!("共 {} 条记录", "{} log entries", report.log.len()));
                        if report.log.len() > 400 {
                            ui.label(tx(
                                "窗口仅显示最近 400 条，完整结果见历史记录。",
                                "This window shows the latest 400 entries; consult history for the complete result.",
                            ));
                        }
                        egui::ScrollArea::vertical()
                            .id_salt("compatibility-report-log")
                            .max_height(260.0)
                            .show(ui, |ui| {
                                for line in report.log.iter().skip(report.log.len().saturating_sub(400)) {
                                    ui.add(egui::Label::new(RichText::new(line).monospace()).wrap());
                                }
                            });
                    });
            });
        }
    }
}

fn allowed_target(target: ExecutionTarget) -> bool {
    matches!(
        target,
        ExecutionTarget::FullSetup
            | ExecutionTarget::DevToolsOnly
            | ExecutionTarget::NetworkOnly
            | ExecutionTarget::StorageOnly
            | ExecutionTarget::WindowsFeaturesOnly
            | ExecutionTarget::VSCodeExtensionsOnly
            | ExecutionTarget::SystemTweaksOnly
    )
}

fn group_label(group: Group) -> &'static str {
    match group {
        Group::WingetCore => tx("WinGet · 基础软件", "WinGet · Core apps"),
        Group::WingetDev => tx("WinGet · 开发工具", "WinGet · Developer tools"),
        Group::Store => tx(
            "Microsoft Store · 附加软件",
            "Microsoft Store · Additional apps",
        ),
        Group::Scoop => tx("Scoop · 开发工具", "Scoop · Developer tools"),
        Group::Cargo => tx("Cargo · Rust 工具", "Cargo · Rust tools"),
        Group::Npm => tx("NPM · 全局工具", "NPM · Global tools"),
        Group::Pip => tx("Pip · Python 包", "Pip · Python packages"),
        Group::Uv => tx("uv · Python 工具", "uv · Python tools"),
        Group::Extension => tx("编辑器扩展", "Editor extensions"),
        Group::Model => tx("Ollama 模型", "Ollama models"),
    }
}

fn target_label(target: ExecutionTarget) -> &'static str {
    match target {
        ExecutionTarget::FullSetup => tx("完整部署", "Complete deployment"),
        ExecutionTarget::DevToolsOnly => tx("开发环境部署", "Developer environment"),
        ExecutionTarget::NetworkOnly => tx("Windows 网络配置", "Windows network configuration"),
        ExecutionTarget::StorageOnly => tx("存储优化", "Storage optimization"),
        ExecutionTarget::WindowsFeaturesOnly => tx("Windows 可选功能", "Windows optional features"),
        ExecutionTarget::VSCodeExtensionsOnly => {
            tx("编辑器设置与扩展", "Editor settings and extensions")
        }
        ExecutionTarget::SystemTweaksOnly => tx("系统性能与隐私", "System performance and privacy"),
        ExecutionTarget::AgentSkillsOnly => {
            tx("Agent 资源（未开放）", "Agent resources (unavailable)")
        }
        ExecutionTarget::RollbackLatest => {
            tx("旧版回滚（未开放）", "Legacy rollback (unavailable)")
        }
    }
}

fn mode_label(mode: NetworkMode) -> &'static str {
    match mode {
        NetworkMode::Basic => tx("Basic · 基础网络", "Basic · Baseline network"),
        NetworkMode::Optimized => tx("Optimized · DNS / TCP", "Optimized · DNS / TCP"),
        NetworkMode::Extreme => tx("Extreme · RSS / ECN / CTCP", "Extreme · RSS / ECN / CTCP"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compatibility_is_opt_in_and_excludes_unreviewed_legacy_targets() {
        let state = UiState::default();
        assert!(!state.confirmed);
        assert!(state.selected.is_empty());
        assert!(state.report.is_none());
        assert!(state.options == Options::default());
        assert!(
            ![
                state.options.extra_apps,
                state.options.dev_tools,
                state.options.git_shell,
                state.options.editor,
                state.options.npm_config,
                state.options.ollama,
                state.options.windows_features,
                state.options.storage,
                state.options.tweaks,
            ]
            .into_iter()
            .any(|enabled| enabled)
        );
        assert!(state.target == ExecutionTarget::DevToolsOnly);
        assert!(state.mode == NetworkMode::Basic);
        assert!(allowed_target(ExecutionTarget::DevToolsOnly));
        assert!(!allowed_target(ExecutionTarget::AgentSkillsOnly));
        assert!(!allowed_target(ExecutionTarget::RollbackLatest));
    }

    #[test]
    fn one_selected_package_is_the_entire_effective_catalog() {
        let mut state = UiState::default();
        let candidates = request::candidates(&state.profile_json).unwrap();
        let entry = candidates
            .iter()
            .find(|entry| entry.group == Group::WingetDev)
            .unwrap();
        state.confirmed = true;
        state.select(&entry.key, true);
        assert!(!state.confirmed);
        state.options.dev_tools = true;
        let effective = state.effective_profile().unwrap();
        let selected = request::candidates(&effective).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].key, entry.key);
        assert!(selected.iter().all(|entry| entry.group != Group::Store));
        let review =
            studio_legacy::review(&effective, state.target, state.mode, &state.options).unwrap();
        assert_eq!(review.config.profile.packages.winget_dev.len(), 1);
        assert_eq!(review.config.profile.packages.winget_dev[0].id, entry.id);
    }

    #[test]
    fn changing_any_candidate_json_clears_selection_and_confirmation() {
        let mut state = UiState::default();
        let key = request::candidates(&state.profile_json).unwrap()[0]
            .key
            .clone();
        state.select(&key, true);
        state.confirmed = true;
        state.profile_json.push(' ');
        state.profile_edited();
        assert!(state.selected.is_empty());
        assert!(!state.confirmed);
        state.select(&key, true);
        state.confirmed = true;
        state.profile_json = "{".into();
        state.profile_edited();
        assert!(state.selected.is_empty());
        assert!(!state.confirmed);
        assert!(state.effective_profile().is_err());
    }

    #[test]
    fn restoring_a_valid_draft_keeps_choices_but_never_confirmation() {
        let mut source = UiState::default();
        let key = request::candidates(&source.profile_json)
            .unwrap()
            .into_iter()
            .find(|entry| entry.group == Group::WingetCore)
            .unwrap()
            .key;
        source.select(&key, true);
        source.target = ExecutionTarget::FullSetup;
        source.options.extra_apps = true;
        source.mode = NetworkMode::Optimized;
        let request = source.draft_request().unwrap();
        let mut restored = UiState {
            confirmed: true,
            ..UiState::default()
        };
        restored.restore_request(request).unwrap();
        assert_eq!(restored.selected, source.selected);
        assert!(restored.target == source.target);
        assert!(restored.mode == source.mode);
        assert_eq!(restored.options, source.options);
        assert!(!restored.confirmed);
    }

    #[test]
    fn rejecting_a_draft_preserves_the_open_selection_and_profile() {
        let mut state = UiState::default();
        let key = request::candidates(&state.profile_json).unwrap()[0]
            .key
            .clone();
        state.select(&key, true);
        let original_profile = state.profile_json.clone();
        let original_selection = state.selected.clone();
        let mut invalid = state.draft_request().unwrap();
        invalid.selection.insert("unknown:package".into());
        assert!(state.restore_request(invalid).is_err());
        assert_eq!(state.profile_json, original_profile);
        assert_eq!(state.selected, original_selection);
        assert!(state.target == ExecutionTarget::DevToolsOnly);
    }

    #[test]
    fn search_and_provider_filters_preserve_hidden_selections() {
        let mut state = UiState::default();
        let candidates = request::candidates(&state.profile_json).unwrap();
        let entry = candidates
            .iter()
            .find(|entry| entry.group == Group::WingetDev)
            .unwrap();
        state.select(&entry.key, true);
        state.query = "this-query-has-no-matches".into();
        assert!(state.visible(&candidates).is_empty());
        assert!(state.selected.contains(&entry.key));
        state.query.clear();
        state.group = Some(Group::Model);
        assert!(
            state
                .visible(&candidates)
                .iter()
                .all(|entry| entry.group == Group::Model)
        );
        assert!(state.selected.contains(&entry.key));
        state.group = None;
        state.query = entry.id.clone();
        assert!(
            state
                .visible(&candidates)
                .iter()
                .any(|candidate| candidate.key == entry.key)
        );
        assert_eq!(state.selected.len(), 1);
    }
}
