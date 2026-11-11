use super::*;
use crate::studio::{check_file, is_hex};
use nix::libc::{O_NOFOLLOW, O_NONBLOCK};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::OpenOptions,
    os::unix::fs::{OpenOptionsExt, symlink},
    sync::LazyLock,
};

const SOURCE: &str = "plugin87/ux-ui-agent-skills";
const URL: &str = "https://github.com/plugin87/ux-ui-agent-skills.git";
const PREFIX: &str = "${CLAUDE_SKILL_DIR}/../../../";
const KIT: &str = ".local/share/design-skill-kit";
const CACHE: &str = ".local/state/design-skill-kit/source";
const MANIFEST: &str = ".design-skill-kit.json";
const PENDING: &str = ".sync-pending";
const ARCHIVE_LIMIT: u64 = 128 * 1024 * 1024;
const FILE_LIMIT: usize = 8192;
const REQUIRED: &[&str] = &[
    "tokens",
    ".claude/rules",
    ".claude/rules/accessibility.md",
    ".claude/rules/brand-and-operations.md",
    ".claude/rules/components.md",
    ".claude/rules/frameworks.md",
    ".claude/rules/review-and-research.md",
    ".claude/rules/tokens-and-color.md",
    ".claude/rules/typography-and-spacing.md",
    "workflows/redesign-audit.md",
    "frameworks",
    "taste",
    "taste/design-taste.md",
    "components",
    "scripts/gate.mjs",
    "scripts/accuracy_report.mjs",
];

type Hashes = BTreeMap<String, String>;

static NAME: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[a-z0-9]+(?:-[a-z0-9]+)*$").expect("static design skill name pattern"));

fn safe_path(value: &str) -> bool {
    !value.contains('\\')
        && !value.chars().any(char::is_control)
        && value.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    commit: String,
    skill_tree_hashes: Hashes,
    source: String,
    tracked_files: Hashes,
}

fn selected(lock: &[u8]) -> Result<Hashes> {
    let data = json(lock)?;
    let mut selected = Hashes::new();
    for (name, item) in lock_entries(&data)?
        .iter()
        .filter(|(_, value)| value["source"] == SOURCE)
    {
        let hash = item["skillFolderHash"].as_str().context("Invalid design skill hash")?;
        ensure!(
            NAME.is_match(name)
                && item["sourceType"] == "github"
                && item["sourceUrl"] == URL
                && item["skillPath"] == skill_file(name)
                && is_hex(hash, 40),
            "Invalid design skill lock entry: {name}"
        );
        selected.insert(name.clone(), hash.to_owned());
    }
    ensure!(
        !selected.is_empty() && selected.len() <= 256,
        "Missing or unbounded design skill selection"
    );
    Ok(selected)
}

fn skill_file(name: &str) -> String {
    format!(".claude/skills/{name}/SKILL.md")
}

fn check_core(root: &Path) -> Result<()> {
    for relative in REQUIRED {
        let path = root.join(relative);
        let meta = fs::symlink_metadata(&path).wrap_err_with(|| format!("Missing design resource {relative}"))?;
        ensure!(
            (meta.is_dir() || meta.is_file()) && path.canonicalize()? == path,
            "Unsafe design resource: {relative}"
        );
    }
    Ok(())
}

