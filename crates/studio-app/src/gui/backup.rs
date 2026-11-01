//! Explicit local macOS inventory backup and handoff to the same migration workbench.
use super::*;

#[derive(Default)]
pub(super) struct BackupUiState {
    pub snapshot: Option<studio_deployment::backup::Snapshot>,
    pub path: Option<PathBuf>,
    pub query: String,
    pub brew_selected: BTreeSet<String>,
    pub brew_query: String,
    pub brew_confirm: bool,
    pub brew_review_requested: bool,
    pub brew_policy: studio_deployment::environment::BrewPolicy,
    pub rust: rust_toolchains::RustUiState,
    pub python_tools: super::python_tools::UiState,
    pub cargo_tools: super::cargo_tools::UiState,
    pub vscode_tools: super::vscode_tools::UiState,
    pub npm_tools: super::npm_tools::UiState,
    pub mise_tools: super::mise_tools::UiState,
    pub scoop_tools: super::scoop_tools::UiState,
    pub git_config: super::git_config::UiState,
    pub topgrade: super::topgrade::UiState,
    pub bundle_report: Option<studio_deployment::environment::BundleReport>,
}

#[derive(Default)]
struct BackupSummary {
    complete: usize,
    partial: usize,
    unavailable: usize,
    failed: usize,
    packages: usize,
}

impl BackupSummary {
    fn from_snapshot(snapshot: &studio_deployment::backup::Snapshot) -> Self {
        let mut summary = Self::default();
        for provider in &snapshot.providers {
            match provider.status.as_str() {
                "complete" => summary.complete += 1,
                "partial" => summary.partial += 1,
                "unavailable" => summary.unavailable += 1,
                _ => summary.failed += 1,
            }
            summary.packages += provider.packages.len();
        }
        summary
    }

    fn needs_review(&self) -> bool {
        self.partial > 0 || self.failed > 0 || self.complete == 0
    }
}

impl Studio {
    pub(super) fn start_backup(&mut self) {
        if !cfg!(any(target_os = "macos", windows)) || self.worker.is_some() {
            return;
        }
        self.start_worker("正在采集本机开发环境", |cancel| {
            studio_deployment::backup::collect(cancel).map(WorkerResult::Backup)
        });
    }

    pub(super) fn accept_backup(
        &mut self,
        snapshot: studio_deployment::backup::Snapshot,
    ) -> Notice {
        let summary = BackupSummary::from_snapshot(&snapshot);
        self.set_backup_snapshot(snapshot, None);
        Notice {
            text: lf!(
                "采集结束：完整 {complete} 类、部分 {partial} 类、未找到 {unavailable} 类、失败 {failed} 类，共 {packages} 个条目。请检查范围并保存清单。",
                "Capture finished: {complete} complete, {partial} partial, {unavailable} unavailable and {failed} failed providers; {packages} entries. Review the scope and save the inventory.",
                complete = summary.complete,
                partial = summary.partial,
                unavailable = summary.unavailable,
                failed = summary.failed,
                packages = summary.packages
            ),
            error: summary.needs_review(),
        }
    }

    fn set_backup_snapshot(
        &mut self,
        snapshot: studio_deployment::backup::Snapshot,
        path: Option<PathBuf>,
    ) {
        self.backup.snapshot = Some(snapshot);
        self.backup.path = path;
        self.backup.query.clear();
        self.backup.brew_selected.clear();
        self.backup.brew_query.clear();
        self.backup.brew_confirm = false;
        self.backup.brew_review_requested = false;
        self.backup.vscode_tools.clear_selection();
        self.backup.npm_tools = super::npm_tools::UiState::default();
        self.backup.scoop_tools = super::scoop_tools::UiState::default();
    }

