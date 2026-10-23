#[cfg(target_os = "macos")]
use crate::utils::{CancellationToken, run_native_cmd_timeout};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};

pub const DEFAULT_INVENTORY_URL: &str = "https://raw.githubusercontent.com/nowaytouse/LTSC_Tools_Backup/main/src/assets/macos_inventory.json";
#[cfg(any(target_os = "windows", test))]
const GITHUB_API_INVENTORY_URL: &str = "https://api.github.com/repos/nowaytouse/LTSC_Tools_Backup/contents/src/assets/macos_inventory.json?ref=main";
const MAX_INVENTORY_BYTES: u64 = 1_048_576;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MacosInventory {
    pub schema_version: u32,
    pub captured_at: String,
    pub homebrew_formulae: Vec<String>,
    pub homebrew_casks: Vec<String>,
    pub homebrew_taps: Vec<String>,
    pub cargo_packages: Vec<String>,
    pub npm_globals: Vec<String>,
    pub uv_tools: Vec<String>,
    pub vscode_extensions: Vec<String>,
    pub cursor_extensions: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Formula,
    Cask,
    Cargo,
    Npm,
    Uv,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ParityProvider {
    Winget,
    Scoop,
    Cargo,
    Npm,
    Pip,
    Uv,
    WindowsBuiltIn,
    Wsl,
    Alternative,
    MacOnly,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ParityRule {
    pub source_kind: SourceKind,
    pub source: String,
    pub provider: ParityProvider,
    pub target: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ParityRules {
    pub schema_version: u32,
    pub rules: Vec<ParityRule>,
}

impl ParityRules {
    pub fn load_embedded() -> anyhow::Result<Self> {
        Ok(serde_json::from_slice(include_bytes!(
            "assets/parity_rules.json"
        ))?)
    }

    pub fn find(&self, source_kind: SourceKind, source: &str) -> Option<&ParityRule> {
        self.rules.iter().find(|rule| {
            rule.source_kind == source_kind && rule.source.eq_ignore_ascii_case(source)
        })
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.schema_version != 1 {
            anyhow::bail!("不支持的 parity_rules schema：{}", self.schema_version);
        }
        let mut keys = BTreeSet::new();
        for rule in &self.rules {
            if rule.source.trim().is_empty()
                || rule.target.trim().is_empty()
                || rule.reason.trim().is_empty()
            {
                anyhow::bail!("parity_rules 包含空字段：{:?}", rule);
            }
            let key = format!(
                "{:?}:{}",
                rule.source_kind,
                rule.source.to_ascii_lowercase()
            );
            if !keys.insert(key) {
                anyhow::bail!(
                    "parity_rules 包含重复映射：{:?}/{}",
                    rule.source_kind,
                    rule.source
                );
            }
        }
        Ok(())
    }
}

impl MacosInventory {
    pub fn load_embedded() -> anyhow::Result<Self> {
        Ok(serde_json::from_slice(include_bytes!(
            "assets/macos_inventory.json"
        ))?)
    }

    pub fn load_file(path: &Path) -> anyhow::Result<Self> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(MAX_INVENTORY_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_INVENTORY_BYTES {
            anyhow::bail!("Mac 清单超过 1 MiB：{}", path.display());
        }
        let inventory: Self = serde_json::from_slice(&bytes)?;
        inventory.validate()?;
        Ok(inventory)
    }

    pub fn cache_path() -> PathBuf {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("LTSCWorkspace")
            .join("macos_inventory.json")
    }

    pub fn source_url_path() -> PathBuf {
        Self::cache_path().with_file_name("inventory_source_url.txt")
    }

    pub fn load_cached_or_embedded() -> anyhow::Result<Self> {
        let path = Self::cache_path();
        let inventory = if path.exists() {
            Self::load_file(&path)
        } else {
            let inventory = Self::load_embedded()?;
            inventory.validate()?;
            Ok(inventory)
        }?;
        inventory.ensure_supported()?;
        Ok(inventory)
    }

    fn ensure_supported(&self) -> anyhow::Result<()> {
        crate::config::SetupProfile::load_default()
            .packages_for_inventory(self)
            .map(|_| ())
            .map_err(|error| {
                anyhow::anyhow!("当前 Windows 程序无法部署这份 Mac 清单，请先更新程序：{error}")
            })
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.schema_version != 1 || self.item_count() == 0 {
            anyhow::bail!("Mac 清单版本无效或没有工具项目");
        }
        for (label, values) in [
            ("formula", &self.homebrew_formulae),
            ("cask", &self.homebrew_casks),
            ("Cargo", &self.cargo_packages),
            ("NPM", &self.npm_globals),
            ("UV", &self.uv_tools),
        ] {
            let mut seen = BTreeSet::new();
            for value in values {
                if value.is_empty()
                    || value.len() > 256
                    || (matches!(label, "formula" | "cask") && value.contains('/'))
                    || value
                        .chars()
                        .any(|ch| ch.is_control() || ch.is_whitespace())
                    || !seen.insert(value.to_ascii_lowercase())
                {
                    anyhow::bail!("Mac 清单中的 {label} 项无效或重复：{value}");
                }
            }
        }
        Ok(())
    }

    #[cfg(target_os = "windows")]
    pub fn fetch_from_url(
        url: &str,
        cancellation: &crate::utils::CancellationToken,
    ) -> anyhow::Result<Self> {
        Self::fetch_with_downloader(url, &Self::cache_path(), cancellation, |source, path| {
            let output = path.to_string_lossy().into_owned();
            let mut args = vec![
                "--fail",
                "--location",
                "--silent",
                "--show-error",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--connect-timeout",
                "8",
                "--max-time",
                "20",
                "--max-filesize",
                "1048576",
                "--user-agent",
                "LTSCWorkspace/2.2",
                "--url",
                source,
                "--output",
                &output,
            ];
            if source == GITHUB_API_INVENTORY_URL {
                args.extend(["--header", "Accept: application/vnd.github.raw+json"]);
            }
            let result = crate::utils::run_native_cmd_timeout("curl.exe", &args, 25, cancellation);
            if !result.succeeded() {
                anyhow::bail!("{}", result.diagnostic());
            }
            Ok(())
        })
    }

    #[cfg(any(target_os = "windows", test))]
    fn fetch_with_downloader(
        url: &str,
        destination: &Path,
        cancellation: &crate::utils::CancellationToken,
        mut download: impl FnMut(&str, &Path) -> anyhow::Result<()>,
    ) -> anyhow::Result<Self> {
        Self::validate_inventory_url(url)?;
        let sources: &[(&str, &str)] = if url == DEFAULT_INVENTORY_URL {
            &[
                ("GitHub Raw", DEFAULT_INVENTORY_URL),
                ("GitHub API", GITHUB_API_INVENTORY_URL),
            ]
        } else {
            &[("自定义 HTTPS 地址", url)]
        };
        if cancellation.is_cancelled() {
            anyhow::bail!("清单更新已取消；仍保留原清单");
        }
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let temporary =
            destination.with_extension(format!("{}-{stamp}.download", std::process::id()));
        let mut failures = Vec::new();
        for (label, source) in sources {
            if cancellation.is_cancelled() {
                anyhow::bail!("清单更新已取消；仍保留原清单");
            }
            let result = download(source, &temporary);
            if cancellation.is_cancelled() {
                let _ = std::fs::remove_file(&temporary);
                anyhow::bail!("清单更新已取消；仍保留原清单");
            }
            let result = result.and_then(|()| Self::adopt_file(&temporary, destination));
            let _ = std::fs::remove_file(&temporary);
            match result {
                Ok(inventory) => return Ok(inventory),
                Err(error) => failures.push(format!("{label}：{error}")),
            }
        }
        anyhow::bail!(
            "清单更新失败，仍保留原清单。可改用可访问的 HTTPS 地址或导入本地 JSON。{}",
            failures.join("；")
        )
    }

    #[cfg(any(target_os = "windows", test))]
    pub fn adopt_file(source: &Path, destination: &Path) -> anyhow::Result<Self> {
        let inventory = Self::load_file(source)?;
        inventory.ensure_supported()?;
        inventory.save_atomic(destination)?;
        Ok(inventory)
    }

    pub fn validate_inventory_url(url: &str) -> anyhow::Result<()> {
        let Some(authority) = url.strip_prefix("https://") else {
            anyhow::bail!("清单地址必须使用 HTTPS");
        };
        let host = authority.split(['/', '?', '#']).next().unwrap_or_default();
        if url.len() > 2048
            || host.is_empty()
            || host.contains('@')
            || url
                .chars()
                .any(|character| character.is_whitespace() || character.is_control())
        {
            anyhow::bail!("清单 HTTPS 地址无效");
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    pub fn capture() -> anyhow::Result<Self> {
        let cancellation = CancellationToken::default();
        let mut inventory = Self {
            schema_version: 1,
            captured_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            ..Self::default()
        };

        inventory.homebrew_formulae =
            required_lines("Homebrew formulae", "brew", &["leaves"], &cancellation)?
                .into_iter()
                .map(|name| name.rsplit('/').next().unwrap_or(&name).to_string())
                .collect();
        inventory.homebrew_casks = optional_lines(
            "Homebrew casks",
            "brew",
            &["list", "--cask"],
            &cancellation,
            &mut inventory.warnings,
        )
        .into_iter()
        .map(|name| name.rsplit('/').next().unwrap_or(&name).to_string())
        .collect();
        // Tap names and tap-qualified formula names can disclose private namespaces.
        inventory.homebrew_taps.clear();
        inventory
            .warnings
            .push("Homebrew tap 名称/前缀未写入公开快照；仅保留 formula/cask 名称".into());

        let cargo_output = optional_output(
            "Cargo tools",
            "cargo",
            &["install", "--list"],
            &cancellation,
            &mut inventory.warnings,
        );
        inventory.cargo_packages = parse_cargo_install_list(&cargo_output);

        let npm_output = optional_output(
            "NPM globals",
            "npm",
            &["--global", "ls", "--depth=0", "--json"],
            &cancellation,
            &mut inventory.warnings,
        );
        inventory.npm_globals = parse_npm_globals(&npm_output).unwrap_or_else(|error| {
            inventory
                .warnings
                .push(format!("NPM globals 解析失败：{error}"));
            Vec::new()
        });

        let uv_output = optional_output(
            "uv tools",
            "uv",
            &["tool", "list"],
            &cancellation,
            &mut inventory.warnings,
        );
        inventory.uv_tools = parse_uv_tool_list(&uv_output);

        inventory.vscode_extensions = optional_lines(
            "VS Code extensions",
            "code",
            &["--list-extensions"],
            &cancellation,
            &mut inventory.warnings,
        );
        inventory.cursor_extensions = optional_lines(
            "Cursor extensions",
            "cursor",
            &["--list-extensions"],
            &cancellation,
            &mut inventory.warnings,
        );

        inventory.normalize();
        Ok(inventory)
    }

    pub fn save_atomic(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(self)?;
        crate::utils::write_atomic(path, &bytes)?;
        Ok(())
    }

    pub fn item_count(&self) -> usize {
        self.homebrew_formulae.len()
            + self.homebrew_casks.len()
            + self.cargo_packages.len()
            + self.npm_globals.len()
            + self.uv_tools.len()
    }

    #[cfg(target_os = "macos")]
    fn normalize(&mut self) {
        normalize_list(&mut self.homebrew_formulae);
        normalize_list(&mut self.homebrew_casks);
        normalize_list(&mut self.homebrew_taps);
        normalize_list(&mut self.cargo_packages);
        normalize_list(&mut self.npm_globals);
        normalize_list(&mut self.uv_tools);
        normalize_list(&mut self.vscode_extensions);
        normalize_list(&mut self.cursor_extensions);
        normalize_list(&mut self.warnings);
    }
}

#[cfg(target_os = "macos")]
fn required_lines(
    label: &str,
    program: &str,
    args: &[&str],
    cancellation: &CancellationToken,
) -> anyhow::Result<Vec<String>> {
    let result = run_native_cmd_timeout(program, args, 90, cancellation);
    if !result.succeeded() {
        anyhow::bail!("{label} 采集失败：{}", result.diagnostic());
    }
    Ok(parse_nonempty_lines(&result.output))
}

#[cfg(target_os = "macos")]
fn optional_lines(
    label: &str,
    program: &str,
    args: &[&str],
    cancellation: &CancellationToken,
    warnings: &mut Vec<String>,
) -> Vec<String> {
    parse_nonempty_lines(&optional_output(
        label,
        program,
        args,
        cancellation,
        warnings,
    ))
}

#[cfg(target_os = "macos")]
fn optional_output(
    label: &str,
    program: &str,
    args: &[&str],
    cancellation: &CancellationToken,
    warnings: &mut Vec<String>,
) -> String {
    let result = run_native_cmd_timeout(program, args, 90, cancellation);
    if result.succeeded() {
        result.output
    } else {
        warnings.push(format!("{label} 未采集：{}", result.diagnostic()));
        String::new()
    }
}

#[cfg(target_os = "macos")]
fn normalize_list(values: &mut Vec<String>) {
    let unique: BTreeSet<_> = values
        .drain(..)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect();
    *values = unique.into_iter().collect();
}

#[cfg(target_os = "macos")]
fn parse_nonempty_lines(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(any(target_os = "macos", test))]
fn parse_cargo_install_list(output: &str) -> Vec<String> {
    output
        .lines()
        .filter(|line| !line.starts_with(char::is_whitespace))
        .filter_map(|line| line.trim_end_matches(':').split_once(" v"))
        .map(|(name, _)| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect()
}

#[cfg(any(target_os = "macos", test))]
fn parse_npm_globals(output: &str) -> anyhow::Result<Vec<String>> {
    if output.trim().is_empty() {
        return Ok(Vec::new());
    }
    let value: serde_json::Value = serde_json::from_str(output)?;
    let dependencies = value
        .get("dependencies")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("缺少 dependencies 对象"))?;
    Ok(dependencies.keys().cloned().collect())
}

#[cfg(any(target_os = "macos", test))]
fn parse_uv_tool_list(output: &str) -> Vec<String> {
    output
        .lines()
        .filter(|line| !line.starts_with(char::is_whitespace) && !line.starts_with('-'))
        .filter_map(|line| {
            line.split_once(" v")
                .map(|(name, _)| name.trim().to_string())
        })
        .filter(|name| !name.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        MacosInventory, ParityRules, parse_cargo_install_list, parse_npm_globals,
        parse_uv_tool_list,
    };

    #[test]
    fn provider_outputs_are_parsed_without_versions_or_binaries() {
        assert_eq!(
            parse_cargo_install_list(
                "cargo-audit v0.22.2:\n    cargo-audit\nrtk v0.42.4 (git):\n    rtk\n"
            ),
            ["cargo-audit", "rtk"]
        );
        assert_eq!(
            parse_npm_globals(
                r#"{"dependencies":{"typescript":{"version":"7"},"@openai/codex":{"version":"1"}}}"#
            )
            .unwrap(),
            ["@openai/codex", "typescript"]
        );
        assert_eq!(
            parse_uv_tool_list("kimi-cli v1.49.0\n- kimi\nosxphotos v0.76.1\n- osxphotos\n"),
            ["kimi-cli", "osxphotos"]
        );
    }

    #[test]
    fn embedded_inventory_is_valid_and_private() {
        let inventory = MacosInventory::load_embedded().unwrap();
        inventory.validate().unwrap();
        assert_eq!(inventory.schema_version, 1);
        assert!(!inventory.captured_at.is_empty());
        assert!(!inventory.homebrew_formulae.is_empty());
        let serialized = serde_json::to_string(&inventory).unwrap();
        assert!(!serialized.contains("/Users/"));
        assert!(!serialized.contains("hostname"));
        assert!(inventory.homebrew_taps.is_empty());
    }

    #[test]
    fn invalid_inventory_cannot_replace_the_deployment_plan() {
        let mut inventory = MacosInventory::load_embedded().unwrap();
        inventory
            .homebrew_formulae
            .push(inventory.homebrew_formulae[0].clone());
        assert!(inventory.validate().is_err());
        inventory.homebrew_formulae.pop();
        inventory.homebrew_formulae.push("private/tap/tool".into());
        assert!(inventory.validate().is_err());
    }

    #[test]
    fn unmapped_download_keeps_the_last_usable_cache() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "ltsc-inventory-cache-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let destination = root.join("macos_inventory.json");
        let download = root.join("download.json");
        let current = MacosInventory::load_embedded().unwrap();
        current.save_atomic(&destination).unwrap();
        let previous_bytes = std::fs::read(&destination).unwrap();

        let mut incompatible = current.clone();
        incompatible
            .homebrew_formulae
            .push("ltsc-unmapped-sentinel".into());
        incompatible.save_atomic(&download).unwrap();
        assert!(MacosInventory::adopt_file(&download, &destination).is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), previous_bytes);

        std::fs::File::create(&download)
            .unwrap()
            .set_len(super::MAX_INVENTORY_BYTES + 1)
            .unwrap();
        assert!(
            MacosInventory::adopt_file(&download, &destination)
                .unwrap_err()
                .to_string()
                .contains("1 MiB")
        );
        assert_eq!(std::fs::read(&destination).unwrap(), previous_bytes);

        let mut compatible = current;
        compatible.captured_at = "2026-09-27T00:00:00Z".into();
        compatible.save_atomic(&download).unwrap();
        assert_eq!(
            MacosInventory::adopt_file(&download, &destination).unwrap(),
            compatible
        );
        assert_eq!(MacosInventory::load_file(&destination).unwrap(), compatible);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inventory_download_fallback_preserves_cache_and_honors_cancellation() {
        use super::{DEFAULT_INVENTORY_URL, GITHUB_API_INVENTORY_URL};
        use crate::utils::CancellationToken;

        for (scenario, succeeds, attempts) in [
            ("raw-blocked", true, 2),
            ("raw-html", true, 2),
            ("raw-unmapped", true, 2),
            ("raw-valid", true, 1),
            ("both-fail", false, 2),
            ("custom-fail", false, 1),
            ("cancel-before", false, 0),
            ("cancel-during", false, 1),
        ] {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "ltsc-inventory-{scenario}-{}-{stamp}",
                std::process::id()
            ));
            let destination = root.join("macos_inventory.json");
            let current = MacosInventory::load_embedded().unwrap();
            current.save_atomic(&destination).unwrap();
            let previous = std::fs::read(&destination).unwrap();
            let mut updated = current.clone();
            updated.captured_at = "2026-10-01T00:00:00Z".into();
            let cancellation = CancellationToken::default();
            if scenario == "cancel-before" {
                cancellation.cancel();
            }
            let url = if scenario == "custom-fail" {
                "https://example.com/inventory.json?token=private"
            } else {
                DEFAULT_INVENTORY_URL
            };
            let mut called = Vec::new();
            let mut temporary_paths = Vec::new();
            let result = MacosInventory::fetch_with_downloader(
                url,
                &destination,
                &cancellation,
                |source, temporary| {
                    called.push(source.to_string());
                    temporary_paths.push(temporary.to_path_buf());
                    if scenario == "cancel-during" {
                        updated.save_atomic(temporary)?;
                        cancellation.cancel();
                    } else if matches!(scenario, "both-fail" | "custom-fail")
                        || (source == DEFAULT_INVENTORY_URL && scenario == "raw-blocked")
                    {
                        std::fs::write(temporary, b"partial")?;
                        anyhow::bail!("network unavailable");
                    } else if source == DEFAULT_INVENTORY_URL && scenario == "raw-html" {
                        std::fs::write(temporary, b"<html>blocked</html>")?;
                    } else if source == DEFAULT_INVENTORY_URL && scenario == "raw-unmapped" {
                        let mut incompatible = updated.clone();
                        incompatible
                            .homebrew_formulae
                            .push("ltsc-unmapped-sentinel".into());
                        incompatible.save_atomic(temporary)?;
                    } else {
                        updated.save_atomic(temporary)?;
                    }
                    Ok(())
                },
            );
            assert_eq!(result.is_ok(), succeeds, "{scenario}: {result:?}");
            assert_eq!(called.len(), attempts, "{scenario}");
            if called.len() == 2 {
                assert_eq!(called, [DEFAULT_INVENTORY_URL, GITHUB_API_INVENTORY_URL]);
            }
            assert!(temporary_paths.iter().all(|path| !path.exists()));
            if succeeds {
                assert_eq!(result.unwrap(), updated);
                assert_eq!(MacosInventory::load_file(&destination).unwrap(), updated);
            } else {
                let error = result.unwrap_err().to_string();
                assert!(!error.contains("token=private"));
                if scenario == "both-fail" {
                    assert!(error.contains("GitHub Raw") && error.contains("GitHub API"));
                }
                assert_eq!(std::fs::read(&destination).unwrap(), previous, "{scenario}");
            }
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn inventory_save_preserves_an_existing_staging_file() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "ltsc-inventory-staging-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let destination = root.join("inventory.json");
        let unrelated = destination.with_extension("json.tmp");
        std::fs::write(&unrelated, b"another writer's data").unwrap();
        let inventory = MacosInventory::load_embedded().unwrap();
        inventory.save_atomic(&destination).unwrap();
        assert_eq!(std::fs::read(&unrelated).unwrap(), b"another writer's data");
        assert_eq!(MacosInventory::load_file(&destination).unwrap(), inventory);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inventory_source_requires_a_valid_https_url() {
        assert!(
            MacosInventory::validate_inventory_url("https://example.com/path/macos_inventory.json")
                .is_ok()
        );
        assert!(
            MacosInventory::validate_inventory_url("http://example.com/inventory.json").is_err()
        );
        assert!(MacosInventory::validate_inventory_url("https:///inventory.json").is_err());
        assert!(
            MacosInventory::validate_inventory_url("https://user@example.com/inventory.json")
                .is_err()
        );
        assert!(
            MacosInventory::validate_inventory_url("https://example.com/path with spaces").is_err()
        );
    }

    #[test]
    fn embedded_parity_rules_are_valid() {
        ParityRules::load_embedded().unwrap().validate().unwrap();
    }
}
