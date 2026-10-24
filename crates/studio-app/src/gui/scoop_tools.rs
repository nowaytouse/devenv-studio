use super::*;
use anyhow::Context;
use std::collections::BTreeSet;
use studio_deployment::scoop_tools as native;

#[derive(Default)]
pub(super) struct UiState {
    pub(super) query: String,
    pub(super) selected: BTreeSet<String>,
    pub(super) confirmed: bool,
    pub(super) report: Option<native::Report>,
}

impl UiState {
    fn request(
        &self,
        snapshot: &studio_deployment::backup::Snapshot,
    ) -> anyhow::Result<native::Request> {
        anyhow::ensure!(
            snapshot.platform == "windows",
            "A saved Windows inventory is required"
        );
        let provider = snapshot
            .providers
            .iter()
            .find(|provider| provider.kind == "scoop")
            .context("The saved Windows inventory has no Scoop source")?;
        anyhow::ensure!(
            matches!(provider.status.as_str(), "complete" | "partial"),
            "A usable Scoop inventory is required before redeployment"
        );
        let available: BTreeSet<_> = provider
            .packages
            .iter()
            .map(|package| package.name.as_str())
            .collect();
        anyhow::ensure!(
            self.selected
                .iter()
                .all(|name| available.contains(name.as_str())),
            "Selected Scoop packages are not in the saved inventory"
        );
        let request = native::Request {
            packages: provider
                .packages
                .iter()
                .filter(|package| self.selected.contains(&package.name))
                .cloned()
                .collect(),
        };
        native::validate(&request)?;
        Ok(request)
    }
}

impl Studio {
    fn start_scoop_redeploy(&mut self) {
        if !cfg!(windows) || self.worker.is_some() || !self.backup.scoop_tools.confirmed {
            return;
        }
        let prepared = (|| -> anyhow::Result<_> {
            let snapshot = self
                .backup
                .snapshot
                .as_ref()
                .context("Capture or load a Windows inventory first")?;
            let request = self.backup.scoop_tools.request(snapshot)?;
            let path = self
                .backup
                .path
                .as_ref()
                .context("Save the Windows inventory before deployment")?;
            anyhow::ensure!(
                studio_deployment::backup::load(path)? == *snapshot,
                "Saved Windows inventory changed; capture or load it again"
            );
            Ok(request)
        })();
        match prepared {
            Ok(request) => {
                self.backup.scoop_tools.confirmed = false;
                self.backup.scoop_tools.report = None;
                self.start_worker(
                    tx("正在重部署 Scoop 包", "Redeploying Scoop packages"),
                    move |cancel| native::configure(request, cancel).map(WorkerResult::ScoopTools),
                );
            }
            Err(error) => {
                self.notice = Some(Notice {
                    text: format!("{error:#}"),
                    error: true,
                });
            }
        }
    }

