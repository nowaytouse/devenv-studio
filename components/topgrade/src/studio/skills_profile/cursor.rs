use super::*;
use crate::studio::{
    github_skills::ownership::{
        CURSOR_NAMES, REVIEWED_REX_SHA, USER_CONFIG, lean_ctx_mirror, lean_ctx_mirror_path, name_matches, repository,
        reviewed_local_rex,
    },
    is_hex,
};
use chrono::Utc;
use regex::Regex;
use serde_json::json;
use std::{collections::BTreeMap, ffi::CString, os::unix::ffi::OsStrExt, sync::LazyLock};
use walkdir::WalkDir;

const SHARED: &str = ".agents/skills";
const NATIVE: &str = ".cursor/skills-cursor";
const OLD_BABYSIT: &str = "626915d181d87db48def05ceec5fbbdc873dd445";
const TREE_LIMIT: usize = 4096;
const MAX_FILE: u64 = 32 * 1024 * 1024;
const MAX_TREE: u64 = 128 * 1024 * 1024;
static MISSING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^!\s+(.*?)\s+has no GitHub metadata\.").expect("static missing pattern"));
static SKIPPED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^!\s+Skipping\s+([^:]+):\s*(.+)$").expect("static skipped pattern"));
static ANSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").expect("static ANSI pattern"));

#[derive(PartialEq, Eq)]
struct Entry {
    hash: String,
    mode: u32,
}

type Tree = BTreeMap<PathBuf, Entry>;

fn tree(root: &Path) -> Result<Tree> {
    check_dir(root, false)?;
    ensure!(root.canonicalize()? == root, "Unexpected Cursor skill root");
    let mut files = Tree::new();
    let mut total = 0;
    for (idx, entry) in WalkDir::new(root).min_depth(1).max_depth(33).into_iter().enumerate() {
        let entry = entry?;
        ensure!(
            idx < TREE_LIMIT && entry.depth() <= 32,
            "Cursor skill tree exceeds its bounds"
        );
        if entry.file_type().is_dir() {
            check_dir(entry.path(), false)?;
            continue;
        }
        ensure!(entry.file_type().is_file(), "Unsafe entry inside Cursor skill");
        let file = open_file(entry.path(), false, MAX_FILE)?;
        let metadata = file.metadata()?;
        let content = bounded(file, MAX_FILE)?;
        total += content.len() as u64;
        ensure!(total <= MAX_TREE, "Cursor skill tree exceeds its byte limit");
        files.insert(
            entry.path().strip_prefix(root)?.to_path_buf(),
            Entry {
                hash: digest(&content),
                mode: metadata.permissions().mode() & 0o777,
            },
        );
    }
    ensure!(
        files.contains_key(Path::new("SKILL.md")),
        "Cursor skill tree lacks SKILL.md"
    );
    Ok(files)
}

fn config(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| USER_CONFIG.contains(&name))
}

fn equivalent(native: &Tree, shared: &Tree) -> bool {
    native
        .iter()
        .all(|(path, entry)| config(path) || shared.get(path).is_some_and(|actual| actual.hash == entry.hash))
        && shared.keys().all(|path| config(path) || native.contains_key(path))
}

fn copy(root: &Path, stage: &Path, path: &Path, entry: &Entry) -> Result<()> {
    let data = bytes(&root.join(path), MAX_FILE)?;
    ensure!(digest(&data) == entry.hash, "Cursor skill changed while copying");
    directory(stage, path.parent().context("Missing Cursor file parent")?, true)?;
    // The installed user's configuration may have a tighter mode than Cursor's bundled copy.
    atomic_write(&stage.join(path), &data, entry.mode)
}

fn exchange(left: &Path, right: &Path) -> Result<()> {
    let left = CString::new(left.as_os_str().as_bytes())?;
    let right = CString::new(right.as_os_str().as_bytes())?;
    // One rename keeps the prior installation at the private backup path even on abrupt exit.
    if unsafe { nix::libc::renamex_np(left.as_ptr(), right.as_ptr(), nix::libc::RENAME_SWAP) } != 0 {
        return Err(io::Error::last_os_error()).context("Atomically exchange Cursor skill and backup");
    }
    Ok(())
}

