//! Reviewable uv-managed Python tool requests.
use super::*;
use studio_deployment::{
    backup::Package,
    python_tools::{self as native, PythonTool, Request},
};

#[derive(Default)]
pub(super) struct UiState {
    pub inventory: Option<Vec<Package>>,
    pub selected: BTreeMap<String, PythonTool>,
    pub query: String,
    pub new_name: String,
    pub new_version: String,
    pub confirmed: bool,
    pub report: Option<native::Report>,
}
impl UiState {
    fn request(&self) -> Request {
        Request {
            schema_version: 1,
            tools: self.selected.values().cloned().collect(),
        }
    }
}
impl Studio {
    pub(super) fn python_tools_file_operation(
        &mut self,
        operation: FileOperation,
        path: &std::path::Path,
    ) -> Result<String, String> {
        if self.worker.is_some() {
            return Err(tx(
                "请等待当前任务结束后再保存或载入 Python 工具请求。",
                "Wait for the current task before saving or loading a Python tools request.",
            )
            .into());
        }
        let result = match operation {
            FileOperation::SavePythonRequest => {
                native::save(&self.backup.python_tools.request(), path)
            }
            FileOperation::LoadPythonRequest => native::load(path).map(|request| {
                self.backup.python_tools.selected = request
                    .tools
                    .into_iter()
                    .map(|tool| {
                        (
                            studio_deployment::normalize_pip_id(&tool.name).unwrap_or_default(),
                            tool,
                        )
                    })
                    .collect();
                self.backup.python_tools.confirmed = false;
                self.backup.python_tools.report = None;
            }),
            FileOperation::SavePythonReport => self
                .backup
                .python_tools
                .report
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("No Python tool deployment result"))
                .and_then(|report| studio_core::export_document(report, path)),
            _ => Err(anyhow::anyhow!("Not a Python tools operation")),
        };
        result.map_err(|error| format!("{error:#}"))?;
        Ok(lf!(
            "Python 工具文件已处理：{}",
            "Python tools file processed: {}",
            path.display()
        ))
    }

    fn start_python_inventory(&mut self) {
        if !cfg!(any(target_os = "macos", windows)) || self.worker.is_some() {
            return;
        }
        self.start_worker("正在读取 uv 工具", |cancel| {
            native::capture(cancel).map(WorkerResult::PythonToolsInventory)
        });
    }

    fn start_python_redeploy(&mut self) {
        if !cfg!(any(target_os = "macos", windows))
            || self.worker.is_some()
            || !self.backup.python_tools.confirmed
        {
            return;
        }
        let request = self.backup.python_tools.request();
        if let Err(error) = native::validate(&request) {
            self.notice = Some(Notice {
                text: format!("{error:#}"),
                error: true,
            });
            return;
        }
        self.backup.python_tools.confirmed = false;
        self.backup.python_tools.report = None;
        self.start_network_worker("正在重部署所选 Python 工具", move |cancel| {
            native::configure(request, cancel).map(WorkerResult::PythonTools)
        });
    }

    pub(super) fn python_tools_page(&mut self, ui: &mut egui::Ui) {
        ui.label(tx(
            "读取 uv 管理的命令行工具，逐项选定固定版本或滚动最新版，再审核重部署。",
            "Read uv-managed command tools, choose exact or rolling versions item by item, then review redeployment.",
        ));
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    cfg!(any(target_os = "macos", windows)) && self.worker.is_none(),
                    egui::Button::new(tx("读取已安装 uv 工具", "Read installed uv tools")),
                )
                .clicked()
            {
                self.start_python_inventory();
            }
            if ui
                .add_enabled(
                    self.worker.is_none(),
                    egui::Button::new(tx("载入请求…", "Load request…")),
                )
                .clicked()
            {
                self.open_file(FileOperation::LoadPythonRequest);
            }
            ui.label(RichText::new("macOS · Windows · uv tool").color(palette().muted));
        });
        if let Some(inventory) = &self.backup.python_tools.inventory {
            ui.label(lf!(
                "已读取 {} 项 uv 工具；采集只读，不会安装或升级。",
                "Read {} uv tools; inventory does not install or upgrade.",
                inventory.len()
            ));
            ui.add(
                egui::TextEdit::singleline(&mut self.backup.python_tools.query)
                    .hint_text(tx("搜索包名…", "Search package names…")),
            );
            let query = self.backup.python_tools.query.to_ascii_lowercase();
            let visible: Vec<_> = inventory
                .iter()
                .filter(|item| item.name.to_ascii_lowercase().contains(&query))
                .collect();
            egui::ScrollArea::vertical()
                .id_salt("uv-python-tools")
                .max_height(210.0)
                .show_rows(
                    ui,
                    ui.spacing().interact_size.y,
                    visible.len(),
                    |ui, range| {
                        for item in &visible[range] {
                            let key =
                                studio_deployment::normalize_pip_id(&item.name).unwrap_or_default();
                            let mut included = self.backup.python_tools.selected.contains_key(&key);
                            ui.horizontal_wrapped(|ui| {
                                if ui
                                    .checkbox(
                                        &mut included,
                                        format!("{} · {}", item.name, item.version),
                                    )
                                    .changed()
                                {
                                    if included {
                                        self.backup.python_tools.selected.insert(
                                            key.clone(),
                                            PythonTool {
                                                name: item.name.clone(),
                                                version: Some(item.version.clone()),
                                            },
                                        );
                                    } else {
                                        self.backup.python_tools.selected.remove(&key);
                                    }
                                    self.backup.python_tools.confirmed = false;
                                }
                                if let Some(tool) = self.backup.python_tools.selected.get_mut(&key)
                                {
                                    let mut rolling = tool.version.is_none();
                                    if ui
                                        .checkbox(&mut rolling, tx("滚动最新版", "Rolling latest"))
                                        .changed()
                                    {
                                        tool.version = if rolling {
                                            None
                                        } else {
                                            Some(item.version.clone())
                                        };
                                        self.backup.python_tools.confirmed = false;
                                    }
                                    if let Some(version) = &mut tool.version
                                        && ui
                                            .add(
                                                egui::TextEdit::singleline(version)
                                                    .desired_width(90.0)
                                                    .hint_text(tx("固定版本", "Exact version")),
                                            )
                                            .changed()
                                    {
                                        self.backup.python_tools.confirmed = false;
                                    }
                                }
                            });
                        }
                    },
                );
        } else {
            ui.label(
                RichText::new(tx(
                    "尚未读取 uv 清单；也可以手动添加可信 PyPI 包名。",
                    "No uv inventory loaded; you can also add a PyPI package name.",
                ))
                .color(palette().muted),
            );
        }

        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.backup.python_tools.new_name)
                    .desired_width(170.0)
                    .hint_text(tx("PyPI 包名", "PyPI package name")),
            );
            ui.add(
                egui::TextEdit::singleline(&mut self.backup.python_tools.new_version)
                    .desired_width(120.0)
                    .hint_text(tx(
                        "固定版本（空白=滚动）",
                        "Exact version (blank = rolling)",
                    )),
            );
            if ui
                .add_enabled(
                    !self.backup.python_tools.new_name.trim().is_empty(),
                    egui::Button::new(tx("加入请求", "Add to request")),
                )
                .clicked()
            {
                let name = self.backup.python_tools.new_name.trim().to_owned();
                let version = (!self.backup.python_tools.new_version.trim().is_empty())
                    .then(|| self.backup.python_tools.new_version.trim().to_owned());
                let tool = PythonTool {
                    name: name.clone(),
                    version,
                };
                let mut candidate = self.backup.python_tools.request();
                candidate.tools.retain(|existing| {
                    studio_deployment::normalize_pip_id(&existing.name)
                        != studio_deployment::normalize_pip_id(&name)
                });
                candidate.tools.push(tool.clone());
                match native::validate(&candidate) {
                    Ok(()) => {
                        let key = studio_deployment::normalize_pip_id(&name).unwrap_or_default();
                        self.backup.python_tools.selected.insert(key, tool);
                        self.backup.python_tools.new_name.clear();
                        self.backup.python_tools.new_version.clear();
                        self.backup.python_tools.confirmed = false;
                    }
                    Err(error) => {
                        self.notice = Some(Notice {
                            text: format!("{error:#}"),
                            error: true,
                        })
                    }
                }
            }
        });

        let request = self.backup.python_tools.request();
        let validation = native::validate(&request);
        ui.horizontal_wrapped(|ui| {
            ui.label(lf!(
                "请求中 {} 项；未选中的包会保留原样。",
                "{} requested; unselected tools remain unchanged.",
                request.tools.len()
            ));
            if ui
                .add_enabled(
                    !self.backup.python_tools.selected.is_empty(),
                    egui::Button::new(tx("清空选择", "Clear selection")),
                )
                .clicked()
            {
                self.backup.python_tools.selected.clear();
                self.backup.python_tools.confirmed = false;
            }
        });
        egui::CollapsingHeader::new(tx("审核本次请求", "Review this request")).show(ui, |ui| {
            if let Err(error) = &validation {
                ui.colored_label(palette().danger, format!("{error:#}"));
            } else {
                for tool in &request.tools {
                    ui.monospace(format!(
                        "{}=={}",
                        tool.name,
                        tool.version.as_deref().unwrap_or("latest")
                    ));
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            for (operation, label) in [
                (
                    FileOperation::SavePythonRequest,
                    tx("保存请求…", "Save request…"),
                ),
                (
                    FileOperation::SavePythonReport,
                    tx("保存结果…", "Save result…"),
                ),
            ] {
                let enabled = self.worker.is_none()
                    && match operation {
                        FileOperation::SavePythonRequest => validation.is_ok(),
                        FileOperation::SavePythonReport => {
                            self.backup.python_tools.report.is_some()
                        }
                        _ => false,
                    };
                if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                    self.open_file(operation);
                }
            }
        });
        ui.label(RichText::new(tx(
            "执行需要已有 uv 与 Python，不会安装运行时；只使用标准 uv 工具目录，忽略自定义 UV_/XDG 路径。固定版本冲突时 uv 会重建所选工具环境，并可能替换同名入口；项目依赖、pipx 环境及自动回滚不在此流程内。",
            "Execution requires existing uv and Python; runtimes are not installed. Only standard uv tool directories are used; custom UV_/XDG paths are ignored. Exact-version conflicts recreate the selected tool environment and may replace colliding entry points. Project dependencies, pipx environments and automatic rollback are outside this flow.",
        )).size(12.0).color(palette().muted));
        ui.checkbox(&mut self.backup.python_tools.confirmed, tx(
            "我已审核 PyPI 来源与版本，并允许执行下载及可能运行包的构建代码",
            "I reviewed the PyPI source and versions and allow downloads and package build code to run",
        ));
        if primary(
            ui,
            tx("重部署所选 Python 工具", "Redeploy selected Python tools"),
            cfg!(any(target_os = "macos", windows))
                && self.worker.is_none()
                && self.backup.python_tools.confirmed
                && validation.is_ok(),
        )
        .clicked()
        {
            self.start_python_redeploy();
        }
        if let Some(report) = &self.backup.python_tools.report {
            ui.label(lf!(
                "最近请求 {} 项 · 已验证 {} · 结果 {} 项",
                "{} requested · verified {} · {} result items",
                report.requested,
                report.verified,
                report.tools.len()
            ));
            for tool in &report.tools {
                ui.monospace(format!(
                    "{} {} · {}",
                    tool.name, tool.installed_version, tool.action
                ));
            }
            ui.label(&report.detail);
        }
    }
}