    pub(super) fn backup_file_operation(
        &mut self,
        operation: FileOperation,
        path: PathBuf,
    ) -> Result<String, String> {
        if self.worker.is_some() {
            return Err(tx(
                "请等待当前任务结束后再处理备份文件。",
                "Wait for the current task before processing backup files.",
            )
            .into());
        }
        match operation {
            FileOperation::SaveBackup => {
                let snapshot = self.backup.snapshot.as_ref().ok_or_else(|| {
                    tx("请先采集或载入备份。", "Collect or load a backup first.").to_owned()
                })?;
                studio_deployment::backup::save(snapshot, &path).map_err(|e| format!("{e:#}"))?;
                self.backup.path = Some(path.clone());
                Ok(lf!(
                    "开发环境备份已保存：{}",
                    "Environment backup saved: {}",
                    path.display()
                ))
            }
            FileOperation::LoadBackup => {
                let snapshot =
                    studio_deployment::backup::load(&path).map_err(|e| format!("{e:#}"))?;
                self.set_backup_snapshot(snapshot, Some(path.clone()));
                Ok(lf!(
                    "已载入开发环境备份：{}",
                    "Environment backup loaded: {}",
                    path.display()
                ))
            }
            FileOperation::ExportBrewfile
            | FileOperation::ExportWinget
            | FileOperation::SaveBundleReport => self.environment_export(operation, &path),
            _ => Err(tx(
                "不是开发环境备份操作。",
                "Not an environment backup operation.",
            )
            .into()),
        }
    }

