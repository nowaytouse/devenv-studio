use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use color_eyre::eyre::Result;

use super::{files, process};

const CARGO_EDGE_UPGRADE: &[&str] = &[
    "cargo",
    "upgrade",
    "--incompatible",
    "allow",
    "--pinned",
    "ignore",
    "--recursive",
    "true",
    "--exclude",
    "bincode",
];

#[derive(Clone, Copy)]
pub(super) enum Specialized {
    Zig,
    Docker,
    Gradle,
}

impl std::fmt::Display for Specialized {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Zig => "Zig",
            Self::Docker => "Docker",
            Self::Gradle => "Gradle",
        })
    }
}

pub(super) enum Action {
    Commands(Vec<Vec<String>>),
    Specialized(Specialized, bool),
}

pub(super) struct Surface {
    pub(super) name: &'static str,
    pub(super) root: PathBuf,
    pub(super) level: &'static str,
    pub(super) action: Action,
}

impl Surface {
    fn commands(name: &'static str, root: PathBuf, level: &'static str, commands: &[&[&str]]) -> Self {
        Self {
            name,
            root,
            level,
            action: Action::Commands(
                commands
                    .iter()
                    .map(|command| command.iter().map(ToString::to_string).collect())
                    .collect(),
            ),
        }
    }

    fn special(name: &'static str, root: PathBuf, level: &'static str, kind: Specialized, edge: bool) -> Self {
        Self {
            name,
            root,
            level,
            action: Action::Specialized(kind, edge),
        }
    }
}

fn topmost(paths: &[PathBuf], matches: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    let unique: BTreeSet<_> = paths
        .iter()
        .filter(|path| matches(path))
        .filter_map(|path| path.parent())
        .collect();
    let mut candidates: Vec<_> = unique.into_iter().collect();
    candidates.sort_unstable_by_key(|path| (path.components().count(), *path));
    let mut roots = Vec::new();
    for root in candidates {
        if !roots.iter().any(|parent: &PathBuf| root.starts_with(parent)) {
            roots.push(root.to_path_buf());
        }
    }
    roots
}

