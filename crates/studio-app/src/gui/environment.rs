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
                    .and_then(|entries| {
                        native::brewfile_with_policy(&entries, self.backup.brew_policy)
                    })
                    .and_then(|text| studio_core::export_bytes(text.as_bytes(), path))
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
            let entries = native::selected_brew(Some(snapshot), &self.backup.brew_selected)?;
            native::brewfile_with_policy(&entries, self.backup.brew_policy)?;
            Ok(entries)
        })();
        match prepared {
            Ok(entries) => {
                self.backup.bundle_report = None;
                self.backup.brew_confirm = false;
                let policy = self.backup.brew_policy;
                self.start_network_worker("正在配置 macOS 开发环境", move |cancel| {
                    native::configure_brew_with_policy(entries, policy, cancel)
                        .map(WorkerResult::Bundle)
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
        ui.label(
            RichText::new(tx(
                "保留熟悉的工具，在下一台机器继续工作。",
                "Keep your tools ready for your next machine.",
            ))
            .color(palette().muted),
        );
        ui.add_space(14.0);
        surface_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.heading(tx("从环境清单开始", "Start with an environment inventory"));
            ui.label(tx(
                "先备份或载入清单，再选择需要配置的工具与渠道。",
                "Back up or load an inventory, then choose your tools and channels.",
            ));
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if primary(ui, tx("备份 / 载入环境", "Back up / load environment"), true).clicked() {
                    self.page = Page::Backup;
                }
                if ui.button(tx("Windows 配置工作区", "Windows configuration workspace")).clicked() {
                    self.page = Page::Recipes;
                }
            });
            ui.add_space(8.0);
            self.network_summary(ui);
            egui::CollapsingHeader::new(tx("Windows 清单与导出", "Windows inventory and export"))
                .show(ui, |ui| {
                    ui.label(tx("Windows 使用 WinGet、Cargo、uv、Pip、NPM、VS Code 与 Scoop。Scoop 清单只保留官方 bucket 的普通用户级包；WinGet 导出只含所选软件身份，其他提供程序仍保存在应用档案中。", "Windows uses reviewed WinGet, Cargo, uv, Pip, NPM, VS Code and Scoop. Scoop inventories retain user packages from official buckets only. Select and review WinGet tasks in the Windows workspace; other providers remain in the app profile."));
                    ui.horizontal_wrapped(|ui| {
                        if ui.button(tx("导出所选 WinGet 清单…", "Export selected WinGet inventory…")).clicked() {
                            self.open_file(FileOperation::ExportWinget);
                        }
                    });
                    egui::CollapsingHeader::new(tx("Scoop 软件包", "Scoop packages"))
                        .id_salt("scoop-package-panel")
                        .default_open(false)
                        .show(ui, |ui| self.scoop_tools_page(ui));
                });
        });
        ui.add_space(16.0);
        surface_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.heading(tx("Rust 工具链", "Rust toolchains"));
            ui.label(RichText::new("macOS · Windows · rustup").color(palette().muted));
            ui.add_space(8.0);
            self.rust_toolchains_page(ui);
        });
        ui.add_space(16.0);
        surface_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.heading(tx("Python 命令行工具", "Python command tools"));
            ui.label(RichText::new("macOS · Windows · uv").color(palette().muted));
            ui.add_space(8.0);
            self.python_tools_page(ui);
        });
        ui.add_space(12.0);
        surface_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.heading(tx("Cargo 命令行工具", "Cargo command-line tools"));
            ui.add_space(6.0);
            self.cargo_tools_page(ui);
        });
        ui.add_space(12.0);
        surface_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.heading(tx(
                "从 Mac 清单恢复开发工具",
                "Restore developer tools from a Mac inventory",
            ));
            ui.label(
                RichText::new(tx(
                    "仅对已保存且审核过的 macOS 清单开放。",
                    "Available only for a saved and reviewed macOS inventory.",
                ))
                .size(12.0)
                .color(palette().muted),
            );
            egui::CollapsingHeader::new(tx("NPM 全局包", "npm global packages"))
                .id_salt("npm-global-panel")
                .default_open(false)
                .show(ui, |ui| self.npm_tools_page(ui));
            egui::CollapsingHeader::new(tx("VS Code 扩展", "VS Code extensions"))
                .id_salt("vscode-extension-panel")
                .default_open(false)
                .show(ui, |ui| self.vscode_tools_page(ui));
        });
        ui.add_space(8.0);
        surface_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::CollapsingHeader::new(tx(
                "其他开发运行时 · mise",
                "Additional developer runtimes · mise",
            ))
            .id_salt("mise-runtime-panel")
            .default_open(false)
            .show(ui, |ui| self.mise_tools_page(ui));
        });
        ui.add_space(16.0);
        surface_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
        ui.heading(tx("Homebrew 工具", "Homebrew tools"));
        ui.label(RichText::new("macOS · Bundle · HEAD").color(palette().muted));
        ui.add_space(8.0);
        ui.label(tx("从基础工具或 Mac 备份中选择 formula / cask，再审核安装清单。", "Choose formulae / casks from starter tools or a Mac backup, then review the install inventory."));
        let old_policy = self.backup.brew_policy;
        egui::ComboBox::from_label(tx("重部署策略", "Redeployment policy"))
            .selected_text(brew_policy_label(self.backup.brew_policy))
            .show_ui(ui, |ui| {
                for policy in [
                    native::BrewPolicy::Missing,
                    native::BrewPolicy::Rolling,
                    native::BrewPolicy::Head,
                ] {
                    ui.selectable_value(
                        &mut self.backup.brew_policy,
                        policy,
                        brew_policy_label(policy),
                    );
                }
            });
        if old_policy != self.backup.brew_policy {
            self.backup.brew_confirm = false;
        }
        ui.add_space(8.0);
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
            native::selected_brew(self.backup.snapshot.as_ref(), &self.backup.brew_selected)
                .and_then(|entries| {
                    native::brewfile_with_policy(&entries, self.backup.brew_policy)?;
                    Ok(entries)
                });
        if self.backup.brew_selected.is_empty() {
            ui.label(RichText::new(tx(
                "还未选择 Mac 工具。可先选择基础工具，或在备份页面载入已有清单。",
                "No Mac tools selected. Choose starter tools or load an inventory from the backup page.",
            )).color(palette().muted));
        } else if let Err(error) = &entries {
            ui.colored_label(palette().danger, error.to_string());
        }
        if let Ok(entries) = &entries {
            egui::CollapsingHeader::new(tx("审核生成的 Brewfile", "Review generated Brewfile"))
                .show(ui, |ui| {
                    if let Ok(text) = native::brewfile_with_policy(entries, self.backup.brew_policy)
                    {
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
        egui::CollapsingHeader::new(tx("Homebrew 范围与限制", "Homebrew scope and limits"))
            .show(ui, |ui| {
                ui.label(tx("补齐模式保留已有包；滚动模式允许更新所选包；HEAD 模式只接受定义了开发源码的 formula，并独立回查 HEAD 已安装。稳定版链接冲突会停止，不强制切换。以上均不保证 PATH 激活或固定 Git 提交。", "Missing-only preserves installed entries; rolling permits selected package upgrades; HEAD accepts formulae with a development source and independently checks the installed HEAD channel. Stable-link conflicts stop without forced switching. PATH activation and fixed Git commits are not guaranteed."));
                ui.label(tx("版本仅作备份记录。通过原生 Bundle 安装并独立检查已安装状态。需要已安装且可用的 Homebrew；--no-upgrade 不保证依赖完全不升级。", "Versions remain backup evidence. Native Bundle installs tools and independently checks their presence. A working Homebrew installation is required; --no-upgrade cannot guarantee unchanged dependencies."));
                ui.label(tx("不执行清理、卸载或导入文件中的任意 Ruby / Shell / 钩子；取消后子安装器可能仍在运行。Cargo、uv、NPM 全局包与 VS Code 扩展均有独立 Mac 重部署面板；NPM 安装脚本关闭，传递依赖不锁定。", "No cleanup, uninstall, or arbitrary Ruby, shell or hooks from imported files are run. Descendant installers may continue after cancellation. Cargo, uv, npm global packages and VS Code extensions each have a dedicated Mac redeployment panel; npm lifecycle scripts are disabled and transitive versions are not locked."));
            });
        ui.add_space(8.0);
        ui.label(tx("执行前需要保存并审核 Mac 清单。载入的清单可以来自另一台 Mac，不能代表本机当前状态。现有备份不被覆盖。", "Save and review a Mac inventory before applying. Loaded backups may come from another Mac and do not prove this machine's current state. Existing backups are preserved."));
        ui.label(RichText::new(tx(
            "配置可能下载软件、运行安装器并改变依赖；没有自动回滚。",
            "Configuration may download software, run installers and change dependencies; automatic rollback is unavailable.",
        )).color(palette().danger));
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
        });
    }
}

