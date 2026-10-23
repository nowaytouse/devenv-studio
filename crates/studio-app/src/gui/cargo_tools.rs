use super::*;
use anyhow::Context;
use studio_deployment::cargo_tools as native;

#[derive(Default)]
pub(super) struct UiState {
    pub selected: BTreeSet<String>,
    pub versions: BTreeMap<String, String>,
    pub git_head: BTreeSet<String>,
    pub confirmed: bool,
    pub report: Option<native::Report>,
}

impl UiState {
    fn request(&self) -> native::Request {
        native::Request {
            schema_version: 1,
            tools: native::TOOLS
                .iter()
                .filter(|tool| self.selected.contains(tool.name))
                .map(|tool| native::ToolRequest {
                    name: tool.name.into(),
                    version: (!self.git_head.contains(tool.name))
                        .then(|| {
                            self.versions
                                .get(tool.name)
                                .map(String::as_str)
                                .unwrap_or_default()
                                .trim()
                        })
                        .filter(|version| !version.is_empty())
                        .map(str::to_owned),
                    git_head: self.git_head.contains(tool.name),
                })
                .collect(),
        }
    }
}

impl Studio {
    pub(super) fn cargo_tools_file_operation(
        &mut self,
        operation: FileOperation,
        path: &std::path::Path,
    ) -> Result<String, String> {
        if self.worker.is_some() {
            return Err(tx(
                "请等待当前任务结束后再保存或载入 Cargo 工具请求。",
                "Wait for the current task before saving or loading a Cargo tools request.",
            )
            .into());
        }
        let state = &mut self.backup.cargo_tools;
        let result = match operation {
            FileOperation::SaveCargoRequest => native::save(&state.request(), path),
            FileOperation::LoadCargoRequest => native::load(path).map(|request| {
                state.selected.clear();
                state.versions.clear();
                state.git_head.clear();
                for tool in request.tools {
                    state.selected.insert(tool.name.clone());
                    if let Some(version) = tool.version {
                        state.versions.insert(tool.name.clone(), version);
                    }
                    if tool.git_head {
                        state.git_head.insert(tool.name);
                    }
                }
                state.confirmed = false;
                state.report = None;
            }),
            FileOperation::SaveCargoReport => state
                .report
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("No Cargo tool deployment result"))
                .and_then(|report| studio_core::export_document(report, path)),
            _ => Err(anyhow::anyhow!("Not a Cargo tools operation")),
        };
        result.map_err(|error| format!("{error:#}"))?;
        Ok(lf!(
            "Cargo 工具文件已处理：{}",
            "Cargo tools file processed: {}",
            path.display()
        ))
    }

    fn start_cargo_redeploy(&mut self) {
        if !cfg!(target_os = "macos") || self.worker.is_some() || !self.backup.cargo_tools.confirmed
        {
            return;
        }
        let prepared = (|| -> anyhow::Result<_> {
            let snapshot = self
                .backup
                .snapshot
                .as_ref()
                .context("Capture or load a Mac inventory first")?;
            anyhow::ensure!(
                snapshot.platform == "macos",
                "A Mac inventory is required before Cargo deployment"
            );
            let path = self
                .backup
                .path
                .as_ref()
                .context("Save the Mac inventory before Cargo deployment")?;
            anyhow::ensure!(
                studio_deployment::backup::load(path)? == *snapshot,
                "Saved Mac inventory changed; capture or load it again"
            );
            let request = self.backup.cargo_tools.request();
            native::validate(&request)?;
            Ok(request)
        })();
        match prepared {
            Ok(request) => {
                self.backup.cargo_tools.confirmed = false;
                self.backup.cargo_tools.report = None;
                self.start_network_worker("正在重部署 Cargo 命令行工具", move |cancel| {
                    native::configure(request, cancel).map(WorkerResult::CargoTools)
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

    pub(super) fn cargo_tools_page(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("macOS · crates.io · reviewed Git HEAD").color(palette().muted));
        ui.label(tx(
            "只显示审核过来源的常用 Rust CLI 工具。版本留空表示 crates.io 滚动最新版；可填写精确版本（含 beta 预发布号），或选择固定上游仓库的 HEAD。",
            "Only reviewed Rust CLI sources are listed. Leave version blank for the rolling crates.io release, enter an exact version (including a beta prerelease), or choose HEAD from the fixed upstream repository.",
        ));

        let (cargo_status, installed): (Option<String>, BTreeMap<String, String>) = self
            .backup
            .snapshot
            .as_ref()
            .filter(|snapshot| snapshot.platform == "macos")
            .and_then(|snapshot| {
                snapshot
                    .providers
                    .iter()
                    .find(|provider| provider.kind == "cargo")
            })
            .map(|provider| {
                (
                    Some(provider.status.clone()),
                    provider
                        .packages
                        .iter()
                        .map(|item| (item.name.clone(), item.version.clone()))
                        .collect(),
                )
            })
            .unwrap_or_default();
        if let Some(status) = &cargo_status {
            ui.label(lf!(
                "备份记录的 Cargo 来源状态：{status} · 版本仅供选择参考，执行前会重新读取本机清单。",
                "Saved Cargo inventory status: {status} · versions are a reference; the local inventory is read again before deployment.",
                status = status
            ));
        } else {
            ui.label(RichText::new(tx(
                "尚无 macOS Cargo 备份记录。先采集并保存 Mac 清单；部署前仍会生成新的本机基线。",
                "No macOS Cargo inventory is saved yet. Capture and save a Mac inventory first; a fresh local baseline is also recorded before deployment.",
            )).color(palette().muted));
        }

        ui.horizontal_wrapped(|ui| {
            if ui.button(tx("保存请求…", "Save request…")).clicked() {
                self.open_file(FileOperation::SaveCargoRequest);
            }
            if ui.button(tx("载入请求…", "Load request…")).clicked() {
                self.open_file(FileOperation::LoadCargoRequest);
            }
            if ui.button(tx("清空选择", "Clear selection")).clicked() {
                self.backup.cargo_tools.selected.clear();
                self.backup.cargo_tools.git_head.clear();
                self.backup.cargo_tools.confirmed = false;
            }
            ui.label(lf!(
                "已选 {} / {}",
                "{} selected / {}",
                self.backup.cargo_tools.selected.len(),
                native::TOOLS.len()
            ));
        });

        egui::Grid::new("cargo-tools-grid")
            .num_columns(1)
            .spacing([8.0, 7.0])
            .show(ui, |ui| {
                for tool in &native::TOOLS {
                    ui.push_id(tool.name, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            let state = &mut self.backup.cargo_tools;
                            let mut selected = state.selected.contains(tool.name);
                            if ui.checkbox(&mut selected, tool.name).changed() {
                                if selected {
                                    state.selected.insert(tool.name.into());
                                } else {
                                    state.selected.remove(tool.name);
                                }
                                state.confirmed = false;
                            }
                            ui.label(
                                RichText::new(tool_description(tool.name)).color(palette().muted),
                            );
                            ui.label(match installed.get(tool.name) {
                                Some(version) => {
                                    lf!("備份：{version}", "Backup: {version}", version = version)
                                }
                                None if cargo_status.as_deref() == Some("complete") => {
                                    tx("备份中未安装", "Not listed in backup").to_owned()
                                }
                                None => tx("版本未知", "Version unknown").to_owned(),
                            });
                            if selected {
                                let mut git_head = state.git_head.contains(tool.name);
                                let response = ui.add_enabled_ui(!git_head, |ui| {
                                    let version =
                                        state.versions.entry(tool.name.into()).or_default();
                                    ui.add(
                                        egui::TextEdit::singleline(version)
                                            .desired_width(105.0)
                                            .hint_text(tx("滚动最新版", "latest")),
                                    )
                                });
                                if response.inner.changed() {
                                    state.confirmed = false;
                                }
                                if ui
                                    .checkbox(&mut git_head, tx("Git HEAD", "Git HEAD"))
                                    .changed()
                                {
                                    if git_head {
                                        state.git_head.insert(tool.name.into());
                                    } else {
                                        state.git_head.remove(tool.name);
                                    }
                                    state.confirmed = false;
                                }
                            }
                        });
                    });
                    ui.end_row();
                }
            });

        ui.label(RichText::new(tx(
            "Git HEAD 会先通过 HTTPS 查询默认分支，再把完整提交号传给 Cargo；会覆盖 ~/.cargo/bin 中同名程序。Cargo 构建脚本以当前用户权限运行，本功能不隔离或回滚这些脚本。",
            "Git HEAD first resolves the default branch over HTTPS and passes the full commit to Cargo; it overwrites matching binaries in ~/.cargo/bin. Cargo build scripts run with your user privileges and are not sandboxed or rolled back.",
        )).color(palette().danger));
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.backup.cargo_tools.confirmed, tx(
                "我已审核来源、版本与构建脚本权限，并了解安装失败可能留下部分变更",
                "I reviewed the sources, versions and build-script permissions, and understand failures may leave partial changes",
            ));
            if !self.backup.cargo_tools.selected.is_empty() {
                let result = native::validate(&self.backup.cargo_tools.request());
                if let Err(error) = &result {
                    ui.colored_label(palette().danger, format!("{error:#}"));
                }
                if primary(
                    ui,
                    tx("重部署所选 Cargo 工具", "Redeploy selected Cargo tools"),
                    cfg!(target_os = "macos")
                        && self.worker.is_none()
                        && self.backup.cargo_tools.confirmed
                        && self.backup.snapshot.as_ref().is_some_and(|snapshot| snapshot.platform == "macos")
                        && self.backup.path.is_some()
                        && result.is_ok(),
                ).clicked() {
                    self.start_cargo_redeploy();
                }
            }
            if self.backup.cargo_tools.report.is_some()
                && ui.button(tx("保存最近结果…", "Save latest result…")).clicked()
            {
                self.open_file(FileOperation::SaveCargoReport);
            }
        });
        if !cfg!(target_os = "macos") {
            ui.label(RichText::new(tx(
                "Cargo 原生重部署目前仅在 macOS 实机启用；Windows 继续使用可信部署工作区中的 Cargo 任务。",
                "Native Cargo redeployment is currently enabled on macOS; Windows keeps using the reviewed Cargo tasks in its deployment workspace.",
            )).color(palette().muted));
        }
        if let Some(report) = &self.backup.cargo_tools.report {
            ui.separator();
            ui.label(lf!(
                "最近请求 {} 项 · 已验证 {} · 已取消 {}",
                "Last request: {} · verified {} · cancelled {}",
                report.requested,
                report.verified,
                report.cancelled
            ));
            for tool in &report.tools {
                ui.monospace(format!(
                    "{} {} · {}{}",
                    tool.name,
                    tool.installed_version,
                    tool.action,
                    tool.source_revision
                        .as_ref()
                        .map(|revision| format!(" · {revision}"))
                        .unwrap_or_default()
                ));
            }
            ui.label(&report.detail);
        }
    }
}

