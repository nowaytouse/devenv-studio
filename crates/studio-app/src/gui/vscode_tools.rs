use super::*;
use anyhow::Context;
use studio_deployment::{vscode_settings::EditorSettingsTarget, vscode_tools as native};

fn editor_settings_target_label(target: EditorSettingsTarget) -> &'static str {
    match target {
        EditorSettingsTarget::VsCodeStable => tx("VS Code 稳定版", "VS Code Stable"),
        EditorSettingsTarget::VsCodeInsiders => tx("VS Code Insiders", "VS Code Insiders"),
        EditorSettingsTarget::Cursor => tx("Cursor 稳定版", "Cursor Stable"),
    }
}

#[derive(Default)]
pub(super) struct SettingsUiState {
    pub profile: Option<studio_deployment::vscode_settings::SettingsProfile>,
    pub target: EditorSettingsTarget,
    pub snapshot: Option<studio_deployment::vscode_settings::SettingsSnapshot>,
    pub selected: BTreeSet<String>,
    pub confirmed: bool,
    pub report: Option<studio_deployment::vscode_settings::SettingsApplyReport>,
}

impl SettingsUiState {
    fn load_profile(&mut self, profile: studio_deployment::vscode_settings::SettingsProfile) {
        self.selected.clear();
        self.profile = Some(profile);
        self.snapshot = None;
        self.confirmed = false;
        self.report = None;
    }

    fn select_target(&mut self, target: EditorSettingsTarget) {
        if self.target == target {
            return;
        }
        self.target = target;
        self.snapshot = None;
        self.selected.clear();
        self.confirmed = false;
        self.report = None;
    }
}

#[derive(Default)]
pub(super) struct UiState {
    pub selected: BTreeSet<String>,
    pub versions: BTreeMap<String, String>,
    pub search: String,
    pub confirmed: bool,
    pub report: Option<native::Report>,
    pub settings: SettingsUiState,
}

impl UiState {
    fn request(&self) -> native::Request {
        native::Request {
            schema_version: 1,
            extensions: self
                .selected
                .iter()
                .map(|id| native::ExtensionRequest {
                    id: id.clone(),
                    version: self
                        .versions
                        .get(id)
                        .filter(|version| !version.is_empty())
                        .cloned(),
                })
                .collect(),
        }
    }

    pub(super) fn clear_selection(&mut self) {
        self.selected.clear();
        self.versions.clear();
        self.confirmed = false;
    }
}

