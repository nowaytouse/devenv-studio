//! Application-level pages. They use the same preferences and real execution state as the workbench.
use super::i18n::tx;
use super::*;

impl Studio {
    pub(super) fn settings_page(&mut self, ui: &mut egui::Ui) {
        ui.heading(tx("设置", "Settings"));
        ui.label(
            RichText::new(tx(
                "立即预览界面变化；点击保存后，下次启动继续使用。",
                "Changes preview immediately. Save to keep them for the next launch.",
            ))
            .color(palette().muted),
        );
        ui.add_space(14.0);
        settings_section(ui, tx("语言与内容", "Language and content"), |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(tx("界面语言", "Interface language"));
                egui::ComboBox::from_id_salt("app-language")
                    .selected_text(match self.preferences.language {
                        Language::ZhCn => "简体中文",
                        Language::EnUs => "English",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.preferences.language,
                            Language::ZhCn,
                            "简体中文",
                        );
                        ui.selectable_value(
                            &mut self.preferences.language,
                            Language::EnUs,
                            "English",
                        );
                    });
            });
            ui.label(RichText::new(tx("设置说明、软件任务与确认对话框随语言切换。注册表地址、包 ID、导出文件和原始诊断保持原样。", "Settings, task descriptions and confirmations follow the selected language. Registry addresses, package IDs, exported documents and raw diagnostics retain their original values.")).size(12.0).color(palette().muted));
            if let Err(error) = crate::catalog_locale::validate() {
                ui.colored_label(
                    palette().danger,
                    tx(
                        "内容翻译目录不可用，将显示原始内容：",
                        "Content translations are unavailable; original catalog text is shown:",
                    ),
                );
                ui.label(format!("{error:#}"));
            }
        });
        settings_section(ui, tx("外观", "Appearance"), |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(tx("主题", "Theme"));
                for (value, zh, en) in [
                    (Appearance::System, "跟随系统", "System"),
                    (Appearance::Light, "浅色", "Light"),
                    (Appearance::Dark, "深色", "Dark"),
                ] {
                    ui.radio_value(&mut self.preferences.appearance, value, tx(zh, en));
                }
            });
            ui.label(tx("强调色", "Accent color"));
            ui.horizontal_wrapped(|ui| {
                for (value, zh, en) in [
                    (Accent::Blue, "蓝", "Blue"),
                    (Accent::Teal, "青绿", "Teal"),
                    (Accent::Violet, "紫", "Violet"),
                    (Accent::Rose, "玫瑰", "Rose"),
                    (Accent::Amber, "琥珀", "Amber"),
                ] {
                    let selected = self.preferences.accent == value;
                    let swatch =
                        theme::Palette::for_theme(ui.ctx().theme() == egui::Theme::Dark, value);
                    let label = format!("{} {}", if selected { "●" } else { "○" }, tx(zh, en));
                    if ui
                        .add(
                            egui::Button::new(RichText::new(label).color(if selected {
                                swatch.on_accent
                            } else {
                                palette().ink
                            }))
                            .fill(if selected {
                                swatch.accent
                            } else {
                                palette().surface
                            })
                            .stroke(Stroke::new(1.0, swatch.accent)),
                        )
                        .clicked()
                    {
                        self.preferences.accent = value;
                    }
                }
            });
            ui.label(RichText::new(tx("强调色用于焦点、链接与主要操作；所有颜色均提供浅色与深色配色。", "Accent colors identify focus, links and primary actions, with separate light and dark palettes.")).size(12.0).color(palette().muted));
        });
        settings_section(ui, tx("阅读与布局", "Reading and layout"), |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(tx("界面缩放", "Interface scale"));
                for scale in [90, 100, 110, 125] {
                    ui.radio_value(
                        &mut self.preferences.scale_percent,
                        scale,
                        format!("{scale}%"),
                    );
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(tx("间距", "Density"));
                ui.radio_value(
                    &mut self.preferences.density,
                    Density::Comfortable,
                    tx("舒适", "Comfortable"),
                );
                ui.radio_value(
                    &mut self.preferences.density,
                    Density::Compact,
                    tx("紧凑", "Compact"),
                );
            });
            ui.label(RichText::new(tx("缩放保留系统显示器 DPI；导航与内容在较小窗口中仍可滚动。", "Scaling preserves the display's native DPI. Navigation and content remain scrollable in smaller windows.")).size(12.0).color(palette().muted));
        });
        ui.horizontal_wrapped(|ui| {
            if primary(
                ui,
                tx("保存设置", "Save settings"),
                self.preferences != self.saved_preferences || self.preferences_error.is_some(),
            )
            .clicked()
            {
                match crate::prefs::save(&self.preferences) {
                    Ok(()) => {
                        self.saved_preferences = self.preferences.clone();
                        self.preferences_error = None;
                        self.notice = Some(Notice {
                            text: tx(
                                "设置已保存。下次启动将继续使用。",
                                "Preferences saved for the next launch.",
                            )
                            .into(),
                            error: false,
                        });
                    }
                    Err(error) => self.preferences_error = Some(format!("{error:#}")),
                }
            }
            if ui
                .add_enabled(
                    self.preferences != self.saved_preferences,
                    egui::Button::new(tx("撤销未保存修改", "Revert unsaved changes")),
                )
                .clicked()
            {
                self.preferences = self.saved_preferences.clone();
            }
            if ui.button(tx("使用默认值", "Use defaults")).clicked() {
                self.preferences = Preferences::default();
            }
            ui.label(
                RichText::new(if self.preferences == self.saved_preferences {
                    tx("已保存", "Saved")
                } else {
                    tx("有未保存的界面设置", "Unsaved interface settings")
                })
                .size(12.0)
                .color(palette().muted),
            );
        });
        if let Some(error) = &self.preferences_error {
            ui.colored_label(palette().danger,tx("设置文件读取或保存失败。当前使用可预览的设置，原文件错误如下：","Preferences could not be read or saved. The current settings remain a preview. Original error:"));
            ui.label(error);
        }
        if let Ok(path) = crate::prefs::path() {
            ui.label(
                RichText::new(path.display().to_string())
                    .size(12.0)
                    .color(palette().muted),
            );
        }
        i18n::set_language(self.preferences.language);
        theme::apply(ui.ctx(), &self.preferences);
    }

    pub(super) fn status_page(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.heading(tx("状态", "Status"));
            if ui
                .add_enabled(
                    self.worker.is_none(),
                    egui::Button::new(tx("刷新本机与记录", "Refresh host and records")),
                )
                .clicked()
            {
                self.host = studio_platform::host();
                self.winget = studio_deployment::winget_capability();
                self.refresh_recovery();
            }
        });
        ui.label(RichText::new(tx("来自当前主机、已验证目录与实际执行报告；不推测系统健康度。","Actual host information, validated catalogs and execution reports. No inferred health score.")).color(palette().muted));
        settings_section(ui, tx("主机与能力", "Host and capabilities"), |ui| {
            property(ui, tx("运行主机", "Host"), &self.host.name);
            property(ui, tx("方案目标", "Plan target"), self.target.label());
            property(
                ui,
                tx("已识别的 Windows", "Detected Windows"),
                self.host.target.map(|target| target.label()).unwrap_or(tx(
                    "未识别受支持的 Windows",
                    "No supported Windows identified",
                )),
            );
            property(
                ui,
                tx("执行模式", "Execution mode"),
                if self.mutation_available() {
                    tx(
                        "可在当前目标执行；仍需最终确认",
                        "Execution available for this target; final confirmation required",
                    )
                } else if self.worker.is_some() {
                    tx(
                        "任务运行中，禁止并发执行",
                        "A task is running; concurrent execution is disabled",
                    )
                } else {
                    tx(
                        "方案预览 / 导出；本机执行不可用",
                        "Plan preview / export; execution unavailable",
                    )
                },
            );
            property(
                ui,
                "WinGet",
                if self.winget.available {
                    tx(
                        "已识别微软应用安装程序执行别名",
                        "Microsoft App Installer alias identified",
                    )
                } else {
                    tx(
                        "当前不可用；查看软件部署页的检测详情",
                        "Unavailable; see capability details on the Software page",
                    )
                },
            );
            egui::CollapsingHeader::new(tx(
                "原始主机与能力诊断",
                "Raw host and capability diagnostics",
            ))
            .id_salt("status-host-raw")
            .show(ui, |ui| {
                ui.label(&self.host.explanation);
                ui.label(&self.winget.detail);
            });
        });
        settings_section(ui, tx("目录与选择", "Catalogs and selection"), |ui| {
            match &self.catalog {
                Ok(catalog) => {
                    property(
                        ui,
                        tx("系统设置目录", "Settings catalog"),
                        &format!(
                            "{} · {} {}",
                            catalog.revision,
                            catalog.actions.len(),
                            tx("项设置", "settings")
                        ),
                    );
                    property(
                        ui,
                        tx("配置组合", "Presets"),
                        &catalog.presets.len().to_string(),
                    );
                }
                Err(error) => {
                    ui.colored_label(
                        palette().danger,
                        tx("系统设置目录校验失败", "Settings catalog validation failed"),
                    );
                    ui.label(error);
                }
            }
            match &self.deployment_catalog {
                Ok(catalog) => property(
                    ui,
                    tx("部署目录", "Deployment catalog"),
                    &format!(
                        "{} · {} {}",
                        catalog.revision,
                        catalog.tasks.len(),
                        tx("项任务", "tasks")
                    ),
                ),
                Err(error) => {
                    ui.colored_label(
                        palette().danger,
                        tx("部署目录校验失败", "Deployment catalog validation failed"),
                    );
                    ui.label(error);
                }
            }
            match &self.power.catalog {
                Ok(catalog) => property(
                    ui,
                    tx("电源参数目录", "Power catalog"),
                    &lf!(
                        "{} 个 AC 选项 / 5 组",
                        "{} AC choices / 5 groups",
                        catalog.len()
                    ),
                ),
                Err(error) => {
                    ui.colored_label(
                        palette().danger,
                        tx("电源目录校验失败", "Power catalog validation failed"),
                    );
                    ui.label(error);
                }
            }
            match crate::catalog_locale::validate() {
                Ok(()) => property(
                    ui,
                    tx("内容翻译", "Content translations"),
                    tx("嵌入目录校验通过", "Embedded catalog validated"),
                ),
                Err(error) => {
                    ui.colored_label(
                        palette().danger,
                        tx(
                            "内容翻译不可用，使用原始目录内容",
                            "Content translations unavailable; using original catalog text",
                        ),
                    );
                    ui.label(format!("{error:#}"));
                }
            }
            property(
                ui,
                tx("已选择", "Selected"),
                &format!(
                    "{} {} / {} {} / {} {}",
                    tx("设置", "Settings"),
                    self.selected.len(),
                    tx("电源", "Power"),
                    self.power_selected.len(),
                    tx("其他", "Other"),
                    self.deployment_selected.len()
                ),
            );
        });
        settings_section(
            ui,
            tx("当前任务与最近结果", "Current task and last result"),
            |ui| {
                if let Some(worker) = &self.worker {
                    ui.label(t(worker.label));
                    if let Some(event) = &self.last_event {
                        ui.label(event);
                    }
                    ui.label(RichText::new(tx("进度来自实际事件。取消需等待当前操作，系统服务或安装器子进程可能继续。","Progress comes from actual events. Cancellation waits for the current operation; servicing or installer subprocesses may continue.")).size(12.0).color(palette().muted));
                    if ui
                        .add_enabled(
                            !worker.cancel.load(Ordering::Relaxed),
                            egui::Button::new(t("取消任务")),
                        )
                        .clicked()
                    {
                        worker.cancel.store(true, Ordering::Relaxed);
                    }
                } else {
                    ui.label(tx("当前没有运行任务", "No task running"));
                }
                if let Some(report) = &self.last_operation {
                    ui.separator();
                    ui.label(RichText::new(&report.text).color(if report.error {
                        palette().danger
                    } else {
                        palette().ink
                    }));
                } else {
                    ui.label(
                        RichText::new(tx(
                            "本次会话尚无操作报告。",
                            "No operation report in this session.",
                        ))
                        .color(palette().muted),
                    );
                }
            },
        );
        self.history_view(ui);
    }

    pub(super) fn history_view(&self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        ui.label(
            RichText::new(tx("部署执行历史", "Deployment execution history"))
                .size(18.0)
                .strong(),
        );
        ui.label(RichText::new(tx("历史记录证明曾执行哪些操作；它不代表当前安装状态，也不提供通用回滚。","History records past operations. It does not establish current installation state or provide general rollback.")).size(12.0).color(palette().muted));
        match &self.history {
            Err(error) => {
                ui.colored_label(
                    palette().danger,
                    tx("无法读取部署历史", "Unable to read deployment history"),
                );
                ui.label(error);
            }
            Ok(records) if records.is_empty() => {
                ui.label(
                    RichText::new(tx(
                        "尚无本地部署执行历史。",
                        "No local deployment execution history.",
                    ))
                    .color(palette().muted),
                );
            }
            Ok(records) => {
                for record in records {
                    egui::CollapsingHeader::new(format!(
                        "{} · {} · {} / {}",
                        record.started_at,
                        history_status(&record.status),
                        record.completed,
                        record.failed
                    ))
                    .id_salt(("history", &record.path))
                    .show(ui, |ui| {
                        property(ui, tx("目标", "Target"), record.target.label());
                        property(
                            ui,
                            tx("目录版本", "Catalog revision"),
                            &record.catalog_revision,
                        );
                        ui.label(&record.summary);
                        if record.cancelled {
                            ui.label(tx(
                                "操作已取消；已完成的变更仍然保留。",
                                "Operation cancelled; completed changes remain.",
                            ));
                        }
                        if record.restart_required {
                            ui.label(tx(
                                "执行报告要求重启。",
                                "The execution report requires a restart.",
                            ));
                        }
                        ui.label(
                            RichText::new(record.path.display().to_string())
                                .size(12.0)
                                .color(palette().muted),
                        );
                        for task in &record.tasks {
                            let title = crate::catalog_locale::text(
                                i18n::language(),
                                "tasks",
                                &task.id,
                                "title",
                                &task.title,
                            );
                            egui::CollapsingHeader::new(format!(
                                "{} · {}",
                                title,
                                history_status(&task.status)
                            ))
                            .id_salt(("history-task", &record.path, &task.id))
                            .show(ui, |ui| {
                                ui.label(format!("{} / {}", task.id, task.kind));
                                for evidence in &task.evidence {
                                    ui.label(
                                        RichText::new(format!(
                                            "{}{}",
                                            evidence.event,
                                            evidence
                                                .state
                                                .as_ref()
                                                .map(|state| format!(" · {state}"))
                                                .unwrap_or_default()
                                        ))
                                        .strong()
                                        .size(12.0),
                                    );
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&evidence.detail).monospace().size(12.0),
                                        )
                                        .wrap(),
                                    );
                                }
                            });
                        }
                    });
                }
            }
        }
    }

    pub(super) fn help_page(&self, ui: &mut egui::Ui) {
        ui.heading(tx("帮助", "Help"));
        settings_section(
            ui,
            tx(
                "开发环境备份与配置",
                "Developer environment backup and configuration",
            ),
            |ui| {
                ui.label(tx("在环境备份页明确采集当前 Mac 或 Windows，再保存本地清单。完整、未找到与失败分别记录；清单不包含项目、凭据、配置文件或虚拟环境。", "Explicitly capture this Mac or Windows machine on the backup page, then save a local inventory. Complete, unavailable and failed are distinct. Projects, credentials, settings and virtual environments are not included."));
                ui.label(tx("Mac 环境配置从基础工具或备份中逐项选择 Homebrew formula/cask，审核并导出 Brewfile，或在已准备 Homebrew 的 Mac 上确认配置。依赖可能改变，取消不保证安装器已停止，没有自动回滚。", "Mac configuration selects Homebrew formulae/casks from starter tools or a backup. Review and export a Brewfile, or confirm local configuration after preparing Homebrew. Dependencies can change, installers may continue after cancellation, and no automatic rollback is available."));
                ui.label(tx("重部署支持 Homebrew 补齐、滚动与 HEAD，HEAD 需要独立渠道回查；Rust 可选择 stable、beta、nightly、固定版本或按日 nightly，保存/载入请求及导出原生 rust-toolchain.toml 和 mise.toml。Rust 需标准位置现有 rustup 与已有官方默认工具链，执行后核对实际编译器版本和提交；mise 目前只导出清单。", "Redeployment supports missing-only, rolling and HEAD Homebrew policies with independent HEAD readback. Rust supports stable, beta, nightly, exact releases and dated nightly, with saved requests and native rust-toolchain.toml/mise.toml export. Rust needs standard rustup with an existing official default; results verify compiler release and commit. mise currently exports manifests only."));
                ui.label(tx("Windows 备份在迁移页按可信身份映射；筛选和分页保留勾选。Windows 配置使用原有方案和提供程序，WinGet 导出仅含所选 WinGet 软件；其他工具继续使用应用档案。", "Windows backups map trusted identities on the review page; filtering and paging preserve checks. Windows configuration uses the existing workspace and providers. WinGet export contains only selected WinGet applications; other tools remain in app profiles."));
            },
        );
        settings_section(ui, tx("开始使用", "Get started"), |ui| {
            for (zh, en) in [
                (
                    "1. 选择目标 Windows 版本；预览目标不代表已检测到那台设备。",
                    "1. Choose a Windows target. A preview target does not mean that device has been detected.",
                ),
                (
                    "2. 逐项勾选系统设置、电源参数、存储、软件、功能或维护任务，也可先检查一个配置组合。",
                    "2. Select settings, power values, storage, software, features or maintenance tasks individually, or review a preset first.",
                ),
                (
                    "3. 检查所选项目状态，进入变更方案核对确切注册表值、AC 参数、磁盘操作范围、包 ID、权限和限制。",
                    "3. Inspect selected items, then review exact registry values, AC parameters, disk operation scope, package IDs, permissions and limits in the plan.",
                ),
                (
                    "4. 可导出完整方案或保存档案；在受支持的 Windows 本机上分别确认系统设置、电源与部署执行。",
                    "4. Export a complete plan or save a profile. On supported Windows, confirm settings, power and deployment separately.",
                ),
            ] {
                ui.label(tx(zh, en));
            }
        });
        settings_section(ui, tx("组合与迁移", "Recipes and migration"), |ui| {
            ui.label(tx("方案工作区可预览日常、Rust、Python工具、Python数据与交互、Web 和 WSL 组合，再明确加入当前选择。组合会一起列出系统设置、软件、开发工具与扩展，仍可逐项修改。", "Review everyday, Rust, Python tools, Python data, Web and WSL recipes in the workspace, then explicitly add them to your selection. Each recipe combines settings, software, developer tools and extensions; every item remains editable."));
            ui.label(tx("迁移与映射先分析旧 SetupProfile 或 Mac 清单，再勾选要合并的项目。分析不会自动选择或执行；未支持内容会保留说明。更换路径或目标后须重新分析。", "Migration first analyzes a legacy SetupProfile or Mac inventory, then lets you select items to merge. Analysis never selects or executes automatically; unsupported content stays in the report. Reanalyze after changing the path or target."));
            ui.label(tx("开发环境页可按 Cargo、uv、Pip、NPM 和 VS Code 筛选。Pip 核对标准 CPython、用户目录和包身份；uv 需要已有 Python，NPM 使用用户前缀并禁用生命周期脚本。", "Filter developer tasks by Cargo, uv, Pip, NPM and VS Code. Pip validates standard CPython, user directories and package identity; uv needs an existing Python interpreter, while NPM uses the user prefix with lifecycle scripts disabled."));
        });
        settings_section(
            ui,
            tx("恢复与历史有什么区别", "Recovery versus history"),
            |ui| {
                ui.label(tx("系统设置先记录原始注册表值，再写入并验证。恢复会检查后来发生的外部修改，遇到冲突停止覆盖。恢复不是完整系统映像，新增空键可能保留。","Settings journal original registry values before verified writes. Recovery checks later external changes and stops on conflicts. It is not a system image; newly created empty keys can remain."));
                ui.label(tx("电源页只修改执行时活动方案的插电 AC 参数，不修改电池 DC，不创建卓越性能方案，也不保证睡眠或休眠硬件能力。电源原值与恢复记录独立保存；恢复会检查活动方案和参数冲突。", "Power changes only the active scheme's plugged-in AC values at execution time. Battery DC values are untouched, no Ultimate Performance scheme is created, and sleep or hibernation support is not guaranteed. Power originals and recovery journals are separate; recovery checks the active scheme and parameter conflicts."));
                ui.label(tx("软件安装、功能启用及系统维护只有实际执行历史，没有通用回滚。请通过应用卸载器、Windows 功能管理或相应系统工具处理。","Software, optional features and maintenance have execution history, without general rollback. Use the application's uninstaller, Windows feature management or the appropriate system tool."));
            },
        );
        settings_section(
            ui,
            tx("为什么 macOS 无法应用", "Why macOS cannot apply a plan"),
            |ui| {
                ui.label(tx("本程序不会把 macOS 或 Linux 当作 Windows 注册表。这里可以整理选择、检查适用性、载入与导出档案；实际应用和恢复只允许已识别的受支持 Windows 版本。","macOS and Linux are not treated as Windows registries. You can prepare selections, review applicability and load or export profiles here. Applying and restoring require an identified, supported Windows host."));
            },
        );
        settings_section(ui, tx("执行前提", "Execution prerequisites"), |ui| {
            ui.label(tx("软件部署需要微软应用安装程序的 WinGet 执行别名和可用软件源。程序不会下载引导脚本，也不会自动接受外部安装器许可。安装器可能显示交互窗口。","Software deployment requires Microsoft's App Installer WinGet alias and working sources. No bootstrap scripts are downloaded and external installer licenses are not automatically accepted. Installers may show interactive windows."));
            ui.hyperlink_to(
                tx("微软 WinGet 文档", "Microsoft WinGet documentation"),
                "https://learn.microsoft.com/windows/package-manager/winget/",
            );
            ui.label(tx("可选功能和系统维护可能需要以管理员身份运行、网络、匹配的安装源以及重启。LTSC 映像、硬件虚拟化和组织策略可能限制组件。","Optional features and maintenance may require administrator rights, network access, matching sources and a restart. LTSC images, hardware virtualization and organization policy may restrict components."));
        });
        settings_section(ui, tx("遇到问题", "Troubleshooting"), |ui| {
            ui.label(tx("先查看状态页、最近结果与恢复 / 部署日志。检查目标是否与本机一致、是否具有管理员权限、WinGet 是否可用，以及重启是否待完成。","Start with Status, the last result and recovery or deployment logs. Check the actual host target, administrator rights, WinGet availability and any pending restart."));
            ui.label(tx("取消不会撤销已完成的操作；先等当前任务返回，再检查实际报告。对于失败、部分完成或损坏日志，保留原始诊断后再决定下一步。","Cancellation does not undo completed operations. Wait for the current task, then review the actual report. Preserve raw diagnostics for failures, partial completion or malformed logs before deciding what to do next."));
            ui.hyperlink_to(
                tx("项目问题与支持", "Project issues and support"),
                concat!(env!("CARGO_PKG_REPOSITORY"), "/issues"),
            );
        });
    }

    pub(super) fn about_page(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            logo(ui, 34.0);
            ui.heading("LTSC Studio");
        });
        property(ui, tx("版本", "Version"), env!("CARGO_PKG_VERSION"));
        property(ui, tx("许可证", "License"), env!("CARGO_PKG_LICENSE"));
        ui.label(tx("为 Windows LTSC 提供可检查、可导出的显式工作方案。每项修改、安装与维护均由用户选择。","Explicit, reviewable and exportable work plans for Windows LTSC. Every setting, installation and maintenance task is chosen by the user."));
        ui.hyperlink_to(
            tx("项目仓库", "Project repository"),
            env!("CARGO_PKG_REPOSITORY"),
        );
        ui.hyperlink_to(
            tx("MIT 许可证", "MIT license"),
            concat!(env!("CARGO_PKG_REPOSITORY"), "/blob/main/LICENSE"),
        );
        ui.add_space(14.0);
        ui.label(
            RichText::new(tx("项目来源与致谢", "Sources and acknowledgements"))
                .size(18.0)
                .strong(),
        );
        if let Ok(catalog) = &self.catalog {
            for source in &catalog.sources {
                ui.hyperlink_to(&source.name, &source.url);
                ui.label(
                    RichText::new(format!("{} / {}", source.revision, t(&source.license)))
                        .size(12.0)
                        .color(palette().muted),
                );
            }
        }
        ui.label(tx("来源映射与第三方许可证范围请见项目来源页。软件包保留各自许可证与安装条款。","See Sources for provenance mappings and third-party license scope. Software packages retain their own licenses and installation terms."));
    }
}

fn settings_section(ui: &mut egui::Ui, title: &str, content: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(10.0);
    egui::Frame::new()
        .fill(palette().surface)
        .stroke(Stroke::new(1.0, palette().line))
        .corner_radius(8)
        .inner_margin(16.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).size(16.0).strong());
            ui.add_space(5.0);
            content(ui);
        });
}
fn property(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(label).color(palette().muted));
        ui.label(value);
    });
}
pub(super) fn history_status(status: &str) -> &str {
    match status {
        "completed" => tx("已完成", "Completed"),
        "failed" => tx("失败", "Failed"),
        "cancelled" => tx("已取消", "Cancelled"),
        "interrupted" => tx("中断 / 未完成", "Interrupted / incomplete"),
        "not_started" => tx("未开始", "Not started"),
        other => other,
    }
}