    pub(super) fn review_backup(&mut self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.worker.is_none(),
            "Wait for the current task before reviewing mappings"
        );
        let path =
            self.backup.path.as_ref().ok_or_else(|| {
                anyhow::anyhow!("Save the backup before reviewing Windows mappings")
            })?;
        let report = crate::migration::load(path, self.target)?;
        self.hybrid.path = path.display().to_string();
        self.hybrid.report_path = Some(self.hybrid.path.clone());
        self.hybrid.report_target = Some(self.target);
        self.hybrid.report = Some(report);
        self.hybrid.reset_review();
        self.hybrid.error = None;
        self.navigate_to(Page::Migration);
        Ok(())
    }

    pub(super) fn backup_page(&mut self, ui: &mut egui::Ui) {
        ui.heading(tx("备份工具清单", "Back up your tool inventory"));
        ui.label(tx(
            "保存当前工具与版本，供以后审核和重部署。按下面三步继续。",
            "Save your tools and versions for later review and redeployment. Follow the three steps below.",
        ));
        ui.add_space(12.0);
        surface_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.heading(tx("1  获取工具清单", "1  Get a tool inventory"));
            ui.label(tx(
                "采集这台电脑，或载入另一份备份。只读取支持的工具，不会安装或更新。",
                "Capture this computer or load a backup. Only supported tools are read; nothing is installed or updated.",
            ));
            ui.horizontal_wrapped(|ui| {
                if primary(
                    ui,
                    tx("采集本机开发环境", "Capture local environment"),
                    cfg!(any(target_os = "macos", windows)) && self.worker.is_none(),
                ).clicked() {
                    self.start_backup();
                }
                if ui.add_enabled(
                    self.worker.is_none(),
                    egui::Button::new(tx("载入备份…", "Load backup…")),
                ).clicked() {
                    self.open_file(FileOperation::LoadBackup);
                }
            });
            if !cfg!(any(target_os = "macos", windows)) {
                ui.label(tx(
                    "当前平台可载入和查看备份；本机采集需要 macOS 或 Windows。",
                    "This platform can load and review backups; local capture requires macOS or Windows.",
                ));
            }
            ui.label(RichText::new(tx(
                "取消或整体失败会保留上一次清单。个人清单只保存在本地，不会上传 GitHub。",
                "Cancellation or an overall failure preserves the previous inventory. Personal inventories stay local and are not uploaded to GitHub.",
            )).size(12.0).color(palette().muted));
        });
        ui.add_space(12.0);
        let Some(snapshot) = &self.backup.snapshot else {
            empty(
                ui,
                tx("尚无工具清单", "No tool inventory yet"),
                tx(
                    "先采集本机或载入以前的 JSON 备份，再检查结果并保存。",
                    "Capture this computer or load an earlier JSON backup, then review and save the result.",
                ),
            );
            return;
        };
        let summary = BackupSummary::from_snapshot(snapshot);
        let source_platform = snapshot.platform.clone();
        let captured = captured_time(&snapshot.captured_at);
        surface_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.heading(tx("2  检查范围并保存", "2  Review the scope and save"));
            ui.label(lf!(
                "完整 {complete} 类 · 部分 {partial} 类 · 未找到 {unavailable} 类 · 失败 {failed} 类 · {packages} 个条目",
                "{complete} complete · {partial} partial · {unavailable} unavailable · {failed} failed · {packages} entries",
                complete = summary.complete,
                partial = summary.partial,
                unavailable = summary.unavailable,
                failed = summary.failed,
                packages = summary.packages
            ));
            if summary.needs_review() {
                ui.colored_label(palette().danger, tx(
                    "这份清单不完整。请展开下方来源查看原因；仍可保存现有记录，但不能把它当作完整环境。",
                    "This inventory is incomplete. Expand the providers below for details. You can save the available records, but they do not represent a complete environment.",
                ));
            }
            if summary.unavailable > 0 {
                ui.label(tx(
                    "“未找到”只表示支持的位置中未找到工具，不代表软件已经卸载。",
                    "Unavailable means a tool was not found in supported locations; it does not prove the software is uninstalled.",
                ));
            }
            ui.label(lf!("来源平台：{}", "Source platform: {}", source_platform));
            ui.label(lf!("采集时间：{}", "Captured at: {}", captured));
            if let Some(path) = &self.backup.path {
                ui.label(lf!("本地文件：{}", "Local file: {}", path.display()));
                ui.label(RichText::new(tx(
                    "清单已关联本地文件；执行配置前还会重新核验。",
                    "The inventory has a local file; it will be verified again before configuration.",
                )).color(palette().muted));
            } else {
                ui.strong(tx(
                    "尚未保存。检查来源后，先保存清单再继续。",
                    "Not saved yet. Review the providers, then save the inventory before continuing.",
                ));
            }
            if primary(
                ui,
                tx("保存备份…", "Save backup…"),
                self.worker.is_none() && self.backup.snapshot.is_some(),
            ).clicked() {
                self.open_file(FileOperation::SaveBackup);
            }
            ui.label(RichText::new(tx(
                "只记录工具名称和版本，不包含二进制、项目、密钥、工具配置或虚拟环境。Git 设置与 Topgrade 配置有独立页面。",
                "Only tool identities and versions are recorded. Binaries, projects, keys, tool settings and virtual environments are excluded. Git settings and Topgrade configuration have separate panels.",
            )).size(12.0).color(palette().muted));
        });
        ui.add_space(12.0);
        ui.heading(tx("工具条目与来源详情", "Tools and provider details"));
        ui.add(
            egui::TextEdit::singleline(&mut self.backup.query)
                .hint_text(tx("搜索包名或版本…", "Search package or version…"))
                .desired_width(450.0),
        );
        let query = self.backup.query.to_lowercase();
        let Some(snapshot) = &self.backup.snapshot else {
            return;
        };
        for provider in &snapshot.providers {
            let state = match provider.status.as_str() {
                "complete" => tx("完整", "Complete"),
                "partial" => tx("部分", "Partial"),
                "unavailable" => tx("未找到", "Unavailable"),
                _ => tx("失败", "Failed"),
            };
            let matched: Vec<_> = provider
                .packages
                .iter()
                .filter(|package| {
                    query.is_empty()
                        || format!("{} {}", package.name, package.version)
                            .to_lowercase()
                            .contains(&query)
                })
                .collect();
            egui::CollapsingHeader::new(format!(
                "{} · {state} · {} / {}",
                provider_name(&provider.kind),
                matched.len(),
                provider.packages.len()
            ))
            .id_salt(("backup-provider", &provider.kind))
            .default_open(false)
            .open((!query.is_empty()).then_some(!matched.is_empty()))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(backup_message(&provider.detail))
                        .size(12.0)
                        .color(palette().muted),
                );
                let row_height = ui
                    .text_style_height(&egui::TextStyle::Body)
                    .max(ui.text_style_height(&egui::TextStyle::Monospace));
                egui::ScrollArea::vertical()
                    .id_salt(("backup-rows", &provider.kind))
                    .max_height(220.0)
                    .auto_shrink([false, true])
                    .show_rows(ui, row_height, matched.len(), |ui, range| {
                        let name_width = (ui.available_width() * 0.6).max(180.0);
                        for package in &matched[range] {
                            ui.scope_builder(
                                egui::UiBuilder::new()
                                    .scope_id(egui::Id::unique((
                                        "backup-row",
                                        &provider.kind,
                                        &package.name,
                                    )))
                                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                                |ui| {
                                    ui.add_sized(
                                        [name_width, row_height],
                                        egui::Label::new(&package.name).truncate(),
                                    );
                                    ui.add_sized(
                                        [ui.available_width(), row_height],
                                        egui::Label::new(
                                            RichText::new(&package.version).monospace(),
                                        )
                                        .truncate(),
                                    );
                                },
                            );
                        }
                    });
            });
        }
        for warning in &snapshot.warnings {
            ui.label(
                RichText::new(backup_message(warning))
                    .size(12.0)
                    .color(palette().muted),
            );
        }
        ui.label(RichText::new(tx("记录的版本是本次观测值，不保证未来可重新安装完全相同的构建。", "Recorded versions are observations, not a guarantee that identical builds can be reinstalled later.")).size(12.0).color(palette().muted));
        ui.add_space(12.0);
        surface_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.heading(tx("3  选择接下来的流程", "3  Choose the next workflow"));
            let saved = self.backup.path.is_some();
            let idle = self.worker.is_none();
            let prefer_mac = cfg!(target_os = "macos") && source_platform == "macos";
            if source_platform == "macos" {
                ui.label(tx(
                    "Mac 清单可用于选择 Homebrew 工具，也可审核到 Windows 的可信映射。载入不会自动勾选项目。",
                    "A Mac inventory can supply Homebrew choices or trusted mappings to Windows. Loading does not select items automatically.",
                ));
            } else {
                ui.label(tx(
                    "Windows 清单先审核可信映射，再逐项加入 Windows 方案。它不会还原成 Mac 环境。",
                    "Review trusted mappings from this Windows inventory, then add items individually to a Windows plan. It does not restore a Mac environment.",
                ));
            }
            if !cfg!(windows) {
                ui.label(RichText::new(tx(
                    "当前设备只能预览 Windows 映射与方案；执行需要受支持的 Windows 本机。",
                    "This device can preview Windows mappings and plans; execution requires a supported Windows host.",
                )).color(palette().muted));
            }
            if !saved {
                ui.label(tx("先在上方保存清单，再继续。", "Save the inventory above before continuing."));
            }
            ui.horizontal_wrapped(|ui| {
                let mac_label = if prefer_mac {
                    tx("进入 Mac 环境配置", "Configure this Mac")
                } else {
                    tx("查看环境配置", "View Environment")
                };
                let mac_clicked = if prefer_mac {
                    primary(ui, mac_label, saved && idle).clicked()
                } else {
                    false
                };
                let windows_clicked = if prefer_mac {
                    ui.add_enabled(saved && idle, egui::Button::new(tx("审核 Windows 映射", "Review Windows mappings"))).clicked()
                } else {
                    primary(ui, tx("审核 Windows 映射", "Review Windows mappings"), saved && idle).clicked()
                };
                let environment_clicked = if !prefer_mac {
                    ui.add_enabled(saved && idle, egui::Button::new(mac_label)).clicked()
                } else {
                    false
                };
                if mac_clicked || environment_clicked {
                    self.navigate_to(Page::Environment);
                }
                if windows_clicked && let Err(error) = self.review_backup() {
                    self.notice = Some(Notice { text: format!("{error:#}"), error: true });
                }
            });
            ui.label(RichText::new(tx(
                "这里只进入审核，不会自动勾选、安装或恢复设置。",
                "Continuing only opens review. It does not select items, install tools or restore settings.",
            )).size(12.0).color(palette().muted));
        });
    }
}

