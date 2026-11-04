use super::{Item, Lock, hex};
use color_eyre::eyre::{Context, ContextCompat, Result, ensure};
use regex::Regex;
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::LazyLock,
};

const SKILL_LIMIT: u64 = 4 * 1024 * 1024;
const TREE_FILE_LIMIT: u64 = 32 * 1024 * 1024;
pub(in crate::studio) const CURSOR_NAMES: &[&str] = &[
    "automate",
    "autopilot",
    "canvas",
    "create-hook",
    "create-rule",
    "create-skill",
    "create-subagent",
    "deploy-with-vercel",
    "goal",
    "loop",
    "migrate-to-skills",
    "new-repo",
    "onboard",
    "origin",
    "rename-chat",
    "review",
    "review-bugbot",
    "review-security",
    "sdk",
    "share",
    "shell",
    "split-to-prs",
    "statusline",
    "update-cli-config",
    "update-cursor-settings",
    "visualize",
];
pub(in crate::studio) const USER_CONFIG: &[&str] = &[
    ".env",
    ".env.local",
    ".env.production",
    ".env.development",
    "settings.json",
    "settings.local.json",
    "config.json",
    "config.toml",
    "config.yaml",
    "config.yml",
    "secrets.json",
    "credentials.json",
];
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Row {
    skill_name: String,
    #[serde(rename = "sourceURL")]
    source_url: String,
    path: PathBuf,
    pinned: bool,
    scope: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(in crate::studio) struct Metadata {
    pub name: String,
    pub source: String,
    pub directory: String,
    pub tree: String,
}

pub(in crate::studio) fn repository(url: &str) -> Result<String> {
    let path = url
        .strip_prefix("https://github.com/")
        .context("Skill source must be a canonical GitHub repository URL")?;
    let path = path.strip_suffix('/').unwrap_or(path);
    let (owner, repo) = path.split_once('/').context("Invalid GitHub repository URL")?;
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    ensure!(component(owner) && component(repo), "Invalid GitHub repository URL");
    Ok(format!("{owner}/{repo}").to_ascii_lowercase())
}

fn component(value: &str) -> bool {
    !matches!(value, "" | "." | "..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

pub(in crate::studio) fn directory(value: &str) -> Result<&str> {
    if matches!(value, "" | ".") {
        return Ok("");
    }
    ensure!(
        !value.contains('\\')
            && !value.chars().any(char::is_control)
            && value.split('/').all(|part| !matches!(part, "" | "." | "..")),
        "Unsafe or ambiguous skill source directory"
    );
    Ok(value)
}

fn regular_directory(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute() && fs::symlink_metadata(path)?.is_dir() && path.canonicalize()? == path,
        "Expected a regular non-symlink skill directory: {}",
        path.display()
    );
    Ok(())
}

fn header(path: &Path) -> Result<String> {
    regular_directory(path)?;
    let file = super::super::open_file(&path.join("SKILL.md"), false, SKILL_LIMIT)?;
    let mut text = String::new();
    file.take(SKILL_LIMIT + 1).read_to_string(&mut text)?;
    ensure!(
        text.len() as u64 <= SKILL_LIMIT,
        "SKILL.md exceeds the verified input limit"
    );
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    normalized
        .strip_prefix("---\n")
        .and_then(|body| body.split_once("\n---\n"))
        .map(|(header, _)| header.to_owned())
        .context("Invalid SKILL.md frontmatter")
}

pub(in crate::studio) fn metadata(path: &Path) -> Result<Metadata> {
    let header = header(path).wrap_err_with(|| format!("Incomplete GitHub skill metadata: {}", path.display()))?;
    let value = |field: &str, nested: bool| -> Result<String> {
        let mut matches = header.lines().filter_map(|line| {
            let indented = line.starts_with([' ', '\t']);
            (indented == nested)
                .then(|| line.trim_start_matches([' ', '\t']))
                .and_then(|line| line.split_once(':'))
                .and_then(|(key, value)| (key == field).then_some(value))
        });
        let first = matches.next().context("Missing GitHub skill metadata field")?;
        ensure!(
            matches.next().is_none() && !first.trim().is_empty(),
            "Missing or duplicate GitHub skill metadata field {field}"
        );
        Ok(first.trim().trim_matches(['\'', '"']).into())
    };
    let result = Metadata {
        name: value("name", false)?,
        source: value("github-repo", true)?,
        directory: value("github-path", true)?,
        tree: value("github-tree-sha", true)?,
    };
    ensure!(
        path.file_name().and_then(|name| name.to_str()) == Some(&result.name) && hex(&result.tree, 40),
        "Invalid GitHub skill identity/hash: {}",
        path.display()
    );
    repository(&result.source)?;
    directory(&result.directory)?;
    Ok(result)
}

pub(in crate::studio) fn name_matches(path: &Path, name: &str) -> Result<bool> {
    static NAME: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?m)^name:\s*['"]?([\w-]+)"#).expect("static skill name regex"));
    let header = header(path)?;
    let mut matches = NAME.captures_iter(&header);
    let first = matches
        .next()
        .and_then(|entry| entry.get(1))
        .map(|value| value.as_str());
    Ok(first == Some(name) && matches.next().is_none())
}

fn tree(root: &Path) -> Result<BTreeMap<PathBuf, String>> {
    regular_directory(root)?;
    let mut work = vec![(root.to_path_buf(), 0)];
    let mut files = BTreeMap::new();
    let mut seen = 0;
    while let Some((folder, depth)) = work.pop() {
        ensure!(depth <= 32, "Skill directory nesting exceeds its verification limit");
        regular_directory(&folder)?;
        for entry in fs::read_dir(folder)? {
            let path = entry?.path();
            seen += 1;
            ensure!(seen <= 4096, "Skill tree exceeds its verification entry limit");
            let file_type = fs::symlink_metadata(&path)?.file_type();
            if file_type.is_dir() {
                work.push((path, depth + 1));
            } else {
                ensure!(
                    file_type.is_file(),
                    "Unsupported file inside verified skill tree: {}",
                    path.display()
                );
                let file = super::super::open_file(&path, false, TREE_FILE_LIMIT)?;
                files.insert(path.strip_prefix(root)?.to_path_buf(), super::super::sha256(file)?);
            }
        }
    }
    ensure!(
        files.contains_key(Path::new("SKILL.md")),
        "Verified skill tree lacks SKILL.md"
    );
    Ok(files)
}

fn cursor_equivalent(native: &Path, shared: &Path, name: &str) -> Result<bool> {
    let native_files = tree(native)?;
    let shared_files = tree(shared)?;
    let config = |path: &Path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| USER_CONFIG.contains(&name))
    };
    Ok(native_files
        .iter()
        .all(|(path, hash)| shared_files.get(path) == Some(hash) || config(path))
        && shared_files
            .keys()
            .all(|path| native_files.contains_key(path) || config(path))
        && name_matches(native, name)?)
}

