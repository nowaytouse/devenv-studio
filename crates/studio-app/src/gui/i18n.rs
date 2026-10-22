//! Application chrome translations. Canonical catalog values and diagnostics stay untouched.
use crate::prefs::Language;
use std::cell::Cell;

thread_local! { static ENGLISH: Cell<bool> = const { Cell::new(false) }; }
pub(super) fn set_language(language: Language) {
    ENGLISH.with(|value| value.set(matches!(language, Language::EnUs)));
}
pub(super) fn english() -> bool {
    ENGLISH.with(Cell::get)
}
pub(super) fn language() -> Language {
    if english() {
        Language::EnUs
    } else {
        Language::ZhCn
    }
}
pub(super) fn tx<'a>(zh: &'a str, en: &'a str) -> &'a str {
    if english() { en } else { zh }
}

pub(super) fn t(source: &str) -> &str {
    if !english() {
        return source;
    }
    match source {
        "设计参考；未复制源码" => "Design reference; no source code copied",
        "拆分其配置清单中的独立注册表设置，保留来源；不运行上游捆绑脚本。" => {
            "Individual registry settings adapted with provenance retained. Bundled upstream scripts are not executed."
        }
        "参考隐私、资源管理器及 LTSC 版本适配；仅整合已审查的单项设置。" => {
            "Privacy, File Explorer and LTSC compatibility references. Only reviewed individual settings are integrated."
        }
        "整合可独立选择的 Explorer 与用户体验设置；不继承整个精简预设。" => {
            "Individually selectable File Explorer and experience settings, without importing a full debloat preset."
        }
        "沿用用户逐项选择、计划预览与先记录原值的设计经验。旧项目独立保留。" => {
            "Design experience for explicit choices, plan review and original-value journals. The legacy project remains separate."
        }
        "状态" => "Status",
        "设置" => "Settings",
        "帮助" => "Help",
        "关于" => "About",
        "导出变更方案" => "Export plan",
        "保存配置档案" => "Save profile",
        "载入配置档案" => "Load profile",
        "{} / {} 项已完成 · {}：{}" => "{} / {} completed · {}: {}",
        "后台任务异常结束，未收到执行报告。请查看恢复记录后再继续。" => {
            "The worker ended without a report. Check recovery records before continuing."
        }
        "后台任务发生异常；请检查已有恢复记录。" => {
            "The worker failed unexpectedly. Check existing recovery records."
        }
        "{}{}\n已变更 {} 项；原值已符合 {} 项。恢复日志：{}" => {
            "{}{}\nChanged: {}; already matched: {}. Recovery journal: {}"
        }
        "任务已取消。" => "Task cancelled. ",
        "{}\n完成 {} 项；失败 {} 项；{}{}执行历史：{}" => {
            "{}\nCompleted: {}; failed: {}; {}{}History: {}"
        }
        "已取消。" => "Cancelled. ",
        "需要重启。" => "Restart required. ",
        "{}已返回 {count} 项检查状态。预览状态不代表读取了 Windows；检查不修改系统。" => {
            "{}Returned {count} inspection states. Preview states do not inspect Windows; inspection makes no changes."
        }
        "检查已取消，仅显示已返回结果。" => {
            "Inspection cancelled; only returned results are shown. "
        }
        "检查结束。" => "Inspection finished. ",
        "当前主机或目标不允许执行，请核对实际 Windows 版本。" => {
            "This host or target cannot apply changes. Check the actual Windows edition and build."
        }
        "方案校验失败，请重新检查所选项目。" => {
            "Plan validation failed. Review the selected items."
        }
        "正在应用变更" => "Applying settings",
        "正在应用电源参数" => "Applying power parameters",
        "正在恢复电源参数" => "Restoring power parameters",
        "当前主机不允许恢复 Windows 配置。" => {
            "This host cannot restore Windows settings."
        }
        "正在恢复配置" => "Restoring settings",
        "正在只读检查所选项目" => "Inspecting selected items (read only)",
        "正在读取存储通知策略" => "Reading storage notification policy",
        "正在执行部署 / 功能 / 维护" => "Running deployment / features / maintenance",
        "让系统，回到你的习惯" => "Your Windows workspace",
        "配置工作台" => "System settings",
        "按需选择系统设置" => "Choose individual settings",
        "软件部署" => "Software",
        "精确包 ID，按需安装" => "Install by exact package ID",
        "Windows 功能" => "Windows features",
        "启用可选系统组件" => "Enable optional components",
        "系统维护" => "Maintenance",
        "内置检查与维护命令" => "Built-in checks and servicing",
        "变更方案" => "Review plan",
        "检查每一处修改" => "Review exact operations",
        "恢复记录" => "Recovery & history",
        "查看真实执行日志" => "Review actual execution evidence",
        "项目来源" => "Sources",
        "透明、可追溯的配置" => "Traceable configuration sources",
        "离线方案 · 逐项选择" => "Explicit choices · Portable plans",
        "每项变更，由你决定" => "Every change is your choice",
        "已选择 {} 项" => "{} items selected",
        "设置 {} / 部署、功能、维护 {}" => "Settings {} / Other tasks {}",
        "检查变更方案" => "Review plan",
        "清空选择" => "Clear selection",
        "本机执行" => "Native execution",
        "交互预览" => "Interactive preview",
        "等待当前操作返回，完成后显示真实结果" => {
            "Waiting for the current operation; actual results follow"
        }
        "正在取消…" => "Cancelling…",
        "取消任务" => "Cancel task",
        "关闭提示" => "Dismiss",
        "方案目标" => "Target",
        "载入档案" => "Load profile",
        "保存档案" => "Save profile",
        "检查所选项目状态" => "Inspect selection",
        "已识别本机 Windows；变更前仍需检查方案并确认。" => {
            "Windows host identified. Review and confirm the plan before applying."
        }
        "系统设置，按你的习惯组合。" => "System settings",
        "分类查找、预设起步，逐项检查实际注册表变更。" => {
            "Browse categories or start with a preset, then review exact registry changes."
        }
        "所有项目默认不选；互斥选项自动替换。" => {
            "Nothing is selected by default. Alternative choices replace each other."
        }
        "无法载入配置目录" => "Unable to load the settings catalog",
        "配置组合 · {} 个可选起点" => "Presets · {} optional starting points",
        "选用组合会替换当前系统设置选择，软件 / 功能 / 维护选择保留。选用后仍可逐项调整。" => {
            "A preset replaces your settings selection and preserves software, feature and maintenance selections. You can adjust every choice."
        }
        "{} 项 · {}" => "{} items · {}",
        "检查并选用…" => "Review preset…",
        "全部分类" => "All categories",
        "搜索设置、用途或 ID…" => "Search settings, purpose or ID…",
        "{} 项设置" => "{} settings",
        "{}  {} 项 / 已选 {}" => "{}  {} items / {} selected",
        "目标不适用" => "Unavailable for target",
        "风险、来源与确切变更" => "Risk, sources and exact changes",
        "风险：{}；生效：{}；ID：{}" => "Risk: {}; takes effect: {}; ID: {}",
        "互斥设置组：{group}。选择此项会移除组内其他已选项。" => {
            "Choice group: {group}. Selecting this item removes other selected alternatives."
        }
        "固定版本原始配置 {}" => "Pinned upstream evidence {}",
        "没有匹配的设置" => "No matching settings",
        "调整关键词或分类继续查找。" => "Try another search or category.",
        "精确的软件包 ID，由 WinGet 执行安装；安装器可能显示额外的许可与交互提示。" => {
            "Install exact package IDs with WinGet. Installers may show additional license or interaction prompts."
        }
        "通过 Windows 自身的可选功能机制启用组件，部分功能需要源文件与重启。" => {
            "Enable optional Windows components. Some features require installation sources or a restart."
        }
        "逐项选择 Windows 内置诊断与维护命令。维护可能耗时，取消需等待当前系统操作返回。" => {
            "Choose built-in Windows diagnostics and servicing commands. Operations can take time; cancellation waits for the current system operation."
        }
        "这些操作不属于注册表恢复范围。执行历史是操作证据，不提供通用回滚。" => {
            "These operations are outside registry recovery. History records execution evidence, not a general rollback."
        }
        "WinGet 已识别；源、权限与许可要求在执行时再次检查。" => {
            "WinGet identified. Sources, permissions and agreement requirements are checked again when executing."
        }
        "WinGet 尚不可用：需要微软应用安装程序及其执行别名。" => {
            "WinGet unavailable. Microsoft App Installer and its execution alias are required."
        }
        "WinGet 仅在 Windows 执行；这里可选择并导出软件部署方案。" => {
            "WinGet runs on Windows. You can select software and export a deployment plan here."
        }
        "刷新检测" => "Refresh capability",
        "查看 WinGet 检测详情" => "WinGet detection details",
        "搜索名称、软件包 ID 或功能名…" => "Search names, package IDs or features…",
        "无法载入部署目录" => "Unable to load the deployment catalog",
        "没有匹配的项目" => "No matching items",
        "调整关键词继续查找。" => "Try another search.",
        "权限、限制与来源" => "Permissions, limits and sources",
        "检查完整工作方案" => "Review workspace plan",
        "目标：{}。导出不会修改本机。" => {
            "Target: {}. Exporting makes no changes to this computer."
        }
        "还没有选择项目" => "No items selected",
        "从系统设置、软件部署、Windows 功能或系统维护中逐项选择。" => {
            "Choose individual items from system settings, software, Windows features or maintenance."
        }
        "设置目录不可用" => "Settings catalog unavailable",
        "系统设置校验失败：{error:#}" => "Settings validation failed: {error:#}",
        "部署项目校验失败：{error:#}" => "Deployment validation failed: {error:#}",
        "导出完整 JSON 方案" => "Export complete JSON plan",
        "应用系统设置…" => "Apply settings…",
        "执行部署 / 功能 / 维护…" => "Execute deployment / features / maintenance…",
        "系统设置 · {} 项 · 原值留档、按日志恢复" => {
            "System settings · {} items · Original values journaled for recovery"
        }
        "风险：{}；生效：{}" => "Risk: {}; takes effect: {}",
        "部署 / 功能 / 维护 · {} 项 · 不属于注册表恢复范围" => {
            "Deployment / features / maintenance · {} items · Outside registry recovery"
        }
        "两类执行分别确认。注册表恢复只覆盖日志中的原值；软件安装、功能启用及维护没有通用回滚。不会自动接受外部安装器的许可协议。" => {
            "Confirm the two execution types separately. Registry recovery covers journaled values only. Software, features and maintenance have no general rollback. External installer licenses are not automatically accepted."
        }
        "给每次变更，留一条回程路" => "Recovery and execution history",
        "刷新记录" => "Refresh records",
        "这里只显示实际生成的恢复日志。恢复会检查原值与当前值的冲突。" => {
            "Only actual recovery journals appear here. Restoring checks for conflicts with current values."
        }
        "日志目录：{}" => "Journal directory: {}",
        "本次会话的部署执行历史" => "Deployment history from this session",
        "这些日志记录执行结果，不是可恢复的系统设置备份。" => {
            "These logs record execution results. They are not restorable settings backups."
        }
        "无法读取恢复记录" => "Unable to read recovery records",
        "还没有恢复记录" => "No recovery records",
        "在受支持的 Windows 主机应用配置后，真实的变更日志会出现在这里。" => {
            "Apply settings on a supported Windows host to create actual recovery journals."
        }
        "检查并恢复…" => "Check and restore…",
        "{} / {} 项配置" => "{} / {} settings",
        "好的配置，也应说清来处" => "Configuration sources",
        "配置由 Rust 目录逐项映射与校验；不会下载或执行上游脚本。" => {
            "Each setting is mapped and validated in the Rust catalog. Upstream scripts are not downloaded or executed."
        }
        "目录不可用" => "Catalog unavailable",
        "固定版本：{}" => "Pinned revision: {}",
        "许可证 / 使用范围：{}" => "License / scope: {}",
        "配置目录版本：{}" => "Settings catalog revision: {}",
        "部署目录版本：{} / {} 项任务" => "Deployment catalog revision: {} / {} tasks",
        "系统设置映射固定版本来源；部署项目逐项显示软件包、功能或内置命令及限制。软件本身遵循各自许可证，安装器许可需由你决定。" => {
            "Settings cite pinned sources. Deployment tasks identify packages, features or built-in commands and their limits. Software retains its own license; you decide any installer agreement."
        }
        "输入现有 JSON 档案的完整路径。载入会替换当前选择。" => {
            "Enter the absolute path of an existing JSON profile. Loading replaces the current selection."
        }
        "输入新文件的完整路径。为保护现有文件，已存在的路径不会被覆盖。" => {
            "Enter the absolute path of a new file. Existing files are never overwritten."
        }
        "完整文件路径，例如 /Users/you/Desktop/lt-sc.json" => {
            "Absolute file path, e.g. /Users/you/Desktop/lt-sc.json"
        }
        "取消" => "Cancel",
        "确认在这台 Windows 上应用" => "Confirm settings on this Windows host",
        "实际主机：{}" => "Actual host: {}",
        "方案目标：{}" => "Plan target: {}",
        "共 {} 项配置，以下是将写入的确切值：" => {
            "{} settings. These are the exact values to write:"
        }
        "每项修改前保存原始值，修改后核对。失败或取消可能留下部分已完成的变更，请按真实报告与恢复日志处理。" => {
            "Original values are saved before each write and verified afterwards. Failure or cancellation can leave completed changes. Review the actual report and recovery journal."
        }
        "确认应用所选配置" => "Confirm selected settings",
        "返回检查" => "Back to review",
        "确认执行部署 / 功能 / 维护" => "Confirm deployment / features / maintenance",
        "这些操作不能由注册表恢复日志撤销。可能需要管理员权限、网络、系统源文件或重启；取消只能在系统允许的边界生效，已经完成的操作仍会保留。" => {
            "Registry journals cannot undo these operations. Administrator rights, network access, sources or a restart may be required. Cancellation follows system boundaries; completed operations remain."
        }
        "不会自动接受外部安装器的许可协议。软件安装器可能显示交互提示，请自行检查并决定。" => {
            "External installer licenses are not automatically accepted. Installers may show prompts for you to review and decide."
        }
        "确认执行以上任务" => "Confirm these tasks",
        "选用配置组合" => "Use a settings preset",
        "将以这 {} 项替换当前 {} 项系统设置选择；部署、功能及维护选择保留。不会立即修改系统。" => {
            "Replace {} items in this preset for the current {} settings selections. Deployment, feature and maintenance selections remain. No system changes are made now."
        }
        "此组合不适合当前目标：{error:#}" => {
            "This preset does not fit the selected target: {error:#}"
        }
        "确认替换系统设置选择" => "Replace settings selection",
        "配置组合已不可用。" => "This preset is unavailable.",
        "确认恢复这条记录" => "Confirm recovery journal",
        "恢复只处理这条日志记录的注册表值。若当前值在执行后被其他程序修改，冲突检查会阻止覆盖。" => {
            "Restore affects only registry values in this journal. Conflict checks prevent overwriting values changed later by another program."
        }
        "确认检查并恢复" => "Confirm check and restore",
        "请输入完整的绝对路径。" => "Enter a complete absolute path.",
        "方案已导出：{}" => "Plan exported: {}",
        "配置档案已保存：{}" => "Profile saved: {}",
        "已载入 {} 项配置：{}" => "Loaded {} items: {}",
        "已请求取消。请等待当前操作返回并查看报告，然后关闭窗口。" => {
            "Cancellation requested. Wait for the current operation and review its report before closing."
        }
        "隐私" => "Privacy",
        "资源管理器" => "File Explorer",
        "桌面体验" => "Desktop experience",
        "开始菜单" => "Start menu",
        "任务栏" => "Taskbar",
        "个性化" => "Personalization",
        "游戏" => "Gaming",
        "辅助功能" => "Accessibility",
        "系统" => "System",
        "更新偏好" => "Update preferences",
        "符合方案" => "Matches plan",
        "与方案不同" => "Differs from plan",
        "无法检查" => "Unavailable",
        "预览，未检查本机 Windows" => "Preview; Windows not inspected",
        "已安装" => "Installed",
        "未安装" => "Not installed",
        "已启用" => "Enabled",
        "未启用" => "Disabled",
        "等待重启启用" => "Enable pending restart",
        "可执行" => "Ready",
        "检查失败" => "Inspection failed",
        "上次检查：{label}" => "Last inspection: {label}",
        "(默认)" => "(Default)",
        "WinGet 软件包：{id}" => "WinGet package: {id}",
        "Windows 功能：{name}" => "Windows feature: {name}",
        "内置维护命令：{} ({})" => "Built-in maintenance: {} ({})",
        "{}；生效：{}" => "{}; takes effect: {}",
        "需要管理员权限" => "Administrator rights required",
        "以当前用户执行，安装器可能要求提升权限" => {
            "Runs as current user; installer may request elevation"
        }
        "来源与执行依据 {}" => "Source and execution reference {}",
        "当前系统" => "Current system",
        "当前 Windows 版本不支持执行。可以选择、检查并导出方案。" => {
            "This Windows edition or build cannot execute plans. You can select, review and export them."
        }
        "当前为预览模式：可选择、检查并导出方案；应用与恢复需在受支持的 Windows 上进行。" => {
            "Preview mode: select, inspect and export plans. Applying and restoring require supported Windows."
        }
        "绝对路径" => "absolute path",
        _ => source,
    }
}

/// Both literals remain visible to review; arguments are evaluated only in the active branch.
macro_rules! lf {
    ($zh:literal, $en:literal, $($args:tt)*) => {
        if $crate::gui::i18n::english() { format!($en, $($args)*) } else { format!($zh, $($args)*) }
    };
    ($zh:literal, $en:literal) => {
        if $crate::gui::i18n::english() { format!($en) } else { format!($zh) }
    };
}
pub(super) use lf;