fn backup_message(message: &str) -> &str {
    if i18n::english() {
        return message;
    }
    match message {
        "Collected installed package identities and versions." => "已采集包名与安装版本。",
        "Tool not found in supported locations." => "在支持的位置未找到工具；不代表未安装。",
        "Homebrew or its required local runtime is unavailable in supported locations." => {
            "Homebrew 或其本地运行时未就绪，已跳过以避免自动下载。"
        }
        "Command failed; package list was not captured." => "命令失败，未取得完整包清单。",
        "Command timed out; package list was not captured." => "命令超时，未取得完整包清单。",
        "Command output was invalid; package list was not captured." => {
            "输出格式无法识别，未取得完整包清单。"
        }
        "Command could not be started or fully captured." => "命令无法启动或输出未能完整读取。",
        "Some entries were omitted because their identities or versions are unsupported." => {
            "部分条目的名称或版本格式不支持，已明确标记遗漏。"
        }
        "Inventory only: tool binaries, credentials, configuration, source code and virtual environments are not included." => {
            "清单备份不含工具二进制、凭据、配置、源码和虚拟环境。"
        }
        "Only supported standard installations are inspected; unavailable does not prove uninstalled." => {
            "只检查支持的标准安装位置；未找到不等于已卸载。"
        }
        "Pip captures distributions visible to one selected registered standard CPython interpreter; other interpreters, virtual environments and editable projects are not backed up." => {
            "Pip 仅采集一个选定的已注册标准 CPython 解释器可见的发行包；不包含其他解释器、虚拟环境或可编辑项目。"
        }
        "Some providers were not completely captured; their entries are not a complete environment." => {
            "部分来源采集不完整；请按各来源状态判断备份范围。"
        }
        _ => message,
    }
}

