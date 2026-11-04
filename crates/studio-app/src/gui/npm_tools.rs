use super::*;
use anyhow::Context;
use std::collections::BTreeSet;
use studio_deployment::npm_tools as native;

#[derive(Default)]
pub(super) struct UiState {
    pub(super) query: String,
    pub(super) selected: BTreeSet<String>,
    pub(super) tag: String,
    pub(super) confirmed: bool,
    pub(super) report: Option<native::Report>,
}

impl UiState {
    fn request(
        &self,
        snapshot: &studio_deployment::backup::Snapshot,
    ) -> anyhow::Result<native::Request> {
        anyhow::ensure!(
            snapshot.platform == "macos",
            "A saved Mac inventory is required"
        );
        let provider = snapshot
            .providers
            .iter()
            .find(|provider| provider.kind == "npm")
            .context("The saved Mac inventory has no npm source")?;
        anyhow::ensure!(
            provider.status == "complete",
            "A complete npm inventory is required before deployment"
        );
        let packages: Vec<_> = provider
            .packages
            .iter()
            .filter(|package| self.selected.contains(&package.name))
            .cloned()
            .collect();
        anyhow::ensure!(
            packages.len() == self.selected.len(),
            "Every selected package must still appear in the saved inventory"
        );
        let request = native::Request {
            packages,
            tag: (!self.tag.trim().is_empty()).then(|| self.tag.trim().to_owned()),
        };
        native::validate(&request)?;
        Ok(request)
    }
}

