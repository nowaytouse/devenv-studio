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
                for (value, zh, en) in [
                    (Appearance::System, "跟随系统", "System"),
                    (Appearance::Light, "浅色", "Light"),
                    (Appearance::Dark, "深色", "Dark"),
                ] {
                    ui.vertical(|ui| {
                        ui.set_width(136.0);
                        appearance_preview(ui, value, self.preferences.accent);
                        if ui
                            .add_sized(
                                [136.0, 32.0],
                                egui::Button::new(tx(zh, en))
                                    .selected(self.preferences.appearance == value),
                            )
                            .clicked()
                        {
                            self.preferences.appearance = value;
                        }
                    });
                }
            });
            ui.add_space(8.0);
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
                    let label = tx(zh, en);
                    if ui
                        .add(
                            egui::Button::new((
                                RichText::new("●").color(swatch.accent),
                                RichText::new(label).color(palette().ink),
                            ))
                            .selected(selected)
                            .fill(if selected {
                                swatch.wash
                            } else {
                                palette().canvas
                            })
                            .stroke(Stroke::new(
                                1.0,
                                if selected {
                                    swatch.accent
                                } else {
                                    palette().line
                                },
                            )),
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
                RichText::new(if self.preferences_error.is_some() {
                    tx("设置文件不可用", "Preferences unavailable")
                } else if self.preferences == self.saved_preferences {
                    tx("已保存", "Saved")
                } else {
                    tx("有未保存的界面设置", "Unsaved interface settings")
                })
                .size(12.0)
                .color(if self.preferences_error.is_some() {
                    palette().danger
                } else {
                    palette().muted
                }),
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
        settings_section(
            ui,
            tx("本机与 Windows 方案", "This device and Windows plans"),
            |ui| {
                ui.label(tx(
                "本节的执行能力仅针对 Windows 系统方案；开发工具的可用条件请在环境配置页分别核对。",
                "Execution capability in this section applies to Windows system plans. Check each developer tool panel on Environment for its requirements.",
            ));
                property(ui, tx("运行平台", "Operating system"), std::env::consts::OS);
                property(
                    ui,
                    tx("Windows 主机检测", "Windows host detection"),
                    &self.host.name,
                );
                property(
                    ui,
                    tx("Windows 方案目标", "Windows plan target"),
                    self.target.label(),
                );
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
                    tx("Windows 方案执行", "Windows plan execution"),
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
                            "仅预览或导出 Windows 方案",
                            "Windows plan preview or export only",
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
            },
        );
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

    pub(super) fn help_page(&mut self, ui: &mut egui::Ui) {
        let origin = self.help_origin;
        let (title, steps) = match origin {
            Some(Page::Backup) => (
                tx("备份工具清单", "Back up your tool inventory"),
                [
                    tx(
                        "采集这台电脑的工具清单，或载入已有备份；先检查哪些来源完整、部分完成或未找到。",
                        "Capture this machine's tool inventory or load a backup; check which sources are complete, partial or unavailable.",
                    ),
                    tx(
                        "将清单保存到本地；它记录工具与版本，不包含项目、工具配置、凭据或完整系统。",
                        "Save the inventory locally. It records tools and versions, excluding projects, tool settings, credentials and a full system image.",
                    ),
                    tx(
                        "按页面的平台提示继续：配置 macOS 工具，或审核 Windows 映射；仍需逐项选择并单独确认执行。",
                        "Follow the platform guidance to configure macOS tools or review Windows mappings; select items and confirm execution separately.",
                    ),
                ],
            ),
            Some(Page::Environment) => (
                tx("配置开发环境", "Set up your developer environment"),
                [
                    tx(
                        "先在环境备份页采集或载入清单，并保存到本地。",
                        "Capture or load an inventory on Environment backup, then save it locally.",
                    ),
                    tx(
                        if cfg!(target_os = "macos") {
                            "按 Rust、Python、Web 用途准备基础工具，或使用通用基础；查看所选项目，再按需展开其他工具面板。"
                        } else {
                            "按用途选择 Windows 方案，查看包含的工具与设置；其他运行时和配置可按需展开。"
                        },
                        if cfg!(target_os = "macos") {
                            "Prepare Rust, Python or Web prerequisites, or general starter tools. Review the selection, then expand other tool panels as needed."
                        } else {
                            "Choose a Windows recipe and review its tools and settings; expand other runtimes and configuration panels as needed."
                        },
                    ),
                    tx(
                        "Mac 在对应工具面板勾选确认后配置；Windows 系统方案到变更方案审核。",
                        "On Mac, confirm in the relevant tool panel before configuring. Review Windows system plans on Change plan.",
                    ),
                ],
            ),
            Some(Page::Recipes) => (
                tx(
                    "按用途准备 Windows 方案",
                    "Prepare a Windows plan by purpose",
                ),
                [
                    tx(
                        "先选择目标 Windows 版本，再选择 Rust、Python、Web 等用途。",
                        "Choose the target Windows version, then a purpose such as Rust, Python or Web.",
                    ),
                    tx(
                        "查看包含的设置和软件，点击加入当前方案；之后仍可逐项修改。",
                        "Review the included settings and software, then add them to the workspace; every item remains editable.",
                    ),
                    tx(
                        "加入后打开变更方案。仅在受支持的 Windows 本机分别确认执行；其他系统可预览。",
                        "Adding opens Change plan. Execute only after separate confirmations on a supported Windows host; other systems can preview.",
                    ),
                ],
            ),
            Some(Page::Network) => (
                tx("设置本应用使用的网络", "Set the network used by this app"),
                [
                    tx(
                        "选择沿用当前网络、直连或手动代理；只有需要自定义软件源时才展开高级设置。",
                        "Choose inherited settings, a direct connection or a manual proxy; open advanced sources only when you need custom package sources.",
                    ),
                    tx(
                        "检查草稿摘要和适用范围，确认后启用；完整前后对比可展开查看，后续适用任务会固定使用该方案。",
                        "Check the draft summary and scope, then confirm and activate. Expand the full comparison if needed; supported later tasks pin that profile.",
                    ),
                    tx(
                        "原生配置只导出为新文件。此页不修改系统代理、DNS 或 VPN。",
                        "Native settings export to new files only. This page does not change system proxy, DNS or VPN.",
                    ),
                ],
            ),
            Some(Page::Migration) => (
                tx(
                    "从旧清单选择要迁移的项目",
                    "Select items from an older inventory",
                ),
                [
                    tx(
                        "选择应用备份、旧档案或 WinGet 导出 JSON，并核对目标 Windows 版本。",
                        "Choose an app backup, legacy profile or WinGet export JSON, then check the Windows target.",
                    ),
                    tx(
                        "点击分析并等待报告。若系统询问文件访问权限，请自行允许或拒绝；期间仍可查看帮助。分析不会自动勾选或安装。",
                        "Analyze and wait for the report. If the system asks for file access, allow or deny it yourself; help remains available meanwhile. Analysis selects and installs nothing automatically.",
                    ),
                    tx(
                        "勾选需要的映射后加入当前方案，再到变更方案检查并确认。",
                        "Select the mappings you need and add them to the workspace, then review and confirm on Change plan.",
                    ),
                ],
            ),
            Some(Page::Compatibility) => (
                tx("使用扩展部署", "Use extended deployment"),
                [
                    tx(
                        "先启用需要的领域，再按来源搜索并勾选软件、扩展或模型。",
                        "Enable the capabilities you need, then filter and select software, extensions or models.",
                    ),
                    tx(
                        "审核本页的实际请求；可以先保存请求，载入后仍需重新确认。",
                        "Review the actual request on this page; you can save it first, and loading it requires fresh confirmation.",
                    ),
                    tx(
                        "确认范围和限制后再执行。自定义安装可能留下部分变更，没有通用回滚。",
                        "Confirm the scope and limits before executing. Custom installations may leave partial changes and have no general rollback.",
                    ),
                ],
            ),
            Some(
                Page::Workbench
                | Page::Developer
                | Page::Software
                | Page::Features
                | Page::Maintenance,
            ) => (
                tx(
                    "选择 Windows 配置项目",
                    "Select Windows configuration items",
                ),
                [
                    tx(
                        "核对顶部的目标 Windows 版本；预览目标不等于已检测到那台电脑。",
                        "Check the Windows target at the top; a preview target does not identify a real machine.",
                    ),
                    tx(
                        "只勾选需要的项目，再打开变更方案查看操作、权限和限制。",
                        "Select only the items you need, then open Change plan to inspect actions, permissions and limits.",
                    ),
                    tx(
                        "在受支持的 Windows 本机分别确认执行。安装与维护不能通过设置恢复撤销。",
                        "Confirm each execution area on a supported Windows host. Settings recovery cannot undo installations or maintenance.",
                    ),
                ],
            ),
            Some(Page::Power) => (
                tx("调整插电时的电源设置", "Adjust plugged-in power settings"),
                [
                    tx(
                        "查看当前电源方案，只选择需要的插电 AC 参数。",
                        "Review the current power scheme and select only the plugged-in AC values you need.",
                    ),
                    tx(
                        "核对原值与拟用值，再确认应用；电池 DC 参数不会改变。",
                        "Review original and requested values, then confirm; battery DC values remain unchanged.",
                    ),
                    tx(
                        "需要恢复时使用独立电源恢复记录；方案改变或原值冲突会停止覆盖。",
                        "Use the separate power recovery record when needed; scheme changes or value conflicts stop overwrites.",
                    ),
                ],
            ),
            Some(Page::Storage) => (
                tx("检查或维护磁盘", "Inspect or maintain disks"),
                [
                    tx(
                        "先查看卷与操作范围，分清分析和优化。",
                        "Review volumes and operation scope first, and distinguish analysis from optimization.",
                    ),
                    tx(
                        "只选择需要的磁盘任务，检查权限和维护限制后确认执行。",
                        "Select the disk tasks you need and confirm only after reviewing permissions and maintenance limits.",
                    ),
                    tx(
                        "完成后查看真实执行记录。优化不是文件备份，也没有通用回滚。",
                        "Review the actual execution record afterwards. Optimization is not a file backup and has no general rollback.",
                    ),
                ],
            ),
            Some(Page::Plan) => (
                tx("确认将要执行的变更", "Confirm the requested changes"),
                [
                    tx(
                        "逐项核对实际操作、适用目标、权限和警告；移除不需要的项目。",
                        "Check exact actions, target applicability, permissions and warnings; remove unwanted items.",
                    ),
                    tx(
                        "可以先保存或导出方案。保存不表示已安装或已应用。",
                        "Save or export the plan first if useful. Saving does not install or apply anything.",
                    ),
                    tx(
                        "满足本机执行条件后，分别确认设置、电源或部署；完成后查看真实结果。",
                        "Once host prerequisites are met, confirm settings, power or deployment separately, then inspect actual results.",
                    ),
                ],
            ),
            Some(Page::Recovery) => (
                tx(
                    "检查记录并恢复支持的原值",
                    "Inspect records and restore supported originals",
                ),
                [
                    tx(
                        "选择对应的原值记录，核对来源、时间和操作范围。",
                        "Choose the original-value record and check its source, time and scope.",
                    ),
                    tx(
                        "先审核恢复计划；发生外部修改或记录校验失败时停止处理。",
                        "Review the recovery plan first; external changes or invalid records stop the operation.",
                    ),
                    tx(
                        "明确确认后只恢复该记录支持的设置；软件安装和系统维护另行处理。",
                        "Explicit confirmation restores only settings supported by that record; handle installations and maintenance separately.",
                    ),
                ],
            ),
            Some(Page::Settings) => (
                tx("调整界面", "Adjust the interface"),
                [
                    tx(
                        "选择语言、明暗主题、强调色和界面缩放，立即查看效果。",
                        "Choose language, light or dark theme, accent and scale to preview the result.",
                    ),
                    tx(
                        "点击保存后，下次启动继续使用；设置文件错误会在页面中说明。",
                        "Save to keep the choices for the next launch; settings-file errors are shown on the page.",
                    ),
                    tx(
                        "返回刚才页面继续工作；外观设置不会运行环境配置。",
                        "Return to your previous page to continue; appearance settings do not configure the environment.",
                    ),
                ],
            ),
            Some(Page::Status | Page::Sources | Page::About) => (
                tx("核对状态、结果与来源", "Check status, results and sources"),
                [
                    tx(
                        "查看本机识别结果和最近操作，分清成功、失败、取消与部分完成。",
                        "Inspect host detection and recent operations, distinguishing success, failure, cancellation and partial completion.",
                    ),
                    tx(
                        "遇到问题时保留真实错误与执行记录；项目来源用于核对功能和许可证边界。",
                        "Preserve actual errors and execution records when troubleshooting; sources document functionality and license boundaries.",
                    ),
                    tx(
                        "返回刚才页面处理相应问题；取消不会自动撤销已完成的变更。",
                        "Return to the relevant page to address the issue; cancellation does not undo completed changes.",
                    ),
                ],
            ),
            Some(Page::Help) | None => (
                tx("从这里开始", "Start here"),
                [
                    tx(
                        "打开环境备份，采集这台电脑或载入已有清单，然后保存。",
                        "Open Environment backup, capture this machine or load an inventory, then save it.",
                    ),
                    tx(
                        "在环境配置选择基础工具；Windows 也可按用途选择常见方案。",
                        "Select starter tools on Environment; for Windows you can also choose a recipe by purpose.",
                    ),
                    tx(
                        "先审核具体变更，再明确确认执行。每页右上角的 ? 帮助会解释当前操作。",
                        "Review exact changes before explicitly confirming execution. The ? Help button explains the current page.",
                    ),
                ],
            ),
        };
        let guide = surface_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(egui::Label::new(RichText::new(title).heading()).wrap());
            ui.add_space(8.0);
            for (index, step) in steps.into_iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!("{}.", index + 1))
                            .color(palette().accent)
                            .strong(),
                    );
                    ui.add(egui::Label::new(step).wrap());
                });
                ui.add_space(6.0);
            }
            if let Some(page) = origin {
                if primary(ui, tx("返回刚才页面", "Back to previous page"), true).clicked() {
                    self.navigate_to(page);
                }
            } else if primary(ui, tx("打开环境配置", "Open Environment"), true).clicked() {
                self.navigate_to(Page::Environment);
            }
        });
        if std::mem::take(&mut self.help_focus_requested) {
            guide.response.scroll_to_me(Some(egui::Align::Min));
        }
        ui.add_space(14.0);
        egui::CollapsingHeader::new(tx("完整使用手册", "Complete user guide"))
            .id_salt(("help-handbook", origin.is_some()))
            .default_open(origin.is_none())
            .show(ui, |ui| {
        ui.heading(tx("帮助", "Help"));
        ui.label(tx(
            "左侧导航分组可点击展开或收起；macOS 默认优先显示跨平台环境，Windows 配置和执行记录可随时展开。右上角 ? 帮助会先说明当前页面的操作，并可返回原页面。预选和加入方案都不会立即执行：Mac 在对应工具面板确认，Windows 系统方案在变更方案复核并分别确认。",
            "Click a left navigation heading to show or hide its pages. On macOS, cross-platform pages start open; Windows configuration and execution history can be opened when needed. The upper-right ? Help explains the current page and lets you return. Presets only prepare choices: confirm Mac tools in their panels and review Windows system plans on Change plan before separate execution confirmations.",
        ));
        settings_section(
            ui,
            tx(
                "开发环境备份与配置",
                "Developer environment backup and configuration",
            ),
            |ui| {
                ui.label(tx("在环境备份页明确采集当前 Mac 或 Windows，再保存本地清单。完整、未找到与失败分别记录；清单不包含项目、凭据、配置文件或虚拟环境。", "Explicitly capture this Mac or Windows machine on the backup page, then save a local inventory. Complete, unavailable and failed are distinct. Projects, credentials, settings and virtual environments are not included."));
                ui.label(tx("Mac 环境配置从基础工具或备份中逐项选择 Homebrew formula/cask，审核并导出 Brewfile，或在已准备 Homebrew 的 Mac 上确认配置。依赖可能改变，取消不保证安装器已停止，没有自动回滚。", "Mac configuration selects Homebrew formulae/casks from starter tools or a backup. Review and export a Brewfile, or confirm local configuration after preparing Homebrew. Dependencies can change, installers may continue after cancellation, and no automatic rollback is available."));
                ui.label(tx("重部署支持 Homebrew 补齐、滚动与 HEAD，HEAD 需要独立渠道回查；Rust 可选择 stable、beta、nightly、固定版本或按日 nightly，保存/载入请求及导出原生 rust-toolchain.toml 和 mise.toml。Rust 需标准位置现有 rustup 与已有官方默认工具链，执行后核对实际编译器版本和提交；mise 另有已安装运行时采集、逐项选择、版本锁定与重部署；需先准备可用的 mise，安装仍须明确确认。", "Redeployment supports missing-only, rolling and HEAD Homebrew policies with independent HEAD readback. Rust supports stable, beta, nightly, exact releases and dated nightly, with saved requests and native rust-toolchain.toml/mise.toml export. Rust needs standard rustup with an existing official default; results verify compiler release and commit. mise also inventories installed runtimes and supports selected, locked redeployment; it requires an existing mise installation and explicit confirmation."));
                ui.label(tx("Windows 备份在迁移页按可信身份映射；筛选和分页保留勾选。Windows 配置使用原有方案和提供程序，WinGet 导出仅含所选 WinGet 软件；其他工具继续使用应用档案。", "Windows backups map trusted identities on the review page; filtering and paging preserve checks. Windows configuration uses the existing workspace and providers. WinGet export contains only selected WinGet applications; other tools remain in app profiles."));
            },
        );
        settings_section(
            ui,
            tx("Topgrade 环境迁移", "Topgrade environment migration"),
            |ui| {
                ui.label(tx("在环境配置页采集标准 Topgrade 主配置与分段配置；自定义脚本和依赖文件通过附件绝对路径逐项添加。保存为本地迁移包后，可在另一台设备载入、查看原文、选择文件并确认还原到全新目录。", "On the Environment page, capture standard Topgrade main and drop-in configs. Explicitly attach custom scripts and dependency files by absolute path. Save a local package, then load it on another device, review original text, select files and confirm extraction into a new directory."));
                ui.label(tx("还原不会运行任何命令或覆盖当前配置；Unix 上不恢复脚本执行位。需要另行安装 Topgrade 和外部工具，并审核主目录路径、include 及自定义命令。Topgrade 的 --config 参数会跳过自动分段配置发现，不应只指向导出的主文件就视作完整恢复。", "Extraction never runs commands or overwrites current settings; Unix executable bits are not restored. Install Topgrade and external tools separately, and review home paths, includes and custom commands. Topgrade --config skips automatic drop-in discovery; pointing it at the extracted main file alone does not restore the complete setup."));
            },
        );
        settings_section(
            ui,
            tx("编辑器设置档案", "Editor settings profile"),
            |ui| {
                ui.label(tx(
                    "选择 VS Code 稳定版、Insiders 或 Cursor，再读取当前电脑的默认用户设置；也可先载入一份档案。逐项勾选要迁移的设置，检查本机差异并确认应用。切换编辑器会清空旧的本机对比和勾选，但保留档案以便迁移。",
                    "Choose VS Code Stable, Insiders or Cursor and read its default user settings, or load a profile. Select settings one by one, review local differences and confirm. Switching editors clears the old comparison and checks while keeping the portable profile.",
                ));
                ui.label(tx(
                    "只处理 21 项通用白名单，保留注释和未选值；不复制扩展、工作区、账号、密钥或命名配置档。应用前关闭所选编辑器；写入不提供自动回滚。",
                    "Only 21 common allowlisted settings are handled; comments and unchecked values remain. Extensions, workspaces, accounts, secrets and named profiles are excluded. Close the selected editor before applying; automatic rollback is unavailable.",
                ));
            },
        );
        settings_section(
            ui,
            tx(
                "扩展部署与可保存请求",
                "Extended deployment and saved requests",
            ),
            |ui| {
                ui.label(tx("在 Windows 配置的扩展部署页，先启用需要的领域，再按来源搜索和勾选软件、扩展或模型。所有项目初始未选；关闭领域会保留选择，但只执行启用领域中的项目。Store 应用也需明确选择。", "In Windows Extended deployment, enable the capabilities you need, then filter and select packages, extensions or models. Items start unchecked. Disabling a capability preserves selections but only enabled capabilities run. Store products also need explicit selection."));
                ui.label(tx("保存请求记录候选配置与选择，不包含执行确认。载入后重新审核；错误不会覆盖当前草稿，已有文件不会被保存操作覆盖。编辑高级 Profile 会清空项目选择和确认。旧仓库已退役，当前流程无需读取其远端清单。", "Saved requests contain candidate configuration and selections, never execution approval. Review again after loading. Failed loads preserve the open draft, and saves never overwrite an existing file. Editing the advanced Profile clears item selections and approval. The retired repository's remote inventory is no longer used."));
            },
        );
        settings_section(
            ui,
            tx("开发网络环境", "Developer network environment"),
            |ui| {
                ui.label(tx("网络页保存本应用部署使用的继承、直连或明确 HTTP 代理与 npm/Python/Cargo 软件源。先审核草稿并启用，后续适用任务才会使用；每次任务固定一份配置。原值可从历史 before.json 重新载入并审核恢复。", "The network page saves inherited, direct or explicit HTTP proxy and npm/Python/Cargo package sources for this app. Review and activate the draft before supported tasks use it; each task pins one profile. Restore prior values by loading and reviewing before.json from history."));
                ui.label(tx("可导出 Git/Cargo/npm/Pip/uv 原生配置新文件；不覆盖全局文件，不修改系统代理、DNS、VPN、WinGet、DISM、VS Code 或 SSH，也不自动测试连接。自定义源需自行审核服务方。", "Export new native Git/Cargo/npm/Pip/uv configuration files. Global files, system proxy, DNS, VPN, WinGet, DISM, VS Code and SSH are not changed, and connectivity is not automatically tested. Review custom source operators."));
            },
        );
        settings_section(ui, tx("Windows 方案：开始使用", "Windows plans: get started"), |ui| {
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
            ui.label(tx("迁移与映射支持应用备份、旧 SetupProfile/Mac 清单及 WinGet 2.0 导出 JSON，分析后再勾选要合并的项目。WinGet 只建议官方社区源中能唯一对应可信目录的软件；其他来源、附加安装要求及未知软件保留说明，版本不作为安装锁定。分析不会自动选择或执行，更换路径或目标后须重新分析。", "Migration accepts app backups, legacy SetupProfile/Mac inventories and WinGet 2.0 export JSON. Select items after analysis. WinGet suggests only community-source packages with unique trusted catalog identities; other sources, extra installation requirements and unknown packages stay visible, and versions are not installation pins. Analysis never selects or executes automatically. Reanalyze after changing the path or target."));
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
            tx("为什么无法应用 Windows 方案", "Why Windows plans cannot be applied here"),
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
            });
    }

    pub(super) fn about_page(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            logo(ui, 34.0);
            ui.heading("DevEnv Studio");
        });
        property(
            ui,
            tx("开发基线", "Development baseline"),
            env!("CARGO_PKG_VERSION"),
        );
        ui.label(tx("当前为开发构建；改进先累计到 Unreleased，版本号在发布检查点确定。", "This is a development build. Changes accumulate in Unreleased; version numbers are chosen at release checkpoints."));
        property(ui, tx("许可证", "License"), env!("CARGO_PKG_LICENSE"));
        ui.label(tx("面向 macOS 与 Windows 的开发环境备份、配置、网络方案与重部署。Windows LTSC 系统配置是其中的独立区域；每项变更由用户选择。", "Development environment backup, configuration, networking and redeployment for macOS and Windows. Windows LTSC system configuration is a separate area; every change is chosen by the user."));
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
                    RichText::new(format!(
                        "{} / {}",
                        source.revision,
                        if source.name == "LTSC_Tools_Backup" {
                            tx(
                                "所有者授权的 Rust 后端整合",
                                "Owner-authorized Rust engine integration",
                            )
                        } else {
                            t(&source.license)
                        }
                    ))
                    .size(12.0)
                    .color(palette().muted),
                );
            }
        }
        ui.label(tx("来源映射与第三方许可证范围请见项目来源页。软件包保留各自许可证与安装条款。","See Sources for provenance mappings and third-party license scope. Software packages retain their own licenses and installation terms."));
    }
}