fn mirror(native: &Path, shared: &Path, name: &str, backup_dir: &Path) -> Result<&'static str> {
    let source = native.join(name);
    let target = shared.join(name);
    let native_before = tree(&source)?;
    let shared_before = tree(&target)?;
    ensure!(
        name_matches(&source, name)? && name_matches(&target, name)?,
        "Cursor skill identity mismatch"
    );
    if equivalent(&native_before, &shared_before) {
        return Ok("CURRENT");
    }
    let stage = tempfile::tempdir_in(backup_dir)?;
    for (path, entry) in &native_before {
        if let Some(local) = shared_before.get(path).filter(|_| config(path)) {
            copy(&target, stage.path(), path, local)?;
        } else {
            copy(&source, stage.path(), path, entry)?;
        }
    }
    for (path, entry) in shared_before
        .iter()
        .filter(|(path, _)| config(path) && !native_before.contains_key(*path))
    {
        copy(&target, stage.path(), path, entry)?;
    }
    let staged = tree(stage.path())?;
    ensure!(
        equivalent(&native_before, &staged) && tree(&source)? == native_before && tree(&target)? == shared_before,
        "Cursor source or installation changed during staging"
    );
    let backup = backup_dir.join(name);
    ensure!(!exists(&backup)?, "Cursor backup path is occupied");
    fs::rename(stage.path(), &backup)?;
    sync_directory(backup_dir)?;
    exchange(&target, &backup)?;
    let verified = (|| -> Result<()> {
        ensure!(
            tree(&backup)? == shared_before,
            "Original Cursor skill failed backup verification"
        );
        ensure!(
            tree(&target)? == staged,
            "Installed Cursor skill failed read-back verification"
        );
        sync_directory(shared)?;
        sync_directory(backup_dir)
    })();
    if let Err(error) = verified {
        ensure!(
            tree(&target)? == staged && tree(&backup)? == shared_before,
            "Cursor exchange verification failed ({error:#}); preserved both trees for review"
        );
        exchange(&target, &backup)?;
        sync_directory(shared)?;
        sync_directory(backup_dir)?;
        return Err(error);
    }
    Ok("UPDATED")
}

fn archive_babysit(home: &Path, native: &Path, shared: &Path, backup: &Path) -> Result<&'static str> {
    let source = shared.join("babysit");
    if !exists(&source)? {
        return Ok("ALREADY_ABSENT");
    }
    let before = tree(&source)?;
    let markdown = source.join("SKILL.md");
    let hash = capture(
        command(home, "git", None)?
            .args(["hash-object", "--no-filters", "--"])
            .arg(&markdown),
        Duration::from_secs(15),
        1024,
    )?;
    ensure!(
        hash.status.success()
            && String::from_utf8(hash.out)?.trim() == OLD_BABYSIT
            && before.len() == 1
            && !exists(&native.join("babysit/SKILL.md"))?
            && tree(&source)? == before,
        "Existing babysit skill differs from the obsolete reviewed copy; preserved"
    );
    let archived = backup.join("babysit");
    ensure!(!exists(&archived)?, "Babysit backup path is occupied");
    fs::rename(source, &archived)?;
    sync_directory(backup)?;
    sync_directory(shared)?;
    Ok("ARCHIVED_OBSOLETE_COPY")
}

fn report_dir(home: &Path, path: &str) -> Result<PathBuf> {
    let root = directory(home, Path::new(path), true)?;
    let report = tempfile::Builder::new().prefix("run-").tempdir_in(&root)?.keep();
    sync_directory(&root)?;
    Ok(report)
}

