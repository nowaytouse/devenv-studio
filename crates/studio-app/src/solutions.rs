//! Mixed workflows resolve against trusted catalogs, never imported command text.
use anyhow::{Context, Result, ensure};
use std::collections::BTreeSet;
use studio_core::{Catalog, Target};

pub struct Recipe {
    pub id: &'static str,
    pub title: [&'static str; 2],
    pub description: [&'static str; 2],
    pub settings: &'static [&'static str],
    pub packages: &'static [(&'static str, &'static str)],
}
pub const RECIPES: &[Recipe] = &[
    Recipe {
        id: "daily",
        title: ["日常桌面", "Everyday desktop"],
        description: [
            "组合 Explorer 与隐私偏好，以及浏览器、压缩和媒体工具。",
            "Combine Explorer and privacy preferences with a browser, archive and media tools.",
        ],
        settings: &[
            "explorer-extensions",
            "explorer-open-this-pc",
            "privacy-advertising-id",
        ],
        packages: &[
            ("package", "7zip.7zip"),
            ("package", "Mozilla.Firefox"),
            ("package", "VideoLAN.VLC"),
        ],
    },
    Recipe {
        id: "rust",
        title: ["Rust 开发环境", "Rust development"],
        description: [
            "选择 Git、VS Code、Rust MSVC、命令行工具与编辑器扩展。链接器及 Windows SDK 仍需按工具链提示配置。",
            "Select Git, VS Code, Rust MSVC, CLI tools and editor extensions. Configure the linker and Windows SDK as instructed by the toolchain.",
        ],
        settings: &["explorer-extensions", "explorer-open-this-pc"],
        packages: &[
            ("package", "Git.Git"),
            ("package", "Microsoft.VisualStudioCode"),
            ("package", "Rustlang.Rust.MSVC"),
            ("cargo", "ripgrep"),
            ("cargo", "fd-find"),
            ("cargo", "bat"),
            ("cargo", "just"),
            ("vscode", "rust-lang.rust-analyzer"),
            ("vscode", "tamasfe.even-better-toml"),
        ],
    },
    Recipe {
        id: "python",
        title: ["Python 工具环境", "Python tools"],
        description: [
            "选择 Git、VS Code、uv、代码检查与 Python 编辑器扩展。需要已经可用且兼容的 Python；不会自动下载解释器。",
            "Select Git, VS Code, uv, lint tools and Python editor extensions. A compatible Python runtime must already be available; interpreters are not automatically downloaded.",
        ],
        settings: &["explorer-extensions"],
        packages: &[
            ("package", "Git.Git"),
            ("package", "Microsoft.VisualStudioCode"),
            ("package", "astral-sh.uv"),
            ("uv", "ruff"),
            ("uv", "pre-commit"),
            ("vscode", "ms-python.python"),
            ("vscode", "charliermarsh.ruff"),
        ],
    },
    Recipe {
        id: "web",
        title: ["Web 开发起点", "Web development"],
        description: [
            "组合 Node.js LTS、Git、VS Code 与格式化和检查扩展；项目依赖仍由各项目管理。",
            "Combine Node.js LTS, Git, VS Code and formatting/lint extensions. Project dependencies remain managed by each project.",
        ],
        settings: &["explorer-extensions"],
        packages: &[
            ("package", "Git.Git"),
            ("package", "Microsoft.VisualStudioCode"),
            ("package", "OpenJS.NodeJS.LTS"),
            ("vscode", "esbenp.prettier-vscode"),
            ("vscode", "dbaeumer.vscode-eslint"),
        ],
    },
    Recipe {
        id: "wsl",
        title: ["WSL 组件准备", "Prepare WSL components"],
        description: [
            "显式启用 WSL 和虚拟机平台，并选择终端工具。不会把组件启用当作发行版或 WSL 内核已安装。",
            "Explicitly enable WSL and Virtual Machine Platform and select terminal tools. Component enablement does not establish a distribution or WSL kernel installation.",
        ],
        settings: &["explorer-extensions"],
        packages: &[
            ("package", "Microsoft.WindowsTerminal"),
            ("package", "Microsoft.PowerShell"),
        ],
    },
];

pub fn resolve(recipe: &Recipe, target: Target) -> Result<(BTreeSet<String>, BTreeSet<String>)> {
    let catalog = studio_deployment::catalog()?;
    let settings = recipe.settings.iter().map(|id| (*id).to_owned()).collect();
    let mut deployment = BTreeSet::new();
    for (kind, package) in recipe.packages {
        let task = catalog
            .tasks
            .iter()
            .find(|task| task.kind == *kind && task.package_id.as_deref() == Some(*package))
            .with_context(|| {
                format!(
                    "Recipe {} requires unavailable catalog entry {kind}:{package}",
                    recipe.id
                )
            })?;
        deployment.insert(task.id.clone());
    }
    if recipe.id == "wsl" {
        deployment.extend([
            "feature-wsl".to_owned(),
            "feature-virtual-machine-platform".to_owned(),
        ]);
    }
    studio_core::build_plan(&studio_core::load_catalog()?, target, &settings)?;
    studio_deployment::build_plan(target, &deployment)?;
    Ok((settings, deployment))
}

pub fn merge(
    catalog: &Catalog,
    target: Target,
    current_settings: &BTreeSet<String>,
    current_tasks: &BTreeSet<String>,
    added_settings: &BTreeSet<String>,
    added_tasks: &BTreeSet<String>,
) -> Result<(BTreeSet<String>, BTreeSet<String>)> {
    studio_core::build_plan(catalog, target, added_settings)?;
    studio_deployment::build_plan(target, added_tasks)?;
    let mut settings = current_settings.clone();
    for id in added_settings {
        let action = catalog
            .actions
            .iter()
            .find(|action| &action.id == id)
            .context("Unknown setting")?;
        if let Some(group) = &action.choice_group {
            settings.retain(|existing| {
                !catalog.actions.iter().any(|other| {
                    &other.id == existing && other.choice_group.as_ref() == Some(group)
                })
            });
        }
        settings.insert(id.clone());
    }
    let tasks = current_tasks
        .union(added_tasks)
        .cloned()
        .collect::<BTreeSet<_>>();
    ensure!(
        tasks.len() <= 128,
        "A deployment session supports at most 128 tasks; split this workspace into profiles"
    );
    studio_core::build_plan(catalog, target, &settings)?;
    studio_deployment::build_plan(target, &tasks)?;
    Ok((settings, tasks))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_mixed_recipes_resolve_and_preserve_existing_choices() {
        let catalog = studio_core::load_catalog().unwrap();
        for recipe in RECIPES {
            for target in Target::ALL {
                let (settings, tasks) = resolve(recipe, target).unwrap();
                assert!(!tasks.is_empty());
                let existing = BTreeSet::from(["privacy-feedback".to_owned()]);
                let (merged, _) = merge(
                    &catalog,
                    target,
                    &existing,
                    &BTreeSet::new(),
                    &settings,
                    &tasks,
                )
                .unwrap();
                assert!(merged.contains("privacy-feedback"));
            }
        }
    }
    #[test]
    fn incompatible_import_does_not_change_existing_selection() {
        let catalog = studio_core::load_catalog().unwrap();
        let current = BTreeSet::from(["explorer-open-quick-access".to_owned()]);
        let original = current.clone();
        assert!(
            merge(
                &catalog,
                Target::Win11Ltsc2024,
                &current,
                &BTreeSet::new(),
                &BTreeSet::from(["unknown".to_owned()]),
                &BTreeSet::new()
            )
            .is_err()
        );
        assert_eq!(current, original);
        let (merged, _) = merge(
            &catalog,
            Target::Win11Ltsc2024,
            &current,
            &BTreeSet::new(),
            &BTreeSet::from(["explorer-open-this-pc".to_owned()]),
            &BTreeSet::new(),
        )
        .unwrap();
        assert!(!merged.contains("explorer-open-quick-access"));
        assert!(merged.contains("explorer-open-this-pc"));
    }
}