fn dependencies(root: &Path) -> Result<Hashes> {
    let manifest = json(&bytes(&root.join("package.json"), DATA_LIMIT)?)?;
    let lock = json(&bytes(&root.join("package-lock.json"), DATA_LIMIT)?)?;
    let packages = lock["packages"]
        .as_object()
        .context("Invalid design package lock metadata")?;
    let requirements = &packages.get("").context("Missing design package lock root")?["devDependencies"];
    ensure!(
        requirements == &manifest["devDependencies"],
        "Design manifest and locked dev dependencies differ"
    );
    for (path, package) in packages.iter().filter(|(path, _)| !path.is_empty()) {
        let resolved = package["resolved"].as_str().context("Missing design npm resolution")?;
        let integrity = package["integrity"].as_str().context("Missing design npm integrity")?;
        ensure!(
            safe_path(path)
                && path.starts_with("node_modules/")
                && resolved.starts_with("https://registry.npmjs.org/")
                && !resolved.chars().any(|ch| ch.is_whitespace() || ch.is_control())
                && package.get("link").is_none()
                && integrity.starts_with("sha512-"),
            "Untrusted design npm dependency resolution: {path}"
        );
    }
    if requirements.is_null() {
        return Ok(Hashes::new());
    }
    requirements
        .as_object()
        .context("Invalid design dev dependency map")?
        .keys()
        .map(|name| {
            let unscoped = |name: &str| {
                !name.is_empty()
                    && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
                    && !matches!(name, "." | "..")
            };
            let valid = if let Some((scope, short)) = name.strip_prefix('@').and_then(|value| value.split_once('/')) {
                unscoped(scope) && unscoped(short)
            } else {
                unscoped(name)
            };
            ensure!(valid, "Unsafe design npm package name");
            let path = format!("node_modules/{name}");
            let version = packages
                .get(&path)
                .and_then(|p| p["version"].as_str())
                .context("Missing exact dependency version")?;
            ensure!(
                semver::Version::parse(version).is_ok(),
                "Invalid exact design npm version"
            );
            Ok((name.clone(), version.to_owned()))
        })
        .collect()
}

fn verify_dependencies(root: &Path) -> Result<()> {
    for (name, expected) in dependencies(root)? {
        let installed = json(&bytes(
            &root.join("node_modules").join(&name).join("package.json"),
            DATA_LIMIT,
        )?)?;
        ensure!(
            installed["version"] == expected,
            "Design npm dependency version differs: {name}"
        );
    }
    Ok(())
}

fn release_manifest(root: &Path, commit: &str) -> Result<Manifest> {
    check_dir(root, false)?;
    check_core(root)?;
    let marker: Manifest = serde_json::from_value(json(&bytes(&root.join(MANIFEST), DATA_LIMIT)?)?)?;
    ensure!(
        marker.commit == commit
            && marker.source == SOURCE
            && is_hex(commit, 40)
            && !marker.skill_tree_hashes.is_empty()
            && !marker.tracked_files.is_empty()
            && marker.tracked_files.len() <= FILE_LIMIT,
        "Design kit release has unexpected identity or manifest"
    );
    for (name, hash) in &marker.skill_tree_hashes {
        ensure!(
            NAME.is_match(name) && is_hex(hash, 40) && marker.tracked_files.contains_key(&skill_file(name)),
            "Invalid design release skill entry"
        );
    }
    for (path, hash) in &marker.tracked_files {
        ensure!(
            safe_path(path) && is_hex(hash, 64) && digest(&bytes(&root.join(path), DATA_LIMIT)?) == *hash,
            "Design release file is missing or changed: {path}"
        );
    }
    verify_dependencies(root)?;
    Ok(marker)
}

fn current(root: &Path) -> Result<Option<String>> {
    let path = root.join("current");
    if !exists(&path)? {
        return Ok(None);
    }
    let target = fs::read_link(path)?;
    let target = target.to_str().context("Non-UTF-8 design current link")?;
    let commit = target
        .strip_prefix("releases/")
        .context("Unexpected design current link")?;
    ensure!(is_hex(commit, 40), "Unexpected design current revision");
    Ok(Some(commit.to_owned()))
}

fn switch(root: &Path, target: Option<&str>) -> Result<()> {
    let path = root.join("current");
    if let Some(commit) = target {
        ensure!(is_hex(commit, 40), "Invalid design activation target");
        let tmp = tempfile::tempdir_in(root)?;
        let link = tmp.path().join("current");
        symlink(format!("releases/{commit}"), &link)?;
        fs::rename(link, &path)?;
    } else if exists(&path)? {
        fs::remove_file(&path)?;
    }
    sync_directory(root)
}

