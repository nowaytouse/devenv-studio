use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, DirBuilder, File},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

use color_eyre::eyre::{Context, ContextCompat, Result, bail, ensure};
use serde::Serialize;
use sha2::{Digest, Sha256};
use walkdir::{DirEntry, WalkDir};

use super::process;

const MAX_DEPENDENCY_BYTES: u64 = 32 * 1024 * 1024;
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    "target",
    ".venv",
    "venv",
    "__pycache__",
    ".tox",
    ".nox",
    ".pytest_cache",
    ".mypy_cache",
    "dist",
    "build",
    ".gradle",
    ".idea",
    ".vscode",
    "DerivedData",
    ".next",
    ".turbo",
    ".cache",
    ".dart_tool",
    "Pods",
];
const NAMES: &[&str] = &[
    ".topgrade-dependencies.json",
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain",
    "rust-toolchain.toml",
    "pyproject.toml",
    "uv.lock",
    "poetry.lock",
    "pdm.lock",
    "Pipfile",
    "Pipfile.lock",
    "pixi.toml",
    "pixi.lock",
    "environment.yml",
    "environment.yaml",
    "package.json",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
    "yarn.lock",
    "bun.lock",
    "bun.lockb",
    "deno.json",
    "deno.jsonc",
    "deno.lock",
    "go.mod",
    "go.sum",
    "build.zig",
    "build.zig.zon",
    "Package.swift",
    "Package.resolved",
    "Podfile",
    "Podfile.lock",
    "pom.xml",
    "settings.gradle",
    "settings.gradle.kts",
    "build.gradle",
    "build.gradle.kts",
    "libs.versions.toml",
    "Directory.Packages.props",
    "packages.lock.json",
    "composer.json",
    "composer.lock",
    "Gemfile",
    "Gemfile.lock",
    "conanfile.py",
    "conanfile.txt",
    "conan.lock",
    "vcpkg.json",
    "vcpkg-configuration.json",
    "mix.exs",
    "mix.lock",
    "rebar.config",
    "rebar.lock",
    "stack.yaml",
    "cabal.project",
    "cabal.project.freeze",
    "Project.toml",
    "Manifest.toml",
    "renv.lock",
    "deps.edn",
    "project.clj",
    "build.sbt",
    "flake.nix",
    "flake.lock",
    "gradle.properties",
    "gradle-wrapper.properties",
    "action.yml",
    "action.yaml",
    ".pre-commit-config.yaml",
    ".pre-commit-config.yml",
    ".gitmodules",
    ".terraform.lock.hcl",
    "mise.toml",
    ".tool-versions",
];
const SUFFIXES: &[&str] = &["csproj", "fsproj", "vbproj", "sln", "cabal", "opam", "rockspec", "tf"];

fn scan_entry(entry: &DirEntry) -> bool {
    entry.depth() == 0
        || !entry.file_type().is_dir()
        || !entry.file_name().to_str().is_some_and(|name| SKIP_DIRS.contains(&name))
}

