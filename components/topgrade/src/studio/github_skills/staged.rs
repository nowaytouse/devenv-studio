use super::{Item, Output, Snapshot, hex, metadata};
use crate::{
    error::PartialStep,
    studio::{
        check_dir,
        skills_profile::cursor::{Tree, exchange, tree},
        state_dir,
    },
};
use color_eyre::eyre::{Context, ContextCompat, Result, ensure};
use std::{fs, path::Path};

fn install(
    home: &Path,
    root: &Path,
    item: &Item,
    reference: &str,
    expected: &str,
    run: &mut impl FnMut(&Path, &[&str]) -> Result<Output>,
) -> Result<()> {
    let path = item.skill_path();
    ensure!(!path.contains('@'), "Ambiguous skill source path");
    let source = item
        .meta
        .source
        .strip_prefix("https://github.com/")
        .context("Invalid GitHub source")?;
    let source = source.strip_suffix('/').unwrap_or(source);
    let source = source.strip_suffix(".git").unwrap_or(source);
    fs::create_dir(root)?;
    let output = run(
        home,
        &[
            "skill",
            "install",
            source,
            &format!("{path}@{reference}"),
            "--dir",
            root.to_str().context("Non-UTF-8 skill staging directory")?,
        ],
    )?;
    ensure!(
        output.success,
        "Staged GitHub skill install failed (exit {:?}{})",
        output.code,
        if output.stderr.contains("rate limit") {
            ", GitHub API rate limit"
        } else {
            ""
        }
    );
    let installed = metadata(&root.join(&item.meta.name))?;
    ensure!(
        installed.name == item.meta.name
            && super::ownership::repository(&installed.source)? == item.repo
            && installed.directory == item.meta.directory
            && installed.tree == expected,
        "Staged skill identity or revision could not be verified"
    );
    Ok(())
}

fn prepare(
    scratch: &Path,
    backup: &Path,
    item: &Item,
    run: &mut impl FnMut(&Path, &[&str]) -> Result<Output>,
) -> Result<(Tree, Tree)> {
    let reference = item.meta.reference.as_deref().context("Missing installed GitHub ref")?;
    ensure!(
        (reference.starts_with("refs/tags/") || reference.starts_with("refs/heads/") || hex(reference, 40))
            && reference.len() <= 1024
            && !reference.contains(['@', '\\'])
            && !reference.chars().any(char::is_whitespace),
        "Unsupported installed GitHub ref"
    );
    // Older branch-based receipts can be reconstructed only while the ref still identifies the installed tree.
    let old = scratch.join("original");
    install(scratch, &old, item, reference, &item.meta.tree, run)
        .context("Could not reconstruct the installed revision")?;
    let original = tree(&old.join(&item.meta.name))?;
    ensure!(
        installed_tree(&item.path)? == original,
        "Installed files differ from the original revision; kept local changes"
    );
    let new = scratch.join("candidate");
    install(scratch, &new, item, &item.commit, &item.expected, run)?;
    let candidate = new.join(&item.meta.name);
    let staged = tree(&candidate)?;
    for path in staged.keys() {
        fs::File::open(candidate.join(path))?.sync_all()?;
    }
    fs::File::open(&candidate)?.sync_all()?;
    super::super::preview::move_exclusive(&candidate, backup)?;
    Ok((original, staged))
}

fn installed_tree(path: &Path) -> Result<Tree> {
    let content = tree(path)?;
    for entry in walkdir::WalkDir::new(path).min_depth(1) {
        let entry = entry?;
        if entry.file_type().is_dir() {
            ensure!(
                fs::read_dir(entry.path())?.next().transpose()?.is_some(),
                "Installed skill contains a local empty directory"
            );
        }
    }
    Ok(content)
}

pub(super) fn update(
    home: &Path,
    items: &[Item],
    snapshot: &Snapshot,
    run: &mut impl FnMut(&Path, &[&str]) -> Result<Output>,
) -> Result<usize> {
    let mut pending = items.iter().filter(|item| item.changed()).peekable();
    if pending.peek().is_none() {
        return Ok(0);
    }
    let state = state_dir(home)?;
    let backups = tempfile::Builder::new()
        .prefix("gh-skill-updates-")
        .tempdir_in(&state)?
        .keep();
    let mut updated = 0;
    let mut deferred = Vec::new();
    for (index, item) in pending.enumerate() {
        let parent = item.path.parent().context("Missing GitHub skill parent")?;
        check_dir(parent, false)?;
        ensure!(
            &Snapshot::read(home)? == snapshot,
            "Shared skill lock changed during update"
        );
        let scratch = tempfile::tempdir_in(&backups)?;
        let backup = backups.join(format!("{index}-{}", item.meta.name));
        let (original, staged) = match prepare(scratch.path(), &backup, item, run) {
            Ok(result) => result,
            Err(error) => {
                deferred.push(format!("{} ({error:#})", item.path.display()));
                continue;
            }
        };
        ensure!(
            &Snapshot::read(home)? == snapshot,
            "Shared skill lock changed during staging"
        );
        if installed_tree(&item.path)? != original {
            deferred.push(format!(
                "{} (installed files changed during staging)",
                item.path.display()
            ));
            continue;
        }
        fs::File::open(&backups)?.sync_all()?;
        exchange(&item.path, &backup)?;
        let active = tree(&item.path);
        let saved = installed_tree(&backup);
        let active_matches = active.is_ok_and(|files| files == staged);
        if !active_matches || !saved.is_ok_and(|files| files == original) {
            if active_matches {
                exchange(&item.path, &backup)?;
            }
            color_eyre::eyre::bail!(
                "Skill exchange verification failed: {}. Backup: {}",
                item.path.display(),
                backup.display()
            );
        }
        fs::File::open(parent)?.sync_all()?;
        fs::File::open(&backups)?.sync_all()?;
        updated += 1;
        println!("Updated {}. Previous copy: {}", item.path.display(), backup.display());
    }
    ensure!(
        &Snapshot::read(home)? == snapshot,
        "Shared skill lock changed during update"
    );
    if !deferred.is_empty() {
        return Err(PartialStep(format!(
            "{updated} GitHub skills updated; {} deferred: {}",
            deferred.len(),
            deferred.join(" | ")
        ))
        .into());
    }
    Ok(updated)
}
