use super::*;
use studio_deployment::topgrade as native;

#[derive(Default)]
pub(super) struct UiState {
    pub profile: Option<native::Profile>,
    pub attachments: String,
    pub selected: BTreeSet<String>,
    pub confirmed: bool,
    pub extracted: Option<PathBuf>,
}

impl UiState {
    pub(super) fn accept(&mut self, profile: native::Profile) {
        self.profile = Some(profile);
        self.selected.clear();
        self.confirmed = false;
        self.extracted = None;
    }

    fn can_extract(&self, busy: bool) -> bool {
        !busy
            && self.confirmed
            && !self.selected.is_empty()
            && self.profile.as_ref().is_some_and(|profile| {
                self.selected
                    .iter()
                    .all(|path| profile.files.iter().any(|file| &file.path == path))
            })
    }
}

impl Studio {
    pub(super) fn start_topgrade_capture(&mut self) {
        if self.worker.is_some() {
            return;
        }
        let attachments: Vec<PathBuf> = self
            .backup
            .topgrade
            .attachments
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            .collect();
        self.start_worker("正在采集 Topgrade 环境", move |cancel| {
            anyhow::ensure!(!cancel.load(Ordering::Acquire), "Topgrade backup cancelled");
            let home = native::default_home()?;
            let profile = native::capture(&home, &attachments)?;
            Ok(WorkerResult::TopgradeCapture(profile))
        });
    }

    pub(super) fn topgrade_file_operation(
        &mut self,
        operation: FileOperation,
        path: &std::path::Path,
    ) -> Result<String, String> {
        if self.worker.is_some() {
            return Err(tx("请等待当前任务结束。", "Wait for the current task.").into());
        }
        match operation {
            FileOperation::LoadTopgrade => {
                let profile = native::load(path).map_err(|error| format!("{error:#}"))?;
                self.backup.topgrade.accept(profile);
            }
            FileOperation::SaveTopgrade => {
                let profile = self.backup.topgrade.profile.as_ref().ok_or_else(|| {
                    tx(
                        "请先采集或载入 Topgrade 迁移包。",
                        "Capture or load a Topgrade package first.",
                    )
                    .to_owned()
                })?;
                native::save(profile, path).map_err(|error| format!("{error:#}"))?;
            }
            FileOperation::ExtractTopgrade => {
                if !self.backup.topgrade.can_extract(false) {
                    return Err(tx(
                        "请先逐文件选择并确认还原范围。",
                        "Select files and confirm the extraction scope first.",
                    )
                    .into());
                }
                let state = &mut self.backup.topgrade;
                let profile = state
                    .profile
                    .as_ref()
                    .ok_or_else(|| "Topgrade package unavailable".to_owned())?;
                state.confirmed = false;
                state.extracted = None;
                native::restore(profile, &state.selected, path)
                    .map_err(|error| format!("{error:#}"))?;
                state.extracted = Some(path.to_owned());
            }
            _ => return Err("Not a Topgrade operation".into()),
        }
        Ok(lf!(
            "Topgrade 文件操作已完成：{}",
            "Topgrade file operation completed: {}",
            path.display()
        ))
    }

