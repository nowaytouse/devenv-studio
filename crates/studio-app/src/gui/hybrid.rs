//! Mixed-source preparation and explicit import review; no imports execute commands.
use super::*;
use crate::{
    migration::{self, MigrationReport},
    solutions,
};

pub(super) struct HybridState {
    pub path: String,
    pub report: Option<MigrationReport>,
    pub report_target: Option<Target>,
    pub report_path: Option<String>,
    pub selected_rows: BTreeSet<usize>,
    pub error: Option<String>,
    pub recipe: Option<String>,
    pub visible_tasks: usize,
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
            error: None,
            recipe: None,
            visible_tasks: 60,
            providers: studio_deployment::provider_capabilities(),
        }
    }
}

impl Studio {
    pub(super) fn recipes_page(&mut self, ui: &mut egui::Ui) {
        ui.heading(tx("方案工作区", "Workspace recipes"));
        ui.label(tx("从旧工具导入，或从一个用途组合开始，再逐项修改。", "Import a legacy profile or start with a mixed workflow, then adjust individual choices."));
        ui.add_space(10.0);
        self.target_controls(ui);
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(tx("导入旧档案 / Mac 清单", "Import legacy / Mac inventory"))
                .clicked()
            {
                self.page = Page::Migration;
            }
            if ui
                .button(tx("浏览开发环境任务", "Browse developer tasks"))
                .clicked()
            {
                self.page = Page::Developer;
                self.query.clear();
            }
            if ui
                .button(tx("查看当前完整方案", "Review current workspace"))
                .clicked()
            {
                self.page = Page::Plan;
            }
        });
        ui.add_space(14.0);
        for recipe in solutions::RECIPES {
            let index = usize::from(i18n::english());
            ui.horizontal_wrapped(|ui| {
                if ui
                    .selectable_label(
                        self.hybrid.recipe.as_deref() == Some(recipe.id),
                        recipe.title[index],
                    )
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
        ui.label(tx("读取旧 SetupProfile 或 schema 1 Mac 清单 JSON，逐项查看映射和未迁移内容。原文件保持不变。", "Read legacy SetupProfile or schema 1 Mac inventory JSON and review every mapping and gap. The source file remains unchanged."));
        self.target_controls(ui);
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
                    egui::Button::new(tx("分析导入", "Analyze import")),
                )
                .clicked()
            {
                self.hybrid.report = None;
                self.hybrid.selected_rows.clear();
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
        ui.label(RichText::new(tx("导入只产生报告。Git 身份、Shell 内容、镜像地址及未支持的系统调优不会被自动写入；报告不会回显这些私密字段值。", "Import only produces a report. Git identity, shell content, mirror addresses and unsupported system tuning are never automatically written; their private values are not echoed.")).size(12.0).color(palette().muted));
        if let Some(report) = &self.hybrid.report {
            ui.separator();
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
                report.source,
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
            let mut added_settings = BTreeSet::new();
            let mut added_tasks = BTreeSet::new();
            for (index, item) in report.items.iter().enumerate() {
                let title = self.migration_title(item);
                let mut selected = self.hybrid.selected_rows.contains(&index);
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
                        "manual" if matches!(item.requested.as_str(), "false" | "(empty list)") => {
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
                if selected {
                    added_settings.extend(item.settings.iter().cloned());
                    added_tasks.extend(item.deployment.iter().cloned());
                }
                ui.separator();
            }
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
                self.notice = Some(Notice { text: tx("已加入方案。可继续逐项修改；进入变更方案检查后再分别确认执行。", "Added to the workspace. Adjust individual choices, then review the plan before confirming execution.").into(), error: false });
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
        for provider in &self.hybrid.providers {
            egui::CollapsingHeader::new(format!(
                "{} · {}",
                provider.provider,
                if provider.available {
                    tx("已找到", "Located")
                } else {
                    tx("不可用 / 预览", "Unavailable / preview")
                }
            ))
            .id_salt(("provider", &provider.provider))
            .show(ui, |ui| {
                ui.label(&provider.detail);
            });
        }
        ui.label(RichText::new(tx("可在软件部署中选择 Rust MSVC、uv 和 VS Code，再勾选对应开发工具。Rust 链接器、SDK 及兼容的 Python 运行时仍需自行准备。", "Select Rust MSVC, uv and VS Code in Software, then choose developer tasks. Prepare the Rust linker, SDK and compatible Python runtime as required.")).size(12.0).color(palette().muted));
    }
}
