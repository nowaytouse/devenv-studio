//! Chinese presentation for the fixed, analyzer-authored migration explanations.
//! Exact phrase replacement preserves identifiers, user-provided data and evidence URLs.
use crate::prefs::Language;

const DETAIL_PHRASES: &[(&str, &str)] = &[
    (
        "The source is not the fixed Microsoft community source or contains unsupported metadata. Source values were not imported.",
        "来源不是固定的微软社区源，或包含未支持的元数据。不会导入来源字段值。",
    ),
    (
        "Multiple entries request this package identity. Repeated exports and version conflicts require manual review; no entry was selected automatically.",
        "多个条目请求同一软件身份。重复导出或版本冲突需手动审核，不会自动选择其中任何一项。",
    ),
    (
        "This package contains unsupported channel, scope, installer arguments or additional properties. Their values were not imported.",
        "此条目包含未支持的渠道、安装范围、安装参数或额外属性。不会导入这些字段值。",
    ),
    (
        "Version metadata does not pin installation. Only the current trusted task is suggested.",
        "来源版本不会锁定安装版本；这里只建议审核当前可信目录中的任务。",
    ),
    (
        "Exported version metadata is present; its value is hidden.",
        "导出清单包含版本元数据，其值已隐藏。",
    ),
    ("Exported version: ", "导出清单中的版本："),
    ("Captured Windows version: ", "备份中的 Windows 版本："),
    (
        ". Only reviewed catalog identities can be selected. Versions are evidence, not installation pins; unknown tools require manual review.",
        "。仅可选择经过审核的目录身份。版本是观测记录，不锁定安装版本；未知工具需手动审核。",
    ),
    (
        "This provider was not completely captured. Installed state is unknown; no installation or removal is inferred.",
        "该来源未完整采集，已安装状态未知，不据此推断安装或卸载。",
    ),
    (
        "Reviewed fixed legacy Homebrew-to-Pip provider mapping. This selects only the named Python distribution, not a macOS binary or the complete Homebrew dependency graph.",
        "经过审核的固定旧版 Homebrew 到 Pip 提供方映射。只选择指定 Python 发行包，不复制 macOS 二进制文件或完整 Homebrew 依赖图。",
    ),
    (
        " Legacy AsciiDoc.py processor; current AsciiDoc syntax and complete DocBook/PDF toolchains are not reproduced.",
        " 这是旧版 AsciiDoc.py 处理器，不复刻当前 AsciiDoc 语法或完整 DocBook/PDF 工具链。",
    ),
    (
        " Project configuration and runtime data are not migrated.",
        " 不迁移项目配置或运行数据。",
    ),
    (
        " External document toolchains are not installed.",
        " 不安装外部文档工具链。",
    ),
    (
        " Python bindings only; no C++ SDK, native tools, optional modules or matching build flags.",
        " 仅提供 Python 绑定，不提供 C++ SDK、原生工具、可选模块或一致构建参数。",
    ),
    (
        " User-site setuptools package only; the interpreter and bundled pip are not replaced.",
        " 仅为当前用户安装 setuptools 包，不替换解释器或随附 pip。",
    ),
    (
        " Requires registered official 64-bit standard Windows CPython 3.11–3.14 in a supported standard location with bundled pip. Uses compatible wheels only for current-user installation from PyPI; pip resolves dependencies. No project, virtual environment, requirement, PATH or source configuration is imported, and Python/pip are not bootstrapped.",
        " 需要在受支持标准位置注册的官方 Windows 64 位标准 CPython 3.11–3.14 及随附 pip。从 PyPI 为当前用户仅安装兼容 wheel，依赖由 pip 解析。不导入项目、虚拟环境、依赖文件、PATH 或来源配置，也不自动安装 Python/pip。",
    ),
    (
        "Exact PEP 503-normalized Pip distribution ID exists in the trusted catalog. This is a review suggestion only; installed state, version and configuration have not been migrated.",
        "可信目录中存在经过 PEP 503 标准化后完全匹配的 Pip 发行包 ID。这只是审核建议；不会迁移已安装状态、版本或配置。",
    ),
    (
        "Only bare Pip distribution names can be mapped. Requirement specifiers, extras, URLs, paths and custom options are withheld and left for manual review; no installation or alternative provider was selected.",
        "只映射裸 Pip 发行包名称。版本约束、可选扩展、URL、路径和自定义选项已隐藏并留待手动审核；未选择安装或替代提供方。",
    ),
    (
        "Matches the legacy defrag /C /O /U operation: Windows chooses optimization for all volumes, potentially including removable volumes, not only fixed drives. This is a review suggestion; no analysis or TRIM-notification policy change is selected. Optimization has no rollback.",
        "精确对应旧版 defrag /C /O /U：由 Windows 为所有卷选择适当优化，可能包括可移动卷，并非只处理固定磁盘。这只是审核建议，未选择额外分析或 TRIM 通知策略更改；优化无法回滚。",
    ),
    (
        "No exact reviewed storage-optimization task supports this Windows target. No storage operation was selected.",
        "没有支持此 Windows 目标且经过审核的精确存储优化任务，未选择任何存储操作。",
    ),
    (
        "TRIM-notification policy requires separate NTFS and ReFS state capture, exact parsing and verified restoration. No fsutil policy write or approximate optimization mapping was selected.",
        "TRIM 通知策略需要分别记录 NTFS 与 ReFS 状态、精确解析并验证恢复。未选择 fsutil 策略写入，也未近似映射为优化操作。",
    ),
    (
        "Legacy power setup creates or switches an Ultimate Performance scheme and may also change CPU and hibernation parameters. Current AC choices modify only the active scheme; no partial or approximate mapping was selected.",
        "旧版电源流程会创建或切换卓越性能方案，还可能修改 CPU 和休眠参数。当前插电选项只修改活动方案，未选择部分或近似映射。",
    ),
    (
        " Requires official Node.js and bundled npm on Windows. Uses the fixed current-user npm prefix and official registry with lifecycle scripts disabled; no PATH, mirror or project configuration is imported.",
        " 需要 Windows 上官方安装的 Node.js 及随附 npm。使用固定的当前用户 npm 前缀和官方源，禁用生命周期脚本；不导入 PATH、镜像或项目配置。",
    ),
    (
        "The source list is empty. It requests no installation or removal.",
        "来源列表为空，不请求安装或移除任何项目。",
    ),
    (
        "Unrecognized custom field: no schema semantics or trusted migration mapping. Field name and value are withheld; no automatic changes.",
        "无法识别的自定义字段：没有对应的格式语义或可信迁移映射。字段名和值已隐藏；不会自动更改。",
    ),
    (
        "No unique exact provider/package ID exists in the current trusted catalog. No approximate match or provider substitution was selected.",
        "当前可信目录中没有唯一且完全匹配的提供方/软件包 ID。未选择近似匹配项或替代提供方。",
    ),
    (
        "The exact trusted task does not support the selected Windows target. Nothing was selected.",
        "可信目录中的精确任务不支持所选 Windows 目标，因此未选择任何项目。",
    ),
    (
        "Exact provider/package ID exists in the trusted catalog. This is a review suggestion only; installed state, version and configuration have not been migrated.",
        "可信目录中存在完全匹配的提供方/软件包 ID。这只是供审核的建议；不会迁移已安装状态、版本或配置。",
    ),
    (
        " Requires an existing working Windows Cargo/Rust toolchain and any crate-specific build dependencies; Mac binaries and build options are not copied.",
        " 需要 Windows 上已有且可用的 Cargo/Rust 工具链及该 crate 所需的构建依赖；不会复制 Mac 二进制文件或构建选项。",
    ),
    (
        " Requires an existing usable Python interpreter and uv on Windows; automatic Python downloads are disabled. The source environment is not copied.",
        " 需要 Windows 上已有且可用的 Python 解释器和 uv；不会自动下载 Python，也不会复制源环境。",
    ),
    (
        " Requires the Microsoft VS Code CLI on Windows. This does not import editor settings or install into Cursor.",
        " 需要 Windows 上的 Microsoft VS Code 命令行工具。不会导入编辑器设置，也不会安装到 Cursor。",
    ),
    (
        " Requires the Windows provider and prerequisites shown by the trusted task; no installed-state or version equivalence is inferred.",
        " 需要可信任务所示的 Windows 提供方及其先决条件；不会推断已安装状态或版本等价。",
    ),
    (
        "This source provider is not executable in the current trusted catalog. Installing through a different provider would change semantics; no substitution was selected.",
        "当前可信目录不支持执行此来源提供方。改用其他提供方安装会改变原有语义，因此未选择替代项。",
    ),
    (
        "Supported legacy source schema. Analysis is read-only; importing does not execute the old deployment stages.",
        "支持此旧版来源格式。分析过程只读；导入不会执行旧部署流程。",
    ),
    (
        "Private/custom configuration is not migrated. Review it separately; no identity, alias, profile, mirror or environment value was copied or written.",
        "不会迁移私人或自定义配置。请单独审核；不会复制或写入身份信息、别名、配置文件、镜像源或环境变量。",
    ),
    (
        "Editor settings may contain private paths, commands and credentials. They are not copied, merged or written by this importer.",
        "编辑器设置可能包含私人路径、命令和凭据。此导入器不会复制、合并或写入这些设置。",
    ),
    (
        "Exact WinGet package ID exists in the current trusted catalog. Legacy display names and installer configuration are not used.",
        "当前可信目录中存在完全匹配的 WinGet 软件包 ID。不会使用旧版显示名称或安装程序配置。",
    ),
    (
        "In the old tool, false skips this tweak; it does not restore or request the opposite value. No opposite setting was selected.",
        "在旧工具中，false 表示跳过此项调整，并不表示恢复或请求相反的值。未选择相反设置。",
    ),
    (
        "Legacy numeric network/performance policy has no exact reviewed mapping. No value is copied or written.",
        "旧版数值型网络/性能策略没有经过审核的精确映射。不会复制或写入任何值。",
    ),
    (
        "Legacy rollback-journal preference is not imported. New registry execution always uses its own durable journal and does not consume old rollback files.",
        "不会导入旧版回滚日志偏好。新的注册表操作始终使用自己的持久化日志，不会读取旧回滚文件。",
    ),
    (
        "No exact reviewed equivalent exists in the current settings catalog. Power, storage, networking, security and broad performance changes are not approximated.",
        "当前设置目录中没有经过审核的精确等效项。不会近似映射电源、存储、网络、安全或广泛性能调整。",
    ),
    (
        "False means the old operation was skipped, not that an inverse operation should run. No changes were selected.",
        "false 表示旧操作已跳过，并不表示应执行相反操作。未选择任何更改。",
    ),
    (
        "Unrecognized system tweak and value withheld. No automatic migration.",
        "无法识别的系统调整；其值已隐藏。不会自动迁移。",
    ),
    (
        "False skips enabling in the old tool. It does not request disabling an installed Windows feature.",
        "在旧工具中，false 表示跳过启用；并不表示要禁用已安装的 Windows 功能。",
    ),
    (
        "Unrecognized feature request; key and value withheld, and no component selected.",
        "无法识别的功能请求；键和值已隐藏，未选择任何组件。",
    ),
    (
        "The source model list is empty; no model download or removal is requested.",
        "来源模型列表为空，不请求下载或移除模型。",
    ),
    (
        "Model choices can be private and models have separate storage/license/runtime requirements. No model download, deletion or configuration is performed.",
        "模型选择可能属于私人信息；不同模型有各自的存储、许可和运行要求。不会下载、删除或配置模型。",
    ),
    (
        "No unique exact registry-equivalent action exists in the current trusted catalog. Similar labels or narrower/broader tweaks are not substituted.",
        "当前可信目录中没有唯一且完全匹配的注册表操作。不会用名称相似或范围更窄/更广的调整替代。",
    ),
    (
        "The exact registry-equivalent setting does not support the selected Windows target.",
        "完全匹配的注册表设置不支持所选 Windows 目标。",
    ),
    (
        "Matched every hive, key, value name and DWORD value against one current trusted action. Original bytes are not copied; the trusted action is suggested for explicit review.",
        "已将注册表配置单元、项路径、值名称和 DWORD 值逐项匹配到当前可信操作。不会复制原始字节；仅建议显式审核该可信操作。",
    ),
    (
        "The complete feature request cannot be represented for this Windows target. No partial feature selection was added.",
        "此 Windows 目标无法完整表示该功能请求，因此未添加部分功能选择。",
    ),
    (
        "WSL2 requires BOTH Windows Subsystem for Linux and Virtual Machine Platform; both trusted tasks are suggested together. This does not install a distribution, kernel update, or configure firmware virtualization; a restart may be needed.",
        "WSL2 需要同时启用 Windows Subsystem for Linux 和 Virtual Machine Platform；建议一并审核这两项可信任务。此操作不会安装 Linux 发行版或内核更新，也不会配置固件虚拟化；可能需要重启。",
    ),
    (
        "Exact Windows optional-feature identity exists in the trusted catalog. Source files, administrator rights and a restart may be required.",
        "可信目录中存在完全匹配的 Windows 可选功能标识。可能需要源文件、管理员权限和重启。",
    ),
    (
        "Imported inventory timestamp is source metadata, not a live inspection or proof of installed Windows software.",
        "导入的清单时间戳只是来源元数据，并非实时检查，也不能证明 Windows 软件已安装。",
    ),
    (
        "No explicit reviewed macOS-to-Windows mapping exists. No same-name search, guessed package, alternative app or macOS binary was selected.",
        "没有经过明确审核的 macOS 到 Windows 映射。未按名称搜索、猜测软件包、选择替代应用或选择 macOS 二进制文件。",
    ),
    (
        "No Homebrew taps were supplied; no Windows package sources are changed.",
        "来源中未提供 Homebrew tap；不会更改 Windows 软件包源。",
    ),
    (
        "Homebrew repository sources are not Windows package sources. Names/URLs are withheld and no package-manager source is added.",
        "Homebrew 仓库源不是 Windows 软件包源。名称和 URL 已隐藏，不会添加任何软件包管理器源。",
    ),
    (
        "The inventory reports no source warnings. This is not a live completeness guarantee.",
        "清单没有报告来源警告；这不代表实时完整性保证。",
    ),
    (
        "The imported inventory contains source collection warnings. Their potentially private text is withheld; the software list may be incomplete. Review the original file before using suggestions.",
        "导入的清单包含来源收集警告。可能含有私人信息的警告文本已隐藏，软件列表可能不完整。使用建议前请审核原始文件。",
    ),
    (
        "Same named upstream product or CLI, mapped to a separately reviewed Windows provider. This does not promise matching versions, binaries, build flags, extensions or configuration.",
        "名称相同的上游产品或命令行工具已映射到单独审核的 Windows 提供方。此映射不保证版本、二进制文件、构建参数、扩展或配置相同。",
    ),
];

