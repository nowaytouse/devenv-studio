use super::*;

#[derive(Default)]
pub(super) struct UiState {
    pub profile: Option<studio_deployment::git_config::Profile>,
    pub current: Option<studio_deployment::git_config::Profile>,
    pub selected: BTreeSet<String>,
    pub confirmed: bool,
    pub report: Option<studio_deployment::git_config::ApplyReport>,
}

impl UiState {
    fn can_apply(&self, worker_active: bool) -> bool {
        !worker_active
            && self.confirmed
            && self.current.is_some()
            && !self.selected.is_empty()
            && self.profile.as_ref().is_some_and(|profile| {
                self.selected
                    .iter()
                    .all(|key| profile.settings.contains_key(key))
            })
    }
}

impl Studio {
    pub(super) fn start_git_capture(&mut self) {
        if self.worker.is_some() {
            return;
        }
        self.start_worker("Git configuration capture", |cancel| {
            let profile = studio_deployment::git_config::capture(&cancel)?;
            Ok(WorkerResult::GitConfigCapture(profile))
        });
    }

    pub(super) fn start_git_refresh(&mut self) {
        if self.worker.is_some() {
            return;
        }
        self.start_worker("Git configuration refresh", |cancel| {
            let profile = studio_deployment::git_config::capture(&cancel)?;
            Ok(WorkerResult::GitConfigRefresh(profile))
        });
    }

    pub(super) fn start_git_apply(&mut self) {
        let state = &self.backup.git_config;
        if !state.can_apply(self.worker.is_some()) {
            return;
        }
        let Some(profile) = state.profile.clone() else {
            return;
        };
        let Some(current) = state.current.clone() else {
            return;
        };
        let selected = state.selected.clone();
        self.start_worker("Git configuration apply", move |cancel| {
            let report =
                studio_deployment::git_config::apply(&profile, &selected, &current, &cancel)?;
            Ok(WorkerResult::GitConfigApply(report))
        });
    }

    pub(super) fn git_config_file_operation(
        &mut self,
        operation: FileOperation,
        path: &std::path::Path,
    ) -> Result<String, String> {
        if self.worker.is_some() {
            return Err(tx(
                "请等待当前任务结束后再处理 Git 配置档案。",
                "Wait for the current task before handling a Git profile.",
            )
            .into());
        }
        match operation {
            FileOperation::SaveGitProfile => {
                let profile = self.backup.git_config.profile.as_ref().ok_or_else(|| {
                    tx(
                        "请先读取或载入 Git 配置。",
                        "Capture or load a Git profile first.",
                    )
                    .to_owned()
                })?;
                studio_deployment::git_config::save(profile, path)
                    .map_err(|error| format!("{error:#}"))?;
            }
            FileOperation::LoadGitProfile => {
                let profile = studio_deployment::git_config::load(path)
                    .map_err(|error| format!("{error:#}"))?;
                self.backup.git_config.profile = Some(profile);
                self.backup.git_config.current = None;
                self.backup.git_config.selected.clear();
                self.backup.git_config.confirmed = false;
                self.backup.git_config.report = None;
            }
            FileOperation::SaveGitReport => {
                let report = self.backup.git_config.report.as_ref().ok_or_else(|| {
                    tx(
                        "还没有 Git 配置结果可保存。",
                        "There is no Git configuration result to save.",
                    )
                    .to_owned()
                })?;
                studio_core::export_document(report, path).map_err(|error| format!("{error:#}"))?;
            }
            _ => return Err("Not a Git configuration file operation".into()),
        }
        Ok(lf!(
            "已完成 Git 配置文件操作：{}",
            "Git configuration file operation completed: {}",
            path.display()
        ))
    }

