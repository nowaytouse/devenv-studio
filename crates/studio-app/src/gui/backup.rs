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
    pub brew_policy: studio_deployment::environment::BrewPolicy,
    pub rust: rust_toolchains::RustUiState,
    pub python_tools: super::python_tools::UiState,
    pub bundle_report: Option<studio_deployment::environment::BundleReport>,
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
        let failed = snapshot
            .providers
            .iter()
            .filter(|p| p.status == "failed")
            .count();
        let complete = snapshot
            .providers
            .iter()
            .filter(|p| p.status == "complete")
            .count();
        let packages: usize = snapshot.providers.iter().map(|p| p.packages.len()).sum();
        self.backup.snapshot = Some(snapshot);
        self.backup.path = None;
        self.backup.brew_selected.clear();
        self.backup.brew_confirm = false;
        Notice {
            text: lf!(
                "已采集 {complete} 类来源、{packages} 个条目；失败 {failed} 类。请检查并保存本地备份。",
                "Collected {complete} providers and {packages} entries; {failed} providers failed. Review and save a local backup.",
                complete = complete,
                packages = packages,
                failed = failed
            ),
            error: failed > 0,
        }
    }

    pub(super) fn backup_file_operation(
        &mut self,
        operation: FileOperation,
        path: PathBuf,
    ) -> Result<String, String> {
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
                self.backup.snapshot = Some(snapshot);
                self.backup.path = Some(path.clone());
                self.backup.brew_selected.clear();
                self.backup.brew_confirm = false;
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
        self.page = Page::Migration;
        Ok(())
    }

    pub(super) fn backup_page(&mut self, ui: &mut egui::Ui) {
        ui.heading(tx("开发环境备份", "Developer environment backup"));
        ui.label(tx("采集本机工具与已安装版本，保存本地备份，选择 macOS 或 Windows 配置流程。", "Capture local tools and installed versions, save a backup, then choose macOS or Windows configuration."));
        ui.label(RichText::new(tx(
            "macOS：Homebrew、Cargo、uv、NPM、VS Code；Windows：Cargo、uv、NPM、Pip、VS Code。仅记录包名与版本，不复制二进制、项目、密钥、配置或虚拟环境；采集不安装或更新。",
            "macOS: Homebrew, Cargo, uv, NPM, VS Code. Windows: Cargo, uv, NPM, Pip, VS Code. Inventories contain identities and versions, not binaries, projects, keys, settings or virtual environments. Capture never installs or updates.",
        )).size(12.0).color(palette().muted));
        ui.add_space(12.0);
        ui.horizontal_wrapped(|ui| {
            if primary(
                ui,
                tx("采集本机开发环境", "Capture local environment"),
                cfg!(any(target_os = "macos", windows)) && self.worker.is_none(),
            )
            .clicked()
            {
                self.start_backup();
            }
            if ui
                .add_enabled(
                    self.worker.is_none(),
                    egui::Button::new(tx("载入备份…", "Load backup…")),
                )
                .clicked()
            {
                self.open_file(FileOperation::LoadBackup);
            }
            if ui
                .add_enabled(
                    self.worker.is_none() && self.backup.snapshot.is_some(),
                    egui::Button::new(tx("保存备份…", "Save backup…")),
                )
                .clicked()
            {
                self.open_file(FileOperation::SaveBackup);
            }
            if ui
                .add_enabled(
                    self.worker.is_none() && self.backup.path.is_some(),
                    egui::Button::new(tx("审核 Windows 映射", "Review Windows mappings")),
                )
                .clicked()
                && let Err(error) = self.review_backup()
            {
                self.notice = Some(Notice {
                    text: format!("{error:#}"),
                    error: true,
                });
            }
        });
        if ui
            .button(tx("进入环境配置", "Configure environment"))
            .clicked()
        {
            self.page = Page::Environment;
        }
        if !cfg!(any(target_os = "macos", windows)) {
            ui.label(tx(
                "当前平台可载入备份；本机采集需要 macOS 或 Windows。",
                "This platform can load backups; local collection requires macOS or Windows.",
            ));
        }
        ui.label(RichText::new(tx("采集仅在明确点击后运行本地只读命令。取消或整体失败保留上一次清单；每类来源独立显示完整、未找到或失败。", "Collection runs local read-only commands only after an explicit click. Cancellation or an overall failure preserves the previous inventory; each provider reports complete, unavailable or failed.")).size(12.0).color(palette().muted));
        if let Some(path) = &self.backup.path {
            ui.label(lf!("本地文件：{}", "Local file: {}", path.display()));
        } else if self.backup.snapshot.is_some() {
            ui.label(tx(
                "尚未保存；保存后可审核迁移映射。",
                "Not saved yet. Save to review migration mappings.",
            ));
        }
        ui.add_space(12.0);
        let Some(snapshot) = &self.backup.snapshot else {
            empty(
                ui,
                tx("尚无开发环境备份", "No environment backup yet"),
                tx(
                    "采集本机或载入先前的 JSON 备份。清单不会上传到 GitHub。",
                    "Capture this machine or load an earlier JSON backup. Inventories are not uploaded to GitHub.",
                ),
            );
            return;
        };
        ui.label(format!(
            "{}: {}",
            tx("来源平台", "Source platform"),
            snapshot.platform
        ));
        ui.label(lf!(
            "采集时间：{}",
            "Captured at: {}",
            captured_time(&snapshot.captured_at)
        ));
        ui.add(
            egui::TextEdit::singleline(&mut self.backup.query)
                .hint_text(tx("搜索包名或版本…", "Search package or version…"))
                .desired_width(450.0),
        );
        let query = self.backup.query.to_lowercase();
        for provider in &snapshot.providers {
            let state = match provider.status.as_str() {
                "complete" => tx("完整", "Complete"),
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
        ui.label(RichText::new(tx("记录的版本是本次观测值，不保证未来可重新安装完全相同的构建。迁移映射需要逐项勾选，不会自动安装或复制 macOS 环境。", "Recorded versions are observations, not a guarantee that identical builds can be reinstalled later. Review and select migration mappings individually; nothing is installed automatically.")).size(12.0).color(palette().muted));
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
        _ => kind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