fn npx_owned(home: &Path, row: &Row, entry: &Value) -> bool {
    let kind = entry["sourceType"].as_str().unwrap_or_default();
    let has_text = |key| entry[key].as_str().is_some_and(|value| !value.is_empty());
    let recorded = has_text("source")
        && match kind {
            "well-known" => has_text("wellKnownDigest"),
            "github" | "git" | "gitlab" => has_text("skillPath") && has_text("skillFolderHash"),
            _ => false,
        };
    if !recorded {
        return false;
    }
    let in_root = |path: &Path| {
        [SHARED, ".claude/skills", ".codex/skills", ".cursor/skills"]
            .iter()
            .any(|root| {
                path.strip_prefix(home.join(root))
                    .is_ok_and(|relative| relative.components().count() > 0)
            })
    };
    in_root(&row.path)
        && row
            .path
            .canonicalize()
            .is_ok_and(|actual| in_root(&actual) && open_file(&actual.join("SKILL.md"), false, DATA_LIMIT).is_ok())
}

fn npx_lean_ctx_mirror(home: &Path, row: &Row, entry: &Value) -> Result<bool> {
    if row.skill_name != "lean-ctx"
        || !lean_ctx_mirror_path(home, &row.path)
        || entry["sourceType"] != "github"
        || entry["source"] != "yvgude/lean-ctx"
        || entry["skillPath"] != "skills/lean-ctx/SKILL.md"
        || !entry["sourceUrl"]
            .as_str()
            .is_some_and(|url| repository(url).is_ok_and(|repo| repo == "yvgude/lean-ctx"))
        || !entry["skillFolderHash"].as_str().is_some_and(|hash| is_hex(hash, 40))
    {
        return Ok(false);
    }
    lean_ctx_mirror(&home.join(SHARED).join("lean-ctx"), &row.path)
}

fn normalized_origin(url: &str) -> String {
    url.trim_end_matches('/').trim_end_matches(".git").to_ascii_lowercase()
}

fn origin_matches(source: &str, entry: &Value) -> bool {
    let source = source.replace("https://www.github.com/", "https://github.com/");
    repository(&source).is_ok_and(|repo| {
        entry["source"]
            .as_str()
            .is_some_and(|owner| repo == normalized_origin(owner.trim_start_matches('/')))
    })
}

fn retained_local_rex(home: &Path, row: &Row, sha: &str) -> Result<bool> {
    if row.skill_name != "rex" || row.pinned || row.source_url.as_deref().is_some_and(|source| !source.is_empty()) {
        return Ok(false);
    }
    reviewed_local_rex(home, &row.path, row.scope.as_deref().unwrap_or_default(), sha)
}