fn git(home: &Path, repo: &Path, route: &Route) -> Result<Command> {
    let mut command = command(home, "git", Some(route))?;
    command.args([
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "credential.helper=",
        "-c",
        "http.sslVerify=true",
        "-c",
        "http.https://github.com/.sslVerify=true",
        "-c",
        "http.followRedirects=false",
        "-c",
        "http.https://github.com/.followRedirects=false",
    ]);
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_ALLOW_PROTOCOL", "https")
        .arg("-C")
        .arg(repo);
    Ok(command)
}

fn git_out(command: &mut Command, limit: u64) -> Result<Vec<u8>> {
    let out = capture(command, Duration::from_secs(180), limit)?;
    ensure!(
        out.status.success(),
        "Design source Git operation failed ({})",
        out.status
    );
    Ok(out.out)
}

fn git_text(command: &mut Command) -> Result<String> {
    Ok(String::from_utf8(git_out(command, DATA_LIMIT)?)?.trim().to_owned())
}

fn fetch(home: &Path, route: &Route) -> Result<(PathBuf, String)> {
    let source = home.join(CACHE);
    let parent = directory(
        home,
        Path::new(CACHE).parent().context("Missing design cache parent")?,
        true,
    )?;
    if !exists(&source)? {
        let stage = tempfile::tempdir_in(&parent)?;
        let repo = stage.path().join("source");
        git_out(
            git(home, stage.path(), route)?
                .args(["clone", "--no-checkout", "--no-tags", "--", URL])
                .arg(&repo),
            DATA_LIMIT,
        )?;
        ensure!(!exists(&source)?, "Design cache appeared while cloning");
        fs::rename(repo, &source)?;
        sync_directory(&parent)?;
    }
    check_dir(&source, false)?;
    check_dir(&source.join(".git"), false)?;
    ensure!(
        git_text(git(home, &source, route)?.args(["remote", "get-url", "origin"]))? == URL
            && git_text(git(home, &source, route)?.args(["ls-remote", "--get-url", URL]))? == URL,
        "Design source cache origin does not match the official URL"
    );
    git_out(
        git(home, &source, route)?.args(["fetch", "--no-tags", "--no-recurse-submodules", URL, "refs/heads/main"]),
        DATA_LIMIT,
    )?;
    let commit = git_text(git(home, &source, route)?.args(["rev-parse", "--verify", "FETCH_HEAD^{commit}"]))?;
    ensure!(is_hex(&commit, 40), "Invalid official design source SHA");
    Ok((source, commit))
}

fn extract(archive: Vec<u8>, stage: &Path) -> Result<Hashes> {
    let mut tar = tar::Archive::new(archive.as_slice());
    let mut seen = BTreeSet::new();
    let mut tracked = Hashes::new();
    let mut total = 0;
    for entry in tar.entries()? {
        let entry = entry?;
        let raw = String::from_utf8(entry.path_bytes().into_owned())?;
        let relative = raw.strip_suffix('/').unwrap_or(&raw);
        ensure!(
            safe_path(relative)
                && !matches!(relative, MANIFEST | "node_modules" | ".npmrc")
                && !relative.starts_with("node_modules/")
                && seen.insert(relative.to_owned())
                && seen.len() <= FILE_LIMIT,
            "Unsafe, duplicate, or excessive design archive member"
        );
        let path = stage.join(relative);
        if entry.header().entry_type().is_dir() {
            directory(stage, Path::new(relative), true)?;
            continue;
        }
        ensure!(
            entry.header().entry_type().is_file(),
            "Unsupported design archive member"
        );
        total += entry.size();
        ensure!(total <= ARCHIVE_LIMIT, "Design archive exceeds its extraction limit");
        directory(
            stage,
            Path::new(relative).parent().context("Invalid design archive parent")?,
            true,
        )?;
        let mode = if entry.header().mode()? & 0o111 == 0 {
            0o644
        } else {
            0o755
        };
        let content = bounded(entry, DATA_LIMIT)?;
        let mut file = OpenOptions::new().create_new(true).write(true).mode(mode).open(path)?;
        file.write_all(&content)?;
        file.sync_all()?;
        tracked.insert(relative.to_owned(), digest(&content));
    }
    Ok(tracked)
}

