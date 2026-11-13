use super::{Item, Output, Snapshot, metadata};
use color_eyre::eyre::{ContextCompat, Result, ensure};
use std::{fs, path::Path};

const RETIRED: &str = "discovering-gcp-data-assets";
const SUCCESSOR: &str = "discover-assets";
const ROOT: &str = ".gemini/config/skills";
const SOURCE: &str = "GoogleCloudPlatform/data-agent-kit-plugin";
const REPO: &str = "googlecloudplatform/data-agent-kit-plugin";
const OLD_TREE: &str = "db4886d8941a99c86ac44b2fe1ad427763c6c7f5";
const NEW_TREE: &str = "bff4d7527cc4b56b232652319d9bbbedb07d2f7d";
const OLD_BYTES: &str = "eafd04bf1dc382568a792bff00426511e7643e23cccb10ccaa788df81013239e";
const NEW_BYTES: &str = "beb34068ecc81c7707df9cd5d6f04fec0a5c89c8a9a51004a8ce4727f7904256";

fn reviewed(path: &Path, name: &str, tree: &str, digest: &str) -> Result<()> {
    let info = metadata(path)?;
    ensure!(
        info.name == name
            && info.directory == format!("skills/{name}")
            && info.tree == tree
            && super::ownership::repository(&info.source)? == REPO
            && fs::read_dir(path)?.count() == 1,
        "Unexpected installed content for {name}"
    );
    super::super::verify_file(&path.join("SKILL.md"), digest)
}

pub(super) fn candidate(home: &Path, item: &Item) -> bool {
    item.path == home.join(ROOT).join(RETIRED) && item.repo == REPO
}

pub(super) fn migrate(
    home: &Path,
    items: &mut Vec<Item>,
    snapshot: &Snapshot,
    stage_install: &mut impl FnMut(&Path, &[&str]) -> Result<Output>,
) -> Result<()> {
    let parent = home.join(ROOT);
    let old = parent.join(RETIRED);
    let Some(index) = items.iter().position(|item| candidate(home, item)) else {
        return Ok(());
    };
    let destination = parent.join(SUCCESSOR);
    super::super::check_dir(&parent, false)?;
    reviewed(&old, RETIRED, OLD_TREE, OLD_BYTES)?;
    ensure!(
        !snapshot.lock.has_entry(RETIRED)
            && !snapshot.lock.has_entry(SUCCESSOR)
            && !super::super::exists(&home.join(".agents/skills").join(SUCCESSOR))?
            && items.iter().all(|item| match item.meta.name.as_str() {
                RETIRED => item.path == old,
                SUCCESSOR => item.path == destination,
                _ => true,
            }),
        "An existing skill owner needs review before replacing {RETIRED}"
    );
    // Official 1.3.0 descends from removal 68924f89. The install ref records provenance without pinning updates.
    let state = super::super::state_dir(home)?;
    if super::super::exists(&destination)? {
        reviewed(&destination, SUCCESSOR, NEW_TREE, NEW_BYTES)?;
    } else {
        let stage = tempfile::tempdir_in(&state)?;
        let output = stage_install(
            stage.path(),
            &[
                "skill",
                "install",
                SOURCE,
                "skills/discover-assets/SKILL.md@1.3.0",
                "--dir",
                stage.path().to_str().context("Non-UTF-8 skill staging directory")?,
            ],
        )?;
        ensure!(output.success, "Could not stage the official {SUCCESSOR} replacement");
        let staged = stage.path().join(SUCCESSOR);
        reviewed(&staged, SUCCESSOR, NEW_TREE, NEW_BYTES)?;
        fs::File::open(staged.join("SKILL.md"))?.sync_all()?;
        fs::File::open(&staged)?.sync_all()?;
        ensure!(
            &Snapshot::read(home)? == snapshot,
            "Shared skill lock changed during migration"
        );
        super::super::preview::move_exclusive(&staged, &destination)?;
        fs::File::open(&parent)?.sync_all()?;
    }

    ensure!(
        &Snapshot::read(home)? == snapshot,
        "Shared skill lock changed during migration"
    );
    reviewed(&destination, SUCCESSOR, NEW_TREE, NEW_BYTES)?;
    reviewed(&old, RETIRED, OLD_TREE, OLD_BYTES)?;
    let archive = tempfile::Builder::new()
        .prefix("gh-skill-retirement-")
        .tempdir_in(&state)?
        .keep();
    let archived = archive.join(RETIRED);
    super::super::preview::move_exclusive(&old, &archived)?;
    reviewed(&archived, RETIRED, OLD_TREE, OLD_BYTES)?;
    fs::File::open(&parent)?.sync_all()?;
    fs::File::open(&archive)?.sync_all()?;
    fs::File::open(state)?.sync_all()?;
    ensure!(
        &Snapshot::read(home)? == snapshot,
        "Shared skill lock changed during migration"
    );
    let old_item = items.remove(index);
    if !items.iter().any(|item| item.path == destination) {
        items.push(Item {
            meta: metadata(&destination)?,
            path: destination,
            alias: SUCCESSOR.into(),
            repo: old_item.repo,
            expected: String::new(),
        });
    }
    println!(
        "Replaced upstream-retired {RETIRED} with {SUCCESSOR}. Archived in {}",
        archive.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_reviewed_gemini_location_can_migrate_and_edited_content_blocks_staging() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().canonicalize()?;
        let old = home.join(ROOT).join(RETIRED);
        fs::create_dir_all(&old)?;
        fs::write(
            old.join("SKILL.md"),
            format!(
                "---\nname: {RETIRED}\nmetadata:\n  github-repo: https://github.com/{SOURCE}\n  github-path: skills/{RETIRED}\n  github-tree-sha: {OLD_TREE}\n---\nEdited local body\n"
            ),
        )?;
        let snapshot = Snapshot::read(&home)?;
        let mut item = Item {
            path: home.join(".agents/skills").join(RETIRED),
            alias: RETIRED.into(),
            meta: metadata(&old)?,
            repo: REPO.into(),
            expected: String::new(),
        };
        assert!(!candidate(&home, &item));
        item.path = old.clone();
        assert!(candidate(&home, &item));
        let error = migrate(&home, &mut vec![item], &snapshot, &mut |_, _| {
            panic!("edited local content must block before staged installation")
        })
        .unwrap_err();
        assert!(error.to_string().contains("Managed file changed"));
        assert!(!home.join(ROOT).join(SUCCESSOR).exists());
        assert!(fs::read_to_string(old.join("SKILL.md"))?.contains("Edited local body"));
        assert_eq!(Snapshot::read(&home)?, snapshot);
        Ok(())
    }
}
