//! Mixed-source preparation and explicit import review; no imports execute commands.
use super::*;
use crate::{
    migration::{self, MigrationReport},
    solutions,
};

const WINGET_EXPORT_COMMAND: &str = "winget export --output winget.json --include-versions";

pub(super) struct HybridState {
    pub path: String,
    pub report: Option<MigrationReport>,
    pub report_target: Option<Target>,
    pub report_path: Option<String>,
    pub selected_rows: BTreeSet<usize>,
    pub review_page: usize,
    pub review_filter: String,
    pub error: Option<String>,
    pub recipe: Option<String>,
    pub visible_tasks: usize,
    pub developer_filter: String,
    pub providers: Vec<studio_deployment::ProviderCapability>,
}
impl Default for HybridState {
    fn default() -> Self {
        Self {
            path: String::new(),
            report: None,
            report_target: None,
            report_path: None,
            selected_rows: BTreeSet::new(),
            review_page: 0,
            review_filter: String::new(),
            error: None,
            recipe: None,
            visible_tasks: 60,
            developer_filter: String::new(),
            providers: studio_deployment::provider_capabilities(),
        }
    }
}

impl HybridState {
    pub(super) fn reset_review(&mut self) {
        self.selected_rows.clear();
        self.review_page = 0;
        self.review_filter.clear();
    }

    pub(super) fn checked_requests(&self) -> (BTreeSet<String>, BTreeSet<String>) {
        let mut settings = BTreeSet::new();
        let mut tasks = BTreeSet::new();
        if let Some(report) = &self.report {
            for item in self
                .selected_rows
                .iter()
                .filter_map(|index| report.items.get(*index))
            {
                if item.status == "mapped" {
                    settings.extend(item.settings.iter().cloned());
                    tasks.extend(item.deployment.iter().cloned());
                }
            }
        }
        (settings, tasks)
    }
}