pub(super) fn detect(repo: &Path, edge: bool) -> Result<Vec<Surface>> {
    let paths = files::walk(repo)?;
    let dirs = |names: &[&str]| topmost(&paths, |path| names.contains(&files::name(path)));
    let suffix_dirs = |suffixes: &[&str]| {
        topmost(&paths, |path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| suffixes.contains(&extension))
        })
    };
    let mut surfaces = Vec::new();
    macro_rules! direct {
        ($name:literal, $files:expr, $level:expr, $($args:expr),+ $(,)?) => {
            for root in dirs($files) {
                surfaces.push(Surface::commands($name, root, $level, &[$($args),+]));
            }
        };
    }
    let compatible = |edge_label| if edge { edge_label } else { "compatible" };

    for root in dirs(&["Cargo.toml"]) {
        let mut commands: Vec<&[&str]> = Vec::new();
        if edge && process::find_tool("cargo-upgrade")?.is_some() {
            commands.push(CARGO_EDGE_UPGRADE);
        }
        commands.push(&["cargo", "update"]);
        surfaces.push(Surface::commands("Cargo", root, compatible("major+git"), &commands));
    }
    direct!("uv", &["uv.lock"], "constraints+git", &["uv", "lock", "--upgrade"]);
    direct!(
        "Poetry",
        &["poetry.lock"],
        "constraints",
        &["poetry", "update", "--lock"]
    );
    direct!(
        "PDM",
        &["pdm.lock"],
        compatible("major+prerelease"),
        if edge {
            &["pdm", "update", "--unconstrained", "--prerelease", "--no-sync"]
        } else {
            &["pdm", "update", "--no-sync"]
        }
    );
    direct!(
        "Pipenv",
        &["Pipfile", "Pipfile.lock"],
        "constraints",
        &["pipenv", "update"]
    );
    direct!(
        "Pixi",
        &["pixi.toml", "pixi.lock"],
        compatible("manifest+lock"),
        &["pixi", if edge { "upgrade" } else { "update" }, "--no-install"]
    );
    for root in dirs(&["environment.yml", "environment.yaml"]) {
        let file = if root.join("environment.yml").exists() {
            "environment.yml"
        } else {
            "environment.yaml"
        };
        surfaces.push(Surface::commands(
            "Conda environment",
            root,
            "manifest-constraints",
            &[&["conda", "env", "update", "--file", file, "--prune"]],
        ));
    }

    for root in dirs(&["package.json"]) {
        let workspace = root.join("pnpm-workspace.yaml").exists();
        if workspace || root.join("pnpm-lock.yaml").exists() {
            let mut command = vec!["pnpm", "up"];
            if edge {
                command.push("--latest");
            }
            if workspace {
                command.push("--recursive");
            }
            command.extend(["--include-github-actions", "--ignore-scripts"]);
            surfaces.push(Surface::commands(
                "pnpm",
                root,
                if edge { "major+actions" } else { "compatible+actions" },
                &[&command],
            ));
        } else if root.join("bun.lock").exists() || root.join("bun.lockb").exists() {
            let mut command = vec!["bun", "update", "--ignore-scripts"];
            if edge {
                command.push("--latest");
            }
            let manifest = files::read(&root.join("package.json"), 1024 * 1024)?;
            let package: serde_json::Value = serde_json::from_slice(&manifest.bytes)?;
            if package.get("workspaces").is_some_and(|value| match value {
                serde_json::Value::Array(values) => !values.is_empty(),
                serde_json::Value::Object(values) => !values.is_empty(),
                _ => false,
            }) {
                command.push("--recursive");
            }
            surfaces.push(Surface::commands("Bun", root, compatible("major"), &[&command]));
        } else if root.join("yarn.lock").exists() {
            let lock = files::read(&root.join("yarn.lock"), 32 * 1024 * 1024)?;
            let classic = std::str::from_utf8(&lock.bytes)?
                .lines()
                .any(|line| line == "# yarn lockfile v1");
            let command: &[&str] = if classic && edge {
                &["yarn", "upgrade", "--latest", "--ignore-scripts"]
            } else if classic {
                &["yarn", "upgrade", "--ignore-scripts"]
            } else if edge {
                &["yarn", "up", "*", "--mode=skip-build"]
            } else {
                &["yarn", "up", "-R", "*", "--mode=skip-build"]
            };
            surfaces.push(Surface::commands("Yarn", root, compatible("major"), &[command]));
        } else if root.join("package-lock.json").exists() || root.join("npm-shrinkwrap.json").exists() {
            let mut commands: Vec<&[&str]> = Vec::new();
            let major = edge && process::find_tool("ncu")?.is_some();
            if major {
                commands.push(&["ncu", "-u"]);
            }
            let npm = ["npm", if major { "install" } else { "update" }, "--ignore-scripts"];
            commands.push(&npm);
            surfaces.push(Surface::commands(
                "npm",
                root,
                if major { "major" } else { "compatible" },
                &commands,
            ));
        }
    }
    if paths.iter().any(|path| {
        path.strip_prefix(repo).is_ok_and(files::actions) || matches!(files::name(path), "action.yml" | "action.yaml")
    }) {
        surfaces.push(Surface::commands(
            "GitHub Actions",
            repo.into(),
            "latest+sha-pin",
            &[&[
                "pinact",
                "run",
                "--update",
                "--branch-to-tag",
                "^main$",
                "--branch-to-tag",
                "^master$",
                "--branch-to-tag",
                "^stable$",
            ]],
        ));
    }
    direct!(
        "Deno",
        &["deno.json", "deno.jsonc"],
        compatible("major"),
        if edge {
            &["deno", "update", "--latest", "--recursive"]
        } else {
            &["deno", "update", "--recursive"]
        }
    );
    direct!(
        "Go Modules",
        &["go.mod"],
        "module-path edge",
        &["go", "get", "-u", "./..."],
        &["go", "mod", "tidy"]
    );
    for root in dirs(&["build.zig.zon"]) {
        surfaces.push(Surface::special(
            "Zig packages",
            root,
            compatible("github-head"),
            Specialized::Zig,
            edge,
        ));
    }
    direct!(
        "SwiftPM",
        &["Package.swift"],
        "constraints",
        &["swift", "package", "update"]
    );
    direct!("CocoaPods", &["Podfile"], "constraints", &["pod", "update"]);
    direct!(
        "Maven",
        &["pom.xml"],
        if edge { "major" } else { "releases" },
        if edge {
            &[
                "mvn",
                "-q",
                "versions:use-latest-versions",
                "versions:update-properties",
                "-DgenerateBackupPoms=false",
            ]
        } else {
            &[
                "mvn",
                "-q",
                "versions:use-latest-releases",
                "-DgenerateBackupPoms=false",
            ]
        }
    );
    for root in dirs(&[
        "settings.gradle",
        "settings.gradle.kts",
        "build.gradle",
        "build.gradle.kts",
    ]) {
        surfaces.push(Surface::special(
            "Gradle",
            root,
            compatible("integration+prerelease+nightly"),
            Specialized::Gradle,
            edge,
        ));
    }
    for root in suffix_dirs(&["csproj", "fsproj", "vbproj", "sln"]) {
        surfaces.push(Surface::commands(
            ".NET / NuGet",
            root,
            "highest-available",
            &[&["dotnet", "package", "update", "--project", "."]],
        ));
    }
    direct!(
        "Composer",
        &["composer.json"],
        "constraints",
        &[
            "composer",
            "update",
            "--with-all-dependencies",
            "--no-interaction",
            "--no-scripts",
            "--no-plugins"
        ]
    );
    direct!("Bundler", &["Gemfile"], "constraints", &["bundle", "update"]);
    direct!(
        "Conan",
        &["conanfile.py", "conanfile.txt"],
        "constraints",
        &["conan", "install", ".", "--update", "--build=missing"]
    );
    direct!(
        "vcpkg",
        &["vcpkg.json"],
        "baseline",
        &["vcpkg", "x-update-baseline", "--add-initial-baseline"]
    );
    direct!("Mix/Hex", &["mix.exs"], "constraints", &["mix", "deps.update", "--all"]);
    direct!("rebar3", &["rebar.config"], "constraints", &["rebar3", "upgrade"]);
    for root in dirs(&["Project.toml"]) {
        if root.join("Manifest.toml").exists() {
            surfaces.push(Surface::commands(
                "Julia",
                root,
                "constraints",
                &[&["julia", "--project=.", "-e", "using Pkg; Pkg.update()"]],
            ));
        }
    }
    direct!(
        "R/renv",
        &["renv.lock"],
        "constraints",
        &["Rscript", "-e", "renv::update(prompt = FALSE)"]
    );
    for root in topmost(&paths, files::dockerfile) {
        surfaces.push(Surface::special(
            "Docker images",
            root,
            compatible("tag+digest-edge"),
            Specialized::Docker,
            edge,
        ));
    }
    if process::find_tool("nix")?.is_some() {
        direct!("Nix Flake", &["flake.lock"], "edge", &["nix", "flake", "update"]);
    }
    direct!(
        "pre-commit",
        &[".pre-commit-config.yaml", ".pre-commit-config.yml"],
        "edge",
        &["pre-commit", "autoupdate"]
    );
    if repo.join(".gitmodules").exists() {
        surfaces.push(Surface::commands(
            "Git submodules",
            repo.into(),
            "remote-head",
            &[&[
                "git",
                "-c",
                "core.hooksPath=/dev/null",
                "submodule",
                "update",
                "--init",
                "--remote",
                "--recursive",
            ]],
        ));
    }
    for root in suffix_dirs(&["tf"]) {
        surfaces.push(Surface::commands(
            "Terraform",
            root,
            "provider/module constraints",
            &[&["terraform", "init", "-upgrade", "-backend=false"]],
        ));
    }
    if process::find_tool("mise")?.is_some() {
        direct!(
            "mise toolchains",
            &["mise.toml", ".tool-versions"],
            "edge",
            &["mise", "upgrade"]
        );
    }
    Ok(surfaces)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_cargo_upgrade_respects_project_pins() {
        assert!(CARGO_EDGE_UPGRADE.windows(2).any(|args| args == ["--pinned", "ignore"]));
        assert!(
            CARGO_EDGE_UPGRADE
                .windows(2)
                .any(|args| args == ["--incompatible", "allow"])
        );
    }

    #[test]
    fn compatible_keeps_existing_package_constraints_and_topmost_workspace() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        std::fs::write(root.join("Cargo.toml"), "")?;
        std::fs::create_dir(root.join("member"))?;
        std::fs::write(root.join("member/Cargo.toml"), "")?;
        std::fs::write(root.join("package.json"), "{}")?;
        std::fs::write(root.join("pnpm-lock.yaml"), "")?;
        let plan = detect(&root, false)?;
        assert_eq!(plan.len(), 2);
        let Action::Commands(cargo) = &plan[0].action else {
            panic!("not Cargo commands")
        };
        assert_eq!(cargo, &[vec!["cargo", "update"]]);
        let Action::Commands(pnpm) = &plan[1].action else {
            panic!("not pnpm commands")
        };
        assert!(!pnpm[0].iter().any(|argument| argument == "--latest"));
        assert!(pnpm[0].iter().any(|argument| argument == "--ignore-scripts"));
        assert_eq!(plan[0].root, root);
        Ok(())
    }
}