fn brew_policy_label(policy: native::BrewPolicy) -> &'static str {
    match policy {
        native::BrewPolicy::Missing => tx("只补齐缺少项", "Install missing only"),
        native::BrewPolicy::Rolling => tx("滚动更新所选包", "Roll selected packages forward"),
        native::BrewPolicy::Head => tx("HEAD 开发源码", "HEAD development source"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preview() -> Studio {
        Studio {
            compatibility: compatibility::UiState::default(),
            hybrid: hybrid::HybridState::default(),
            backup: backup::BackupUiState::default(),
            network: network::NetworkUiState::default(),
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
    fn rust_requests_are_opt_in_and_export_native_formats_without_changing_windows_selection()
    -> anyhow::Result<()> {
        let directory = TestDir::new()?;
        let mut app = preview();
        app.deployment_selected
            .insert("existing-windows-task".into());
        assert!(!app.backup.rust.confirmed);
        app.start_rust_redeploy();
        assert!(app.worker.is_none());
        app.backup.rust.channel = "nightly-2026-10-20".into();
        let path = directory.0.join("request.json");
        app.file_operation(FileOperation::SaveRustRequest, path.clone())
            .map_err(anyhow::Error::msg)?;
        app.backup.rust.channel = "stable".into();
        app.backup.rust.confirmed = true;
        app.file_operation(FileOperation::LoadRustRequest, path)
            .map_err(anyhow::Error::msg)?;
        assert_eq!(app.backup.rust.channel, "nightly-2026-10-20");
        assert!(!app.backup.rust.confirmed);
        for (operation, name, expected) in [
            (
                FileOperation::ExportRustToolchain,
                "rust-toolchain.toml",
                "[toolchain]",
            ),
            (FileOperation::ExportMise, "mise.toml", "[tools]"),
        ] {
            let output = directory.0.join(name);
            app.file_operation(operation, output.clone())
                .map_err(anyhow::Error::msg)?;
            let text = std::fs::read_to_string(&output)?;
            assert!(text.contains(expected) && text.contains("nightly-2026-10-20"));
            assert!(app.file_operation(operation, output.clone()).is_err());
            assert_eq!(std::fs::read_to_string(output)?, text);
        }
        app.backup.rust.channel = "--help".into();
        app.backup.rust.confirmed = true;
        app.start_rust_redeploy();
        assert!(app.worker.is_none());
        assert_eq!(
            app.deployment_selected,
            BTreeSet::from(["existing-windows-task".into()])
        );
        Ok(())
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