    pub(super) fn scoop_tools_page(&mut self, ui: &mut egui::Ui) {
        let Some(snapshot) = self.backup.snapshot.as_ref() else {
            empty(
                ui,
                tx(
                    "尚无 Windows 环境清单",
                    "No Windows environment inventory yet",
                ),
                tx(
                    "先采集或载入并保存 Windows 清单，再审核 Scoop 包。",
                    "Capture or load and save a Windows inventory before reviewing Scoop packages.",
                ),
            );
            return;
        };
        if snapshot.platform != "windows" {
            ui.label(
                RichText::new(tx(
                    "Scoop 面板需要 Windows 清单。macOS 预览不会检查 Windows。",
                    "The Scoop panel requires a Windows inventory. A macOS preview does not inspect Windows.",
                ))
                .color(palette().muted),
            );
            return;
        }
        let Some(provider) = snapshot
            .providers
            .iter()
            .find(|provider| provider.kind == "scoop")
        else {
            ui.colored_label(
                palette().danger,
                tx(
                    "清单缺少 Scoop 来源。",
                    "The inventory has no Scoop source.",
                ),
            );
            return;
        };
        if !matches!(provider.status.as_str(), "complete" | "partial") {
            ui.colored_label(
                palette().danger,
                tx(
                    "Scoop 清单不可用于重部署；请检查状态并重新采集。",
                    "The Scoop inventory cannot be used for redeployment. Review its status and capture it again.",
                ),
            );
            return;
        }
        if provider.status == "partial" {
            ui.colored_label(
                palette().muted,
                tx(
                    "这是部分清单。只保留了可审核的官方来源用户级包。",
                    "This is a partial inventory. Only reviewable user packages from official buckets were retained.",
                ),
            );
        }

        let available: BTreeSet<_> = provider
            .packages
            .iter()
            .map(|package| package.name.as_str())
            .collect();
        self.backup
            .scoop_tools
            .selected
            .retain(|name| available.contains(name.as_str()));

        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    self.worker.is_none() && !provider.packages.is_empty(),
                    egui::Button::new(tx("选择当前筛选项", "Select visible")),
                )
                .clicked()
            {
                self.backup.scoop_tools.selected.extend(
                    provider
                        .packages
                        .iter()
                        .filter(|package| {
                            let query = self.backup.scoop_tools.query.to_ascii_lowercase();
                            query.is_empty()
                                || format!("{} {}", package.name, package.version)
                                    .to_ascii_lowercase()
                                    .contains(&query)
                        })
                        .map(|package| package.name.clone()),
                );
                self.backup.scoop_tools.confirmed = false;
                self.backup.scoop_tools.report = None;
            }
            if ui
                .add_enabled(
                    !self.backup.scoop_tools.selected.is_empty(),
                    egui::Button::new(tx("清空选择", "Clear selection")),
                )
                .clicked()
            {
                self.backup.scoop_tools.selected.clear();
                self.backup.scoop_tools.confirmed = false;
                self.backup.scoop_tools.report = None;
            }
            ui.label(lf!(
                "已选 {} / {} 项",
                "{} selected / {} packages",
                self.backup.scoop_tools.selected.len(),
                provider.packages.len()
            ));
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.backup.scoop_tools.query).hint_text(tx(
                "搜索 Scoop 包名或版本…",
                "Search Scoop package or version…",
            )),
        );
        let query = self.backup.scoop_tools.query.to_ascii_lowercase();
        let visible: Vec<_> = provider
            .packages
            .iter()
            .filter(|package| {
                query.is_empty()
                    || format!("{} {}", package.name, package.version)
                        .to_ascii_lowercase()
                        .contains(&query)
            })
            .collect();
        egui::ScrollArea::vertical()
            .id_salt("scoop-packages")
            .max_height(190.0)
            .show_rows(
                ui,
                ui.text_style_height(&egui::TextStyle::Body),
                visible.len(),
                |ui, range| {
                    for package in &visible[range] {
                        let mut selected = self.backup.scoop_tools.selected.contains(&package.name);
                        ui.horizontal(|ui| {
                            if ui.checkbox(&mut selected, &package.name).changed() {
                                if selected {
                                    self.backup
                                        .scoop_tools
                                        .selected
                                        .insert(package.name.clone());
                                } else {
                                    self.backup.scoop_tools.selected.remove(&package.name);
                                }
                                self.backup.scoop_tools.confirmed = false;
                                self.backup.scoop_tools.report = None;
                            }
                            ui.label(
                                RichText::new(&package.version)
                                    .monospace()
                                    .color(palette().muted),
                            );
                        });
                    }
                },
            );

        let request = self.backup.scoop_tools.request(snapshot);
        if let Err(error) = &request
            && !self.backup.scoop_tools.selected.is_empty()
        {
            ui.colored_label(palette().danger, error.to_string());
        }
        ui.label(
            RichText::new(tx(
                "只恢复清单中所选的精确版本；目标机器必须已注册对应官方 bucket。Scoop 清单脚本可能运行安装步骤并安装依赖。",
                "Restores only selected exact versions from the saved inventory. The matching official bucket must already be registered. Scoop manifests can run install steps and add dependencies.",
            ))
            .size(12.0)
            .color(palette().muted),
        );
        ui.checkbox(
            &mut self.backup.scoop_tools.confirmed,
            tx(
                "我已审核所选包，并了解安装脚本、依赖变更和无自动回滚",
                "I reviewed the selected packages and understand install scripts, dependency changes and no automatic rollback",
            ),
        );
        if primary(
            ui,
            tx(
                "在这台 Windows 上重部署所选 Scoop 包",
                "Redeploy selected Scoop packages on this Windows machine",
            ),
            cfg!(windows)
                && self.worker.is_none()
                && request.is_ok()
                && self.backup.path.is_some()
                && self.backup.scoop_tools.confirmed,
        )
        .clicked()
        {
            self.start_scoop_redeploy();
        }
        if !cfg!(windows) {
            ui.label(
                RichText::new(tx(
                    "当前是预览模式；实际 Scoop 重部署需要在 Windows 上运行。",
                    "This is preview mode; native Scoop redeployment must run on Windows.",
                ))
                .color(palette().muted),
            );
        }
        if let Some(report) = &self.backup.scoop_tools.report {
            ui.add_space(6.0);
            ui.label(lf!(
                "最近结果：请求 {} 项 · 已核验 {} · 已取消 {}",
                "Latest result: {} requested · verified {} · cancelled {}",
                report.requested,
                report.verified,
                report.cancelled
            ));
            ui.label(&report.detail);
            egui::ScrollArea::vertical()
                .id_salt("scoop-results")
                .max_height(140.0)
                .show_rows(
                    ui,
                    ui.text_style_height(&egui::TextStyle::Body),
                    report.packages.len(),
                    |ui, range| {
                        for package in &report.packages[range] {
                            ui.label(format!(
                                "{} · {} → {} · {}",
                                package.name,
                                package.requested,
                                package.installed.as_deref().unwrap_or("—"),
                                action_label(&package.action)
                            ));
                        }
                    },
                );
        }
    }
}

