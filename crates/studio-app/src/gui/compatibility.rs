//! Reviewed access to the retained Windows deployment engine in the Studio window.
use super::*;
use studio_legacy::{ExecutionTarget, NetworkMode, Options, Report};

pub(super) struct UiState {
    pub profile_json: String,
    pub target: ExecutionTarget,
    pub mode: NetworkMode,
    pub options: Options,
    pub confirmed: bool,
    pub report: Option<Report>,
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
            profile_error,
        }
    }
}

impl Studio {
    pub(super) fn start_compatibility(&mut self) {
        if !self.host.can_apply
            || self.worker.is_some()
            || !self.compatibility.confirmed
            || !allowed_target(self.compatibility.target)
        {
            return;
        }
        self.compatibility.confirmed = false;
        let review = match studio_legacy::review(
            &self.compatibility.profile_json,
            self.compatibility.target,
            self.compatibility.mode,
            &self.compatibility.options,
        ) {
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

    pub(super) fn compatibility_page(&mut self, ui: &mut egui::Ui) {
        ui.heading(tx("扩展部署", "Extended deployment"));
        ui.label(
            RichText::new(tx(
                "已合并的旧版工作流：在同一工作台审核 Windows 部署能力与完整 Profile。",
                "Merged legacy workflows: review Windows deployment capabilities and the complete Profile in this workbench.",
            ))
            .color(palette().muted),
        );
        ui.add_space(14.0);
        let editable = self.worker.is_none();
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
                for (selected, title) in [
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
                ] {
                    ui.checkbox(selected, title);
                }
            });
            ui.add_space(8.0);
            egui::CollapsingHeader::new(tx(
                "完整 Profile · 包矩阵与高级配置",
                "Complete Profile · Package matrix and advanced configuration",
            ))
            .id_salt("compatibility-profile")
            .show(ui, |ui| {
                ui.label(tx(
                    "可粘贴原版 SetupProfile JSON。Git 身份等字段应先改为自己的值；编辑只保留在当前窗口，不会自动保存或联网。",
                    "Paste an existing SetupProfile JSON. Review Git identity and other personal fields before applying. Edits remain in this window and are not saved or sent online automatically.",
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
                            self.compatibility.confirmed = false;
                            self.compatibility.profile_error = None;
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
        surface_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.heading(tx("审核与执行", "Review and apply"));
            ui.label(tx(
                "此工作流的网络设置独立于应用的现代网络方案。开启全局软件源配置会写入 Cargo/Pip/NPM 原生配置；网络配置目标即使在 Basic 模式下也会刷新 DNS。完整部署的实际范围以以下审核结果为准。",
                "This workflow uses network settings independently of the application's modern network profile. Global package sources write Cargo/Pip/NPM native configuration; the network configuration target flushes DNS even in Basic mode. The review below defines the actual scope of complete deployment.",
            ));
            ui.add_space(8.0);
            let review = studio_legacy::review(
                &self.compatibility.profile_json,
                self.compatibility.target,
                self.compatibility.mode,
                &self.compatibility.options,
            );
            match &review {
                Ok(review) => {
                    ui.label(&review.summary);
                }
                Err(error) => {
                    ui.colored_label(palette().danger, format!("{error:#}"));
                }
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
                        "我已审核完整 Profile、所选领域及上述网络与配置变更",
                        "I reviewed the complete Profile, selected capabilities, and the network and configuration changes above",
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
                "原现代迁移页面继续可用；本页直接审核完整 Profile，不自动采集或载入个人工具清单。",
                "The modern migration page remains available. This page reviews the complete Profile directly without automatically collecting or loading a personal tool inventory.",
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
}