fn audit(home: &Path, native: &Path, shared: &Path) -> Result<()> {
    let rows = rows(home, None)?;
    let lock_path = lock_path(home)?;
    let lock = bytes(&lock_path, DATA_LIMIT)?;
    let data = super::json(&lock)?;
    let entries = lock_entries(&data)?;
    let dry = capture(
        command(home, "gh", None)?.args(["skill", "update", "--all", "--dry-run"]),
        Duration::from_secs(600),
        DATA_LIMIT,
    )?;
    let mut raw = format!("{}\n{}", String::from_utf8(dry.out)?, String::from_utf8(dry.err)?);
    for key in ["GH_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(secret) = std::env::var(key)
            && !secret.is_empty()
        {
            raw = raw.replace(&secret, "[REDACTED]");
        }
    }
    let plain = ANSI.replace_all(&raw, "").replace('\r', "\n");
    let skipped: BTreeMap<_, _> = SKIPPED
        .captures_iter(&plain)
        .map(|m| (m[1].to_owned(), m[2].to_owned()))
        .collect();
    let failed: BTreeMap<_, _> = rows
        .iter()
        .filter_map(|row| {
            let source = row.source_url.as_ref().filter(|value| !value.is_empty())?;
            skipped
                .get(&row.skill_name)
                .map(|reason| (normalized_origin(source), reason))
        })
        .collect();
    let mut counts = BTreeMap::new();
    let mut unresolved = Vec::new();
    let mut all = Vec::new();
    let mut untracked = BTreeMap::new();
    for row in &rows {
        let name = &row.skill_name;
        let source = row.source_url.as_deref().filter(|source| !source.is_empty());
        let entry = entries.get(name).unwrap_or(&Value::Null);
        let mirror = source.is_none() && !row.pinned && npx_lean_ctx_mirror(home, row, entry)?;
        let npx = if source.is_none() && lean_ctx_mirror_path(home, &row.path) {
            mirror
        } else {
            npx_owned(home, row, entry)
        };
        if source.is_none() {
            *untracked.entry(name.as_str()).or_insert(0usize) += 1;
        }
        let reason = source.and_then(|value| failed.get(&normalized_origin(value))).copied();
        let mut cursor_source = None;
        let status = if reason.is_some() {
            if npx && source.is_some_and(|value| origin_matches(value, entry)) {
                "NPX_OVERLAP_SOURCE_UNDISCOVERABLE"
            } else {
                "GH_SOURCE_UNDISCOVERABLE"
            }
        } else if row.pinned {
            "PINNED_NOT_LATEST"
        } else if source.is_some() {
            "GH_TRACKED"
        } else if mirror {
            "NPX_MIRROR_VERIFIED"
        } else if npx {
            "NPX_TRACKED"
        } else if retained_local_rex(home, row, REVIEWED_REX_SHA)? {
            "REVIEWED_LOCAL_RETAINED_NOT_UPDATED"
        } else if CURSOR_NAMES.contains(&name.as_str()) && row.path == shared.join(name) {
            let native = native.join(name);
            if name_matches(&native, name)? && equivalent(&tree(&native)?, &tree(&row.path)?) {
                cursor_source = Some(native);
                "CURSOR_NATIVE_MIRROR_VERIFIED"
            } else {
                "NO_VERIFIED_UPDATER"
            }
        } else {
            "NO_VERIFIED_UPDATER"
        };
        let mut report = json!({
            "skill": name, "path": row.path, "status": status, "gh_source": source,
            "npx_source": npx.then(|| entry["source"].clone()), "reason": reason,
        });
        if let Some(path) = cursor_source {
            report["cursor_source"] = json!(path);
        }
        if matches!(
            status,
            "NO_VERIFIED_UPDATER" | "GH_SOURCE_UNDISCOVERABLE" | "PINNED_NOT_LATEST"
        ) {
            unresolved.push(report.clone());
        }
        *counts.entry(status).or_insert(0usize) += 1;
        all.push(report);
    }
    let mut errors = Vec::new();
    if !dry.status.success() {
        errors.push(format!("GitHub skill dry run exited {}", dry.status));
    }
    if plain.lines().any(|line| line.trim_start().starts_with("Error:")) {
        errors.push("GitHub skill dry run reported an error".into());
    }
    let missing: Vec<_> = MISSING.captures_iter(&plain).collect();
    for warning in &missing {
        match untracked.get_mut(&warning[1]) {
            Some(remaining) if *remaining > 0 => *remaining -= 1,
            _ => errors.push(format!(
                "GitHub warning has no installed source-less row: {}",
                &warning[1]
            )),
        }
    }
    for name in skipped.keys() {
        if !rows
            .iter()
            .any(|row| &row.skill_name == name && row.source_url.as_ref().is_some_and(|source| !source.is_empty()))
        {
            errors.push(format!("GitHub skip has no installed source-bearing row: {name}"));
        }
    }
    if bytes(&lock_path, DATA_LIMIT)? != lock {
        errors.push("Shared skill lock changed during coverage audit".into());
    }
    let report_dir = report_dir(home, ".local/state/topgrade-skills-integrity")?;
    atomic_write(&report_dir.join("gh-dry-run.log"), raw.as_bytes(), 0o600)?;
    atomic_write(
        &report_dir.join("report.json"),
        &serde_json::to_vec_pretty(&json!({
            "checked_at": Utc::now().to_rfc3339(),
            "installed_entries": rows.len(),
            "gh_missing_source_warnings": missing.len(),
            "gh_skipped_source_warnings": SKIPPED.captures_iter(&plain).count(),
            "gh_exit_code": dry.status.code(),
            "statuses": counts,
            "unresolved": unresolved,
            "all_entries": all,
            "errors": errors,
            "note": "Ownership audit. Reviewed local retained entries were not updated or checked for newer versions. Separate updater steps must still succeed.",
        }))?,
        0o600,
    )?;
    println!("Skills update coverage: {} installed entries", rows.len());
    for (status, count) in counts {
        println!("  {status}: {count}");
    }
    println!("Full report: {}", report_dir.join("report.json").display());
    ensure!(
        errors.is_empty() && unresolved.is_empty(),
        "Skills coverage audit has unresolved entries or warnings"
    );
    Ok(())
}