fn action_label(action: &str) -> &str {
    match action {
        "already satisfied" => tx("已符合", "already satisfied"),
        "installed exact version" => tx("已安装精确版本", "installed exact version"),
        "version mismatch" => tx("版本不匹配", "version mismatch"),
        _ => tx("未安装", "not installed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(status: &str) -> studio_deployment::backup::Snapshot {
        studio_deployment::backup::Snapshot {
            schema_version: 2,
            captured_at: "1".into(),
            platform: "windows".into(),
            providers: studio_deployment::backup::WINDOWS_PROVIDERS
                .iter()
                .map(|kind| studio_deployment::backup::ProviderSnapshot {
                    kind: (*kind).into(),
                    status: if *kind == "scoop" {
                        status.into()
                    } else {
                        "unavailable".into()
                    },
                    packages: if *kind == "scoop" && matches!(status, "complete" | "partial") {
                        vec![studio_deployment::backup::Package {
                            name: "main/7zip".into(),
                            version: "24.09".into(),
                        }]
                    } else {
                        vec![]
                    },
                    detail: if *kind == "scoop" && status == "complete" {
                        "Collected installed package identities and versions.".into()
                    } else if *kind == "scoop" && status == "partial" {
                        "Some entries were omitted because their identities or versions are unsupported.".into()
                    } else if *kind == "scoop" {
                        "Command output was invalid; package list was not captured.".into()
                    } else {
                        "Tool not found in supported locations.".into()
                    },
                })
                .collect(),
            warnings: vec![
                "Inventory only: tool binaries, credentials, configuration, source code and virtual environments are not included.".into(),
                "Only supported standard installations are inspected; unavailable does not prove uninstalled.".into(),
                "Some providers were not completely captured; their entries are not a complete environment.".into(),
                "Pip captures distributions visible to one selected registered standard CPython interpreter; other interpreters, virtual environments and editable projects are not backed up.".into(),
            ],
        }
    }

    #[test]
    fn request_uses_only_selected_packages_from_a_usable_saved_windows_inventory() {
        let inventory = snapshot("partial");
        let mut state = UiState::default();
        state.selected.insert("main/7zip".into());
        let request = state.request(&inventory).unwrap();
        assert_eq!(request.packages[0].name, "main/7zip");
        assert!(state.request(&snapshot("failed")).is_err());
        state.selected.insert("main/not-saved".into());
        assert!(state.request(&inventory).is_err());
    }
}