    pub(super) fn git_config_page(&mut self, ui: &mut egui::Ui) {
        let worker_active = self.worker.is_some();
        let mut capture = false;
        let mut refresh = false;
        let mut load = false;
        let mut save = false;
        let mut save_report = false;
        let mut apply = false;
        let state = &mut self.backup.git_config;

        surface_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.heading(tx("Git 全局配置", "Git global configuration"));
                let label = match (state.profile.is_some(), state.current.is_some()) {
                    (false, _) => tx("尚未载入", "Not loaded"),
                    (true, false) => tx("等待读取本机", "Refresh this device"),
                    (true, true) => tx("已完成对比", "Comparison ready"),
                };
                badge(ui, label, palette().wash, palette().accent);
            });
            ui.label(RichText::new(tx(
                "在 macOS 与 Windows 之间迁移已审核的 Git 身份和常用行为设置。",
                "Move reviewed Git identity and common behavior settings between macOS and Windows.",
            )).color(palette().muted));
            ui.label(RichText::new(tx(
                "只包含本页列出的键；不会读取或迁移凭据、代理地址、别名、include、SSH 或脚本。身份信息保存在你选择的位置。",
                "Only the keys listed here are included. Credentials, proxy URLs, aliases, includes, SSH settings and scripts are excluded. Identity details are saved only where you choose.",
            )).size(12.0).color(palette().muted));
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(!worker_active, egui::Button::new(tx("读取本机设置", "Capture this device"))).clicked() {
                    capture = true;
                }
                if ui.add_enabled(!worker_active, egui::Button::new(tx("载入档案…", "Load profile…"))).clicked() {
                    load = true;
                }
                if ui.add_enabled(!worker_active && state.profile.is_some(), egui::Button::new(tx("保存档案…", "Save profile…"))).clicked() {
                    save = true;
                }
                if ui.add_enabled(!worker_active && state.profile.is_some(), egui::Button::new(tx("刷新本机对比", "Refresh comparison"))).clicked() {
                    refresh = true;
                }
                if ui.add_enabled(!worker_active && state.report.is_some(), egui::Button::new(tx("保存结果…", "Save result…"))).clicked() {
                    save_report = true;
                }
            });
            if worker_active {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tx("Git 配置任务处理中…", "Git configuration task is running…"));
                });
            }
            if let Some(profile) = state.profile.as_ref() {
                if state.current.is_none() {
                    ui.label(RichText::new(tx(
                        "先刷新本机对比，再逐项选择要应用的值。",
                        "Refresh the comparison, then select the values to apply.",
                    )).color(palette().accent));
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt("git-config-review")
                        .max_height(250.0)
                        .show(ui, |ui| {
                            egui::Grid::new("git-config-grid")
                                .num_columns(4)
                                .striped(true)
                                .show(ui, |ui| {
                                    for title in [
                                        tx("设置", "Setting"),
                                        tx("本机当前值", "Current value"),
                                        tx("档案目标值", "Profile value"),
                                        tx("应用", "Apply"),
                                    ] {
                                        ui.label(RichText::new(title).strong());
                                    }
                                    ui.end_row();
                                    for key in studio_deployment::git_config::KEYS {
                                        let Some(target) = profile.settings.get(key) else {
                                            continue;
                                        };
                                        let current = state
                                            .current
                                            .as_ref()
                                            .and_then(|current| current.settings.get(key));
                                        let current_text = current.map_or(
                                            tx("未设置", "Not set"),
                                            String::as_str,
                                        );
                                        ui.label(setting_label(key));
                                        ui.add(egui::Label::new(current_text).wrap());
                                        ui.add(egui::Label::new(RichText::new(target).color(
                                            if current.map(String::as_str) == Some(target) {
                                                palette().muted
                                            } else {
                                                palette().accent
                                            },
                                        )).wrap());
                                        let mut selected = state.selected.contains(key);
                                        if ui.checkbox(&mut selected, "").changed() {
                                            if selected {
                                                state.selected.insert(key.to_owned());
                                            } else {
                                                state.selected.remove(key);
                                            }
                                            state.confirmed = false;
                                        }
                                        ui.end_row();
                                    }
                                });
                        });
                    ui.label(RichText::new(tx(
                        "缺少目标值的设置会保持本机原样；需要的设置必须在档案中明确提供。",
                        "Settings without a target value stay unchanged; every selected setting must have an explicit profile value.",
                    )).size(12.0).color(palette().muted));
                    ui.checkbox(
                        &mut state.confirmed,
                        tx("我已核对所选当前值与目标值，并了解应用后可能留下部分变更。", "I reviewed the selected current and target values and understand a failed apply may leave partial changes."),
                    );
                    if primary(
                        ui,
                        tx("应用所选 Git 设置", "Apply selected Git settings"),
                        state.can_apply(worker_active),
                    ).clicked() {
                        apply = true;
                    }
                }
            } else {
                empty(
                    ui,
                    tx("先读取或载入 Git 档案", "Capture or load a Git profile"),
                    tx(
                        "读取仅访问本页的允许列表；载入档案后还需刷新本机值并逐项审核。",
                        "Capture reads only this page's allowlist. After loading a profile, refresh current values and review each setting.",
                    ),
                );
            }
            if let Some(report) = &state.report {
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    badge(
                        ui,
                        if report.verified && !report.cancelled {
                            tx("已验证", "Verified")
                        } else if report.cancelled {
                            tx("已取消", "Cancelled")
                        } else {
                            tx("未完全验证", "Not fully verified")
                        },
                        if report.verified && !report.cancelled { palette().wash } else { palette().danger },
                        if report.verified && !report.cancelled { palette().accent } else { palette().danger },
                    );
                    ui.label(lf!(
                        "所选 {} 项 · 已变更 {} 项",
                        "{} selected · {} changed",
                        report.selected.len(),
                        report.changed.len()
                    ));
                });
                if report.verified && !report.cancelled {
                    ui.label(tx(
                        "所选设置已应用，并通过回读核验。",
                        "Selected settings were applied and verified by reading them back.",
                    ));
                } else {
                    ui.label(RichText::new(tx(
                        "操作未完整完成，Git 配置可能已有部分变更。诊断信息：",
                        "The operation did not complete fully; some Git settings may have changed. Details:",
                    )).color(palette().danger));
                    ui.label(&report.detail);
                }
                ui.label(RichText::new(lf!(
                    "本地记录：{}",
                    "Local history: {}",
                    report.history.display()
                )).size(12.0).color(palette().muted));
                ui.label(RichText::new(tx(
                    "记录用于核查前后值，不会自动回滚 Git 配置。",
                    "The history records before and after values; Git changes are not automatically rolled back.",
                )).size(12.0).color(palette().muted));
            }
        });

        if capture {
            self.start_git_capture();
        }
        if refresh {
            self.start_git_refresh();
        }
        if load {
            self.open_file(FileOperation::LoadGitProfile);
        }
        if save {
            self.open_file(FileOperation::SaveGitProfile);
        }
        if save_report {
            self.open_file(FileOperation::SaveGitReport);
        }
        if apply {
            self.start_git_apply();
        }
    }
}

fn setting_label(key: &str) -> &str {
    match key {
        "user.name" => tx("Git 用户名", "Git name"),
        "user.email" => tx("Git 邮箱", "Git email"),
        "init.defaultBranch" => tx("默认分支", "Default branch"),
        "core.autocrlf" => "core.autocrlf",
        "core.eol" => "core.eol",
        "core.longpaths" => "core.longpaths",
        "pull.rebase" => "pull.rebase",
        "fetch.prune" => "fetch.prune",
        "push.default" => "push.default",
        _ => key,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applying_requires_a_loaded_profile_fresh_comparison_selection_and_confirmation() {
        let mut state = UiState::default();
        assert!(!state.can_apply(false));
        state.profile = Some(studio_deployment::git_config::Profile::default());
        state.current = Some(studio_deployment::git_config::Profile::default());
        state.selected.insert("user.name".into());
        state.confirmed = true;
        assert!(!state.can_apply(false));
        state
            .profile
            .as_mut()
            .unwrap()
            .settings
            .insert("user.name".into(), "Example".into());
        assert!(state.can_apply(false));
        assert!(!state.can_apply(true));
        state.confirmed = false;
        assert!(!state.can_apply(false));
    }
}