pub(in crate::studio) fn lean_ctx_mirror_path(home: &Path, path: &Path) -> bool {
    [
        ".cursor/skills/lean-ctx",
        ".copilot/skills/lean-ctx",
        ".config/opencode/skills/lean-ctx",
    ]
    .iter()
    .any(|relative| path == home.join(relative))
}

pub(in crate::studio) fn lean_ctx_mirror(canonical: &Path, mirror: &Path) -> Result<bool> {
    let reviewed = |path: &Path| -> Result<_> {
        let mut files = tree(path)?;
        files.remove(Path::new("SKILL.md.bak"));
        ensure!(
            files
                .remove(Path::new("SKILL.md.v4-migration.lock"))
                .is_none_or(|sha| sha == EMPTY_SHA256),
            "Nonempty LeanCTX migration lock: {}",
            path.display()
        );
        Ok(files)
    };
    Ok(reviewed(canonical)? == reviewed(mirror)? && name_matches(canonical, "lean-ctx")?)
}

#[derive(Default)]
pub(super) struct Delegated {
    pub npx: usize,
    pub cursor: usize,
}

pub(super) fn preflight(rows: Vec<Row>, home: &Path, lock: &Lock) -> Result<(Vec<Item>, Delegated)> {
    ensure!(
        !rows.is_empty() && rows.len() <= 4096,
        "GH skill discovery did not provide a bounded inventory"
    );
    let mut seen = BTreeSet::new();
    let mut selected = Vec::new();
    let mut delegated = Delegated::default();
    for row in rows {
        let name = row
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .context("Invalid skill directory name")?;
        ensure!(
            row.path.is_absolute()
                && component(name)
                && name
                    .as_bytes()
                    .first()
                    .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_'),
            "Unsafe skill directory name"
        );
        ensure!(
            !row.pinned && matches!(row.scope.as_str(), "user" | "project") && seen.insert(row.path.clone()),
            "Ambiguous, pinned or unsupported skill entry: {}",
            row.path.display()
        );
        regular_directory(&row.path)?;
        super::super::open_file(&row.path.join("SKILL.md"), false, SKILL_LIMIT)?;
        if !row.source_url.is_empty() {
            let meta = metadata(&row.path)?;
            let repo = repository(&row.source_url)?;
            ensure!(
                repository(&meta.source)? == repo,
                "GH source and installed metadata disagree: {}",
                row.path.display()
            );
            selected.push(Item {
                path: row.path,
                alias: row.skill_name,
                repo,
                meta,
                expected: String::new(),
            });
            continue;
        }
        let canonical = home.join(".agents/skills").join(name);
        let registered = lock.npx_registered(name);
        if row.path == canonical && registered && name_matches(&row.path, name)? {
            delegated.npx += 1;
        } else if name == "lean-ctx" && registered && lean_ctx_mirror_path(home, &row.path) {
            ensure!(
                lean_ctx_mirror(&canonical, &row.path)?,
                "Registered LeanCTX copy differs from its canonical owner: {}",
                row.path.display()
            );
            delegated.npx += 1;
        } else if row.path == canonical && CURSOR_NAMES.contains(&name) {
            ensure!(
                cursor_equivalent(&home.join(".cursor/skills-cursor").join(name), &row.path, name)?,
                "Cursor native skill mirror is not verified: {}",
                row.path.display()
            );
            delegated.cursor += 1;
        } else {
            color_eyre::eyre::bail!(
                "No verified updater owns this metadata-less skill: {}",
                row.path.display()
            );
        }
    }
    Ok((selected, delegated))
}