impl Studio {
    pub(super) fn recipes_page(&mut self, ui: &mut egui::Ui) {
        ui.heading(tx("按用途快速配置", "Quick setup by purpose"));
        ui.label(tx("选择 Rust、Python、Web 等常见用途，先预览会加入哪些项目；加入后自动打开变更方案供你审核，再分别确认执行。", "Choose a common use such as Rust, Python or Web. Preview included items first; adding opens the change plan for review before any separate execution confirmation."));
        ui.add_space(10.0);
        self.target_controls(ui);
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(tx("导入旧档案 / Mac 清单", "Import legacy / Mac inventory"))
                .clicked()
            {
                self.navigate_to(Page::Migration);
            }
            if ui
                .button(tx("浏览开发环境任务", "Browse developer tasks"))
                .clicked()
            {
                self.navigate_to(Page::Developer);
                self.query.clear();
            }
            if ui
                .button(tx("查看当前完整方案", "Review current workspace"))
                .clicked()
            {
                self.navigate_to(Page::Plan);
            }
        });
        ui.add_space(14.0);
        for recipe in solutions::RECIPES {
            let index = usize::from(i18n::english());
            let selected = self.hybrid.recipe.as_deref() == Some(recipe.id);
            egui::Frame::new()
                .fill(if selected {
                    palette().wash
                } else {
                    palette().surface
                })
                .stroke(Stroke::new(
                    1.0,
                    if selected {
                        palette().accent
                    } else {
                        palette().line
                    },
                ))
                .corner_radius(8)
                .inner_margin(egui::Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .selectable_label(selected, RichText::new(recipe.title[index]).strong())
                            .clicked()
                        {
                            self.hybrid.recipe = Some(recipe.id.into());
                            self.hybrid.error = None;
                        }
                        ui.label(
                            RichText::new(recipe.description[index])
                                .size(12.0)
                                .color(palette().muted),
                        );
                    });
                });
            ui.add_space(6.0);
        }
        if let Some(id) = self.hybrid.recipe.as_deref()
            && let Some(recipe) = solutions::RECIPES.iter().find(|recipe| recipe.id == id)
        {
            ui.separator();
            ui.heading(recipe.title[usize::from(i18n::english())]);
            match solutions::resolve(recipe, self.target) {
                Ok((settings, tasks)) => {
                    ui.label(lf!(
                        "{} 项系统设置 / {} 项部署任务",
                        "{} settings / {} deployment tasks",
                        settings.len(),
                        tasks.len()
                    ));
                    if let Ok(catalog) = &self.catalog {
                        for action in catalog
                            .actions
                            .iter()
                            .filter(|action| settings.contains(&action.id))
                        {
                            ui.label(format!("• {}", localized_action(action).title));
                        }
                    }
                    if let Ok(catalog) = &self.deployment_catalog {
                        for task in catalog.tasks.iter().filter(|task| tasks.contains(&task.id)) {
                            ui.label(format!(
                                "• {}  [{}] {}",
                                localized_task(task).title,
                                task.kind,
                                task.package_id.as_deref().unwrap_or(&task.id)
                            ));
                        }
                    }
                    ui.label(RichText::new(tx("加入会保留其他选择，并替换同组互斥设置。软件先于开发工具执行；提供程序、权限、源和工具链仍需满足。不会立即安装或修改系统。", "Adding preserves other choices and replaces conflicting alternatives. Software runs before developer tasks; providers, permissions, sources and toolchains must still be ready. Adding makes no system changes.")).size(12.0).color(palette().muted));
                    if primary(
                        ui,
                        tx("加入当前方案", "Add to workspace"),
                        self.worker.is_none(),
                    )
                    .clicked()
                    {
                        self.merge_workspace(&settings, &tasks);
                    }
                }
                Err(error) => {
                    ui.colored_label(palette().danger, format!("{error:#}"));
                }
            }
        }
        if let Some(error) = &self.hybrid.error {
            ui.colored_label(palette().danger, error);
        }
    }

    pub(super) fn migration_page(&mut self, ui: &mut egui::Ui) {
        ui.heading(tx("迁移与映射", "Migration and mapping"));
        ui.label(tx("导入 WinGet 导出清单、DevEnv Studio 的 Mac/Windows 备份，或旧 SetupProfile/Mac 清单。先分析，再勾选要加入 Windows 方案的项目；原文件保持不变。", "Import a WinGet export, a DevEnv Studio Mac/Windows backup, or a legacy SetupProfile/Mac inventory. Analyze first, then select items for a Windows plan; the source file stays unchanged."));
        self.target_controls(ui);
        ui.strong(tx("1. 导入现成清单", "1. Import an existing inventory"));
        ui.label(
            RichText::new(tx(
                "选择清单路径或拖入文件，分析后再逐项审核。",
                "Enter an inventory path or drop a file, then analyze and review its items.",
            ))
            .size(12.0)
            .color(palette().muted),
        );
        egui::CollapsingHeader::new(tx("如何导出 WinGet 清单", "How to export a WinGet inventory"))
            .id_salt("migration-winget-export-help")
            .show(ui, |ui| {
                ui.label(tx("在装有 WinGet 的 Windows 电脑上，打开 PowerShell，按需自行运行：", "On a Windows computer with WinGet, open PowerShell and run this yourself when needed:"));
                ui.add(egui::Label::new(RichText::new(WINGET_EXPORT_COMMAND).monospace()).wrap());
                if ui.button(tx("复制导出命令", "Copy export command")).clicked() {
                    ui.ctx().copy_text(WINGET_EXPORT_COMMAND.to_owned());
                }
                ui.label(tx("请将示例 winget.json 改为一个新的输出文件名，并保留之前的清单。导出后拖入本页，或输入其绝对路径后分析；本页只提供说明与复制，不执行命令。", "Use a new output filename in place of winget.json and keep earlier inventories. Drop the export here or enter its absolute path to analyze. This page only explains and copies the command; it does not run it."));
                ui.label(tx("WinGet 可能遗漏未识别的软件。导出不是完整系统备份，记录的版本不代表本应用会锁定或恢复该版本。", "WinGet may omit unrecognized software. The export is not a full system backup, and recorded versions are not installation pins or version-restoration guarantees here."));
                ui.label(tx("此处的 Windows 软件方案在 Mac 上只供预览或导出；实际安装须在受支持的 Windows 上另行核对并确认。", "Windows software plans here can only be previewed or exported on Mac. Installation requires separate review and confirmation on supported Windows."));
            });
        for dropped in ui.ctx().input(|input| input.raw.dropped_files.clone()) {
            self.hybrid.path = dropped.path().display().to_string();
        }
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.hybrid.path)
                    .hint_text(tx(
                        "JSON 文件绝对路径，也可拖入文件",
                        "Absolute JSON path, or drop a file here",
                    ))
                    .desired_width(480.0),
            );
            if ui
                .add_enabled(
                    self.worker.is_none(),
                    egui::Button::new(tx("分析清单", "Analyze inventory")),
                )
                .clicked()
            {
                self.hybrid.report = None;
                self.hybrid.reset_review();
                self.hybrid.error = None;
                self.hybrid.report_target = None;
                self.hybrid.report_path = None;
                let path = PathBuf::from(self.hybrid.path.trim());
                if !path.is_absolute() {
                    self.hybrid.error = Some(
                        tx(
                            "请选择绝对路径的 JSON 文件。",
                            "Choose an absolute JSON file path.",
                        )
                        .into(),
                    );
                } else {
                    match migration::load(&path, self.target) {
                        Ok(report) => {
                            self.hybrid.report = Some(report);
                            self.hybrid.report_target = Some(self.target);
                            self.hybrid.report_path = Some(self.hybrid.path.trim().to_owned());
                        }
                        Err(error) => self.hybrid.error = Some(format!("{error:#}")),
                    }
                }
            }
        });
        ui.label(RichText::new(tx("分析只生成报告，不会勾选或安装。未知软件、未支持的来源或安装选项保留为说明；私密源地址、安装参数、Git 身份与 Shell 内容不会原样回显或写入。", "Analysis only creates a report; it selects and installs nothing. Unknown packages, unsupported sources and installation options remain review notes. Private source addresses, installer arguments, Git identity and shell content are not echoed or written.")).size(12.0).color(palette().muted));
        if let Some(report) = &self.hybrid.report {
            ui.separator();
            ui.strong(tx("2. 查看并勾选映射", "2. Review and select mappings"));
            ui.label(lf!(
                "已分析文件：{}",
                "Analyzed file: {}",
                self.hybrid.report_path.as_deref().unwrap_or("")
            ));
            let mapped = report
                .items
                .iter()
                .filter(|item| item.status == "mapped")
                .count();
            ui.label(lf!(
                "来源：{}；可映射 {} 项；其余 {} 项说明",
                "Source: {}; mapped {}; other notes {}",
                crate::migration_locale::source(&report.source, self.preferences.language),
                mapped,
                report.items.len() - mapped
            ));
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button(tx("勾选所有已映射项目", "Select all mapped items"))
                    .clicked()
                {
                    self.hybrid.selected_rows = report
                        .items
                        .iter()
                        .enumerate()
                        .filter(|(_, item)| item.status == "mapped")
                        .map(|(index, _)| index)
                        .collect();
                }
                if ui.button(tx("取消勾选", "Clear checked items")).clicked() {
                    self.hybrid.selected_rows.clear();
                }
            });
            let previous_filter = self.hybrid.review_filter.clone();
            ui.horizontal_wrapped(|ui| {
                for (value, label) in [
                    ("", tx("全部", "All")),
                    ("mapped", tx("已映射", "Mapped")),
                    ("other", tx("其余说明", "Other")),
                ] {
                    ui.selectable_value(&mut self.hybrid.review_filter, value.to_owned(), label);
                }
            });
            if previous_filter != self.hybrid.review_filter {
                self.hybrid.review_page = 0;
            }
            let indices: Vec<_> = report
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| match self.hybrid.review_filter.as_str() {
                    "mapped" => item.status == "mapped",
                    "other" => item.status != "mapped",
                    _ => true,
                })
                .map(|(index, _)| index)
                .collect();
            let pages = indices.len().div_ceil(20).max(1);
            self.hybrid.review_page = self.hybrid.review_page.min(pages - 1);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        self.hybrid.review_page > 0,
                        egui::Button::new(tx("上一页", "Previous")),
                    )
                    .clicked()
                {
                    self.hybrid.review_page -= 1;
                }
                ui.label(lf!(
                    "第 {} / {} 页 · {} 项",
                    "Page {} / {} · {} items",
                    self.hybrid.review_page + 1,
                    pages,
                    indices.len()
                ));
                if ui
                    .add_enabled(
                        self.hybrid.review_page + 1 < pages,
                        egui::Button::new(tx("下一页", "Next")),
                    )
                    .clicked()
                {
                    self.hybrid.review_page += 1;
                }
            });
            for &index in indices.iter().skip(self.hybrid.review_page * 20).take(20) {
                let item = &report.items[index];
                let title = self.migration_title(item);
                let mut selected = self.hybrid.selected_rows.contains(&index);
                ui.scope_builder(
                    egui::UiBuilder::new().scope_id(egui::Id::unique(("migration-row", index))),
                    |ui| {
                        ui.horizontal_wrapped(|ui| {
                            if ui
                                .add_enabled(
                                    item.status == "mapped",
                                    egui::Checkbox::new(&mut selected, title),
                                )
                                .changed()
                            {
                                if selected {
                                    self.hybrid.selected_rows.insert(index);
                                } else {
                                    self.hybrid.selected_rows.remove(&index);
                                }
                            }
                            let status = match item.status.as_str() {
                                "mapped" => tx("已映射", "Mapped"),
                                "manual" if item.origin == "profile_version" => {
                                    tx("格式已识别", "Format identified")
                                }
                                "manual"
                                    if matches!(
                                        item.requested.as_str(),
                                        "false" | "(empty list)"
                                    ) =>
                                {
                                    tx("无需变更", "No change requested")
                                }
                                "manual" => tx("需手动处理", "Manual"),
                                _ => tx("尚未迁移", "Unsupported"),
                            };
                            ui.label(
                                RichText::new(format!("{status} · {}", item.origin))
                                    .size(11.0)
                                    .color(if item.status == "mapped" {
                                        palette().accent
                                    } else {
                                        palette().muted
                                    }),
                            );
                        });
                        ui.label(
                            RichText::new(crate::migration_locale::detail(
                                &item.detail,
                                self.preferences.language,
                            ))
                            .size(12.0)
                            .color(palette().muted),
                        );
                        ui.separator();
                    },
                );
            }
            ui.strong(tx(
                "3. 加入当前 Windows 方案",
                "3. Add to the Windows workspace",
            ));
            let (added_settings, added_tasks) = self.hybrid.checked_requests();
            let target_matches = self.migration_current();
            if !target_matches {
                ui.colored_label(
                    palette().danger,
                    tx(
                        "输入文件或目标已改变，请重新分析后再合并。",
                        "The input path or target changed. Analyze again before merging.",
                    ),
                );
            }
            ui.label(lf!(
                "待加入：{} 项设置 / {} 项任务",
                "Ready to add: {} settings / {} tasks",
                added_settings.len(),
                added_tasks.len()
            ));
            if primary(
                ui,
                tx(
                    "仅将勾选项目合并到方案",
                    "Merge checked items into workspace",
                ),
                target_matches
                    && self.worker.is_none()
                    && (!added_settings.is_empty() || !added_tasks.is_empty()),
            )
            .clicked()
            {
                self.merge_workspace(&added_settings, &added_tasks);
            }
        }
        if let Some(error) = &self.hybrid.error {
            ui.colored_label(palette().danger, error);
        }
    }

    fn migration_title(&self, item: &crate::migration::MigrationItem) -> String {
        let mut names = Vec::new();
        if let Ok(catalog) = &self.catalog {
            for id in &item.settings {
                if let Some(action) = catalog.actions.iter().find(|action| &action.id == id) {
                    names.push(localized_action(action).title);
                }
            }
        }
        if let Ok(catalog) = &self.deployment_catalog {
            for id in &item.deployment {
                if let Some(task) = catalog.tasks.iter().find(|task| &task.id == id) {
                    names.push(localized_task(task).title);
                }
            }
        }
        if names.is_empty() {
            crate::migration_locale::requested(&item.requested, self.preferences.language)
        } else {
            names.join(" / ")
        }
    }

    pub(super) fn migration_current(&self) -> bool {
        self.hybrid.report.is_some()
            && self.hybrid.report_target == Some(self.target)
            && self.hybrid.report_path.as_deref() == Some(self.hybrid.path.trim())
    }

    fn merge_workspace(&mut self, settings: &BTreeSet<String>, tasks: &BTreeSet<String>) {
        let result = self
            .catalog
            .as_ref()
            .map_err(|error| anyhow::anyhow!("{error}"))
            .and_then(|catalog| {
                solutions::merge(
                    catalog,
                    self.target,
                    &self.selected,
                    &self.deployment_selected,
                    settings,
                    tasks,
                )
            });
        match result {
            Ok((settings, tasks)) => {
                self.selected = settings;
                self.deployment_selected = tasks;
                self.setting_states.clear();
                self.deployment_states.clear();
                self.hybrid.error = None;
                self.notice = Some(Notice { text: tx("已加入方案；请在此处核对具体变更后再确认执行。", "Added to the workspace. Review the exact changes here before confirming execution.").into(), error: false });
                self.navigate_to(Page::Plan);
            }
            Err(error) => self.hybrid.error = Some(format!("{error:#}")),
        }
    }

    pub(super) fn providers_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(tx("执行提供程序", "Execution providers")).strong());
            if ui
                .add_enabled(
                    self.worker.is_none(),
                    egui::Button::new(tx("刷新", "Refresh")),
                )
                .clicked()
            {
                self.hybrid.providers = studio_deployment::provider_capabilities();
            }
        });
        ui.horizontal_wrapped(|ui| {
            for (kind, label) in [
                ("", tx("全部", "All")),
                ("cargo", "Cargo"),
                ("uv", "uv"),
                ("pip", "Pip"),
                ("npm", "NPM"),
                ("vscode", "VS Code"),
            ] {
                if ui
                    .selectable_label(self.hybrid.developer_filter == kind, label)
                    .clicked()
                {
                    self.hybrid.developer_filter = kind.into();
                    self.hybrid.visible_tasks = 60;
                }
            }
        });
        match self.hybrid.developer_filter.as_str() {
            "npm" => {
                ui.label(RichText::new(tx("NPM 使用当前用户的 %APPDATA%\\npm，禁用生命周期脚本，不修改 PATH、镜像或项目依赖。安装后核对包身份；工具可运行性需另行验证。", "NPM uses the current user’s %APPDATA%\\npm, disables lifecycle scripts and leaves PATH, mirrors and project dependencies unchanged. Readback checks package identity; verify tool functionality separately.")).size(12.0).color(palette().muted));
            }
            "pip" => {
                ui.label(RichText::new(tx("Pip 使用已安装的标准 CPython，安装到当前用户范围，只接受兼容 wheel。已安装的所选包会跳过；依赖可能随安装调整。不会修改项目、虚拟环境、镜像或 PATH。", "Pip uses an installed standard CPython and user installs with compatible wheels only. Selected packages already visible are skipped; dependencies may change during installation. Projects, virtual environments, mirrors and PATH are not modified.")).size(12.0).color(palette().muted));
            }
            _ => {
                ui.label(RichText::new(tx("按提供程序筛选工具；找到程序路径后，检查和执行时还会验证运行条件。", "Filter tools by provider. Located paths are checked again for runtime prerequisites during inspection and execution.")).size(12.0).color(palette().muted));
            }
        }
        ui.horizontal_wrapped(|ui| {
            for provider in &self.hybrid.providers {
                let state = if provider.available && provider.provider == "pip" {
                    tx("候选待预检", "Candidate; preflight required")
                } else if provider.available {
                    tx("已找到", "Located")
                } else if cfg!(windows) {
                    tx("未找到", "Unavailable")
                } else {
                    tx("预览", "Preview")
                };
                badge(
                    ui,
                    &format!("{} · {state}", provider.provider),
                    palette().wash,
                    if provider.available {
                        palette().accent
                    } else {
                        palette().muted
                    },
                );
            }
        });
        egui::CollapsingHeader::new(tx(
            "提供程序详情与安装前提",
            "Provider details and prerequisites",
        ))
        .id_salt("developer-provider-details")
        .show(ui, |ui| {
            for provider in &self.hybrid.providers {
                ui.label(RichText::new(&provider.provider).strong());
                ui.label(&provider.detail);
            }
        });
        ui.label(RichText::new(tx("可在软件部署中明确选择 Rust MSVC、uv、Node.js LTS、Python 和 VS Code，再勾选对应工具。Pip 还需已有可用的 pip；Rust 链接器、SDK 等前提需按工具链准备。", "Explicitly select Rust MSVC, uv, Node.js LTS, Python and VS Code in Software, then choose tools. Pip also requires a working pip installation; prepare the Rust linker, SDK and other toolchain prerequisites as required.")).size(12.0).color(palette().muted));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn winget_review_starts_unselected_and_merges_only_checked_trusted_rows() -> anyhow::Result<()>
    {
        let mut app = super::super::tests::preview();
        app.navigate_to(Page::Migration);
        app.preferences.language = Language::EnUs;
        i18n::set_language(Language::EnUs);
        app.catalog = Ok(studio_core::load_catalog()?);
        let catalog = studio_deployment::catalog()?;
        let trusted: Vec<_> = catalog
            .tasks
            .iter()
            .filter(|task| task.kind == "package" && task.targets.contains(&app.target))
            .take(2)
            .collect();
        assert_eq!(trusted.len(), 2);
        let added = trusted[0].id.clone();
        let identity = trusted[0].package_id.clone().unwrap();
        let previous = trusted[1].id.clone();
        app.deployment_selected.insert(previous.clone());
        app.deployment_catalog = Ok(catalog);
        let document = serde_json::json!({
            "$schema": "https://aka.ms/winget-packages.schema.2.0.json",
            "Sources": [{
                "SourceDetails": {
                    "Argument": "https://cdn.winget.microsoft.com/cache",
                    "Identifier": "Microsoft.Winget.Source_8wekyb3d8bbwe",
                    "Name": "winget",
                    "Type": "Microsoft.PreIndexed.Package"
                },
                "Packages": [
                    {"PackageIdentifier": identity, "Version": "2.0"},
                    {"PackageIdentifier": "Unknown.Tool", "InitialCustomSwitches": "secret-options-marker"}
                ]
            }]
        });
        let mut report = migration::analyze(&serde_json::to_vec(&document)?, app.target)?;
        assert_eq!(report.source, "winget_export");
        assert!(!serde_json::to_string(&report)?.contains("secret-options-marker"));
        let mapped = report
            .items
            .iter()
            .position(|item| item.status == "mapped")
            .unwrap();
        let manual = report
            .items
            .iter()
            .position(|item| item.status == "manual")
            .unwrap();
        report.items[manual]
            .settings
            .push("manual-setting-must-not-merge".into());
        report.items[manual]
            .deployment
            .push("manual-task-must-not-merge".into());
        app.hybrid.report = Some(report);
        app.hybrid.path = std::env::temp_dir()
            .join("winget-review.json")
            .display()
            .to_string();
        app.hybrid.report_path = Some(app.hybrid.path.clone());
        app.hybrid.report_target = Some(app.target);

        let context = egui::Context::default();
        context.enable_accesskit();
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(980.0, 1200.0),
                )),
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| app.migration_page(ui));
            },
        );
        output.textures_delta.clear();
        let nodes = output
            .platform_output
            .accesskit_update
            .expect("accessibility tree")
            .nodes;
        for required in ["Analyze inventory", "Target", short_target(app.target)] {
            assert!(
                nodes
                    .iter()
                    .any(|(_, node)| { node.label().or_else(|| node.value()) == Some(required) }),
                "{required} is reachable"
            );
        }
        for hidden in ["Load profile", "Save profile", "Inspect selection"] {
            assert!(
                !nodes
                    .iter()
                    .any(|(_, node)| { node.label().or_else(|| node.value()) == Some(hidden) }),
                "{hidden} is outside the inventory workflow"
            );
        }
        assert!(nodes.iter().any(|(_, node)| {
            node.label()
                .or_else(|| node.value())
                .is_some_and(|label| label.contains("Source: WinGet export"))
        }));
        assert!(!nodes.iter().any(|(_, node)| {
            node.label()
                .or_else(|| node.value())
                .is_some_and(|label| label.contains("secret-options-marker"))
        }));
        assert!(app.hybrid.selected_rows.is_empty());
        assert!(
            app.hybrid.checked_requests().0.is_empty()
                && app.hybrid.checked_requests().1.is_empty()
        );
        assert_eq!(app.deployment_selected, BTreeSet::from([previous.clone()]));
        assert!(app.worker.is_none() && app.dialog.is_none());

        app.hybrid.selected_rows.extend([manual, usize::MAX]);
        assert!(
            app.hybrid.checked_requests().0.is_empty()
                && app.hybrid.checked_requests().1.is_empty()
        );
        app.hybrid.selected_rows.insert(mapped);
        let (settings, tasks) = app.hybrid.checked_requests();
        assert!(settings.is_empty());
        assert_eq!(tasks, BTreeSet::from([added.clone()]));
        assert!(app.migration_current());
        let original_path = app.hybrid.path.clone();
        app.hybrid.path.push_str(".changed");
        assert!(!app.migration_current());
        app.hybrid.path = original_path;
        app.target = Target::Win10Ltsc2021;
        assert!(!app.migration_current());
        app.target = Target::Win11Ltsc2024;
        assert!(app.migration_current());
        app.merge_workspace(&settings, &tasks);
        assert_eq!(app.deployment_selected, BTreeSet::from([previous, added]));
        assert!(app.selected.is_empty());
        assert!(app.worker.is_none() && app.dialog.is_none());
        assert!(app.page == Page::Plan);
        i18n::set_language(Language::ZhCn);
        Ok(())
    }
}