pub(super) fn run(home: &Path) -> Result<()> {
    let native = directory(home, Path::new(NATIVE), false)?;
    let shared = directory(home, Path::new(SHARED), false)?;
    let report = report_dir(home, ".local/state/topgrade-cursor-skills")?;
    let backup = directory(&report, Path::new("backups"), true)?;
    let mut counts = BTreeMap::new();
    let mut results = Vec::new();
    let mut failed = false;
    for name in CURSOR_NAMES {
        let status = match mirror(&native, &shared, name, &backup) {
            Ok(status) => {
                results.push(json!({"name": name, "status": status}));
                status
            }
            Err(error) => {
                failed = true;
                eprintln!("FAILED {name}: {error:#}");
                results.push(json!({"name": name, "status": "FAILED", "error": format!("{error:#}")}));
                "FAILED"
            }
        };
        *counts.entry(status).or_insert(0usize) += 1;
        println!("{status:7} {name}");
    }
    match archive_babysit(home, &native, &shared, &backup) {
        Ok(status) => results.push(json!({"name": "babysit", "status": status})),
        Err(error) => {
            failed = true;
            eprintln!("FAILED babysit: {error:#}");
            results.push(json!({"name": "babysit", "status": "FAILED", "error": format!("{error:#}")}));
        }
    }
    atomic_write(
        &report.join("sync-report.json"),
        &serde_json::to_vec_pretty(&json!({
            "results": results, "summary": counts, "archive_babysit": true, "backup": backup,
        }))?,
        0o600,
    )?;
    println!(
        "Cursor skill sync report: {}",
        report.join("sync-report.json").display()
    );
    audit(home, &native, &shared)?;
    ensure!(!failed, "At least one Cursor skill did not synchronize");
    Ok(())
}

#[cfg(test)]
#[test]
fn mirror_keeps_configuration_and_archives_previous_tree() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let home = fixture.path().canonicalize()?;
    let native = directory(&home, Path::new("native"), true)?;
    let shared = directory(&home, Path::new("shared"), true)?;
    let backup = directory(&home, Path::new("backups"), true)?;
    for (root, content) in [(&native, "new"), (&shared, "old")] {
        let path = directory(root, Path::new("design"), true)?;
        fs::write(path.join("SKILL.md"), format!("---\nname: design\n---\n{content}\n"))?;
        fs::write(path.join(".env"), content)?;
    }
    assert_eq!(mirror(&native, &shared, "design", &backup)?, "UPDATED");
    assert_eq!(bytes(&shared.join("design/.env"), MAX_FILE)?, b"old");
    assert!(String::from_utf8(bytes(&shared.join("design/SKILL.md"), MAX_FILE)?)?.ends_with("new\n"));
    assert!(String::from_utf8(bytes(&backup.join("design/SKILL.md"), MAX_FILE)?)?.ends_with("old\n"));
    assert_eq!(mirror(&native, &shared, "design", &backup)?, "CURRENT");
    Ok(())
}