impl Studio {
    fn start_npm_redeploy(&mut self) {
        if !cfg!(target_os = "macos") || self.worker.is_some() || !self.backup.npm_tools.confirmed {
            return;
        }
        let prepared = (|| -> anyhow::Result<_> {
            let snapshot = self
                .backup
                .snapshot
                .as_ref()
                .context("Capture or load a Mac inventory first")?;
            let request = self.backup.npm_tools.request(snapshot)?;
            let path = self
                .backup
                .path
                .as_ref()
                .context("Save the Mac inventory before deployment")?;
            anyhow::ensure!(
                studio_deployment::backup::load(path)? == *snapshot,
                "Saved Mac inventory changed; capture or load it again"
            );
            Ok(request)
        })();
        match prepared {
            Ok(request) => {
                self.backup.npm_tools.confirmed = false;
                self.backup.npm_tools.report = None;
                self.start_mutating_worker(
                    tx("正在重部署 NPM 全局包", "Redeploying npm global packages"),
                    move |cancel| native::configure(request, cancel).map(WorkerResult::NpmTools),
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

    pub(super) fn npm_tools_page(&mut self, ui: &mut egui::Ui) {
        let Some(snapshot) = self.backup.snapshot.as_ref() else {
            empty(
                ui,
                tx("尚无环境清单", "No environment inventory yet"),
                tx(
                    "先采集或载入 Mac 备份，再逐项选择 NPM 全局包。",
                    "Capture or load a Mac backup, then select npm global packages.",
                ),
            );
            return;
        };
        if snapshot.platform != "macos" {
            ui.label(
                RichText::new(tx(
                    "此恢复面板需要 macOS 清单。Windows 的 NPM 任务在 Windows 配置工作区中审核。",
                    "This restore panel requires a macOS inventory. Review Windows npm tasks in the Windows configuration workspace.",
                ))
                .color(palette().muted),
            );
            return;
        }
        let Some(provider) = snapshot
            .providers
            .iter()
            .find(|provider| provider.kind == "npm")
        else {
            ui.colored_label(
                palette().danger,
                tx("清单缺少 NPM 来源。", "The inventory has no npm source."),
            );
            return;
        };
        if provider.status != "complete" {
            ui.colored_label(
                palette().danger,
                tx(
                    "NPM 清单不完整；请重新采集后再重部署。",
                    "The npm inventory is incomplete. Capture it again before redeployment.",
                ),
            );
            return;
        }

        let available: BTreeSet<_> = provider
            .packages
            .iter()
            .map(|package| package.name.as_str())
            .collect();
        self.backup
            .npm_tools
            .selected
            .retain(|name| available.contains(name.as_str()));

        ui.label(RichText::new(tx(
            "只使用已保存清单中的包名。空渠道恢复记录版本；填写 latest、beta、next、nightly 或该包实际提供的其他 dist-tag，可滚动到对应渠道；所有已选包共用同一标签，不支持的标签会报告失败。",
            "Only package identities from the saved inventory are used. Leave the tag blank for exact versions, or enter latest, beta, next, nightly, or another dist-tag published for the package. One tag applies to every selected package; unsupported tags are reported as failures.",
        )).size(12.0).color(palette().muted));
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    self.worker.is_none() && !provider.packages.is_empty(),
                    egui::Button::new(tx("选择当前筛选项", "Select visible")),
                )
                .clicked()
            {
                self.backup.npm_tools.selected.extend(
                    provider
                        .packages
                        .iter()
                        .filter(|package| {
                            let query = self.backup.npm_tools.query.to_ascii_lowercase();
                            query.is_empty()
                                || format!("{} {}", package.name, package.version)
                                    .to_ascii_lowercase()
                                    .contains(&query)
                        })
                        .map(|package| package.name.clone()),
                );
                self.backup.npm_tools.confirmed = false;
                self.backup.npm_tools.report = None;
            }
            if ui
                .add_enabled(
                    !self.backup.npm_tools.selected.is_empty(),
                    egui::Button::new(tx("清空选择", "Clear selection")),
                )
                .clicked()
            {
                self.backup.npm_tools.selected.clear();
                self.backup.npm_tools.confirmed = false;
                self.backup.npm_tools.report = None;
            }
            ui.label(lf!(
                "已选 {} / {} 项",
                "{} selected / {} packages",
                self.backup.npm_tools.selected.len(),
                provider.packages.len()
            ));
        });
        ui.horizontal(|ui| {
            ui.label(tx("滚动渠道", "Rolling dist-tag"));
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.backup.npm_tools.tag)
                    .desired_width(180.0)
                    .hint_text("latest · beta · next · nightly"),
            );
            if response.changed() {
                self.backup.npm_tools.confirmed = false;
                self.backup.npm_tools.report = None;
            }
            ui.label(
                RichText::new(tx("留空 = 清单版本", "blank = saved versions"))
                    .color(palette().muted),
            );
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.backup.npm_tools.query)
                .hint_text(tx("搜索 NPM 包名或版本…", "Search npm package or version…")),
        );
        let query = self.backup.npm_tools.query.to_ascii_lowercase();
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
            .id_salt("npm-global-packages")
            .max_height(190.0)
            .show_rows(
                ui,
                ui.text_style_height(&egui::TextStyle::Body),
                visible.len(),
                |ui, range| {
                    for package in &visible[range] {
                        let mut selected = self.backup.npm_tools.selected.contains(&package.name);
                        ui.horizontal(|ui| {
                            if ui.checkbox(&mut selected, &package.name).changed() {
                                if selected {
                                    self.backup.npm_tools.selected.insert(package.name.clone());
                                } else {
                                    self.backup.npm_tools.selected.remove(&package.name);
                                }
                                self.backup.npm_tools.confirmed = false;
                                self.backup.npm_tools.report = None;
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

        let request = self.backup.npm_tools.request(snapshot);
        if let Err(error) = &request
            && !self.backup.npm_tools.selected.is_empty()
        {
            ui.colored_label(palette().danger, error.to_string());
        }
        ui.label(RichText::new(tx(
            "使用现有 Homebrew Node.js / npm 前缀；写入前保存基线，安装后回读核验。安装脚本默认关闭，依赖脚本的包可能无法工作；私有源认证不导入，传递依赖不锁定；不会自动回滚。",
            "Uses the existing Homebrew Node.js / npm prefix, records a baseline and verifies the installed state. Lifecycle scripts are disabled; packages that need them may not work. Private registry credentials are not imported and transitive versions are not locked. No automatic rollback is available.",
        )).size(12.0).color(palette().muted));
        ui.checkbox(
            &mut self.backup.npm_tools.confirmed,
            tx(
                "我已审核所选包与渠道，并了解网络安装、安装脚本关闭和无自动回滚",
                "I reviewed the packages and channel and understand network installs, disabled scripts and no automatic rollback",
            ),
        );
        if primary(
            ui,
            tx(
                "在这台 Mac 上重部署所选 NPM 包",
                "Redeploy selected npm packages on this Mac",
            ),
            cfg!(target_os = "macos")
                && self.worker.is_none()
                && request.is_ok()
                && self.backup.path.is_some()
                && self.backup.npm_tools.confirmed,
        )
        .clicked()
        {
            self.start_npm_redeploy();
        }
        if let Some(report) = &self.backup.npm_tools.report {
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
                .id_salt("npm-global-results")
                .max_height(150.0)
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
                                package.action
                            ));
                        }
                    },
                );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_requires_complete_saved_mac_inventory_and_opt_in_packages() {
        let package = studio_deployment::backup::Package {
            name: "@scope/tool".into(),
            version: "1.2.3".into(),
        };
        let mut state = UiState::default();
        state.selected.insert(package.name.clone());
        let mut snapshot = studio_deployment::backup::Snapshot {
            schema_version: 1,
            captured_at: "1".into(),
            platform: "macos".into(),
            providers: studio_deployment::backup::provider_kinds("macos")
                .unwrap()
                .iter()
                .map(|kind| studio_deployment::backup::ProviderSnapshot {
                    kind: (*kind).into(),
                    status: "complete".into(),
                    detail: "complete".into(),
                    packages: if *kind == "npm" {
                        vec![package.clone()]
                    } else {
                        vec![]
                    },
                })
                .collect(),
            warnings: vec![],
        };
        assert!(state.request(&snapshot).is_ok());
        snapshot.platform = "windows".into();
        assert!(state.request(&snapshot).is_err());
    }
}