fn appearance_preview(ui: &mut egui::Ui, appearance: Appearance, accent: Accent) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(136.0, 58.0), egui::Sense::hover());
    let colors = theme::Palette::for_theme(matches!(appearance, Appearance::Dark), accent);
    let painter = ui.painter();
    painter.rect_filled(rect, 7, colors.canvas);
    let sidebar =
        egui::Rect::from_min_size(rect.min + egui::vec2(4.0, 4.0), egui::vec2(29.0, 50.0));
    painter.rect_filled(sidebar, 4, colors.sidebar);
    painter.rect_filled(
        egui::Rect::from_min_size(sidebar.min + egui::vec2(4.0, 10.0), egui::vec2(21.0, 5.0)),
        2,
        colors.accent,
    );
    for (offset, width) in [(10.0, 65.0), (22.0, 88.0), (36.0, 88.0)] {
        painter.rect_filled(
            egui::Rect::from_min_size(rect.min + egui::vec2(40.0, offset), egui::vec2(width, 7.0)),
            3,
            colors.surface,
        );
    }
    if matches!(appearance, Appearance::System) {
        let dark = theme::Palette::for_theme(true, accent);
        painter.rect_filled(
            egui::Rect::from_min_max(rect.center_top(), rect.right_bottom()),
            7,
            dark.surface,
        );
        painter.line_segment(
            [
                rect.center_top() + egui::vec2(8.0, 18.0),
                rect.center_top() + egui::vec2(48.0, 18.0),
            ],
            Stroke::new(3.0, dark.accent),
        );
    }
    painter.rect_stroke(
        rect,
        7,
        Stroke::new(1.0, palette().line),
        egui::StrokeKind::Inside,
    );
}

fn settings_section(ui: &mut egui::Ui, title: &str, content: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(10.0);
    surface_frame().show(ui, |ui| {
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
