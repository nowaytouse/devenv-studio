//! Native configuration workbench; Mac selections are independent of the Windows workspace.
use super::*;
use studio_deployment::environment as native;

const RUST_STARTER_TOOLS: &[&str] = &["git", "rustup", "cmake", "visual-studio-code"];
const PYTHON_STARTER_TOOLS: &[&str] = &["git", "uv", "python@3.14", "visual-studio-code"];
const WEB_STARTER_TOOLS: &[&str] = &["git", "node", "visual-studio-code"];

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

    fn select_mac_starter_tools(&mut self) {
        self.prepare_mac_starter_tools(None);
    }

    fn prepare_mac_starter_tools(&mut self, requested: Option<&[&str]>) {
        if self.worker.is_some() {
            return;
        }
        let starter = native::brew_candidates(None).and_then(|candidates| {
            let Some(names) = requested else {
                return Ok(candidates);
            };
            anyhow::ensure!(!names.is_empty(), "A starter purpose must select tools");
            let selected = names
                .iter()
                .map(|name| {
                    candidates
                        .iter()
                        .find(|entry| entry.name == *name)
                        .cloned()
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "Starter tool is absent from the reviewed catalog: {name}"
                            )
                        })
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            anyhow::ensure!(
                selected
                    .iter()
                    .map(|entry| entry.id())
                    .collect::<BTreeSet<_>>()
                    .len()
                    == names.len(),
                "Starter purpose contains duplicate tools"
            );
            Ok(selected)
        });
        match starter {
            Ok(starter) => {
                self.backup
                    .brew_selected
                    .extend(starter.iter().map(|entry| entry.id()));
                self.backup.brew_policy = native::BrewPolicy::Missing;
                self.backup.brew_query.clear();
                self.backup.brew_confirm = false;
                self.backup.bundle_report = None;
                self.backup.brew_review_requested = true;
                self.notice = Some(Notice {
                    text: lf!(
                        "已准备 {} 项基础工具并保留已有选择，使用补齐模式；尚未执行。请在下方核对。",
                        "Prepared {} starter tools and kept existing choices in missing-only mode; nothing has run yet. Review them below.",
                        starter.len()
                    ),
                    error: false,
                });
            }
            Err(error) => {
                self.notice = Some(Notice {
                    text: lf!(
                        "读取 Mac 基础工具清单失败：{error:#}",
                        "Could not load Mac starter tools: {error:#}",
                    ),
                    error: true,
                });
            }
        }
    }

    fn homebrew_page(&mut self, ui: &mut egui::Ui) {
        let response = ui.add_enabled_ui(self.worker.is_none(), |ui| {
        surface_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
        ui.heading(tx("核对要配置的工具", "Review the tools to configure"));
        ui.label(RichText::new("macOS · Bundle · HEAD").color(palette().muted));
        ui.add_space(8.0);
        ui.label(tx("选择需要的工具，核对清单后再配置。", "Choose the tools you need, then review the list before configuring."));
        let old_policy = self.backup.brew_policy;
        egui::ComboBox::from_label(tx("安装方式", "Installation policy"))
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
            self.backup.bundle_report = None;
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
                .button(tx("加入基础工具 · 补齐模式", "Add starter tools · missing-only"))
                .clicked()
            {
                self.select_mac_starter_tools();
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
            .min_scrolled_height(150.0)
            .max_height(220.0)
            .auto_shrink([false, false])
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
        self.network_summary(ui);
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
        }).response;
        if self.backup.brew_review_requested {
            self.backup.brew_review_requested = false;
            response.scroll_to_me(Some(egui::Align::Min));
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
            ui.heading(tx("从这里开始", "Start here"));
            ui.label(RichText::new(tx(
                "先准备一份可检查的方案，再决定何时安装。",
                "Prepare a plan you can review, then choose when to install.",
            )).color(palette().muted));
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                ui.strong(tx("1  保存环境备份", "1  Save an environment backup"));
                if ui.button(tx("备份或载入清单…", "Back up or load an inventory…")).clicked() {
                    self.navigate_to(Page::Backup);
                }
            });
            if cfg!(target_os = "macos") {
                let saved_mac = self.backup.path.is_some()
                    && self.backup.snapshot.as_ref().is_some_and(|s| s.platform == "macos");
                ui.label(RichText::new(if saved_mac {
                    tx("Mac 备份已保存；执行前还会重新核验文件。", "Mac backup saved; its file will be verified again before installation.")
                } else {
                    tx("安装前需要已保存的 Mac 清单。先留存现有工具，方便以后迁移。", "Save a Mac inventory before installation to keep a record of your current tools.")
                }).color(palette().muted));
            } else {
                ui.label(RichText::new(tx(
                    "建议先保存现有工具清单；也可以载入其他电脑的备份。",
                    "Keep a record of your current tools, or load a backup from another computer.",
                )).color(palette().muted));
            }
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.strong(tx("2  准备工具方案", "2  Prepare your tools"));
                if cfg!(target_os = "macos") {
                    if primary(ui, tx("一键准备基础工具", "Prepare starter tools"), self.worker.is_none()).clicked() {
                        self.select_mac_starter_tools();
                    }
                } else if primary(ui, tx("按用途选择 Windows 方案", "Choose a Windows recipe"), true).clicked() {
                    self.navigate_to(Page::Recipes);
                }
            });
            ui.label(RichText::new(if cfg!(target_os = "macos") {
                tx("加入常用开发工具，并选用补齐模式。保留已有选择，不会开始安装。", "Add common developer tools in missing-only mode. Existing choices stay selected; installation does not start.")
            } else {
                tx("选择日常、Rust、Python 或 Web 等用途，再加入待审方案。", "Choose everyday, Rust, Python or Web tools, then add them to your plan.")
            }).color(palette().muted));
            if cfg!(target_os = "macos") {
                ui.label(RichText::new(tx(
                    "也可按用途加入基础工具：",
                    "Or add starter tools by purpose:",
                )).color(palette().muted));
                ui.horizontal_wrapped(|ui| {
                    for (label, names, detail) in [
                        (tx("Rust 基础", "Rust basics"), RUST_STARTER_TOOLS,
                            tx("加入 Git、rustup、CMake、VS Code；只准备待审选择，不会安装。", "Add Git, rustup, CMake and VS Code to review; nothing is installed.")),
                        (tx("Python 基础", "Python basics"), PYTHON_STARTER_TOOLS,
                            tx("加入 Git、uv、Python 3.14、VS Code；只准备待审选择，不会安装。", "Add Git, uv, Python 3.14 and VS Code to review; nothing is installed.")),
                        (tx("Web 基础", "Web basics"), WEB_STARTER_TOOLS,
                            tx("加入 Git、Homebrew 的 Node.js、VS Code；只准备待审选择，不会安装。", "Add Git, Homebrew's Node.js and VS Code to review; nothing is installed.")),
                    ] {
                        if ui.add_enabled(self.worker.is_none(), egui::Button::new(label))
                            .on_hover_text(detail).clicked() {
                            self.prepare_mac_starter_tools(Some(names));
                        }
                    }
                });
                ui.label(RichText::new(tx(
                    "用途方案只准备基础依赖，保留已有选择；不代表完整语言环境、项目依赖或扩展已就绪。",
                    "Purpose presets prepare only base dependencies and keep existing choices. They do not establish a complete language environment, project dependencies or extensions.",
                )).size(12.0).color(palette().muted));
                egui::CollapsingHeader::new(tx("后续工具与前置条件", "Next tools and prerequisites"))
                    .id_salt("mac-purpose-prerequisites")
                    .show(ui, |ui| {
                        ui.label(tx("Rust：工具链面板需要标准位置已有 rustup 和官方默认工具链；所选渠道仍由你单独审核，基础方案不更改它。", "Rust: the toolchain panel requires existing standard-location rustup and an official default toolchain. Review the channel separately; starter preparation does not change it."));
                        ui.label(tx("Python：uv 工具面板需要已有 uv 和兼容 Python，不会自动下载解释器；按所需工具逐项选择版本。", "Python: the uv tool panel requires existing uv and compatible Python and does not download interpreters automatically. Choose the tools and versions separately."));
                        ui.label(tx("Web：NPM 与 VS Code 扩展只恢复已保存 Mac 清单中完整来源的条目；方案不会添加未备份的软件包或扩展。", "Web: npm and VS Code extensions restore only entries from complete providers in a saved Mac inventory. Presets do not add packages or extensions absent from that inventory."));
                        ui.label(tx("其他运行时可使用 mise 面板锁定与安装；需要先准备可用 mise，仍需另行选择和确认。", "Other runtimes can be locked and installed in the mise panel. A working mise installation, separate selection and confirmation are required."));
                    });
            }
            ui.add_space(8.0);
            ui.strong(tx("3  核对后再配置", "3  Review before installing"));
            ui.label(tx(
                if cfg!(target_os = "macos") { "在下方检查工具和安装方式，确认后再开始。需要 Homebrew；安装可能改变依赖。" } else { "Windows 系统方案在变更方案中审核；Rust、uv 等工具在各自面板核对条件并确认执行。" },
                if cfg!(target_os = "macos") { "Review the tools and install policy below, then confirm. Homebrew is required; dependencies may change." } else { "Review Windows system plans on Change plan. For Rust, uv and other tools, review requirements and confirm in each tool panel." },
            ));
            if cfg!(target_os = "macos")
                && ui.small_button(tx("查看 Windows 方案预览", "Preview Windows recipes")).clicked()
            {
                self.navigate_to(Page::Recipes);
            }
            ui.add_space(10.0);
            egui::CollapsingHeader::new(tx("Windows 清单与导出", "Windows inventory and export"))
                .show(ui, |ui| {
                    ui.label(tx("Windows 使用 WinGet、Cargo、uv、Pip、NPM、VS Code 与 Scoop。Scoop 清单只保留官方 bucket 的普通用户级包；WinGet 导出只含所选软件身份，其他提供程序仍保存在应用档案中。", "Windows uses reviewed WinGet, Cargo, uv, Pip, NPM, VS Code and Scoop. Scoop inventories retain user packages from official buckets only. Select and review WinGet tasks in the Windows workspace; other providers remain in the app profile."));
                    ui.horizontal_wrapped(|ui| {
                        if ui.button(tx("从 WinGet 清单迁移…", "Migrate a WinGet inventory…")).clicked() {
                            self.navigate_to(Page::Migration);
                        }
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
        if cfg!(target_os = "macos") {
            ui.add_space(16.0);
            self.homebrew_page(ui);
        }
        ui.add_space(16.0);
        let topgrade_has_content = self.backup.topgrade.profile.is_some()
            || !self.backup.topgrade.attachments.trim().is_empty();
        let topgrade_title = if self.backup.topgrade.extracted.is_some() {
            tx(
                "Topgrade 环境迁移 · 有提取结果，尚未启用",
                "Topgrade migration · extracted files, not activated",
            )
        } else if topgrade_has_content {
            tx(
                "Topgrade 环境迁移 · 有待审内容",
                "Topgrade migration · content to review",
            )
        } else {
            tx("Topgrade 环境迁移", "Topgrade environment migration")
        };
        egui::CollapsingHeader::new(topgrade_title)
            .id_salt("environment-topgrade-section")
            .default_open(topgrade_has_content || self.backup.topgrade.extracted.is_some())
            .show(ui, |ui| self.topgrade_page(ui));
        ui.add_space(16.0);
        let git_has_content =
            self.backup.git_config.profile.is_some() || self.backup.git_config.current.is_some();
        let git_title = if self.backup.git_config.report.is_some() {
            tx(
                "Git 全局配置 · 有上次结果",
                "Git global configuration · previous result",
            )
        } else if git_has_content {
            tx(
                "Git 全局配置 · 有待审内容",
                "Git global configuration · content to review",
            )
        } else {
            tx("Git 全局配置", "Git global configuration")
        };
        egui::CollapsingHeader::new(git_title)
            .id_salt("environment-git-section")
            .default_open(git_has_content || self.backup.git_config.report.is_some())
            .show(ui, |ui| self.git_config_page(ui));
        ui.add_space(16.0);
        surface_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            let has_content = self.backup.rust.channel != "stable"
                || self.backup.rust.confirmed
                || self.backup.rust.installed.is_some();
            let title = if self.backup.rust.report.is_some() {
                tx(
                    "Rust 工具链 · 有上次结果",
                    "Rust toolchains · previous result",
                )
            } else if has_content {
                tx(
                    "Rust 工具链 · 有清单或待审内容",
                    "Rust toolchains · inventory or draft",
                )
            } else {
                tx("Rust 工具链", "Rust toolchains")
            };
            egui::CollapsingHeader::new(title)
                .id_salt("environment-rust-section")
                .default_open(has_content || self.backup.rust.report.is_some())
                .show(ui, |ui| {
                    ui.label(RichText::new("macOS · Windows · rustup").color(palette().muted));
                    ui.add_space(8.0);
                    self.rust_toolchains_page(ui);
                });
        });
        ui.add_space(16.0);
        surface_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            let has_content = self.backup.python_tools.inventory.is_some()
                || !self.backup.python_tools.selected.is_empty()
                || !self.backup.python_tools.new_name.trim().is_empty()
                || !self.backup.python_tools.new_version.trim().is_empty();
            let title = if self.backup.python_tools.report.is_some() {
                tx(
                    "Python 命令行工具 · 有上次结果",
                    "Python command tools · previous result",
                )
            } else if has_content {
                tx(
                    "Python 命令行工具 · 有清单或待审内容",
                    "Python command tools · inventory or draft",
                )
            } else {
                tx("Python 命令行工具", "Python command tools")
            };
            egui::CollapsingHeader::new(title)
                .id_salt("environment-python-section")
                .default_open(has_content || self.backup.python_tools.report.is_some())
                .show(ui, |ui| {
                    ui.label(RichText::new("macOS · Windows · uv").color(palette().muted));
                    ui.add_space(8.0);
                    self.python_tools_page(ui);
                });
        });
        ui.add_space(12.0);
        surface_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            let has_content = !self.backup.cargo_tools.selected.is_empty()
                || !self.backup.cargo_tools.versions.is_empty()
                || !self.backup.cargo_tools.git_head.is_empty();
            let title = if self.backup.cargo_tools.report.is_some() {
                tx(
                    "Cargo 命令行工具 · 有上次结果",
                    "Cargo command-line tools · previous result",
                )
            } else if has_content {
                tx(
                    "Cargo 命令行工具 · 有待审内容",
                    "Cargo command-line tools · draft to review",
                )
            } else {
                tx("Cargo 命令行工具", "Cargo command-line tools")
            };
            egui::CollapsingHeader::new(title)
                .id_salt("environment-cargo-section")
                .default_open(has_content || self.backup.cargo_tools.report.is_some())
                .show(ui, |ui| {
                    ui.add_space(6.0);
                    self.cargo_tools_page(ui);
                });
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
            egui::CollapsingHeader::new(tx("编辑器设置与扩展", "Editor settings and extensions"))
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
        if !cfg!(target_os = "macos") {
            egui::CollapsingHeader::new(tx("macOS 工具方案预览", "Preview macOS tools"))
                .id_salt("macos-tools-preview")
                .show(ui, |ui| self.homebrew_page(ui));
        }
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
    use anyhow::Context as _;

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
            help_origin: None,
            help_focus_requested: false,
            navigation_groups: DEFAULT_NAVIGATION_GROUPS,
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
            use std::sync::atomic::AtomicU64;
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let directory = Self(std::env::temp_dir().join(format!(
                "studio-environment-gui-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )));
            std::fs::create_dir(&directory.0)
                .context("Create isolated environment GUI test directory")?;
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
    fn mac_starter_selection_is_opt_in_and_resets_confirmation() {
        let mut app = preview();
        app.backup.brew_confirm = true;
        app.backup.brew_policy = native::BrewPolicy::Head;
        app.backup.brew_query = "hidden-by-filter".into();
        app.backup
            .brew_selected
            .insert("existing-hidden-choice".into());
        app.selected.insert("explorer-extensions".into());
        app.deployment_selected.insert("software-git-git".into());
        app.backup.bundle_report = Some(native::BundleReport {
            requested: 1,
            verified: true,
            cancelled: false,
            resolved_versions: Vec::new(),
            detail: "Previous request".into(),
        });
        let mut expected = native::brew_candidates(None)
            .unwrap()
            .iter()
            .map(|entry| entry.id())
            .collect::<BTreeSet<_>>();
        expected.insert("existing-hidden-choice".into());

        app.select_mac_starter_tools();

        assert_eq!(app.backup.brew_selected, expected);
        assert_eq!(app.backup.brew_policy, native::BrewPolicy::Missing);
        assert!(app.backup.brew_query.is_empty());
        assert!(!app.backup.brew_confirm);
        assert!(app.backup.bundle_report.is_none());
        assert!(app.backup.brew_review_requested);
        assert_eq!(app.selected, BTreeSet::from(["explorer-extensions".into()]));
        assert_eq!(
            app.deployment_selected,
            BTreeSet::from(["software-git-git".into()])
        );
        assert!(app.worker.is_none());
        let notice = app.notice.as_ref().unwrap();
        assert!(!notice.error);
        assert!(notice.text.contains("nothing has run yet") || notice.text.contains("尚未执行"));
    }

    #[test]
    fn purpose_starters_match_reviewed_candidates_and_merge_without_changing_other_tools() {
        for (names, expected_ids) in [
            (
                RUST_STARTER_TOOLS,
                &[
                    "brew_formula:git",
                    "brew_formula:rustup",
                    "brew_formula:cmake",
                    "brew_cask:visual-studio-code",
                ][..],
            ),
            (
                PYTHON_STARTER_TOOLS,
                &[
                    "brew_formula:git",
                    "brew_formula:uv",
                    "brew_formula:python@3.14",
                    "brew_cask:visual-studio-code",
                ][..],
            ),
            (
                WEB_STARTER_TOOLS,
                &[
                    "brew_formula:git",
                    "brew_formula:node",
                    "brew_cask:visual-studio-code",
                ][..],
            ),
        ] {
            let mut app = preview();
            app.backup
                .brew_selected
                .insert("brew_formula:keep-existing".into());
            app.backup.brew_policy = native::BrewPolicy::Head;
            app.backup.brew_query = "previous search".into();
            app.backup.brew_confirm = true;
            app.backup.rust.channel = "nightly".into();
            app.backup
                .npm_tools
                .selected
                .insert("existing-npm-tool".into());
            app.backup
                .vscode_tools
                .selected
                .insert("existing.extension".into());
            app.deployment_selected.insert("keep-windows".into());
            app.prepare_mac_starter_tools(Some(names));
            let mut expected = expected_ids
                .iter()
                .map(|id| (*id).to_owned())
                .collect::<BTreeSet<_>>();
            expected.insert("brew_formula:keep-existing".into());
            assert_eq!(app.backup.brew_selected, expected);
            app.prepare_mac_starter_tools(Some(names));
            assert_eq!(app.backup.brew_selected, expected);
            assert_eq!(app.backup.brew_policy, native::BrewPolicy::Missing);
            assert!(app.backup.brew_query.is_empty());
            assert!(!app.backup.brew_confirm);
            assert!(app.backup.brew_review_requested);
            assert_eq!(app.backup.rust.channel, "nightly");
            assert_eq!(
                app.backup.npm_tools.selected,
                BTreeSet::from(["existing-npm-tool".into()])
            );
            assert_eq!(
                app.backup.vscode_tools.selected,
                BTreeSet::from(["existing.extension".into()])
            );
            assert_eq!(
                app.deployment_selected,
                BTreeSet::from(["keep-windows".into()])
            );
            assert!(app.worker.is_none() && app.dialog.is_none());
            assert!(app.notice.as_ref().is_some_and(|notice| !notice.error));
        }
    }

    #[test]
    fn invalid_starter_purpose_is_rejected_without_partial_selection_or_confirmation_reset() {
        for names in [
            &["git", "unreviewed-tool"][..],
            &["git", "git"][..],
            &[][..],
        ] {
            let mut app = preview();
            app.backup.brew_selected.insert("keep-current".into());
            app.backup.brew_policy = native::BrewPolicy::Head;
            app.backup.brew_query = "keep filter".into();
            app.backup.brew_confirm = true;
            app.prepare_mac_starter_tools(Some(names));
            assert_eq!(
                app.backup.brew_selected,
                BTreeSet::from(["keep-current".into()])
            );
            assert_eq!(app.backup.brew_policy, native::BrewPolicy::Head);
            assert_eq!(app.backup.brew_query, "keep filter");
            assert!(app.backup.brew_confirm);
            assert!(!app.backup.brew_review_requested);
            assert!(app.notice.as_ref().is_some_and(|notice| notice.error));
            assert!(app.worker.is_none());
        }
    }

    #[test]
    fn progressive_sections_open_and_close_without_changing_reviewed_drafts() {
        fn render(
            app: &mut Studio,
            context: &egui::Context,
            events: Vec<egui::Event>,
        ) -> Vec<(egui::accesskit::NodeId, egui::accesskit::Node)> {
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(980.0, 4000.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| app.environment_page(ui));
                },
            );
            output.textures_delta.clear();
            output
                .platform_output
                .accesskit_update
                .expect("accessibility tree")
                .nodes
        }

        fn click_rust_header(
            nodes: &[(egui::accesskit::NodeId, egui::accesskit::Node)],
        ) -> Vec<egui::Event> {
            let bounds = nodes
                .iter()
                .map(|(_, node)| node)
                .find(|node| {
                    node.role() == egui::accesskit::Role::DisclosureTriangle
                        && node
                            .label()
                            .is_some_and(|label| label.starts_with("Rust toolchains"))
                })
                .and_then(|node| node.bounds())
                .expect("visible Rust section");
            let pos = egui::pos2(
                ((bounds.x0 + bounds.x1) / 2.0) as f32,
                ((bounds.y0 + bounds.y1) / 2.0) as f32,
            );
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                },
            ]
        }

        let mut app = preview();
        i18n::set_language(Language::EnUs);
        let context = egui::Context::default();
        context.enable_accesskit();
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            context.style_mut_of(theme, |style| style.animation_time = 0.0);
        }
        let initial = render(&mut app, &context, Vec::new());
        for title in [
            "Topgrade environment migration",
            "Git global configuration",
            "Rust toolchains",
            "Python command tools",
            "Cargo command-line tools",
        ] {
            assert!(
                initial.iter().any(|(_, node)| node.role()
                    == egui::accesskit::Role::DisclosureTriangle
                    && node.label() == Some(title)),
                "{title}"
            );
        }
        for hidden in [
            "Capture config and attachments",
            "Capture this device",
            "Read installed Rust toolchains",
            "Read installed uv tools",
            "Redeploy selected Cargo tools",
        ] {
            assert!(
                !initial.iter().any(|(_, node)| node.label() == Some(hidden)),
                "{hidden}"
            );
        }
        let opened = render(&mut app, &context, click_rust_header(&initial));
        assert!(
            opened
                .iter()
                .any(|(_, node)| node.label() == Some("Read installed Rust toolchains"))
        );

        app.backup.rust.channel = "nightly".into();
        app.backup.rust.confirmed = true;
        app.backup.brew_selected.insert("brew_formula:git".into());
        app.backup.cargo_tools.selected.insert("ripgrep".into());
        let reviewed = render(&mut app, &context, Vec::new());
        assert!(
            reviewed
                .iter()
                .any(|(_, node)| node.label() == Some("Rust toolchains · inventory or draft"))
        );
        let closed = render(&mut app, &context, click_rust_header(&reviewed));
        assert!(
            !closed
                .iter()
                .any(|(_, node)| node.label() == Some("Read installed Rust toolchains"))
        );
        let still_closed = render(&mut app, &context, Vec::new());
        assert!(
            !still_closed
                .iter()
                .any(|(_, node)| node.label() == Some("Read installed Rust toolchains"))
        );
        assert_eq!(app.backup.rust.channel, "nightly");
        assert!(app.backup.rust.confirmed);
        assert_eq!(
            app.backup.brew_selected,
            BTreeSet::from(["brew_formula:git".into()])
        );
        assert_eq!(
            app.backup.cargo_tools.selected,
            BTreeSet::from(["ripgrep".into()])
        );
        assert!(app.worker.is_none() && app.dialog.is_none());
        assert!(app.notice.is_none());
        i18n::set_language(Language::ZhCn);
    }

    #[test]
    fn starter_preparation_does_not_change_an_active_request() {
        let mut app = preview();
        app.backup
            .brew_selected
            .insert("keep-current-request".into());
        app.backup.brew_policy = native::BrewPolicy::Head;
        app.backup.brew_confirm = true;
        app.backup.brew_query = "current-filter".into();
        app.backup.brew_review_requested = true;
        let (_sender, receiver) = std::sync::mpsc::channel();
        app.worker = Some(Worker {
            cancel: Arc::new(AtomicBool::new(false)),
            receiver,
            events: None,
            thread: None,
            label: "Synthetic active task",
        });

        app.select_mac_starter_tools();
        app.prepare_mac_starter_tools(Some(RUST_STARTER_TOOLS));

        assert_eq!(
            app.backup.brew_selected,
            BTreeSet::from(["keep-current-request".into()])
        );
        assert_eq!(app.backup.brew_policy, native::BrewPolicy::Head);
        assert!(app.backup.brew_confirm);
        assert_eq!(app.backup.brew_query, "current-filter");
        assert!(app.backup.brew_review_requested);
        assert!(app.notice.is_none());
    }

    #[test]
    fn review_scroll_is_consumed_once_without_starting_configuration() {
        let mut app = preview();
        app.select_mac_starter_tools();
        let selected = app.backup.brew_selected.clone();
        assert!(app.backup.brew_review_requested);
        let context = egui::Context::default();
        for _ in 0..2 {
            let mut output = context.run_ui(egui::RawInput::default(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| app.homebrew_page(ui));
            });
            output.textures_delta.clear();
            assert!(!app.backup.brew_review_requested);
            assert_eq!(app.backup.brew_selected, selected);
            assert!(!app.backup.brew_confirm);
            assert!(app.worker.is_none());
        }
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