pub(super) fn discover(root: &Path, max_depth: usize) -> Result<Vec<PathBuf>> {
    ensure!(root.is_dir(), "Dependency root is not a directory: {}", root.display());
    let mut walker = WalkDir::new(root)
        .max_depth(max_depth)
        .into_iter()
        .filter_entry(scan_entry);
    let mut repos = Vec::new();
    while let Some(entry) = walker.next() {
        let entry = entry?;
        if entry.file_type().is_dir() {
            let marker = entry.path().join(".git");
            match fs::symlink_metadata(&marker) {
                Ok(meta) => {
                    ensure!(
                        meta.is_file() || meta.is_dir(),
                        "Invalid Git marker: {}",
                        marker.display()
                    );
                    repos.push(entry.into_path());
                    walker.skip_current_dir();
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(repos)
}

pub(super) fn walk(repo: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in WalkDir::new(repo).into_iter().filter_entry(scan_entry) {
        let entry = entry?;
        if !entry.file_type().is_dir() {
            paths.push(entry.into_path());
        }
    }
    paths.sort_unstable();
    Ok(paths)
}

pub(super) fn name(path: &Path) -> &str {
    path.file_name().and_then(|name| name.to_str()).unwrap_or_default()
}

pub(super) fn dockerfile(path: &Path) -> bool {
    let name = name(path);
    name == "Dockerfile" || name.starts_with("Dockerfile.")
}

pub(super) fn actions(relative: &Path) -> bool {
    relative.starts_with(".github/workflows")
        && matches!(
            relative.extension().and_then(|value| value.to_str()),
            Some("yml" | "yaml")
        )
}

fn watched(path: &Path) -> bool {
    NAMES.contains(&name(path))
        || path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| SUFFIXES.contains(&extension))
        || dockerfile(path)
        || actions(path)
        || gradle_source(path)
}

pub(super) fn gradle_source(path: &Path) -> bool {
    let name = name(path);
    name.ends_with(".gradle")
        || name.ends_with(".gradle.kts")
        || matches!(name, "gradle.properties" | "libs.versions.toml")
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(super) struct Stamp {
    sha256: String,
    size: usize,
    mode: u32,
    mtime_ns: i64,
}

pub(super) struct Content {
    pub(super) bytes: Vec<u8>,
    pub(super) mode: u32,
    mtime_ns: i64,
}

impl Content {
    fn stamp(&self) -> Stamp {
        Stamp {
            sha256: format!("{:x}", Sha256::digest(&self.bytes)),
            size: self.bytes.len(),
            mode: self.mode,
            mtime_ns: self.mtime_ns,
        }
    }
}

pub(super) fn read(path: &Path, limit: u64) -> Result<Content> {
    let mut file = super::super::open_file(path, false, limit)?;
    let before = file.metadata()?;
    let mut bytes = Vec::new();
    (&mut file).take(limit + 1).read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    let current = fs::symlink_metadata(path)?;
    ensure!(
        bytes.len() as u64 == before.len()
            && (before.dev(), before.ino()) == (current.dev(), current.ino())
            && (before.mtime(), before.mtime_nsec(), before.ctime(), before.ctime_nsec())
                == (after.mtime(), after.mtime_nsec(), after.ctime(), after.ctime_nsec())
            && (after.mtime(), after.mtime_nsec(), after.ctime(), after.ctime_nsec())
                == (
                    current.mtime(),
                    current.mtime_nsec(),
                    current.ctime(),
                    current.ctime_nsec()
                )
            && path.canonicalize()? == path,
        "Dependency changed while reading: {}",
        path.display()
    );
    Ok(Content {
        bytes,
        mode: before.mode() & 0o7777,
        mtime_ns: before
            .mtime()
            .checked_mul(1_000_000_000)
            .and_then(|seconds| seconds.checked_add(before.mtime_nsec()))
            .ok_or_else(|| color_eyre::eyre::eyre!("Dependency modification time exceeds supported range"))?,
    })
}

pub(super) fn repository_edge(repo: &Path, edge: bool) -> Result<bool> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Policy {
        allow_incompatible: bool,
    }
    let path = repo.join(".topgrade-dependencies.json");
    if !super::super::exists(&path)? {
        return Ok(edge);
    }
    let content = read(&path, 1024)?;
    let policy: Policy = serde_json::from_slice(&content.bytes)
        .wrap_err_with(|| format!("Invalid repository dependency policy at {}", path.display()))?;
    ensure!(
        !policy.allow_incompatible,
        "Repository policy may only disable incompatible updates"
    );
    Ok(false)
}

pub(super) fn snapshot(repo: &Path) -> Result<BTreeMap<PathBuf, Stamp>> {
    let mut entries = BTreeMap::new();
    for path in walk(repo)? {
        let relative = path.strip_prefix(repo)?;
        if watched(relative) {
            entries.insert(relative.to_path_buf(), read(&path, MAX_DEPENDENCY_BYTES)?.stamp());
        }
    }
    Ok(entries)
}

pub(super) fn changed(before: &BTreeMap<PathBuf, Stamp>, after: &BTreeMap<PathBuf, Stamp>) -> Vec<PathBuf> {
    before
        .keys()
        .chain(after.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|&path| {
            before.get(path).map(|stamp| (&stamp.sha256, stamp.mode))
                != after.get(path).map(|stamp| (&stamp.sha256, stamp.mode))
        })
        .cloned()
        .collect()
}

pub(super) fn rewrite(path: &Path, original: &Content, bytes: &[u8]) -> Result<()> {
    if bytes == original.bytes {
        return Ok(());
    }
    ensure!(
        original.mode & !0o777 == 0,
        "Refusing to rewrite dependency with special file mode: {}",
        path.display()
    );
    let parent = path.parent().context("Dependency has no parent directory")?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    staged.write_all(bytes)?;
    staged
        .as_file()
        .set_permissions(fs::Permissions::from_mode(original.mode))?;
    staged.as_file().sync_all()?;
    ensure!(
        read(path, MAX_DEPENDENCY_BYTES)?.stamp() == original.stamp(),
        "Dependency changed before replacement: {}",
        path.display()
    );
    staged.persist(path)?;
    File::open(parent)?.sync_all()?;
    let installed = read(path, MAX_DEPENDENCY_BYTES)?;
    ensure!(
        installed.bytes == bytes && installed.mode == original.mode,
        "Dependency replacement failed readback: {}",
        path.display()
    );
    Ok(())
}

pub(super) fn repo_stem(repo: &Path) -> Result<String> {
    let name = repo
        .file_name()
        .and_then(|name| name.to_str())
        .context("Invalid repository basename")?;
    let digest = format!("{:x}", Sha256::digest(repo.as_os_str().as_encoded_bytes()));
    Ok(format!("{name}-{}", &digest[..16]))
}

pub(super) fn create_private_dir(path: &Path) -> Result<()> {
    DirBuilder::new().mode(0o700).create(path)?;
    super::super::check_dir(path, true)
}

fn ensure_dir(path: &Path) -> Result<()> {
    match create_private_dir(path) {
        Ok(()) => Ok(()),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|cause| cause.kind() == std::io::ErrorKind::AlreadyExists) =>
        {
            super::super::check_dir(path, false)
        }
        Err(error) => Err(error),
    }
}

pub(super) fn cache_dir(home: &Path) -> Result<PathBuf> {
    let parent = home.join(".cache");
    let root = parent.join("rolling-deps");
    ensure_dir(&parent)?;
    ensure_dir(&root)?;
    Ok(root)
}

pub(super) fn backup(repo: &Path, before: &BTreeMap<PathBuf, Stamp>, run: &Path, stem: &str) -> Result<PathBuf> {
    ensure!(
        !run.starts_with(repo),
        "Dependency backup must be outside the repository"
    );
    let root = run.join("before");
    ensure_dir(&root)?;
    let backup = root.join(stem);
    create_private_dir(&backup)?;
    let files = backup.join("files");
    create_private_dir(&files)?;
    let mut dirs = BTreeSet::from([run.to_path_buf(), root, backup.clone(), files.clone()]);
    for (relative, stamp) in before {
        let data = read(&repo.join(relative), MAX_DEPENDENCY_BYTES)?;
        ensure!(
            data.stamp() == *stamp,
            "Dependency changed during backup: {}",
            relative.display()
        );
        let mut parent = files.clone();
        for component in relative.parent().context("Invalid dependency path")? {
            parent.push(component);
            if dirs.insert(parent.clone()) {
                create_private_dir(&parent)?;
            }
        }
        let mut out = super::super::create_file(&files.join(relative))?;
        out.write_all(&data.bytes)?;
        out.sync_all()?;
    }
    ensure!(
        snapshot(repo)? == *before,
        "Dependency set changed during backup: {}",
        repo.display()
    );
    let mut receipt = super::super::create_file(&backup.join("manifest.json"))?;
    serde_json::to_writer_pretty(&mut receipt, &serde_json::json!({ "repo": repo, "files": before }))?;
    receipt.write_all(b"\n")?;
    receipt.sync_all()?;
    let mut dirs = dirs.into_iter().collect::<Vec<_>>();
    dirs.sort_unstable_by_key(|dir| std::cmp::Reverse(dir.components().count()));
    for dir in dirs {
        File::open(dir)?.sync_all()?;
    }
    Ok(backup)
}

pub(super) fn record_git_status(repo: &Path, log: &mut File, when: &str) -> Result<()> {
    let Some(git) = process::find_tool("git")? else {
        bail!("Missing Git executable for dependency status verification");
    };
    let mut git = process::command(&git, repo);
    git.args([
        "--no-optional-locks",
        "-c",
        "core.fsmonitor=false",
        "status",
        "--porcelain=v1",
    ]);
    let output = process::capture(git, process::QUERY_TIMEOUT)?;
    ensure!(
        output.status.success(),
        "Cannot record {when} Git status for {}",
        repo.display()
    );
    writeln!(log, "# {when} status:\n{}", output.stdout.trim_end())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_cannot_escalate_edge_or_hide_invalid_entries() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let repo = temp.path().canonicalize()?;
        let policy = repo.join(".topgrade-dependencies.json");
        assert!(repository_edge(&repo, true)?);
        assert!(!repository_edge(&repo, false)?);
        fs::write(&policy, br#"{"allow_incompatible":false}"#)?;
        assert!(!repository_edge(&repo, true)?);
        for text in [
            r#"{"allow_incompatible":true}"#,
            r#"{"allow_incompatible":false,"allow_incompatible":false}"#,
            r#"{"allow_incompatible":false,"extra":0}"#,
            r#"{"allow_incompatible":0}"#,
            r#"{}"#,
        ] {
            fs::write(&policy, text)?;
            assert!(repository_edge(&repo, true).is_err());
        }
        fs::remove_file(&policy)?;
        std::os::unix::fs::symlink("somewhere", &policy)?;
        assert!(repository_edge(&repo, true).is_err());
        Ok(())
    }

    #[test]
    fn backup_records_bytes_mode_and_scope_before_writes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let base = temp.path().canonicalize()?;
        let repo = base.join("project");
        let run = base.join("run");
        create_private_dir(&repo)?;
        create_private_dir(&run)?;
        fs::write(repo.join("Cargo.toml"), b"manifest")?;
        fs::create_dir(repo.join("node_modules"))?;
        fs::write(repo.join("node_modules/package.json"), b"excluded")?;
        let before = snapshot(&repo)?;
        let backup = backup(&repo, &before, &run, "project")?;
        assert_eq!(fs::read(backup.join("files/Cargo.toml"))?, b"manifest");
        assert!(before.contains_key(Path::new("Cargo.toml")));
        assert_eq!(before.len(), 1);
        assert!(backup.join("manifest.json").is_file());
        fs::write(repo.join("Cargo.toml"), b"changed")?;
        assert_eq!(changed(&before, &snapshot(&repo)?), [PathBuf::from("Cargo.toml")]);
        Ok(())
    }
}