pub fn detail(value: &str, language: Language) -> String {
    if language == Language::EnUs {
        return value.to_owned();
    }
    DETAIL_PHRASES
        .iter()
        .fold(value.to_owned(), |text, (english, chinese)| {
            text.replace(english, chinese)
        })
}

pub fn source(value: &str, language: Language) -> String {
    let (chinese, english) = match value {
        "setup_profile" => ("旧版配置档案", "Legacy setup profile"),
        "macos_inventory" => ("旧版 Mac 工具清单", "Legacy Mac inventory"),
        "studio_macos_backup" => ("DevEnv Studio Mac 备份", "DevEnv Studio Mac backup"),
        "studio_windows_backup" => ("DevEnv Studio Windows 备份", "DevEnv Studio Windows backup"),
        "winget_export" => ("WinGet 导出清单", "WinGet export"),
        _ => ("其他导入清单", "Other imported inventory"),
    };
    if language == Language::EnUs {
        english
    } else {
        chinese
    }
    .to_owned()
}

pub fn requested(value: &str, language: Language) -> String {
    if language == Language::EnUs {
        return value.to_owned();
    }
    match value {
        "[value redacted]" => "［值已隐藏］".to_owned(),
        "(empty list)" => "（空列表）".to_owned(),
        "(unsupported source)" => "（未支持的软件源）".to_owned(),
        _ => value.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::Target;

    fn contains_cjk(text: &str) -> bool {
        text.chars()
            .any(|character| ('\u{4e00}'..='\u{9fff}').contains(&character))
    }

    #[test]
    fn english_and_unknown_values_are_preserved() {
        let detail = "Unknown detail: PRIVATE_TOKEN_42";
        assert_eq!(detail, super::detail(detail, Language::EnUs));
        assert_eq!(detail, super::detail(detail, Language::ZhCn));
        assert_eq!(
            "PRIVATE_PACKAGE_ID",
            super::requested("PRIVATE_PACKAGE_ID", Language::ZhCn)
        );
    }

    #[test]
    fn pip_migration_details_have_chinese_explanations_without_private_requirements() {
        let mut profile: serde_json::Value =
            serde_json::from_slice(include_bytes!("../../../examples/migration-profile.json"))
                .unwrap();
        profile["packages"]["pip_packages"] =
            serde_json::json!(["REQUESTS", "unknown-pip", "requests[extra]"]);
        let report = crate::migration::analyze(
            &serde_json::to_vec(&profile).unwrap(),
            Target::Win11Ltsc2024,
        )
        .unwrap();
        let items: Vec<_> = report
            .items
            .iter()
            .filter(|item| item.origin == "packages.pip_packages")
            .collect();
        assert_eq!(items.len(), 3);
        for item in items {
            let translated = super::detail(&item.detail, Language::ZhCn);
            assert!(contains_cjk(&translated), "{}", item.detail);
            assert!(!translated.contains("requests[extra]"));
            assert_eq!(super::detail(&item.detail, Language::EnUs), item.detail);
        }
    }

    #[test]
    fn storage_migration_details_have_chinese_explanations() {
        let mut profile: serde_json::Value =
            serde_json::from_slice(include_bytes!("../../../examples/migration-profile.json"))
                .unwrap();
        profile["system_tweaks"] =
            serde_json::json!({"optimize_storage": true, "enable_trim": true});
        let report = crate::migration::analyze(
            &serde_json::to_vec(&profile).unwrap(),
            Target::Win11Ltsc2024,
        )
        .unwrap();
        for item in report
            .items
            .iter()
            .filter(|item| item.origin.starts_with("system_tweaks."))
        {
            let translated = super::detail(&item.detail, Language::ZhCn);
            assert!(contains_cjk(&translated), "{}", item.origin);
            assert_eq!(super::detail(&item.detail, Language::EnUs), item.detail);
        }
    }

    #[test]
    fn requested_placeholders_are_localized_only_in_chinese() {
        assert_eq!(
            "[value redacted]",
            super::requested("[value redacted]", Language::EnUs)
        );
        assert_eq!(
            "［值已隐藏］",
            super::requested("[value redacted]", Language::ZhCn)
        );
        assert_eq!(
            "(empty list)",
            super::requested("(empty list)", Language::EnUs)
        );
        assert_eq!(
            "（空列表）",
            super::requested("(empty list)", Language::ZhCn)
        );
    }

    #[test]
    fn analyzer_sample_details_have_chinese_explanations() {
        let report = crate::migration::analyze(
            include_bytes!("../../../examples/migration-profile.json"),
            Target::Win11Ltsc2024,
        )
        .expect("synthetic migration fixture should analyze");
        assert_eq!(report.items.len(), 23);
        for item in report.items {
            let translated = super::detail(&item.detail, Language::ZhCn);
            assert!(
                contains_cjk(&translated),
                "untranslated analyzer detail for {}: {}",
                item.origin,
                translated
            );
        }
    }

    #[test]
    fn known_phrases_translate_without_rewriting_raw_ids_or_urls() {
        let source = "Exact provider/package ID exists in the trusted catalog. This is a review suggestion only; installed state, version and configuration have not been migrated. Requires a Windows provider. Evidence: https://example.invalid/PRIVATE_ID.";
        let translated = super::detail(source, Language::ZhCn);
        assert!(contains_cjk(&translated));
        assert!(translated.contains("https://example.invalid/PRIVATE_ID"));
        assert!(translated.contains("Windows provider."));
    }

    #[test]
    fn migration_sources_have_readable_names_and_unknown_values_are_not_echoed() {
        for value in [
            "setup_profile",
            "macos_inventory",
            "studio_macos_backup",
            "studio_windows_backup",
            "winget_export",
        ] {
            assert!(contains_cjk(&source(value, Language::ZhCn)));
            assert!(!source(value, Language::EnUs).contains('_'));
        }
        let private = "https://user:secret@private.example/source";
        assert_eq!(source(private, Language::EnUs), "Other imported inventory");
        assert_eq!(source(private, Language::ZhCn), "其他导入清单");
    }

    #[test]
    fn winget_review_details_are_translated_without_changing_safe_version_tokens() {
        for text in [
            "The source is not the fixed Microsoft community source or contains unsupported metadata. Source values were not imported.",
            "Multiple entries request this package identity. Repeated exports and version conflicts require manual review; no entry was selected automatically.",
            "This package contains unsupported channel, scope, installer arguments or additional properties. Their values were not imported.",
            "Version metadata does not pin installation. Only the current trusted task is suggested.",
            "Exported version metadata is present; its value is hidden.",
        ] {
            let chinese = detail(text, Language::ZhCn);
            assert!(contains_cjk(&chinese), "{text}");
            assert_ne!(chinese, text);
            assert_eq!(detail(text, Language::EnUs), text);
        }
        assert_eq!(
            detail("Exported version: 2.47.1. ", Language::ZhCn),
            "导出清单中的版本：2.47.1. "
        );
        assert_eq!(
            requested("(unsupported source)", Language::ZhCn),
            "（未支持的软件源）"
        );
    }
}
