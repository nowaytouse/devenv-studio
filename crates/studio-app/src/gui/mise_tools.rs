//! Select and redeploy reviewed mise core runtimes.
use super::*;
use anyhow::Context;
use std::{fs, io::Read};
use studio_deployment::mise_tools::{self as native, InstalledRuntime, Request, RuntimeRequest};

const MAX_LOCK_BYTES: u64 = 4 * 1024 * 1024;

pub(super) struct UiState {
    pub inventory: Option<Vec<InstalledRuntime>>,
    pub selected: BTreeMap<(String, String), String>,
    pub query: String,
    pub new_name: String,
    pub new_version: String,
    pub confirmed: bool,
    pub report: Option<native::Report>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            inventory: None,
            selected: BTreeMap::new(),
            query: String::new(),
            new_name: "node".into(),
            new_version: "latest".into(),
            confirmed: false,
            report: None,
        }
    }
}

impl UiState {
    fn request(&self) -> Request {
        Request {
            schema_version: 1,
            runtimes: self
                .selected
                .iter()
                .map(|((name, _), version)| RuntimeRequest {
                    name: name.clone(),
                    version: version.clone(),
                })
                .collect(),
        }
    }

    fn changed(&mut self) {
        self.confirmed = false;
        self.report = None;
    }
}

fn read_generated_file(path: &std::path::Path, max_bytes: u64) -> anyhow::Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).context("Inspect generated mise file")?;
    anyhow::ensure!(
        metadata.file_type().is_file() && metadata.len() <= max_bytes,
        "Generated mise file is not a bounded regular file"
    );
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    fs::OpenOptions::new()
        .read(true)
        .open(path)
        .context("Open generated mise file")?
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .context("Read generated mise file")?;
    anyhow::ensure!(
        bytes.len() as u64 <= max_bytes,
        "Generated mise file exceeds its size limit"
    );
    Ok(bytes)
}

