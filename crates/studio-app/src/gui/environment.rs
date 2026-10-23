//! Native configuration workbench; Mac selections are independent of the Windows workspace.
use super::*;
use studio_deployment::environment as native;

impl Studio {
    pub(super) fn environment_export(
        &self,
        operation: FileOperation,
        path: &std::path::Path,
    ) -> Result<String, String> {
        let result = match operation {
            FileOperation::ExportBrewfile => {
                native::selected_brew(self.backup.snapshot.as_ref(), &self.backup.brew_selected)
                    .and_then(|entries| native::export_brewfile(&entries, path))
            }
            FileOperation::ExportWinget => self
                .deployment_catalog
                .as_ref()
                .map_err(|e| anyhow::anyhow!(e.clone()))
                .and_then(|_| studio_deployment::build_plan(self.target, &self.deployment_selected))
                .and_then(|plan| native::export_winget(&plan, path)),
            FileOperation::SaveBundleReport => self
                .backup
                .bundle_report
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("No Mac configuration result"))
                .and_then(|report| studio_core::export_document(report, path)),
            _ => Err(anyhow::anyhow!("Not an environment export")),
        };
        result.map_err(|e| format!("{e:#}"))?;
        Ok(lf!("已保存：{}", "Saved: {}", path.display()))
    }

    fn start_bundle(&mut self) {
        if !cfg!(target_os = "macos") || self.worker.is_some() || !self.backup.brew_confirm {
            return;
        }
        let prepared = (|| -> anyhow::Result<_> {
            let snapshot = self.backup.snapshot.as_ref().ok_or_else(|| {
                anyhow::anyhow!("Load or capture and save a reviewed Mac backup first")
            })?;
            anyhow::ensure!(
                snapshot.platform == "macos",
                "A Mac backup is required before configuring this Mac"
            );
            let path = self
                .backup
                .path
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Save the backup before configuration"))?;
            anyhow::ensure!(
                &studio_deployment::backup::load(path)? == snapshot,
                "Saved backup changed; capture or load it again"
            );
            native::selected_brew(Some(snapshot), &self.backup.brew_selected)
        })();
        match prepared {
            Ok(entries) => {
                self.backup.bundle_report = None;
                self.backup.brew_confirm = false;
                self.start_worker("正在配置 macOS 开发环境", move |cancel| {
                    native::configure_brew(entries, cancel).map(WorkerResult::Bundle)
                });
            }
            Err(error) => {
                self.notice = Some(Notice {
                    text: format!("{error:#}"),
                    error: true,
                })
            }
        }
    }

    pub(super) fn environment_page(&mut self, ui: &mut egui::Ui) {
        ui.heading(tx("开发环境配置", "Developer environment configuration"));
        ui.label(tx("选择目标平台，再审核该平台实际使用的工具清单。备份、配置和结果分别保留。", "Choose a platform and review its native tool inventory. Backups, configuration and results are separate."));
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(tx("备份 / 载入环境", "Back up / load environment"))
                .clicked()
            {
                self.page = Page::Backup;
            }
            if ui
                .button(tx("Windows 配置工作区", "Windows configuration workspace"))
                .clicked()
            {
                self.page = Page::Recipes;
            }
            if ui
                .button(tx(
                    "导出所选 WinGet 清单…",
                    "Export selected WinGet inventory…",
                ))
                .clicked()
            {
                self.open_file(FileOperation::ExportWinget);
            }
        });
        ui.label(RichText::new(tx("Windows 使用 WinGet、Cargo、uv、Pip、NPM 与 VS Code 的可信目录；在 Windows 方案中逐项选择并审核。WinGet 导出只含所选软件身份，其他提供程序仍保存在应用档案中。", "Windows uses reviewed WinGet, Cargo, uv, Pip, NPM and VS Code tasks. Select and review them in the Windows workspace. WinGet exports contain selected application identities; other providers remain in the app profile.")).size(12.0).color(palette().muted));
        ui.separator();
        ui.heading(tx("macOS · Homebrew Bundle", "macOS · Homebrew Bundle"));
        ui.label(tx("从基础开发工具或 Mac 备份中选择 formula/cask。版本仅作备份记录；通过原生 Bundle 安装缺少的工具，再独立检查已安装状态。", "Select formulae/casks from starter tools or a Mac backup. Versions remain backup evidence. Native Bundle installs missing tools and independently checks their presence."));
        ui.label(RichText::new(tx("需要已安装且可用的 Homebrew。配置可以下载软件、运行安装器并改变依赖；--no-upgrade 不保证依赖完全不升级。不执行清理、卸载或导入文件中的任意 Ruby / Shell / 钩子，也没有自动回滚；取消后子安装器可能仍在运行。Cargo/uv/NPM/扩展的原生 Mac 配置尚未接入。", "A working Homebrew installation is required. Configuration may download software, run installers and change dependencies; --no-upgrade cannot guarantee unchanged dependencies. No cleanup, uninstall, or arbitrary Ruby, shell or hooks from imported files are run. Descendant installers may continue after cancellation. Native Mac configuration of Cargo/uv/NPM/extensions is not yet integrated.")).size(12.0).color(palette().muted));
        let candidates = match native::brew_candidates(self.backup.snapshot.as_ref()) {
            Ok(value) => value,
            Err(error) => {
                ui.colored_label(palette().danger, format!("{error:#}"));
                return;
            }
        };
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(tx("选择基础开发工具", "Select starter tools"))
                .clicked()
                && let Ok(starter) = native::brew_candidates(None)
            {
                self.backup
                    .brew_selected
                    .extend(starter.iter().map(|e| e.id()));
                self.backup.brew_confirm = false;
            }
            if ui
                .button(tx("清空 Mac 选择", "Clear Mac selections"))
                .clicked()
            {
                self.backup.brew_selected.clear();
                self.backup.brew_confirm = false;
            }
            ui.label(lf!(
                "Mac 已选 {} 项 / 候选 {} 项",
                "Mac selected {} / candidates {}",
                self.backup.brew_selected.len(),
                candidates.len()
            ));
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.backup.brew_query)
                .hint_text(tx("搜索 Mac 工具…", "Search Mac tools…")),
        );
        let query = self.backup.brew_query.to_ascii_lowercase();
        let visible: Vec<_> = candidates
            .iter()
            .filter(|e| e.name.to_ascii_lowercase().contains(&query))
            .collect();
        egui::ScrollArea::vertical()
            .id_salt("mac-config-tools")
            .max_height(220.0)
            .show_rows(
                ui,
                ui.spacing().interact_size.y,
                visible.len(),
                |ui, range| {
                    for entry in &visible[range] {
                        let id = entry.id();
                        let mut selected = self.backup.brew_selected.contains(&id);
                        ui.push_id(&id, |ui| {
                            if ui
                                .checkbox(
                                    &mut selected,
                                    format!(
                                        "{} · {}",
                                        entry.name,
                                        if entry.kind == "brew_cask" {
                                            "cask"
                                        } else {
                                            "formula"
                                        }
                                    ),
                                )
                                .changed()
                            {
                                if selected {
                                    self.backup.brew_selected.insert(id.clone());
                                } else {
                                    self.backup.brew_selected.remove(&id);
                                }
                                self.backup.brew_confirm = false;
                            }
                        });
                    }
                },
            );
        let entries =
            native::selected_brew(self.backup.snapshot.as_ref(), &self.backup.brew_selected);
        if let Ok(entries) = &entries {
            egui::CollapsingHeader::new(tx("审核生成的 Brewfile", "Review generated Brewfile"))
                .show(ui, |ui| {
                    if let Ok(text) = native::brewfile(entries) {
                        ui.monospace(text);
                    }
                });
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    entries.is_ok() && self.worker.is_none(),
                    egui::Button::new(tx("导出 Brewfile…", "Export Brewfile…")),
                )
                .clicked()
            {
                self.open_file(FileOperation::ExportBrewfile);
            }
            if ui
                .add_enabled(
                    self.backup.bundle_report.is_some(),
                    egui::Button::new(tx("保存配置结果…", "Save configuration result…")),
                )
                .clicked()
            {
                self.open_file(FileOperation::SaveBundleReport);
            }
        });
        ui.label(tx("执行前需要保存并审核 Mac 清单。载入的清单可以来自另一台 Mac，不能代表本机当前状态。现有备份不被覆盖。", "Save and review a Mac inventory before applying. Loaded backups may come from another Mac and do not prove this machine's current state. Existing backups are preserved."));
        ui.checkbox(&mut self.backup.brew_confirm, tx("我已审核所选工具，并了解依赖变更与无自动回滚的范围", "I reviewed the selected tools and understand dependency changes and the lack of automatic rollback"));
        if primary(
            ui,
            tx(
                "在这台 Mac 上配置所选工具",
                "Configure selected tools on this Mac",
            ),
            cfg!(target_os = "macos")
                && self.worker.is_none()
                && entries.is_ok()
                && self.backup.brew_confirm
                && self.backup.path.is_some()
                && self
                    .backup
                    .snapshot
                    .as_ref()
                    .is_some_and(|s| s.platform == "macos"),
        )
        .clicked()
        {
            self.start_bundle();
        }
        if let Some(report) = &self.backup.bundle_report {
            ui.label(lf!(
                "最近配置：{} 项 · 已验证 {} · 已取消 {}",
                "Last configuration: {} requested · verified {} · cancelled {}",
                report.requested,
                report.verified,
                report.cancelled
            ));
            ui.label(&report.detail);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preview() -> Studio {
        Studio {
            hybrid: hybrid::HybridState::default(),
            backup: backup::BackupUiState::default(),
            power: power::PowerUiState::default(),
            storage_status: None,
            preferences: Preferences::default(),
            saved_preferences: Preferences::default(),
            preferences_error: None,
            last_operation: None,
            history: Ok(Vec::new()),
            catalog: Ok(studio_core::load_catalog().unwrap()),
            deployment_catalog: Ok(studio_deployment::catalog().unwrap()),
            winget: studio_deployment::Capability {
                available: false,
                detail: "Synthetic preview".into(),
            },
            host: HostInfo {
                name: "Synthetic preview".into(),
                target: None,
                can_apply: false,
                explanation: "Preview only".into(),
            },
            target: Target::Win11Ltsc2024,
            selected: BTreeSet::new(),
            deployment_selected: BTreeSet::new(),
            power_selected: BTreeSet::new(),
            setting_states: BTreeMap::new(),
            deployment_states: BTreeMap::new(),
            last_event: None,
            page: Page::Environment,
            category: String::new(),
            query: String::new(),
            dialog: None,
            notice: None,
            recovery: Vec::new(),
            recovery_error: None,
            worker: None,
            screenshot: None,
            frames: 0,
        }
    }

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> anyhow::Result<Self> {
            let directory = Self(std::env::temp_dir().join(format!(
                "studio-environment-gui-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos()
            )));
            std::fs::create_dir(&directory.0)?;
            Ok(directory)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn snapshot() -> studio_deployment::backup::Snapshot {
        serde_json::from_slice(include_bytes!(
            "../../../../examples/mac-development-backup.json"
        ))
        .unwrap()
    }

    #[test]
    fn mac_configuration_starts_empty_and_refuses_unreviewed_or_unsaved_requests()
    -> anyhow::Result<()> {
        let directory = TestDir::new()?;
        let mut app = preview();
        let context = egui::Context::default();
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| app.environment_page(ui));
        });
        output.textures_delta.clear();
        assert!(app.backup.brew_selected.is_empty());
        assert!(!app.backup.brew_confirm);
        assert!(app.worker.is_none());

        app.backup.brew_selected.insert("brew_formula:git".into());
        app.backup.snapshot = Some(snapshot());
        app.start_bundle();
        assert!(app.worker.is_none(), "Unreviewed requests cannot start");
        app.backup.brew_confirm = true;
        app.start_bundle();
        assert!(app.worker.is_none(), "An unsaved inventory cannot start");
        #[cfg(target_os = "macos")]
        assert!(app.notice.as_ref().is_some_and(|notice| notice.error));

        app.backup.path = Some(directory.0.join("absent-backup.json"));
        app.start_bundle();
        assert!(
            app.worker.is_none(),
            "A missing saved inventory cannot start"
        );
        #[cfg(target_os = "macos")]
        assert!(app.notice.as_ref().is_some_and(|notice| notice.error));

        let saved = snapshot();
        let path = directory.0.join("saved-environment.json");
        studio_deployment::backup::save(&saved, &path)?;
        app.backup.path = Some(path);
        app.backup.snapshot.as_mut().unwrap().captured_at = "1792800000".into();
        app.start_bundle();
        assert!(app.worker.is_none(), "Changed saved inventory cannot start");
        #[cfg(target_os = "macos")]
        assert!(app.notice.as_ref().is_some_and(|notice| notice.error));

        app.backup.snapshot.as_mut().unwrap().platform = "windows".into();
        app.start_bundle();
        assert!(
            app.worker.is_none(),
            "A Windows inventory cannot configure a Mac"
        );
        #[cfg(target_os = "macos")]
        assert!(app.notice.as_ref().is_some_and(|notice| notice.error));
        assert!(app.selected.is_empty() && app.deployment_selected.is_empty());
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn non_mac_hosts_refuse_even_reviewed_saved_mac_requests() -> anyhow::Result<()> {
        let directory = TestDir::new()?;
        let mut app = preview();
        let snapshot = snapshot();
        let path = directory.0.join("mac-environment.json");
        studio_deployment::backup::save(&snapshot, &path)?;
        app.backup.snapshot = Some(snapshot);
        app.backup.path = Some(path);
        app.backup.brew_selected.insert("brew_formula:git".into());
        app.backup.brew_confirm = true;
        app.start_bundle();
        assert!(app.worker.is_none());
        assert!(app.backup.bundle_report.is_none());
        assert!(app.backup.brew_confirm);
        Ok(())
    }

    #[test]
    fn loading_backup_clears_mac_review_without_changing_windows_workspace() -> anyhow::Result<()> {
        let directory = TestDir::new()?;
        let path = directory.0.join("environment.json");
        let saved = snapshot();
        studio_deployment::backup::save(&saved, &path)?;
        let mut app = preview();
        app.backup.brew_selected.insert("brew_formula:git".into());
        app.backup.brew_confirm = true;
        app.selected.insert("existing-windows-setting".into());
        app.deployment_selected
            .insert("existing-windows-task".into());
        let windows_settings = app.selected.clone();
        let windows_tasks = app.deployment_selected.clone();
        app.file_operation(FileOperation::LoadBackup, path.clone())
            .map_err(anyhow::Error::msg)?;
        assert!(app.backup.brew_selected.is_empty());
        assert!(!app.backup.brew_confirm);
        assert_eq!(app.backup.snapshot.as_ref(), Some(&saved));
        assert_eq!(app.backup.path.as_ref(), Some(&path));
        assert_eq!(app.selected, windows_settings);
        assert_eq!(app.deployment_selected, windows_tasks);
        assert!(app.worker.is_none());

        app.backup.brew_selected.insert("brew_formula:git".into());
        app.backup.brew_confirm = true;
        let invalid = directory.0.join("invalid.json");
        std::fs::write(&invalid, b"{}")?;
        assert!(
            app.file_operation(FileOperation::LoadBackup, invalid)
                .is_err()
        );
        assert_eq!(
            app.backup.brew_selected,
            BTreeSet::from(["brew_formula:git".into()])
        );
        assert!(app.backup.brew_confirm);
        assert_eq!(app.backup.snapshot.as_ref(), Some(&saved));
        assert_eq!(app.backup.path.as_ref(), Some(&path));
        Ok(())
    }

    #[test]
    fn native_gui_exports_reviewed_identities_without_overwriting_destinations()
    -> anyhow::Result<()> {
        let directory = TestDir::new()?;
        let mut app = preview();
        let brew_path = directory.0.join("Brewfile");
        let winget_path = directory.0.join("winget.json");
        assert!(
            app.file_operation(FileOperation::ExportBrewfile, brew_path.clone())
                .is_err()
        );
        assert!(!brew_path.exists());
        assert!(
            app.file_operation(FileOperation::ExportWinget, winget_path.clone())
                .is_err()
        );
        assert!(!winget_path.exists());

        app.backup.brew_selected.insert("brew_formula:git".into());
        let catalog = app.deployment_catalog.as_ref().unwrap();
        let git = catalog
            .tasks
            .iter()
            .find(|task| task.kind == "package" && task.package_id.as_deref() == Some("Git.Git"))
            .unwrap()
            .id
            .clone();
        let developer = catalog
            .tasks
            .iter()
            .find(|task| task.kind == "pip")
            .unwrap()
            .id
            .clone();
        app.deployment_selected.extend([git, developer]);
        let selected_mac = app.backup.brew_selected.clone();
        let selected_windows = app.deployment_selected.clone();
        app.file_operation(FileOperation::ExportBrewfile, brew_path.clone())
            .map_err(anyhow::Error::msg)?;
        app.file_operation(FileOperation::ExportWinget, winget_path.clone())
            .map_err(anyhow::Error::msg)?;
        let brew_bytes = std::fs::read(&brew_path)?;
        assert!(std::str::from_utf8(&brew_bytes)?.contains("brew \"git\""));
        let winget_bytes = std::fs::read(&winget_path)?;
        let document: serde_json::Value = serde_json::from_slice(&winget_bytes)?;
        assert_eq!(
            document["Sources"][0]["Packages"],
            serde_json::json!([
                {"PackageIdentifier": "Git.Git"}
            ])
        );
        for (operation, path, original) in [
            (FileOperation::ExportBrewfile, brew_path, brew_bytes),
            (FileOperation::ExportWinget, winget_path, winget_bytes),
        ] {
            assert!(app.file_operation(operation, path.clone()).is_err());
            assert_eq!(std::fs::read(path)?, original);
        }

        let external = directory.0.join("external-existing-file");
        std::fs::write(&external, b"Existing external content")?;
        for operation in [FileOperation::ExportBrewfile, FileOperation::ExportWinget] {
            assert!(app.file_operation(operation, external.clone()).is_err());
            assert_eq!(std::fs::read(&external)?, b"Existing external content");
        }
        #[cfg(unix)]
        {
            let link = directory.0.join("external-link");
            std::os::unix::fs::symlink(&external, &link)?;
            for operation in [FileOperation::ExportBrewfile, FileOperation::ExportWinget] {
                assert!(app.file_operation(operation, link.clone()).is_err());
                assert!(std::fs::symlink_metadata(&link)?.file_type().is_symlink());
                assert_eq!(std::fs::read(&external)?, b"Existing external content");
            }
        }
        assert_eq!(app.backup.brew_selected, selected_mac);
        assert_eq!(app.deployment_selected, selected_windows);
        assert!(app.worker.is_none() && app.backup.bundle_report.is_none());
        Ok(())
    }
}
