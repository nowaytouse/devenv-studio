use crate::inventory::{MacosInventory, ParityProvider, ParityRule, ParityRules, SourceKind};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum NetworkMode {
    Basic,
    Optimized,
    Extreme,
}

impl std::fmt::Display for NetworkMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetworkMode::Basic => write!(f, "Basic (刷新 DNS 缓存)"),
            NetworkMode::Optimized => write!(f, "Optimized (刷新 DNS & 优化 TCP 窗口)"),
            NetworkMode::Extreme => write!(f, "Extreme (RSS/RSC + Fast Open + CTCP/ECN)"),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExecutionTarget {
    FullSetup,
    NetworkOnly,
    StorageOnly,
    WindowsFeaturesOnly,
    AgentSkillsOnly,
    DevToolsOnly,
    VSCodeExtensionsOnly,
    SystemTweaksOnly,
    RollbackLatest,
}

impl ExecutionTarget {
    pub const ALL: [Self; 9] = [
        Self::DevToolsOnly,
        Self::FullSetup,
        Self::NetworkOnly,
        Self::StorageOnly,
        Self::WindowsFeaturesOnly,
        Self::VSCodeExtensionsOnly,
        Self::AgentSkillsOnly,
        Self::SystemTweaksOnly,
        Self::RollbackLatest,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::FullSetup => "完整部署",
            Self::DevToolsOnly => "开发环境部署",
            Self::NetworkOnly => "网络优化",
            Self::StorageOnly => "存储优化",
            Self::WindowsFeaturesOnly => "Windows 功能",
            Self::VSCodeExtensionsOnly => "IDE 同步",
            Self::AgentSkillsOnly => "Agent 配置",
            Self::SystemTweaksOnly => "系统优化",
            Self::RollbackLatest => "撤销上次调优",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::FullSetup => "按依赖顺序部署软件、开发环境和系统设置",
            Self::DevToolsOnly => "按已审核 Profile 配置 Windows 软件与开发工具",
            Self::NetworkOnly => "保存原配置后刷新 DNS 并应用选定的 TCP 设置",
            Self::StorageOnly => "启用 TRIM，并按 SSD/HDD 介质类型执行 Windows 官方优化",
            Self::WindowsFeaturesOnly => "按能力检测启用 WSL2、.NET 3.5、Sandbox 等可选功能",
            Self::VSCodeExtensionsOnly => "合并 VS Code / Cursor 设置并补齐扩展",
            Self::AgentSkillsOnly => "从程序内置资源恢复 Skills、Plugins 与 MCP 配置",
            Self::SystemTweaksOnly => "通过 Windows 原生注册表接口应用性能与隐私设置",
            Self::RollbackLatest => "按最近一次本机账本逆序恢复注册表、电源与网络设置",
        }
    }

    pub fn requires_admin(self) -> bool {
        matches!(
            self,
            Self::FullSetup
                | Self::NetworkOnly
                | Self::StorageOnly
                | Self::WindowsFeaturesOnly
                | Self::SystemTweaksOnly
                | Self::RollbackLatest
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WingetApp {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitProfileConfig {
    pub user_name: String,
    pub user_email: String,
    pub post_buffer_bytes: u64,
    pub safe_directory: String,
    pub enable_long_paths: bool,
    pub enable_autocrlf: bool,
    pub global_gitignore_rules: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PowerShellProfileConfig {
    pub enable_utf8_encoding: bool,
    pub init_starship: bool,
    pub init_zoxide: bool,
    pub aliases: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MirrorsProfileConfig {
    pub cargo_sparse_index: String,
    pub pip_index_url: String,
    pub npm_registry: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VSCodeProfileConfig {
    pub user_settings: serde_json::Value,
    pub extensions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageMatrixConfig {
    pub winget_core: Vec<WingetApp>,
    pub winget_dev: Vec<WingetApp>,
    #[serde(default)]
    pub store_apps: Vec<WingetApp>,
    pub scoop_tools: Vec<String>,
    pub cargo_packages: Vec<String>,
    pub npm_globals: Vec<String>,
    pub pip_packages: Vec<String>,
    pub uv_tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemTweaksProfileConfig {
    pub create_rollback_journal: bool,
    pub activate_ultimate_performance: bool,
    pub extreme_ac_power_settings: bool,
    pub disable_hibernation: bool,
    pub enable_trim: bool,
    pub optimize_storage: bool,
    pub disable_telemetry: bool,
    pub disable_bing_search: bool,
    pub disable_consumer_features: bool,
    pub disable_advertising_id: bool,
    pub disable_activity_history: bool,
    pub disable_feedback_prompts: bool,
    pub explorer_open_to_this_pc: bool,
    pub explorer_show_file_extensions: bool,
    pub explorer_show_hidden_files: bool,
    pub enable_ntfs_long_paths: bool,
    pub enable_developer_mode: bool,
    pub system_responsiveness: u32,
    pub network_throttling_index: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowsFeaturesProfileConfig {
    pub enable_wsl2: bool,
    pub enable_netfx3: bool,
    pub enable_windows_sandbox: bool,
    pub enable_hyper_v: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupProfile {
    pub profile_version: String,
    pub metadata: serde_json::Value,
    pub git_config: GitProfileConfig,
    pub powershell_profile: PowerShellProfileConfig,
    pub environment_mirrors: MirrorsProfileConfig,
    pub vscode_config: VSCodeProfileConfig,
    pub packages: PackageMatrixConfig,
    pub system_tweaks: SystemTweaksProfileConfig,
    pub windows_features: WindowsFeaturesProfileConfig,
    pub ollama_models: Vec<String>,
}

impl SetupProfile {
    pub fn load_default() -> Self {
        let json_bytes = include_bytes!("assets/setup_profile.json");
        serde_json::from_slice(json_bytes).unwrap_or_else(|e| {
            eprintln!("Failed to parse default setup_profile.json: {}", e);
            panic!("Invalid default setup_profile.json embedded payload");
        })
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.profile_version != "2.2.0" {
            anyhow::bail!("兼容部署仅支持 SetupProfile 2.2.0");
        }
        validate_winget_apps("winget_core", &self.packages.winget_core)?;
        validate_winget_apps("winget_dev", &self.packages.winget_dev)?;
        validate_store_apps(&self.packages.store_apps)?;
        validate_unique_strings("scoop_tools", &self.packages.scoop_tools)?;
        validate_unique_strings("cargo_packages", &self.packages.cargo_packages)?;
        validate_unique_strings("npm_globals", &self.packages.npm_globals)?;
        validate_unique_strings("pip_packages", &self.packages.pip_packages)?;
        validate_unique_strings("uv_tools", &self.packages.uv_tools)?;
        validate_unique_strings("vscode extensions", &self.vscode_config.extensions)?;
        validate_unique_strings("ollama_models", &self.ollama_models)?;
        if self.system_tweaks.system_responsiveness > 100 {
            anyhow::bail!("system_responsiveness 必须在 0..=100 范围内");
        }
        if self.system_tweaks.network_throttling_index > u32::MAX as u64 {
            anyhow::bail!("network_throttling_index 超出 DWORD 范围");
        }
        for (label, value) in [
            ("git.user_name", self.git_config.user_name.as_str()),
            ("git.user_email", self.git_config.user_email.as_str()),
            (
                "git.safe_directory",
                self.git_config.safe_directory.as_str(),
            ),
        ] {
            if value.len() > 1024 || value.chars().any(char::is_control) {
                anyhow::bail!("无效 Git 配置字段：{label}");
            }
        }
        if self.git_config.safe_directory.contains('*') {
            anyhow::bail!("不接受 safe.directory 全局通配符");
        }
        for rule in &self.git_config.global_gitignore_rules {
            if rule.len() > 1024 || rule.chars().any(char::is_control) {
                anyhow::bail!("Git ignore 必须为单行规则");
            }
        }
        for (alias, command) in &self.powershell_profile.aliases {
            if !plain_identifier(alias) || !plain_identifier(command) {
                anyhow::bail!("Shell alias 仅接受命令标识，不接受路径或脚本");
            }
        }
        validate_https_source(
            self.environment_mirrors
                .cargo_sparse_index
                .strip_prefix("sparse+")
                .ok_or_else(|| anyhow::anyhow!("Cargo 源必须使用 sparse+https"))?,
        )?;
        validate_https_source(&self.environment_mirrors.pip_index_url)?;
        validate_https_source(&self.environment_mirrors.npm_registry)?;
        if !self.vscode_config.user_settings.is_object() {
            anyhow::bail!("编辑器设置必须为 JSON object");
        }
        ParityRules::load_embedded()?.validate()?;
        Ok(())
    }

    pub fn parity_report(&self, inventory: &MacosInventory) -> anyhow::Result<ParityReport> {
        inventory.validate()?;
        let rules = ParityRules::load_embedded()?;
        rules.validate()?;
        let mut report = ParityReport::default();

        for source in &inventory.homebrew_formulae {
            self.classify_parity(&rules, SourceKind::Formula, source, &mut report);
        }
        for source in &inventory.homebrew_casks {
            self.classify_parity(&rules, SourceKind::Cask, source, &mut report);
        }
        for source in &inventory.cargo_packages {
            self.classify_parity(&rules, SourceKind::Cargo, source, &mut report);
        }
        for source in &inventory.npm_globals {
            self.classify_parity(&rules, SourceKind::Npm, source, &mut report);
        }
        for source in &inventory.uv_tools {
            self.classify_parity(&rules, SourceKind::Uv, source, &mut report);
        }
        Ok(report)
    }

    pub fn packages_for_inventory(
        &self,
        inventory: &MacosInventory,
    ) -> anyhow::Result<PackageMatrixConfig> {
        let report = self.parity_report(inventory)?;
        if !report.unmapped.is_empty() {
            anyhow::bail!("Mac 工具尚无可执行映射：{}", report.unmapped.join("；"));
        }
        let rules = ParityRules::load_embedded()?;
        let mut selected = std::collections::BTreeSet::new();
        for (kind, values) in [
            (SourceKind::Formula, &inventory.homebrew_formulae),
            (SourceKind::Cask, &inventory.homebrew_casks),
            (SourceKind::Cargo, &inventory.cargo_packages),
            (SourceKind::Npm, &inventory.npm_globals),
            (SourceKind::Uv, &inventory.uv_tools),
        ] {
            for source in values {
                let package = match rules.find(kind, source) {
                    Some(rule) if rule.provider == ParityProvider::Alternative => {
                        self.find_package(&rule.target, kind)
                    }
                    Some(rule) => self
                        .canonical_package(rule.provider, &rule.target)
                        .map(|target| (rule.provider, target)),
                    None => self.find_package(source, kind),
                };
                if let Some((provider, target)) = package {
                    selected.insert(package_key(provider, &target));
                }
            }
        }

        // Runtime providers are part of the plan, even when they are absent on a fresh LTSC.
        if selected.iter().any(|key| key.starts_with("Cargo:")) {
            selected.insert(package_key(ParityProvider::Winget, "Rustlang.Rustup"));
        }
        if selected.iter().any(|key| key.starts_with("Npm:")) {
            selected.insert(package_key(ParityProvider::Scoop, "nodejs-lts"));
        }
        if selected.iter().any(|key| key.starts_with("Pip:")) {
            selected.insert(package_key(ParityProvider::Scoop, "python"));
        }
        if selected.iter().any(|key| key.starts_with("Uv:")) {
            selected.insert(package_key(ParityProvider::Scoop, "uv"));
        }
        if selected.iter().any(|key| key.starts_with("Scoop:")) {
            selected.insert(package_key(ParityProvider::Winget, "Git.Git"));
        }
        if selected.contains(&package_key(ParityProvider::Scoop, "gradle")) {
            selected.insert(package_key(ParityProvider::Winget, "Microsoft.OpenJDK.21"));
        }

        let packages = &self.packages;
        let selected_strings = |provider, values: &Vec<String>| {
            values
                .iter()
                .filter(|value| selected.contains(&package_key(provider, value)))
                .cloned()
                .collect()
        };
        let selected_winget = |apps: &Vec<WingetApp>| {
            apps.iter()
                .filter(|app| selected.contains(&package_key(ParityProvider::Winget, &app.id)))
                .cloned()
                .collect()
        };
        let result = PackageMatrixConfig {
            winget_core: selected_winget(&packages.winget_core),
            winget_dev: selected_winget(&packages.winget_dev),
            store_apps: Vec::new(),
            scoop_tools: selected_strings(ParityProvider::Scoop, &packages.scoop_tools),
            cargo_packages: selected_strings(ParityProvider::Cargo, &packages.cargo_packages),
            npm_globals: selected_strings(ParityProvider::Npm, &packages.npm_globals),
            pip_packages: selected_strings(ParityProvider::Pip, &packages.pip_packages),
            uv_tools: selected_strings(ParityProvider::Uv, &packages.uv_tools),
        };
        for required in [
            (ParityProvider::Winget, "Rustlang.Rustup"),
            (ParityProvider::Winget, "Git.Git"),
            (ParityProvider::Winget, "Microsoft.OpenJDK.21"),
            (ParityProvider::Scoop, "nodejs-lts"),
            (ParityProvider::Scoop, "python"),
            (ParityProvider::Scoop, "uv"),
        ] {
            if selected.contains(&package_key(required.0, required.1))
                && !self.rule_target_exists(&ParityRule {
                    source_kind: SourceKind::Formula,
                    source: String::new(),
                    provider: required.0,
                    target: required.1.into(),
                    reason: String::new(),
                })
            {
                anyhow::bail!("缺少运行时依赖包：{}", required.1);
            }
        }
        Ok(result)
    }

    fn find_package(&self, name: &str, kind: SourceKind) -> Option<(ParityProvider, String)> {
        let preferred = match kind {
            SourceKind::Formula => [
                ParityProvider::Scoop,
                ParityProvider::Winget,
                ParityProvider::Cargo,
                ParityProvider::Npm,
                ParityProvider::Pip,
                ParityProvider::Uv,
            ],
            SourceKind::Cask => [
                ParityProvider::Winget,
                ParityProvider::Scoop,
                ParityProvider::Npm,
                ParityProvider::Cargo,
                ParityProvider::Pip,
                ParityProvider::Uv,
            ],
            SourceKind::Cargo => [
                ParityProvider::Cargo,
                ParityProvider::Scoop,
                ParityProvider::Winget,
                ParityProvider::Npm,
                ParityProvider::Pip,
                ParityProvider::Uv,
            ],
            SourceKind::Npm => [
                ParityProvider::Npm,
                ParityProvider::Scoop,
                ParityProvider::Winget,
                ParityProvider::Cargo,
                ParityProvider::Pip,
                ParityProvider::Uv,
            ],
            SourceKind::Uv => [
                ParityProvider::Uv,
                ParityProvider::Scoop,
                ParityProvider::Winget,
                ParityProvider::Cargo,
                ParityProvider::Npm,
                ParityProvider::Pip,
            ],
        };
        preferred.into_iter().find_map(|provider| {
            self.canonical_package(provider, name)
                .map(|target| (provider, target))
        })
    }

    fn canonical_package(&self, provider: ParityProvider, name: &str) -> Option<String> {
        match provider {
            ParityProvider::Winget => self
                .packages
                .winget_core
                .iter()
                .chain(&self.packages.winget_dev)
                .find(|app| {
                    app.id.eq_ignore_ascii_case(name)
                        || app.name.eq_ignore_ascii_case(name)
                        || app
                            .id
                            .rsplit('.')
                            .next()
                            .is_some_and(|tail| tail.eq_ignore_ascii_case(name))
                })
                .map(|app| app.id.clone()),
            ParityProvider::Scoop => canonical_string(&self.packages.scoop_tools, name),
            ParityProvider::Cargo => canonical_string(&self.packages.cargo_packages, name),
            ParityProvider::Npm => canonical_string(&self.packages.npm_globals, name),
            ParityProvider::Pip => canonical_string(&self.packages.pip_packages, name),
            ParityProvider::Uv => canonical_string(&self.packages.uv_tools, name),
            _ => None,
        }
    }

    fn classify_parity(
        &self,
        rules: &ParityRules,
        source_kind: SourceKind,
        source: &str,
        report: &mut ParityReport,
    ) {
        if self.has_package(source) {
            report.automatic += 1;
            return;
        }

        let Some(rule) = rules.find(source_kind, source) else {
            report.unmapped.push(format!("{source_kind:?}:{source}"));
            return;
        };
        match rule.provider {
            ParityProvider::Winget
            | ParityProvider::Scoop
            | ParityProvider::Cargo
            | ParityProvider::Npm
            | ParityProvider::Pip
            | ParityProvider::Uv => {
                if self.rule_target_exists(rule) {
                    report.automatic += 1;
                } else {
                    report.unmapped.push(format!(
                        "{source_kind:?}:{source} -> {:?}:{}（目标未进入安装矩阵）",
                        rule.provider, rule.target
                    ));
                }
            }
            ParityProvider::Alternative => {
                if self.has_package(&rule.target) {
                    report.compatible += 1;
                } else {
                    report.unmapped.push(format!(
                        "{source_kind:?}:{source} -> alternative:{}（目标未进入安装矩阵）",
                        rule.target
                    ));
                }
            }
            ParityProvider::WindowsBuiltIn => report.compatible += 1,
            ParityProvider::Wsl => report.manual.push(format!(
                "{source_kind:?}:{source} -> WSL:{}（需在 WSL 中另行安装）",
                rule.target
            )),
            ParityProvider::MacOnly => report.mac_only += 1,
            ParityProvider::Manual => report.manual.push(format!(
                "{source_kind:?}:{source} -> {}（{}）",
                rule.target, rule.reason
            )),
        }
    }

    fn rule_target_exists(&self, rule: &ParityRule) -> bool {
        match rule.provider {
            ParityProvider::Winget => self
                .packages
                .winget_core
                .iter()
                .chain(&self.packages.winget_dev)
                .any(|app| app.id.eq_ignore_ascii_case(&rule.target)),
            ParityProvider::Scoop => contains_ci(&self.packages.scoop_tools, &rule.target),
            ParityProvider::Cargo => contains_ci(&self.packages.cargo_packages, &rule.target),
            ParityProvider::Npm => contains_ci(&self.packages.npm_globals, &rule.target),
            ParityProvider::Pip => contains_ci(&self.packages.pip_packages, &rule.target),
            ParityProvider::Uv => contains_ci(&self.packages.uv_tools, &rule.target),
            _ => self.has_package(&rule.target),
        }
    }

    fn has_package(&self, name: &str) -> bool {
        let winget_match = self
            .packages
            .winget_core
            .iter()
            .chain(&self.packages.winget_dev)
            .any(|app| {
                app.id.eq_ignore_ascii_case(name)
                    || app.name.eq_ignore_ascii_case(name)
                    || app
                        .id
                        .rsplit('.')
                        .next()
                        .is_some_and(|tail| tail.eq_ignore_ascii_case(name))
            });
        winget_match
            || contains_ci(&self.packages.scoop_tools, name)
            || contains_ci(&self.packages.cargo_packages, name)
            || contains_ci(&self.packages.npm_globals, name)
            || contains_ci(&self.packages.pip_packages, name)
            || contains_ci(&self.packages.uv_tools, name)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParityReport {
    pub automatic: usize,
    pub compatible: usize,
    pub mac_only: usize,
    pub manual: Vec<String>,
    pub unmapped: Vec<String>,
}

impl ParityReport {
    pub fn covered(&self) -> usize {
        self.automatic + self.compatible + self.mac_only + self.manual.len()
    }
}

fn contains_ci(values: &[String], needle: &str) -> bool {
    values
        .iter()
        .any(|value| value.eq_ignore_ascii_case(needle))
}

fn canonical_string(values: &[String], needle: &str) -> Option<String> {
    values
        .iter()
        .find(|value| value.eq_ignore_ascii_case(needle))
        .cloned()
}

fn package_key(provider: ParityProvider, target: &str) -> String {
    format!("{provider:?}:{}", target.to_ascii_lowercase())
}

fn validate_winget_apps(label: &str, apps: &[WingetApp]) -> anyhow::Result<()> {
    let mut ids = std::collections::BTreeSet::new();
    for app in apps {
        if app.id.trim().is_empty() || app.name.trim().is_empty() {
            anyhow::bail!("{label} 中存在空的 id 或 name");
        }
        if !plain_identifier(&app.id) {
            anyhow::bail!("{label} 中包含无效包 ID: {}", app.id);
        }
        if !ids.insert(app.id.to_ascii_lowercase()) {
            anyhow::bail!("{label} 中存在重复包 ID: {}", app.id);
        }
    }
    Ok(())
}

fn validate_store_apps(apps: &[WingetApp]) -> anyhow::Result<()> {
    let mut ids = std::collections::BTreeSet::new();
    for app in apps {
        anyhow::ensure!(
            app.id.len() == 12 && app.id.bytes().all(|ch| ch.is_ascii_alphanumeric()),
            "Store product ID must contain exactly 12 ASCII letters or digits: {}",
            app.id
        );
        anyhow::ensure!(
            !app.name.trim().is_empty()
                && app.name.len() <= 1024
                && !app.name.chars().any(char::is_control),
            "Invalid Store product name"
        );
        anyhow::ensure!(
            ids.insert(app.id.to_ascii_lowercase()),
            "Duplicate Store product ID: {}",
            app.id
        );
    }
    Ok(())
}

fn validate_unique_strings(label: &str, values: &[String]) -> anyhow::Result<()> {
    let mut seen = std::collections::BTreeSet::new();
    for value in values {
        if value.trim().is_empty() {
            anyhow::bail!("{label} 中存在空项目");
        }
        let valid = match label {
            "npm_globals" => npm_identifier(value),
            "ollama_models" => model_identifier(value),
            "vscode extensions" => plain_identifier(value) && value.contains('.'),
            _ => plain_identifier(value),
        };
        if !valid {
            anyhow::bail!("{label} 中包含不安全的命令参数字符: {value}");
        }
        if !seen.insert(value.to_ascii_lowercase()) {
            anyhow::bail!("{label} 中存在重复项目: {value}");
        }
    }
    Ok(())
}

fn plain_identifier(value: &str) -> bool {
    value.len() <= 256
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || b"-._+".contains(&ch))
}

fn npm_identifier(value: &str) -> bool {
    match value.strip_prefix('@') {
        Some(scoped) => scoped
            .split_once('/')
            .is_some_and(|(scope, name)| plain_identifier(scope) && plain_identifier(name)),
        None => plain_identifier(value),
    }
}

fn model_identifier(value: &str) -> bool {
    if value.len() > 256 {
        return false;
    }
    let (name, tag) = value
        .split_once(':')
        .map_or((value, None), |(name, tag)| (name, Some(tag)));
    name.split('/').count() <= 2
        && name.split('/').all(plain_identifier)
        && tag.is_none_or(plain_identifier)
}

fn validate_https_source(value: &str) -> anyhow::Result<()> {
    let Some(rest) = value.strip_prefix("https://") else {
        anyhow::bail!("软件源必须为 HTTPS");
    };
    let authority = rest.split('/').next().unwrap_or("");
    if authority.is_empty()
        || authority.starts_with(['.', '-'])
        || value.len() > 2048
        || rest.contains("://")
        || !value
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || b"-._~:/%+".contains(&ch))
    {
        anyhow::bail!("软件源不接受凭据、查询参数、片段或配置注入");
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupConfig {
    pub include_curated_extras: bool,
    pub include_dev_tools: bool,
    pub include_agent_skills: bool,
    pub include_docker_wsl: bool,
    pub include_storage_optimization: bool,
    pub include_deep_win_tweaks: bool,
    pub include_git_shell_configs: bool,
    pub include_vscode_extensions: bool,
    pub include_npmrc_config: bool,
    pub include_ollama_models: bool,
    pub network_mode: NetworkMode,
    pub target_mode: ExecutionTarget,
    pub profile: SetupProfile,
    pub source_inventory: MacosInventory,
}

impl Default for SetupConfig {
    fn default() -> Self {
        Self {
            include_curated_extras: false,
            include_dev_tools: true,
            include_agent_skills: false,
            include_docker_wsl: true,
            include_storage_optimization: false,
            include_deep_win_tweaks: false,
            include_git_shell_configs: false,
            include_vscode_extensions: false,
            include_npmrc_config: false,
            include_ollama_models: false,
            network_mode: NetworkMode::Basic,
            target_mode: ExecutionTarget::DevToolsOnly,
            profile: SetupProfile::load_default(),
            source_inventory: MacosInventory::load_embedded().unwrap_or_default(),
        }
    }
}

impl SetupConfig {
    pub fn resolved(&self) -> anyhow::Result<Self> {
        // Studio reviews the selected profile directly; no personal inventory filters it.
        self.profile.validate()?;
        Ok(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::SetupProfile;

    #[test]
    fn embedded_profile_is_valid() {
        let profile = SetupProfile::load_default();
        profile.validate().unwrap();
        let audit = &profile.metadata["provider_audit"];
        assert_eq!(
            audit["winget_manifest_ids_verified"].as_u64(),
            Some((profile.packages.winget_core.len() + profile.packages.winget_dev.len()) as u64)
        );
        assert_eq!(
            audit["scoop_manifests_verified"].as_u64(),
            Some(profile.packages.scoop_tools.len() as u64)
        );
    }

    #[test]
    fn custom_sources_aliases_and_package_arguments_are_bounded() {
        for value in [
            "--upgrade",
            "../package",
            "https://example.invalid/x",
            "C:/package",
            "tool;cmd",
        ] {
            let mut profile = SetupProfile::load_default();
            profile.packages.scoop_tools = vec![value.into()];
            assert!(profile.validate().is_err(), "{value}");
        }
        for value in [
            "http://example.invalid/simple",
            "https://user:secret@example.invalid/simple",
            "https://example.invalid/\nextra-index-url=bad",
        ] {
            let mut profile = SetupProfile::load_default();
            profile.environment_mirrors.pip_index_url = value.into();
            assert!(profile.validate().is_err(), "{value}");
        }
        let mut profile = SetupProfile::load_default();
        profile
            .powershell_profile
            .aliases
            .insert("g".into(), "git;cmd".into());
        assert!(profile.validate().is_err());
        profile.powershell_profile.aliases.clear();
        profile.packages.npm_globals = vec!["@scope/package".into()];
        profile.ollama_models = vec!["library/model:beta".into()];
        profile.validate().unwrap();
    }

    #[test]
    fn duplicate_package_ids_are_rejected() {
        let mut profile = SetupProfile::load_default();
        profile
            .packages
            .winget_core
            .push(profile.packages.winget_core[0].clone());

        let error = profile.validate().unwrap_err().to_string();
        assert!(error.contains("重复包 ID"));
    }

    #[test]
    fn source_inventory_has_no_silent_unmapped_tools() {
        let profile = SetupProfile::load_default();
        let inventory = crate::inventory::MacosInventory::load_embedded().unwrap();
        let report = profile.parity_report(&inventory).unwrap();
        assert!(
            report.unmapped.is_empty(),
            "unmapped source tools: {:?}",
            report.unmapped
        );
        assert_eq!(report.covered(), inventory.item_count());
    }

    #[test]
    fn source_plan_installs_mapped_tools_without_unrelated_catalog_apps() {
        let mut profile = SetupProfile::load_default();
        let inventory = crate::inventory::MacosInventory::load_embedded().unwrap();
        profile.packages = profile.packages_for_inventory(&inventory).unwrap();
        assert!(
            profile
                .parity_report(&inventory)
                .unwrap()
                .unmapped
                .is_empty()
        );
        assert!(
            profile
                .packages
                .winget_dev
                .iter()
                .any(|app| app.id == "Git.Git")
        );
        assert!(profile.packages.scoop_tools.iter().any(|name| name == "uv"));
        assert!(
            profile
                .packages
                .npm_globals
                .iter()
                .any(|name| name == "@moonshot-ai/kimi-code")
        );
        assert!(
            profile
                .packages
                .uv_tools
                .iter()
                .any(|name| name == "serena-agent")
        );
        assert!(
            !profile
                .packages
                .winget_dev
                .iter()
                .any(|app| app.id == "Valve.Steam")
        );
        assert!(
            profile.packages.npm_globals.len()
                < SetupProfile::load_default().packages.npm_globals.len()
        );
    }

    #[test]
    fn gradle_runtime_and_ui_cli_alias_are_selected_without_conflicts() {
        let profile = SetupProfile::load_default();
        let mut inventory = crate::inventory::MacosInventory {
            schema_version: 1,
            homebrew_formulae: vec!["gradle".into()],
            ..Default::default()
        };
        let plan = profile.packages_for_inventory(&inventory).unwrap();
        assert!(
            plan.winget_dev
                .iter()
                .any(|app| app.id == "Microsoft.OpenJDK.21")
        );

        inventory.homebrew_formulae.clear();
        inventory.npm_globals = vec!["uipro-cli".into(), "ui-ux-pro-max-cli".into()];
        let plan = profile.packages_for_inventory(&inventory).unwrap();
        assert_eq!(plan.npm_globals, ["ui-ux-pro-max-cli"]);
        assert!(
            !plan
                .winget_dev
                .iter()
                .any(|app| app.id == "Microsoft.OpenJDK.21")
        );
    }

    #[test]
    fn retired_scoop_jhead_remains_a_visible_manual_item() {
        let profile = SetupProfile::load_default();
        let inventory = crate::inventory::MacosInventory {
            schema_version: 1,
            homebrew_formulae: vec!["jhead".into()],
            ..Default::default()
        };
        let report = profile.parity_report(&inventory).unwrap();
        assert_eq!(report.automatic, 0);
        assert_eq!(report.covered(), 1);
        assert!(report.unmapped.is_empty());
        assert_eq!(report.manual.len(), 1);
        assert!(report.manual[0].contains("jhead"));
        assert!(
            !profile
                .packages_for_inventory(&inventory)
                .unwrap()
                .scoop_tools
                .iter()
                .any(|package| package == "jhead")
        );
    }

    #[test]
    fn wsl_equivalents_are_not_counted_as_installed() {
        let profile = SetupProfile::load_default();
        let mut inventory = crate::inventory::MacosInventory::load_embedded().unwrap();
        inventory.homebrew_formulae = vec!["fish".into()];
        inventory.homebrew_casks.clear();
        inventory.cargo_packages.clear();
        inventory.npm_globals.clear();
        inventory.uv_tools.clear();
        let report = profile.parity_report(&inventory).unwrap();
        assert_eq!(report.compatible, 0);
        assert_eq!(report.manual.len(), 1);
    }
}