fn tool_description(name: &str) -> String {
    match name {
        "ripgrep" => tx("递归搜索 · rg", "Recursive search · rg").into(),
        "fd-find" => tx("文件查找 · fd", "File finder · fd").into(),
        "bat" => tx("语法高亮查看器", "Syntax-aware viewer").into(),
        "eza" => tx("目录列表", "Directory listing").into(),
        "zoxide" => tx("目录跳转", "Directory jumper").into(),
        "starship" => tx("跨 Shell 提示符", "Cross-shell prompt").into(),
        "just" => tx("命令运行器", "Command runner").into(),
        "git-delta" => tx("Git 差异查看器", "Git diff viewer").into(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_keeps_defaults_opt_in_and_exact_channels_distinct() {
        let mut state = UiState::default();
        assert!(state.request().tools.is_empty());
        state.selected.insert("ripgrep".into());
        state
            .versions
            .insert("ripgrep".into(), "15.0.0-beta.1".into());
        state.selected.insert("bat".into());
        state.git_head.insert("bat".into());
        let request = state.request();
        assert_eq!(request.tools[0].version.as_deref(), Some("15.0.0-beta.1"));
        assert!(request.tools[1].git_head);
        assert_eq!(request.tools[1].version, None);
        assert!(native::validate(&request).is_ok());
    }
}