impl Studio {
    pub(super) fn vscode_tools_file_operation(
        &mut self,
        operation: FileOperation,
        path: &std::path::Path,
    ) -> Result<String, String> {
        if self.worker.is_some() {
            return Err(tx(
                "请等待当前任务结束后再处理编辑器或扩展文件。",
                "Wait for the current task before handling editor or extension files.",
            )
            .into());
        }
        let state = &mut self.backup.vscode_tools;
        let result = match operation {
            FileOperation::SaveVscodeRequest => native::save(&state.request(), path),
            FileOperation::LoadVscodeRequest => native::load(path).map(|request| {
                state.clear_selection();
                for extension in request.extensions {
                    state.selected.insert(extension.id.clone());
                    state
                        .versions
                        .insert(extension.id, extension.version.unwrap_or_default());
                }
                state.report = None;
            }),
            FileOperation::SaveVscodeReport => state
                .report
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("No VS Code extensions deployment result"))
                .and_then(|report| studio_core::export_document(report, path)),
            FileOperation::SaveVscodeSettingsProfile => state
                .settings
                .profile
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("No editor settings profile to save"))
                .and_then(|profile| {
                    studio_deployment::vscode_settings::save_settings_profile(profile, path)
                }),
            FileOperation::LoadVscodeSettingsProfile => {
                studio_deployment::vscode_settings::load_settings_profile(path).map(|profile| {
                    state.settings.load_profile(profile);
                })
            }
            _ => Err(anyhow::anyhow!("Not a VS Code file operation")),
        };
        result.map_err(|error| format!("{error:#}"))?;
        Ok(lf!(
            "编辑器或扩展文件已处理：{}",
            "Editor or extension file processed: {}",
            path.display()
        ))
    }

    fn start_vscode_redeploy(&mut self) {
        if !cfg!(target_os = "macos")
            || self.worker.is_some()
            || !self.backup.vscode_tools.confirmed
        {
            return;
        }
        let prepared = (|| -> anyhow::Result<_> {
            let snapshot = self
                .backup
                .snapshot
                .as_ref()
                .context("Capture or load a Mac inventory first")?;
            anyhow::ensure!(snapshot.platform == "macos", "A Mac inventory is required");
            let provider = snapshot
                .providers
                .iter()
                .find(|provider| provider.kind == "vscode")
                .context("The saved Mac inventory has no VS Code source")?;
            anyhow::ensure!(
                provider.status == "complete",
                "A complete VS Code inventory is required before deployment"
            );
            let path = self
                .backup
                .path
                .as_ref()
                .context("Save the Mac inventory before deployment")?;
            anyhow::ensure!(
                studio_deployment::backup::load(path)? == *snapshot,
                "Saved Mac inventory changed; capture or load it again"
            );
            let request = self.backup.vscode_tools.request();
            native::validate(&request)?;
            let available: BTreeSet<_> = provider
                .packages
                .iter()
                .map(|package| package.name.as_str())
                .collect();
            anyhow::ensure!(
                request
                    .extensions
                    .iter()
                    .all(|extension| available.contains(extension.id.as_str())),
                "Every selected extension must appear in the reviewed Mac inventory"
            );
            Ok(request)
        })();
        match prepared {
            Ok(request) => {
                self.backup.vscode_tools.confirmed = false;
                self.backup.vscode_tools.report = None;
                self.start_mutating_worker("正在重部署 VS Code 扩展", move |cancel| {
                    native::configure(request, cancel).map(WorkerResult::VSCodeTools)
                });
            }
            Err(error) => {
                self.notice = Some(Notice {
                    text: format!("{error:#}"),
                    error: true,
                });
            }
        }
    }

    fn start_vscode_settings_capture(&mut self) {
        if self.worker.is_some() {
            return;
        }
        let target = self.backup.vscode_tools.settings.target;
        self.start_worker("Reading editor user settings", move |cancel| {
            studio_deployment::vscode_settings::capture_settings(target, &cancel)
                .map(WorkerResult::VSCodeSettingsCapture)
        });
    }

    fn start_vscode_settings_apply(&mut self) {
        let state = &self.backup.vscode_tools.settings;
        if self.worker.is_some() || !state.confirmed || state.selected.is_empty() {
            return;
        }
        let Some(profile) = state.profile.clone() else {
            return;
        };
        let Some(current) = state.snapshot.clone() else {
            return;
        };
        let selected = state.selected.clone();
        self.backup.vscode_tools.settings.confirmed = false;
        self.backup.vscode_tools.settings.report = None;
        self.start_mutating_worker("Applying reviewed editor settings", move |cancel| {
            studio_deployment::vscode_settings::apply_settings(
                &profile, &selected, &current, &cancel,
            )
            .map(WorkerResult::VSCodeSettingsApply)
        });
    }

    fn vscode_settings_page(&mut self, ui: &mut egui::Ui) {
        let profile = self.backup.vscode_tools.settings.profile.clone();
        let mut snapshot = self.backup.vscode_tools.settings.snapshot.clone();
        let mut report = self.backup.vscode_tools.settings.report.clone();
        let idle = self.worker.is_none();
        let supported = cfg!(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux"
        ));

        egui::CollapsingHeader::new(tx(
            "编辑器设置 · 安全合并",
            "Editor settings · safe merge",
        ))
        .default_open(true)
        .show(ui, |ui| {
            egui::Frame::new()
                .fill(palette().surface)
                .stroke(Stroke::new(1.0, palette().line))
                .corner_radius(10)
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.label(tx(
                        "选择编辑器并读取其默认用户设置。21 项通用设置可在支持的编辑器间迁移；应用时只改勾选项，并保留注释、其他设置与未选中的本机值。",
                        "Choose an editor and read its default user settings. The 21 common settings can move between supported editors; applying changes only checked items and preserves comments, unrelated settings and unselected local values.",
                    ));
                    let current_target = self.backup.vscode_tools.settings.target;
                    let mut next_target = current_target;
                    ui.horizontal_wrapped(|ui| {
                        ui.label(tx("目标编辑器", "Editor"));
                        ui.add_enabled_ui(idle, |ui| {
                            egui::ComboBox::from_id_salt("editor-settings-target")
                                .selected_text(editor_settings_target_label(current_target))
                                .show_ui(ui, |ui| {
                                    for target in EditorSettingsTarget::ALL {
                                        ui.selectable_value(
                                            &mut next_target,
                                            target,
                                            editor_settings_target_label(target),
                                        );
                                    }
                                });
                        });
                    });
                    if next_target != current_target {
                        self.backup.vscode_tools.settings.select_target(next_target);
                        snapshot = None;
                        report = None;
                    }
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled(idle && supported, egui::Button::new(tx("读取当前设置", "Read current settings")))
                            .clicked()
                        {
                            self.start_vscode_settings_capture();
                        }
                        if ui
                            .add_enabled(idle && supported, egui::Button::new(tx("载入档案…", "Load profile…")))
                            .clicked()
                        {
                            self.open_file(FileOperation::LoadVscodeSettingsProfile);
                        }
                        if ui
                            .add_enabled(idle && profile.is_some(), egui::Button::new(tx("保存档案…", "Save profile…")))
                            .clicked()
                        {
                            self.open_file(FileOperation::SaveVscodeSettingsProfile);
                        }
                        if !self.backup.vscode_tools.settings.selected.is_empty()
                            && ui
                                .add_enabled(idle, egui::Button::new(tx("清空选择", "Clear selection")))
                                .clicked()
                        {
                            self.backup.vscode_tools.settings.selected.clear();
                            self.backup.vscode_tools.settings.confirmed = false;
                        }
                        ui.label(lf!(
                            "已选 {} 项",
                            "{} selected",
                            self.backup.vscode_tools.settings.selected.len()
                        ));
                    });

                    if let Some(snapshot) = &snapshot {
                        ui.label(RichText::new(lf!(
                            "当前文件：{} · 检出 {} 项白名单设置",
                            "Current file: {} · {} allowlisted settings found",
                            snapshot.path().display(),
                            snapshot.settings.len()
                        )).color(palette().muted));
                    } else {
                        ui.label(RichText::new(tx(
                            "先读取当前设置，或载入档案后再刷新本机对比。",
                            "Read current settings first, or load a profile and then refresh the local comparison.",
                        )).color(palette().muted));
                    }

                    if let Some(profile) = &profile {
                        if profile.settings.is_empty() {
                            ui.label(RichText::new(tx(
                                "档案中没有可迁移的白名单设置。",
                                "The profile contains no allowlisted settings to migrate.",
                            )).color(palette().muted));
                        } else {
                            egui::ScrollArea::vertical()
                                .id_salt("vscode-settings-review")
                                .max_height(240.0)
                                .show(ui, |ui| {
                                    for (key, target) in &profile.settings {
                                        let current = snapshot
                                            .as_ref()
                                            .and_then(|value| value.settings.get(key));
                                        let matches = current == Some(target);
                                        let current_text = current
                                            .map(ToString::to_string)
                                            .unwrap_or_else(|| tx("未设置", "not set").to_owned());
                                        let target_text = target.to_string();
                                        egui::Frame::new()
                                            .fill(palette().surface)
                                            .stroke(Stroke::new(1.0, palette().line))
                                            .corner_radius(8)
                                            .inner_margin(egui::Margin::symmetric(10, 7))
                                            .show(ui, |ui| {
                                                ui.horizontal_wrapped(|ui| {
                                                    let mut checked = self
                                                        .backup
                                                        .vscode_tools
                                                        .settings
                                                        .selected
                                                        .contains(key);
                                                    if ui.checkbox(&mut checked, key).changed() {
                                                        if checked {
                                                            self.backup
                                                                .vscode_tools
                                                                .settings
                                                                .selected
                                                                .insert(key.clone());
                                                        } else {
                                                            self.backup
                                                                .vscode_tools
                                                                .settings
                                                                .selected
                                                                .remove(key);
                                                        }
                                                        self.backup
                                                            .vscode_tools
                                                            .settings
                                                            .confirmed = false;
                                                    }
                                                    if snapshot.is_none() {
                                                        badge(
                                                            ui,
                                                            tx("待读取", "Read first"),
                                                            palette().line,
                                                            palette().muted,
                                                        );
                                                    } else if matches {
                                                        badge(
                                                            ui,
                                                            tx("已一致", "Matches"),
                                                            palette().accent,
                                                            palette().on_accent,
                                                        );
                                                    } else {
                                                        badge(
                                                            ui,
                                                            tx("将更新", "Will update"),
                                                            palette().line,
                                                            palette().muted,
                                                        );
                                                    }
                                                });
                                                ui.label(lf!(
                                                    "当前：{current} · 档案：{target}",
                                                    "Current: {current} · Profile: {target}",
                                                    current = current_text,
                                                    target = target_text
                                                ));
                                            });
                                        ui.add_space(4.0);
                                    }
                                });
                        }
                    }

                    ui.label(RichText::new(lf!(
                        "应用前请关闭 {editor} 并检查每项差异。只会写入选中的白名单值；扩展、账户、代理、终端命令和密钥不包含在内，操作不会自动回滚。",
                        "Close {editor} and review each difference before applying. Only selected allowlisted values are written; extensions, accounts, proxies, terminal commands and secrets are excluded. Changes are not automatically rolled back.",
                        editor = editor_settings_target_label(self.backup.vscode_tools.settings.target)
                    )).color(palette().muted));
                    ui.checkbox(
                        &mut self.backup.vscode_tools.settings.confirmed,
                        tx(
                            "我已检查比较结果，并了解设置写入不会自动回滚",
                            "I reviewed the comparison and understand settings writes are not automatically rolled back",
                        ),
                    );
                    if primary(
                        ui,
                        tx("应用所选设置", "Apply selected settings"),
                        supported
                            && idle
                            && profile.is_some()
                            && snapshot.is_some()
                            && !self.backup.vscode_tools.settings.selected.is_empty()
                            && self.backup.vscode_tools.settings.confirmed,
                    )
                    .clicked()
                    {
                        self.start_vscode_settings_apply();
                    }
                    if let Some(report) = &report {
                        ui.separator();
                        badge(
                            ui,
                            if report.verified && report.history_complete {
                                tx("已核对", "Verified")
                            } else {
                                tx("需检查", "Review")
                            },
                            if report.verified && report.history_complete {
                                palette().accent
                            } else {
                                palette().line
                            },
                            if report.verified && report.history_complete {
                                palette().on_accent
                            } else {
                                palette().muted
                            },
                        );
                        ui.label(&report.detail);
                        ui.label(RichText::new(lf!(
                            "执行历史：{}",
                            "Run history: {}",
                            report.history.display()
                        )).color(palette().muted));
                    }
                    if !supported {
                        ui.label(RichText::new(tx(
                            "此平台未启用编辑器设置读写。",
                            "Editor settings read and write are not enabled on this platform.",
                        )).color(palette().muted));
                    }
                });
        });
    }

    pub(super) fn vscode_tools_page(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new(tx(
                "编辑器设置与 VS Code 扩展",
                "Editor settings and VS Code extensions",
            ))
            .color(palette().muted),
        );
        ui.label(tx(
            "设置支持 VS Code 稳定版、Insiders 与 Cursor；扩展重部署仅在 macOS 使用已保存的扩展清单。",
            "Settings support VS Code Stable, Insiders and Cursor; extension redeployment uses the saved inventory on macOS.",
        ));
        ui.separator();
        ui.heading(tx("扩展重部署", "Extension redeployment"));

        let provider = self
            .backup
            .snapshot
            .as_ref()
            .filter(|snapshot| snapshot.platform == "macos")
            .and_then(|snapshot| {
                snapshot
                    .providers
                    .iter()
                    .find(|provider| provider.kind == "vscode")
            })
            .map(|provider| (Some(provider.status.clone()), provider.packages.clone()));
        let (provider_status, candidates) = provider.unwrap_or_default();
        if let Some(status) = provider_status.as_deref() {
            ui.label(lf!(
                "备份记录：{} · {} 个扩展。执行要求来源状态完整，并会重新读取默认配置文件。",
                "Saved source: {} · {} extensions. Deployment requires a complete inventory and reads the default profile again.",
                status,
                candidates.len()
            ));
        } else {
            ui.label(RichText::new(tx(
                "尚无已保存的 Mac VS Code 扩展清单。先采集并保存 Mac 清单。",
                "No saved Mac VS Code extensions inventory. Capture and save a Mac inventory first.",
            )).color(palette().muted));
        }

        ui.horizontal_wrapped(|ui| {
            if ui.button(tx("保存请求…", "Save request…")).clicked() {
                self.open_file(FileOperation::SaveVscodeRequest);
            }
            if ui.button(tx("载入请求…", "Load request…")).clicked() {
                self.open_file(FileOperation::LoadVscodeRequest);
            }
            if ui.button(tx("清空选择", "Clear selection")).clicked() {
                self.backup.vscode_tools.clear_selection();
            }
            ui.label(lf!(
                "已选 {} / {}",
                "{} selected / {}",
                self.backup.vscode_tools.selected.len(),
                candidates.len()
            ));
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.backup.vscode_tools.search)
                .hint_text(tx("搜索扩展 ID…", "Search extension IDs…")),
        );
        let query = self.backup.vscode_tools.search.to_ascii_lowercase();
        let visible: Vec<_> = candidates
            .iter()
            .filter(|extension| extension.name.to_ascii_lowercase().contains(&query))
            .collect();
        egui::ScrollArea::vertical()
            .id_salt("vscode-extension-inventory")
            .max_height(220.0)
            .show_rows(
                ui,
                ui.spacing().interact_size.y,
                visible.len(),
                |ui, range| {
                    for extension in &visible[range] {
                        let id = extension.name.as_str();
                        ui.push_id(id, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                let state = &mut self.backup.vscode_tools;
                                let mut selected = state.selected.contains(id);
                                if ui.checkbox(&mut selected, id).changed() {
                                    if selected {
                                        state.selected.insert(id.into());
                                        state.versions.insert(id.into(), extension.version.clone());
                                    } else {
                                        state.selected.remove(id);
                                        state.versions.remove(id);
                                    }
                                    state.confirmed = false;
                                }
                                ui.label(lf!(
                                    "备份：{version}",
                                    "Backup: {version}",
                                    version = extension.version
                                ));
                                if selected {
                                    let version = state.versions.entry(id.into()).or_default();
                                    if ui
                                        .add(
                                            egui::TextEdit::singleline(version)
                                                .desired_width(120.0)
                                                .hint_text(tx("最新版本", "latest")),
                                        )
                                        .changed()
                                    {
                                        state.confirmed = false;
                                    }
                                    ui.label(if version.is_empty() {
                                        tx("滚动最新版", "Marketplace latest")
                                    } else {
                                        tx("精确版本", "Exact version")
                                    });
                                }
                            });
                        });
                    }
                },
            );
        if candidates.is_empty() {
            ui.label(
                RichText::new(tx(
                    "当前清单没有可选扩展。",
                    "The current inventory contains no selectable extensions.",
                ))
                .color(palette().muted),
            );
        }

        let request = self.backup.vscode_tools.request();
        let request_validation = native::validate(&request);
        let source_is_ready = provider_status.as_deref() == Some("complete")
            && self.backup.path.is_some()
            && self
                .backup
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.platform == "macos");
        if !self.backup.vscode_tools.selected.is_empty()
            && let Err(error) = &request_validation
        {
            ui.colored_label(palette().danger, format!("{error:#}"));
        }
        ui.label(RichText::new(tx(
            "扩展拥有与 VS Code 相同的权限，安装可能运行扩展代码；只选择你信任的发布者。只操作稳定版 VS Code 的默认用户配置文件，不合并设置，不会自动回滚。",
            "Extensions have the same privileges as VS Code and may run extension code; select only publishers you trust. This uses the default user profile of stable VS Code, does not merge settings, and cannot roll back automatically.",
        )).color(palette().danger));
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(
                &mut self.backup.vscode_tools.confirmed,
                tx(
                    "我已审核扩展身份、版本与发布者权限，并了解无法自动回滚",
                    "I reviewed extension identities, versions and publisher privileges, and understand there is no automatic rollback",
                ),
            );
            if primary(
                ui,
                tx("重部署所选 VS Code 扩展", "Redeploy selected VS Code extensions"),
                cfg!(target_os = "macos")
                    && self.worker.is_none()
                    && source_is_ready
                    && self.backup.vscode_tools.confirmed
                    && request_validation.is_ok(),
            )
            .clicked()
            {
                self.start_vscode_redeploy();
            }
            if self.backup.vscode_tools.report.is_some()
                && ui.button(tx("保存最近结果…", "Save latest result…")).clicked()
            {
                self.open_file(FileOperation::SaveVscodeReport);
            }
        });
        if !cfg!(target_os = "macos") {
            ui.label(RichText::new(tx(
                "此原生面板仅在 macOS 启用；Windows 扩展部署继续使用可信部署工作区。",
                "This native panel is enabled only on macOS; Windows extension deployment remains in the reviewed deployment workspace.",
            )).color(palette().muted));
        }
        if let Some(report) = &self.backup.vscode_tools.report {
            ui.separator();
            ui.label(lf!(
                "最近请求 {} 项 · 已验证 {} · 已取消 {}",
                "Last request: {} · verified {} · cancelled {}",
                report.requested,
                report.verified,
                report.cancelled
            ));
            for extension in &report.extensions {
                ui.monospace(format!(
                    "{} · {} · {}",
                    extension.id,
                    extension
                        .installed_version
                        .as_deref()
                        .unwrap_or("not installed"),
                    extension.action
                ));
            }
            ui.label(&report.detail);
        }
        ui.separator();
        self.vscode_settings_page(ui);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loaded_settings_profile_starts_unselected() {
        let profile = studio_deployment::vscode_settings::SettingsProfile {
            schema_version: 1,
            settings: BTreeMap::from([("editor.fontSize".to_owned(), serde_json::json!(16))]),
        };
        let mut state = SettingsUiState::default();
        state.selected.insert("editor.tabSize".to_owned());
        state.load_profile(profile.clone());
        assert!(state.selected.is_empty());
        assert_eq!(state.profile, Some(profile));
        assert!(state.snapshot.is_none());
        assert!(!state.confirmed);
    }

    #[test]
    fn changing_editor_clears_review_state_and_keeps_migratable_profile() {
        let profile = studio_deployment::vscode_settings::SettingsProfile {
            schema_version: 1,
            settings: BTreeMap::from([("editor.fontSize".to_owned(), serde_json::json!(16))]),
        };
        let mut state = SettingsUiState::default();
        state.load_profile(profile.clone());
        state.selected.insert("editor.fontSize".to_owned());
        state.confirmed = true;
        state.report = Some(studio_deployment::vscode_settings::SettingsApplyReport {
            schema_version: 1,
            selected: vec!["editor.fontSize".to_owned()],
            changed: vec!["editor.fontSize".to_owned()],
            verified: true,
            history_complete: true,
            cancelled: false,
            detail: String::new(),
            history: std::path::PathBuf::new(),
        });

        state.select_target(EditorSettingsTarget::Cursor);

        assert_eq!(state.target, EditorSettingsTarget::Cursor);
        assert_eq!(state.profile, Some(profile));
        assert!(state.selected.is_empty());
        assert!(!state.confirmed);
        assert!(state.snapshot.is_none());
        assert!(state.report.is_none());
    }

    #[test]
    fn request_is_opt_in_and_blank_version_means_rolling_marketplace_release() {
        let mut state = UiState::default();
        assert!(state.request().extensions.is_empty());
        state.selected.insert("rust-lang.rust-analyzer".into());
        state
            .versions
            .insert("rust-lang.rust-analyzer".into(), "0.4.2100-beta.1".into());
        state.selected.insert("ms-python.python".into());
        state
            .versions
            .insert("ms-python.python".into(), String::new());
        let request = state.request();
        assert_eq!(request.extensions.len(), 2);
        let exact = request
            .extensions
            .iter()
            .find(|extension| extension.id == "rust-lang.rust-analyzer")
            .unwrap();
        let rolling = request
            .extensions
            .iter()
            .find(|extension| extension.id == "ms-python.python")
            .unwrap();
        assert_eq!(exact.version.as_deref(), Some("0.4.2100-beta.1"));
        assert_eq!(rolling.version, None);
        assert!(native::validate(&request).is_ok());
    }
}