fn export(home: &Path, root: &Path, source: &Path, commit: &str, selected: &Hashes, route: &Route) -> Result<Manifest> {
    let releases = directory(root, Path::new("releases"), true)?;
    let release = releases.join(commit);
    for (name, expected) in selected {
        let tree = format!("{commit}:.claude/skills/{name}");
        ensure!(
            git_text(git(home, source, route)?.args(["rev-parse", "--verify", &tree]))? == *expected,
            "Official design source and skill lock disagree for {name}"
        );
    }
    if !exists(&release)? {
        let stage = tempfile::tempdir_in(&releases)?;
        // Archiving a commit adds a PAX global header. Export its verified tree instead.
        let tree = format!("{commit}^{{tree}}");
        let payload = git_out(
            git(home, source, route)?.args(["archive", "--format=tar", &tree]),
            ARCHIVE_LIMIT,
        )?;
        let tracked = extract(payload, stage.path())?;
        check_core(stage.path())?;
        dependencies(stage.path())?;
        let configs = tempfile::tempdir()?;
        let mut npm = command(home, "npm", Some(route))?;
        npm_config(&mut npm, configs.path())?;
        let npm = capture(
            npm.current_dir(stage.path()).args(["ci", "--include=dev"]),
            Duration::from_secs(600),
            DATA_LIMIT,
        )?;
        ensure!(
            npm.status.success(),
            "Design kit dependency installation failed ({})",
            npm.status
        );
        let manifest = Manifest {
            commit: commit.to_owned(),
            skill_tree_hashes: selected.clone(),
            source: SOURCE.to_owned(),
            tracked_files: tracked,
        };
        atomic_write(&stage.path().join(MANIFEST), &serde_json::to_vec(&manifest)?, 0o644)?;
        release_manifest(stage.path(), commit)?;
        ensure!(!exists(&release)?, "Design release appeared while staging");
        fs::rename(stage.path(), &release)?;
        sync_directory(&releases)?;
    }
    let manifest = release_manifest(&release, commit)?;
    ensure!(
        &manifest.skill_tree_hashes == selected,
        "Existing design release does not match selected skill hashes"
    );
    Ok(manifest)
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt {
    commit: String,
    name: String,
    rendered_sha256: String,
    source: String,
    source_sha256: String,
    source_tree_hash: String,
    version: u32,
}

fn render(root: &Path, commit: &str, name: &str, hash: &str, manifest: &Manifest) -> Result<(Vec<u8>, Receipt)> {
    let path = skill_file(name);
    ensure!(
        manifest.tracked_files.contains_key(&path),
        "Design manifest omits skill source {name}"
    );
    let source = bytes(&root.join("releases").join(commit).join(path), DATA_LIMIT)?;
    let replacement = format!("{}/current/", root.to_str().context("Non-UTF-8 design kit path")?);
    let rendered = String::from_utf8(source.clone())?
        .replace(PREFIX, &replacement)
        .into_bytes();
    let receipt = Receipt {
        commit: commit.to_owned(),
        name: name.to_owned(),
        rendered_sha256: digest(&rendered),
        source: SOURCE.to_owned(),
        source_sha256: digest(&source),
        source_tree_hash: hash.to_owned(),
        version: 1,
    };
    Ok((rendered, receipt))
}

fn read_receipt(raw: &[u8]) -> Result<Receipt> {
    let receipt: Receipt = serde_json::from_value(json(raw)?)?;
    ensure!(
        receipt.version == 1
            && receipt.source == SOURCE
            && is_hex(&receipt.commit, 40)
            && is_hex(&receipt.rendered_sha256, 64)
            && is_hex(&receipt.source_sha256, 64)
            && is_hex(&receipt.source_tree_hash, 40)
            && NAME.is_match(&receipt.name),
        "Invalid design skill receipt"
    );
    Ok(receipt)
}

fn paths(home: &Path, root: &Path, name: &str) -> Result<(PathBuf, PathBuf)> {
    ensure!(NAME.is_match(name), "Unsafe design skill path");
    Ok((
        directory(home, &Path::new(".agents/skills").join(name), false)?.join("SKILL.md"),
        root.join("receipts").join(format!("{name}.json")),
    ))
}

fn check(home: &Path, root: &Path) -> Result<()> {
    let lock = bytes(&lock_path(home)?, DATA_LIMIT)?;
    let selected = selected(&lock)?;
    let commit = current(root)?.context("Complete design kit current release is absent")?;
    let manifest = release_manifest(&root.join("releases").join(&commit), &commit)?;
    for (name, hash) in &selected {
        ensure!(
            manifest.skill_tree_hashes.get(name) == Some(hash),
            "Current design kit does not match lock: {name}"
        );
        let (skill, receipt) = paths(home, root, name)?;
        let (rendered, expected) = render(root, &commit, name, hash, &manifest)?;
        let receipt = read_receipt(&bytes(&receipt, DATA_LIMIT)?)?;
        ensure!(
            bytes(&skill, DATA_LIMIT)? == rendered
                && receipt.name == *name
                && receipt.commit == commit
                && receipt.source_tree_hash == *hash
                && receipt.source_sha256 == expected.source_sha256
                && receipt.rendered_sha256 == expected.rendered_sha256,
            "Installed design skill or receipt differs: {name}"
        );
    }
    ensure!(
        bytes(&lock_path(home)?, DATA_LIMIT)? == lock && current(root)?.as_deref() == Some(&commit),
        "Design skill lock or activation changed during verification"
    );
    println!("OK: {} skills resolve against kit {commit}", selected.len());
    Ok(())
}

pub(super) fn verify(home: &Path) -> Result<()> {
    let root = directory(home, Path::new(KIT), false)?;
    ensure!(
        !exists(&root.join(PENDING))?,
        "Design kit has an incomplete activation; run its guarded sync"
    );
    check(home, &root)
}

mod transaction;

pub(super) fn sync(home: &Path) -> Result<()> {
    let root = directory(home, Path::new(KIT), true)?;
    let lock_path = root.join(".sync.lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(&lock_path)?;
    check_file(&lock, &lock_path, false, 0)?;
    lock.try_lock().wrap_err("Another design kit sync is running")?;
    transaction::recover(home, &root)?;
    transaction::sync(home, &root)
}

#[cfg(test)]
#[test]
#[ignore = "read-only check for an enrolled local design kit"]
fn current_installed_design_kit_verifies() {
    verify(&account_home().unwrap()).unwrap();
}

#[cfg(test)]
#[test]
fn design_archive_rejects_symlinks_and_retains_checked_file_mode() -> Result<()> {
    let source = tempfile::tempdir()?;
    let git = |args: &[&str]| -> Result<Vec<u8>> {
        let output = Command::new("/usr/bin/git")
            .current_dir(source.path())
            .env_clear()
            .env("HOME", source.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .output()?;
        ensure!(output.status.success(), "Synthetic Git fixture failed: {args:?}");
        Ok(output.stdout)
    };
    git(&["init", "--template=", "--quiet"])?;
    let payload = b"#!/bin/sh\n";
    fs::create_dir(source.path().join("scripts"))?;
    fs::write(source.path().join("scripts/gate"), payload)?;
    git(&["add", "--chmod=+x", "scripts/gate"])?;
    let tree = String::from_utf8(git(&["write-tree"])?)?;
    let safe = git(&["archive", "--format=tar", tree.trim()])?;
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    let files = extract(safe, &root)?;
    assert_eq!(files["scripts/gate"], digest(payload));
    assert_eq!(
        fs::metadata(root.join("scripts/gate"))?.permissions().mode() & 0o100,
        0o100
    );

    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_size(0);
    builder.append_link(&mut header, "escape", "/etc")?;
    assert!(extract(builder.into_inner()?, &root).is_err());
    assert!(!exists(&root.join("escape"))?);
    Ok(())
}