#[cfg(test)]
#[test]
fn coverage_requires_exact_lean_ctx_mirror_and_owner() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let home = fixture.path().canonicalize()?;
    let canonical = directory(&home, Path::new(".agents/skills/lean-ctx"), true)?;
    let entry = json!({
        "sourceType": "github",
        "source": "yvgude/lean-ctx",
        "sourceUrl": "https://github.com/yvgude/lean-ctx.git",
        "skillPath": "skills/lean-ctx/SKILL.md",
        "skillFolderHash": "a".repeat(40),
    });
    let skill = b"---\nname: lean-ctx\n---\nVerified content\n";
    fs::write(canonical.join("SKILL.md"), skill)?;
    for relative in [
        ".cursor/skills/lean-ctx",
        ".copilot/skills/lean-ctx",
        ".config/opencode/skills/lean-ctx",
    ] {
        let mirror = directory(&home, Path::new(relative), true)?;
        let row = Row {
            skill_name: "lean-ctx".to_owned(),
            source_url: None,
            path: mirror.clone(),
            pinned: false,
            scope: None,
        };
        fs::write(mirror.join("SKILL.md"), skill)?;
        fs::write(mirror.join("SKILL.md.bak"), "Local backup")?;
        assert!(npx_lean_ctx_mirror(&home, &row, &entry)?);
        let mut foreign = entry.clone();
        foreign["sourceUrl"] = json!("https://github.com/other/lean-ctx");
        assert!(!npx_lean_ctx_mirror(&home, &row, &foreign)?);
        fs::write(mirror.join("SKILL.md"), "Drift")?;
        assert!(!npx_lean_ctx_mirror(&home, &row, &entry)?);
        fs::write(mirror.join("SKILL.md.v4-migration.lock"), "Unexpected")?;
        assert!(npx_lean_ctx_mirror(&home, &row, &entry).is_err());
    }
    Ok(())
}

#[cfg(test)]
#[test]
fn reviewed_local_rex_coverage_accepts_home_relative_project_scope_for_exact_copies() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().canonicalize()?;
    let canonical = home.join(".agents/skills/rex");
    let mirror = home.join(".claude/skills/rex");
    fs::create_dir_all(&canonical)?;
    fs::create_dir_all(&mirror)?;
    let content = b"synthetic reviewed fixture";
    fs::write(canonical.join("SKILL.md"), content)?;
    fs::write(mirror.join("SKILL.md"), content)?;
    let hash = digest(content);
    let mut row = Row {
        skill_name: "rex".into(),
        source_url: None,
        path: canonical.clone(),
        pinned: false,
        scope: Some("user".into()),
    };
    assert!(retained_local_rex(&home, &row, &hash)?);
    row.path = mirror.clone();
    assert!(retained_local_rex(&home, &row, &hash)?);
    row.scope = Some("project".into());
    assert!(retained_local_rex(&home, &row, &hash)?);
    row.scope = Some("unknown".into());
    assert!(!retained_local_rex(&home, &row, &hash)?);
    row.scope = None;
    assert!(!retained_local_rex(&home, &row, &hash)?);
    row.scope = Some("user".into());
    row.pinned = true;
    assert!(!retained_local_rex(&home, &row, &hash)?);
    row.pinned = false;
    row.source_url = Some("https://example.invalid/repo".into());
    assert!(!retained_local_rex(&home, &row, &hash)?);
    row.source_url = None;
    row.path = home.join("rex");
    assert!(!retained_local_rex(&home, &row, &hash)?);
    row.path = mirror.clone();
    fs::write(mirror.join("extra"), b"unknown")?;
    assert!(!retained_local_rex(&home, &row, &hash)?);
    fs::remove_file(mirror.join("extra"))?;
    fs::write(mirror.join("SKILL.md"), b"unknown")?;
    assert!(!retained_local_rex(&home, &row, &hash)?);
    Ok(())
}

#[cfg(test)]
#[test]
#[ignore = "read-only check for the enrolled local Cursor mirrors"]
fn current_cursor_mirrors_verify() -> Result<()> {
    let home = account_home()?;
    let native = directory(&home, Path::new(NATIVE), false)?;
    let shared = directory(&home, Path::new(SHARED), false)?;
    for name in CURSOR_NAMES {
        let source = native.join(name);
        let target = shared.join(name);
        ensure!(
            name_matches(&source, name)?
                && name_matches(&target, name)?
                && equivalent(&tree(&source)?, &tree(&target)?),
            "Installed Cursor mirror differs: {name}"
        );
    }
    Ok(())
}
