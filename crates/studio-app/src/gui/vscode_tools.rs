use super::*;
use anyhow::Context;
use studio_deployment::vscode_tools as native;

#[derive(Default)]
pub(super) struct UiState {
    pub selected: BTreeSet<String>,
    pub versions: BTreeMap<String, String>,
    pub search: String,
    pub confirmed: bool,
    pub report: Option<native::Report>,
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
                "请等待当前任务结束后再保存或载入 VS Code 扩展请求。",
                "Wait for the current task before saving or loading a VS Code extensions request.",
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
                    if let Some(version) = extension.version {
                        state.versions.insert(extension.id, version);
                    } else {
                        state.versions.insert(extension.id, String::new());
                    }
                }
                state.report = None;
            }),
            FileOperation::SaveVscodeReport => state
                .report
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("No VS Code extensions deployment result"))
                .and_then(|report| studio_core::export_document(report, path)),
            _ => Err(anyhow::anyhow!("Not a VS Code extensions operation")),
        };
        result.map_err(|error| format!("{error:#}"))?;
        Ok(lf!(
            "VS Code 扩展文件已处理：{}",
            "VS Code extensions file processed: {}",
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
                self.start_worker("正在重部署 VS Code 扩展", move |cancel| {
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

    pub(super) fn vscode_tools_page(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("macOS · Visual Studio Code Marketplace").color(palette().muted));
        ui.label(tx(
            "从已保存的 Mac 扩展清单中逐项选择。版本框保留备份版本时会精确安装；清空后使用 Marketplace 当前版本。",
            "Choose extensions from a saved Mac inventory. Keeping the backup version requests that exact version; clearing it installs the current Marketplace version.",
        ));

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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