impl Studio {
    pub(super) fn mise_file_operation(
        &mut self,
        operation: FileOperation,
        path: &std::path::Path,
    ) -> Result<String, String> {
        if self.worker.is_some() {
            return Err(tx(
                "请等待当前任务结束后再保存或载入 mise 运行时文件。",
                "Wait for the current task before saving or loading mise runtime files.",
            )
            .into());
        }
        let request = self.backup.mise_tools.request();
        let result: anyhow::Result<()> = match operation {
            FileOperation::SaveMiseRuntimeRequest => native::save(&request, path),
            FileOperation::LoadMiseRuntimeRequest => native::load(path).map(|request| {
                self.backup.mise_tools.selected = request
                    .runtimes
                    .into_iter()
                    .map(|runtime| {
                        let key = (runtime.name.clone(), runtime.version.clone());
                        (key, runtime.version)
                    })
                    .collect();
                self.backup.mise_tools.changed();
            }),
            FileOperation::ExportMiseRuntimeManifest => (|| {
                let bytes = if let Some(report) = &self.backup.mise_tools.report {
                    let source = report
                        .manifest
                        .as_ref()
                        .context("No resolved mise manifest in the last report")?;
                    read_generated_file(source, 1024 * 1024)?
                } else {
                    native::mise_toml(&request)?.into_bytes()
                };
                studio_core::export_bytes(&bytes, path)
            })(),
            FileOperation::ExportMiseRuntimeLock => self
                .backup
                .mise_tools
                .report
                .as_ref()
                .and_then(|report| report.lockfile.as_ref())
                .ok_or_else(|| anyhow::anyhow!("No generated mise lockfile"))
                .and_then(|source| {
                    let bytes = read_generated_file(source, MAX_LOCK_BYTES)?;
                    studio_core::export_bytes(&bytes, path)
                }),
            FileOperation::SaveMiseRuntimeReport => self
                .backup
                .mise_tools
                .report
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("No mise runtime deployment result"))
                .and_then(|report| studio_core::export_document(report, path)),
            _ => Err(anyhow::anyhow!("Not a mise runtime operation")),
        };
        result.map_err(|error| format!("{error:#}"))?;
        Ok(lf!(
            "mise 运行时文件已处理：{}",
            "mise runtime file processed: {}",
            path.display()
        ))
    }

    fn start_mise_inventory(&mut self) {
        if !cfg!(any(target_os = "macos", windows)) || self.worker.is_some() {
            return;
        }
        self.start_worker("正在读取 mise 核心运行时", |cancel| {
            native::capture(cancel).map(WorkerResult::MiseRuntimeInventory)
        });
    }

    fn start_mise_redeployment(&mut self) {
        if !cfg!(any(target_os = "macos", windows))
            || self.worker.is_some()
            || !self.backup.mise_tools.confirmed
        {
            return;
        }
        let request = self.backup.mise_tools.request();
        if let Err(error) = native::validate(&request) {
            self.notice = Some(Notice {
                text: format!("{error:#}"),
                error: true,
            });
            return;
        }
        self.backup.mise_tools.confirmed = false;
        self.backup.mise_tools.report = None;
        self.start_network_mutating_worker("正在重部署所选 mise 运行时", move |cancel| {
            native::configure(request, cancel).map(WorkerResult::MiseRuntime)
        });
    }

    pub(super) fn mise_tools_page(&mut self, ui: &mut egui::Ui) {
        ui.label(tx(
            "逐项选择 mise 内置核心运行时；固定版本、前缀和受支持渠道会先解析为具体版本，再生成跨 macOS / Windows 锁文件。",
            "Choose mise built-in core runtimes individually. Exact versions, prefixes and supported channels resolve to concrete versions before a macOS / Windows lockfile is generated.",
        ));
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    cfg!(any(target_os = "macos", windows)) && self.worker.is_none(),
                    egui::Button::new(tx("读取已安装运行时", "Read installed runtimes")),
                )
                .clicked()
            {
                self.start_mise_inventory();
            }
            if ui
                .add_enabled(
                    self.worker.is_none(),
                    egui::Button::new(tx("载入请求…", "Load request…")),
                )
                .clicked()
            {
                self.open_file(FileOperation::LoadMiseRuntimeRequest);
            }
            ui.label(RichText::new("macOS · Windows · mise core").color(palette().muted));
        });

        if let Some(inventory) = self.backup.mise_tools.inventory.clone() {
            ui.label(lf!(
                "已读取 {} 个核心运行时；采集只读。",
                "Read {} core runtimes; inventory is read-only.",
                inventory.len()
            ));
            ui.add(
                egui::TextEdit::singleline(&mut self.backup.mise_tools.query)
                    .hint_text(tx("搜索已安装运行时…", "Search installed runtimes…")),
            );
            let query = self.backup.mise_tools.query.to_ascii_lowercase();
            let visible: Vec<_> = inventory
                .into_iter()
                .filter(|runtime| {
                    native::supports_runtime(&runtime.name)
                        && runtime.name.to_ascii_lowercase().contains(&query)
                })
                .collect();
            ui.horizontal_wrapped(|ui| {
                ui.label(lf!(
                    "显示 {} 项可用运行时。",
                    "{} supported runtimes shown.",
                    visible.len()
                ));
                if ui
                    .button(tx("选择显示项作为备份", "Select shown versions for backup"))
                    .clicked()
                {
                    for runtime in &visible {
                        let key = (runtime.name.clone(), runtime.version.clone());
                        self.backup
                            .mise_tools
                            .selected
                            .entry(key)
                            .or_insert_with(|| runtime.version.clone());
                    }
                    self.backup.mise_tools.changed();
                }
            });
            egui::ScrollArea::vertical()
                .id_salt("mise-core-runtimes")
                .max_height(160.0)
                .show_rows(
                    ui,
                    ui.spacing().interact_size.y,
                    visible.len(),
                    |ui, range| {
                        for runtime in &visible[range] {
                            let key = (runtime.name.clone(), runtime.version.clone());
                            let mut selected = self.backup.mise_tools.selected.contains_key(&key);
                            ui.horizontal_wrapped(|ui| {
                                if ui
                                    .checkbox(
                                        &mut selected,
                                        format!("{} · {}", runtime.name, runtime.version),
                                    )
                                    .changed()
                                {
                                    if selected {
                                        self.backup
                                            .mise_tools
                                            .selected
                                            .insert(key.clone(), runtime.version.clone());
                                    } else {
                                        self.backup.mise_tools.selected.remove(&key);
                                    }
                                    self.backup.mise_tools.changed();
                                }
                                if runtime.scheduled_for_pruning {
                                    ui.label(
                                        RichText::new(tx("清理候选", "Prune candidate"))
                                            .color(palette().muted),
                                    );
                                }
                            });
                        }
                    },
                );
        } else {
            ui.label(
                RichText::new(tx(
                    "尚未读取本机 mise 清单；也可以从下方手动选运行时。",
                    "No local mise inventory loaded; you can select runtimes manually below.",
                ))
                .color(palette().muted),
            );
        }

        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_label(tx("运行时", "Runtime"))
                .selected_text(self.backup.mise_tools.new_name.clone())
                .show_ui(ui, |ui| {
                    for runtime in native::CORE_RUNTIMES {
                        ui.selectable_value(
                            &mut self.backup.mise_tools.new_name,
                            (*runtime).to_owned(),
                            *runtime,
                        );
                    }
                });
            ui.add(
                egui::TextEdit::singleline(&mut self.backup.mise_tools.new_version)
                    .desired_width(150.0)
                    .hint_text(tx(
                        "版本 / latest / lts / nightly / beta",
                        "Version / latest / lts / nightly / beta",
                    )),
            );
            if ui
                .add_enabled(
                    !self.backup.mise_tools.new_version.trim().is_empty(),
                    egui::Button::new(tx("加入请求", "Add to request")),
                )
                .clicked()
            {
                let name = self.backup.mise_tools.new_name.clone();
                let version = self.backup.mise_tools.new_version.trim().to_owned();
                self.backup
                    .mise_tools
                    .selected
                    .insert((name, version.clone()), version);
                self.backup.mise_tools.changed();
            }
        });

        if !self.backup.mise_tools.selected.is_empty() {
            ui.label(tx("本次请求", "This request"));
            let keys: Vec<_> = self.backup.mise_tools.selected.keys().cloned().collect();
            for key in keys {
                let mut remove = false;
                let mut edited = false;
                ui.horizontal_wrapped(|ui| {
                    ui.monospace(format!("{} · {}", key.0, key.1));
                    if let Some(version) = self.backup.mise_tools.selected.get_mut(&key) {
                        edited = ui
                            .add(
                                egui::TextEdit::singleline(version)
                                    .desired_width(150.0)
                                    .hint_text(tx(
                                        "精确版 / 前缀 / 渠道",
                                        "Exact / prefix / channel",
                                    )),
                            )
                            .changed();
                    }
                    if ui.small_button(tx("移除", "Remove")).clicked() {
                        remove = true;
                    }
                });
                if remove {
                    self.backup.mise_tools.selected.remove(&key);
                    self.backup.mise_tools.changed();
                } else if edited {
                    self.backup.mise_tools.changed();
                }
            }
        }

        let request = self.backup.mise_tools.request();
        let validation = native::validate(&request);
        ui.horizontal_wrapped(|ui| {
            ui.label(lf!(
                "已选 {} 项运行时。",
                "{} runtimes selected.",
                request.runtimes.len()
            ));
            if ui
                .add_enabled(
                    !self.backup.mise_tools.selected.is_empty(),
                    egui::Button::new(tx("清空选择", "Clear selection")),
                )
                .clicked()
            {
                self.backup.mise_tools.selected.clear();
                self.backup.mise_tools.changed();
            }
        });
        egui::CollapsingHeader::new(tx("审核生成的 mise.toml", "Review generated mise.toml"))
            .default_open(false)
            .show(ui, |ui| match (&validation, native::mise_toml(&request)) {
                (Ok(()), Ok(manifest)) => {
                    ui.monospace(manifest);
                    ui.label(tx(
                        "nightly / beta 等渠道由各运行时支持情况决定；不支持时会在安装前停止。任意 Git HEAD 继续使用来源专属流程。",
                        "nightly / beta availability depends on each runtime; unsupported requests stop before installation. Arbitrary Git HEAD stays in its source-specific workflow.",
                    ));
                }
                (Err(error), _) => {
                    ui.colored_label(palette().danger, format!("{error:#}"));
                }
                (_, Err(error)) => {
                    ui.colored_label(palette().danger, format!("{error:#}"));
                }
            });
        ui.horizontal_wrapped(|ui| {
            for operation in [
                FileOperation::SaveMiseRuntimeRequest,
                FileOperation::ExportMiseRuntimeManifest,
                FileOperation::ExportMiseRuntimeLock,
                FileOperation::SaveMiseRuntimeReport,
            ] {
                let enabled = self.worker.is_none()
                    && match operation {
                        FileOperation::SaveMiseRuntimeRequest
                        | FileOperation::ExportMiseRuntimeManifest => validation.is_ok(),
                        FileOperation::ExportMiseRuntimeLock => self
                            .backup
                            .mise_tools
                            .report
                            .as_ref()
                            .is_some_and(|report| report.lockfile.is_some()),
                        FileOperation::SaveMiseRuntimeReport => {
                            self.backup.mise_tools.report.is_some()
                        }
                        _ => false,
                    };
                let label = match operation {
                    FileOperation::ExportMiseRuntimeManifest
                        if self.backup.mise_tools.report.is_some() =>
                    {
                        tx("导出已解析 mise.toml", "Export resolved mise.toml")
                    }
                    FileOperation::ExportMiseRuntimeManifest => {
                        tx("导出请求 mise.toml", "Export request mise.toml")
                    }
                    _ => operation.title(),
                };
                if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                    self.open_file(operation);
                }
            }
        });
        ui.label(
            RichText::new(tx(
                "需要本机已安装 mise。执行会解析版本、生成仅含所选 core 工具的跨平台锁文件，再按锁安装并回读。工具安装到 mise 用户目录，但不会启用 Shell 路径或改全局/项目配置；成功后导出的固定版 mise.toml 与 mise.lock 配套。配置、插件、任务、模板和钩子不会导入；安装可能下载文件，失败后部分运行时可能保留且无自动回滚。",
                "Requires mise already installed. Execution resolves versions, generates a cross-platform lockfile containing only selected core tools, installs from that lock and reads back the result. Tools are installed in mise's user data directory without changing shell activation or global/project config; after success, the exported pinned mise.toml matches the mise.lock. Configs, plugins, tasks, templates and hooks are not imported; downloads may occur and partial runtimes may remain after failure without automatic rollback.",
            ))
            .size(12.0)
            .color(palette().muted),
        );
        ui.checkbox(
            &mut self.backup.mise_tools.confirmed,
            tx(
                "我已审核运行时与版本渠道，并允许解析、下载和安装。",
                "I reviewed the runtimes and channels and allow resolution, downloads and installation.",
            ),
        );
        if primary(
            ui,
            tx("重部署所选运行时", "Redeploy selected runtimes"),
            cfg!(any(target_os = "macos", windows))
                && self.worker.is_none()
                && validation.is_ok()
                && self.backup.mise_tools.confirmed,
        )
        .clicked()
        {
            self.start_mise_redeployment();
        }
        if let Some(report) = &self.backup.mise_tools.report {
            ui.label(lf!(
                "最近请求 {} 项 · 已验证 {} 项。",
                "{} requested · {} verified.",
                report.requested,
                report.runtimes.len()
            ));
            for runtime in &report.runtimes {
                ui.monospace(format!(
                    "{} {} · {}",
                    runtime.name, runtime.resolved_version, runtime.action
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
    fn gui_request_preserves_multiple_versions_and_moving_channels_for_one_runtime() {
        let mut state = UiState::default();
        state
            .selected
            .insert(("node".into(), "24.11.0".into()), "24.11.0".into());
        state
            .selected
            .insert(("node".into(), "rolling".into()), "nightly".into());

        let request = state.request();
        assert!(native::validate(&request).is_ok());
        assert_eq!(request.runtimes.len(), 2);
        let manifest = native::mise_toml(&request).unwrap();
        assert!(manifest.contains("[\"24.11.0\", \"nightly\"]"));
    }

    #[test]
    fn generated_mise_exports_are_bounded_regular_files() {
        use std::{
            fs,
            path::PathBuf,
            sync::atomic::{AtomicU64, Ordering},
        };

        struct Temp(PathBuf);
        impl Drop for Temp {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "devenv-studio-mise-export-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        let temp = Temp(path);
        let file = temp.0.join("mise.lock");
        fs::write(&file, b"lockfile").unwrap();
        assert_eq!(read_generated_file(&file, 16).unwrap(), b"lockfile");
        assert!(read_generated_file(&file, 4).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let link = temp.0.join("linked.lock");
            symlink(&file, &link).unwrap();
            assert!(read_generated_file(&link, 16).is_err());
        }
    }
}