    pub(super) fn topgrade_page(&mut self, ui: &mut egui::Ui) {
        let busy = self.worker.is_some();
        let mut capture = false;
        let mut load = false;
        let mut save = false;
        let mut extract = false;
        surface_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.heading(tx("Topgrade 环境迁移", "Topgrade environment migration"));
            ui.label(tx("保留主配置、分段配置与明确添加的本地脚本，在新设备上审核重建。", "Keep the main config, drop-in configs and explicitly attached local scripts for review on another device."));
            ui.label(RichText::new(tx("迁移包包含原始文件与本机路径，请保存在私人位置。还原只写入新目录，不安装 Topgrade，不运行更新或脚本，也不会覆盖当前配置。", "Packages contain original files and local paths; keep them private. Extraction writes a new directory only. It does not install Topgrade, run updates or scripts, or overwrite active configuration.")).size(12.0).color(palette().muted));
            let state = &mut self.backup.topgrade;
            egui::CollapsingHeader::new(tx("添加本地脚本或依赖文件", "Attach local scripts or dependency files"))
                .id_salt("topgrade-attachments").show(ui, |ui| {
                    ui.label(tx("配置中的自定义命令不会被自动解析。把所需文件的绝对路径逐行填入；仅支持当前主目录内的普通文本文件，不跟随符号链接。", "Custom commands are not parsed automatically. Enter one absolute file path per line. Only regular text files inside your home directory are supported; symbolic links are rejected."));
                    ui.add_enabled(!busy, egui::TextEdit::multiline(&mut state.attachments)
                        .desired_rows(3).desired_width(f32::INFINITY).hint_text("/Users/you/.local/bin/example-update"));
                });
            ui.horizontal_wrapped(|ui| {
                capture = ui.add_enabled(!busy, egui::Button::new(tx("采集 Topgrade 配置与附件", "Capture config and attachments"))).clicked();
                load = ui.add_enabled(!busy, egui::Button::new(tx("载入迁移包…", "Load package…"))).clicked();
                save = ui.add_enabled(!busy && state.profile.is_some(), egui::Button::new(tx("保存迁移包…", "Save package…"))).clicked();
            });
            if let Some(profile) = &state.profile {
                ui.label(lf!("已载入 {} 个文件 · 已选择 {} 个", "{} files loaded · {} selected", profile.files.len(), state.selected.len()));
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(!busy, egui::Button::new(tx("选择全部文件", "Select all files"))).clicked() {
                        state.selected = profile.files.iter().map(|file| file.path.clone()).collect();
                        state.confirmed = false;
                        state.extracted = None;
                    }
                    if ui.add_enabled(!busy, egui::Button::new(tx("清空文件选择", "Clear file selection"))).clicked() {
                        state.selected.clear();
                        state.confirmed = false;
                        state.extracted = None;
                    }
                });
                egui::ScrollArea::vertical().id_salt("topgrade-files").min_scrolled_height(220.0).max_height(290.0).auto_shrink([false, false]).show(ui, |ui| {
                    for file in &profile.files {
                        let mut selected = state.selected.contains(&file.path);
                        ui.horizontal_wrapped(|ui| {
                            if ui.add_enabled(!busy, egui::Checkbox::new(&mut selected, &file.path)).changed() {
                                if selected { state.selected.insert(file.path.clone()); }
                                else { state.selected.remove(&file.path); }
                                state.confirmed = false;
                                state.extracted = None;
                            }
                            ui.label(RichText::new(lf!("{} 字节", "{} bytes", file.contents.len())).size(11.0).color(palette().muted));
                            if file.executable { ui.label(tx("原为可执行文件", "Originally executable")); }
                        });
                        egui::CollapsingHeader::new(tx("查看原文", "Review original text")).id_salt((&file.path, "contents"))
                            .show(ui, |ui| { ui.add(egui::Label::new(RichText::new(&file.contents).monospace()).wrap()); });
                    }
                });
                ui.label(RichText::new(tx("文件不会被执行；Unix 上不恢复脚本执行位。安装相应工具、核对主目录路径与依赖后，再自行启用；二进制、缓存和项目工作区不在迁移包中。", "Files are never executed; Unix executable bits are not restored. Install required tools and review home paths and dependencies before enabling them. Binaries, caches and project workspaces are not included.")).size(12.0).color(palette().muted));
                ui.add_enabled(!busy, egui::Checkbox::new(&mut state.confirmed, tx("我已审核所选原文，确认仅还原到一个全新的目录。", "I reviewed the selected files and confirm extraction into a new directory only.")));
                extract = primary(ui, tx("还原所选文件到新目录…", "Extract selected files to a new directory…"), state.can_extract(busy)).clicked();
                if let Some(path) = &state.extracted {
                    ui.label(lf!("文件已回读核验：{}。尚未安装或启用。", "File readback verified: {}. Nothing has been installed or activated.", path.display()));
                }
            } else {
                ui.label(tx("支持 ~/.config/topgrade.toml、~/.config/topgrade/topgrade.toml 及对应 topgrade.d/*.toml；外部 include、远程主机和第三方工具需另行审核。", "Supports ~/.config/topgrade.toml, ~/.config/topgrade/topgrade.toml and their topgrade.d/*.toml files. Review external includes, remote hosts and third-party tools separately."));
            }
        });
        if capture {
            self.start_topgrade_capture();
        }
        if load {
            self.open_file(FileOperation::LoadTopgrade);
        }
        if save {
            self.open_file(FileOperation::SaveTopgrade);
        }
        if extract {
            self.open_file(FileOperation::ExtractTopgrade);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loaded_package_never_carries_extraction_approval() {
        let mut state = UiState::default();
        state.accept(native::Profile {
            schema_version: 1,
            files: vec![native::Entry {
                path: ".config/topgrade.toml".into(),
                contents: "[misc]\n".into(),
                executable: false,
            }],
        });
        assert!(!state.can_extract(false));
        state.selected.insert(".config/topgrade.toml".into());
        state.confirmed = true;
        assert!(state.can_extract(false));
        assert!(!state.can_extract(true));
        state.selected.insert("unknown".into());
        assert!(!state.can_extract(false));
        state.extracted = Some(PathBuf::from("previous-review"));
        state.accept(state.profile.clone().unwrap());
        assert!(state.extracted.is_none());
        assert!(state.selected.is_empty() && !state.confirmed && !state.can_extract(false));
    }
}