fn captured_time(value: &str) -> String {
    let Some(seconds) = value.parse::<u64>().ok().filter(|v| *v <= 253_402_300_799) else {
        return value.into();
    };
    let days = (seconds / 86400) as i64 + 719468;
    let era = days / 146097;
    let day_of_era = days - era * 146097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = month_index + if month_index < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        seconds % 86400 / 3600,
        seconds % 3600 / 60
    )
}

fn provider_name(kind: &str) -> &str {
    match kind {
        "brew_formula" => tx("Homebrew 命令行", "Homebrew formulae"),
        "brew_cask" => tx("Homebrew 应用", "Homebrew casks"),
        "cargo" => "Cargo",
        "uv" => "uv",
        "npm" => "NPM",
        "pip" => "Pip",
        "vscode" => "VS Code",
        "scoop" => "Scoop",
        _ => kind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context as _;
    #[test]
    fn utc_backup_time_preserves_day_and_leap_year_boundaries() {
        assert_eq!(captured_time("0"), "1970-01-01 00:00 UTC");
        assert_eq!(captured_time("1709164800"), "2024-02-29 00:00 UTC");
        assert_eq!(captured_time("253402300799"), "9999-12-31 23:59 UTC");
        assert_eq!(
            captured_time("18446744073709551615"),
            "18446744073709551615"
        );
    }

    fn fixture() -> studio_deployment::backup::Snapshot {
        serde_json::from_slice(include_bytes!(
            "../../../../examples/mac-development-backup.json"
        ))
        .expect("reviewed backup fixture")
    }

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> anyhow::Result<Self> {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "studio-backup-guide-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).context("Create isolated backup test directory")?;
            Ok(Self(path))
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn partial_failed_and_unavailable_sources_remain_distinct() {
        let mut snapshot = fixture();
        let provider = snapshot.providers[0].clone();
        snapshot.providers = ["complete", "partial", "unavailable", "failed"]
            .into_iter()
            .map(|status| {
                let mut provider = provider.clone();
                provider.status = status.into();
                provider
            })
            .collect();
        let summary = BackupSummary::from_snapshot(&snapshot);
        assert_eq!(
            (
                summary.complete,
                summary.partial,
                summary.unavailable,
                summary.failed
            ),
            (1, 1, 1, 1)
        );
        assert_eq!(summary.packages, provider.packages.len() * 4);
        assert!(summary.needs_review());

        snapshot.providers.truncate(1);
        assert!(!BackupSummary::from_snapshot(&snapshot).needs_review());
        snapshot.providers[0].status = "partial".into();
        assert!(BackupSummary::from_snapshot(&snapshot).needs_review());
        let mut app = super::super::tests::preview();
        assert!(app.accept_backup(snapshot.clone()).error);
        snapshot.providers[0].status = "unavailable".into();
        assert!(BackupSummary::from_snapshot(&snapshot).needs_review());
    }

    #[test]
    fn new_inventories_clear_only_related_drafts_and_failed_load_preserves_them()
    -> anyhow::Result<()> {
        let directory = TestDir::new()?;
        let mut app = super::super::tests::preview();
        let snapshot = fixture();
        let path = directory.0.join("inventory.json");
        studio_deployment::backup::save(&snapshot, &path).context("Save backup test fixture")?;
        app.backup.query = "old search".into();
        app.backup.brew_query = "old brew search".into();
        app.backup.brew_selected.insert("old brew choice".into());
        app.backup.brew_confirm = true;
        app.backup.brew_review_requested = true;
        app.selected.insert("explorer-extensions".into());
        app.backup.bundle_report = Some(studio_deployment::environment::BundleReport {
            requested: 1,
            verified: true,
            cancelled: false,
            resolved_versions: Vec::new(),
            detail: "Previous configuration history".into(),
        });
        app.backup_file_operation(FileOperation::LoadBackup, path.clone())
            .map_err(anyhow::Error::msg)?;
        assert_eq!(app.backup.snapshot.as_ref(), Some(&snapshot));
        assert_eq!(app.backup.path.as_ref(), Some(&path));
        assert!(app.backup.query.is_empty() && app.backup.brew_query.is_empty());
        assert!(app.backup.brew_selected.is_empty());
        assert!(!app.backup.brew_confirm && !app.backup.brew_review_requested);
        assert_eq!(app.selected, BTreeSet::from(["explorer-extensions".into()]));
        assert!(app.backup.bundle_report.is_some());

        let invalid = directory.0.join("invalid.json");
        std::fs::write(&invalid, b"{}").context("Write invalid backup test fixture")?;
        app.backup.query = "keep search".into();
        app.backup.brew_confirm = true;
        assert!(
            app.backup_file_operation(FileOperation::LoadBackup, invalid)
                .is_err()
        );
        assert_eq!(app.backup.snapshot.as_ref(), Some(&snapshot));
        assert_eq!(app.backup.path.as_ref(), Some(&path));
        assert_eq!(app.backup.query, "keep search");
        assert!(app.backup.brew_confirm);

        app.accept_backup(snapshot);
        assert!(app.backup.path.is_none());
        assert!(app.backup.query.is_empty());
        assert!(!app.backup.brew_confirm);
        assert!(app.backup.bundle_report.is_some());
        assert!(app.worker.is_none());
        Ok(())
    }

    #[test]
    fn busy_backup_operations_preserve_current_inventory_and_do_not_write() -> anyhow::Result<()> {
        let directory = TestDir::new()?;
        let mut app = super::super::tests::preview();
        let snapshot = fixture();
        let saved_path = directory.0.join("previous.json");
        app.set_backup_snapshot(snapshot.clone(), Some(saved_path.clone()));
        app.backup.query = "current search".into();
        app.backup.brew_confirm = true;
        let (_sender, receiver) = mpsc::channel();
        app.worker = Some(Worker {
            cancel: Arc::new(AtomicBool::new(false)),
            receiver,
            events: None,
            thread: None,
            label: "Synthetic active task",
        });
        let destination = directory.0.join("new.json");
        assert!(
            app.backup_file_operation(FileOperation::SaveBackup, destination.clone())
                .is_err()
        );
        assert!(
            app.backup_file_operation(FileOperation::LoadBackup, destination.clone())
                .is_err()
        );
        assert!(app.review_backup().is_err());
        assert!(!destination.exists());
        assert_eq!(app.backup.snapshot.as_ref(), Some(&snapshot));
        assert_eq!(app.backup.path.as_ref(), Some(&saved_path));
        assert_eq!(app.backup.query, "current search");
        assert!(app.backup.brew_confirm);
        assert!(app.worker.is_some());
        Ok(())
    }
}
