//! Adopt only verified legacy host copies. Later NPX updates switch all hosts by updating the owner once.
use super::*;
use crate::studio::github_skills::ownership::{LEAN_CTX_HOSTS, lean_ctx_link, lean_ctx_owner, name_matches};

const OWNER: &str = ".agents/skills/lean-ctx";

pub(super) fn locked_hash(entry: &Value) -> Option<&str> {
    let hash = entry["skillFolderHash"].as_str()?;
    (entry["ref"].is_null()
        && entry["pinnedRef"].is_null()
        && lean_ctx_owner(
            entry["sourceType"].as_str()?,
            entry["source"].as_str()?,
            entry["sourceUrl"].as_str()?,
            entry["skillPath"].as_str(),
            hash,
        ))
    .then_some(hash)
}

fn verify_tree(root: &Path, expected: &str) -> Result<cursor::Tree> {
    // Ask Git for NPX's tree ID using raw blobs. The temporary index never evaluates
    // attributes, clean filters, hooks, or repository configuration from a skill.
    let scratch = tempfile::tempdir()?;
    let git = || {
        let mut cmd = crate::studio::command(Path::new("/usr/bin/git"));
        cmd.current_dir(scratch.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        cmd
    };
    let run = |cmd: &mut Command| -> Result<String> {
        let out = capture(cmd, Duration::from_secs(20), 256 * 1024)?;
        ensure!(
            out.status.success(),
            "Git could not verify LeanCTX: {}",
            String::from_utf8_lossy(&out.err)
        );
        Ok(String::from_utf8(out.out)?)
    };
    run(git().args(["init", "--bare", "--quiet", "--template=", "--object-format=sha1", "."]))?;
    let mut blobs = git();
    blobs.args(["hash-object", "--no-filters", "-w", "--"]);
    let files = cursor::tree(root)?;
    let mut paths = Vec::new();
    for (path, file) in &files {
        if path == Path::new("SKILL.md.bak") {
            continue;
        }
        if path == Path::new("SKILL.md.v4-migration.lock") {
            ensure!(
                file.hash == digest(b""),
                "Active LeanCTX migration lock: {}",
                root.display()
            );
            continue;
        }
        let data = bytes(&root.join(path), 32 * 1024 * 1024)?;
        ensure!(
            digest(&data) == file.hash,
            "LeanCTX changed during verification: {}",
            root.display()
        );
        let input = scratch.path().join(format!("input-{}", paths.len()));
        fs::write(&input, data)?;
        blobs.arg(input);
        paths.push((path, if file.mode & 0o100 == 0 { "100644" } else { "100755" }));
    }
    let ids = run(&mut blobs)?;
    ensure!(ids.lines().count() == paths.len(), "Incomplete Git blob inventory");
    let mut index = git();
    index.args(["update-index", "--add"]);
    for ((path, mode), id) in paths.into_iter().zip(ids.lines()) {
        index.args(["--cacheinfo", mode, id]).arg(path);
    }
    run(&mut index)?;
    ensure!(
        run(git().arg("write-tree"))?.trim() == expected && cursor::tree(root)? == files,
        "LeanCTX files differ from the Git tree in the NPX lock; preserved for review: {}",
        root.display()
    );
    Ok(files)
}

fn hosts(home: &Path) -> Result<Vec<PathBuf>> {
    let mut hosts = Vec::new();
    for relative in LEAN_CTX_HOSTS {
        let host = home.join(relative);
        if exists(&host)? {
            directory(
                home,
                Path::new(relative).parent().context("Missing LeanCTX host parent")?,
                false,
            )?;
            hosts.push(host);
        }
    }
    Ok(hosts)
}

pub(super) fn audit(home: &Path, entry: &Value) -> Result<Vec<PathBuf>> {
    let hosts = hosts(home)?;
    if !hosts.is_empty() {
        let expected = locked_hash(entry).context("Unverified or pinned LeanCTX owner in the global skill lock")?;
        let owner = directory(home, Path::new(OWNER), false)?;
        ensure!(name_matches(&owner, "lean-ctx")?, "Invalid LeanCTX owner identity");
        verify_tree(&owner, expected)?;
        for host in &hosts {
            if fs::symlink_metadata(host)?.is_symlink() {
                ensure!(
                    lean_ctx_link(&owner, host)?,
                    "Unexpected LeanCTX host link: {}",
                    host.display()
                );
            } else {
                verify_tree(host, expected)?;
            }
        }
    }
    Ok(hosts)
}

pub(super) fn prepare(home: &Path, entry: &Value) -> Result<Vec<PathBuf>> {
    // Inspect every host before the first exchange, including executable modes.
    let hosts = audit(home, entry)?;
    let owner = home.join(OWNER);
    for host in &hosts {
        if fs::symlink_metadata(host)?.is_symlink() {
            continue;
        }
        let before = verify_tree(host, locked_hash(entry).context("Missing LeanCTX tree ID")?)?;
        let root = directory(home, Path::new(".local/state/topgrade-skill-lock"), true)?;
        let archive = tempfile::Builder::new().prefix("lean-ctx-host-").tempdir_in(&root)?;
        let backup = archive.path().join("lean-ctx");
        // A retained directory is also the recovery location if the process stops immediately after exchange.
        std::os::unix::fs::symlink(&owner, &backup)?;
        sync_directory(archive.path())?;
        sync_directory(&root)?;
        cursor::exchange(host, &backup)?;
        let archive = archive.keep();
        sync_directory(&archive)?;
        sync_directory(&root)?;
        sync_directory(host.parent().context("Missing LeanCTX host parent")?)?;
        let validated = (|| -> Result<bool> { Ok(lean_ctx_link(&owner, host)? && cursor::tree(&backup)? == before) })();
        if !matches!(validated, Ok(true)) {
            ensure!(
                lean_ctx_link(&owner, host)?,
                "Concurrent LeanCTX change; previous entry retained at {}",
                backup.display()
            );
            cursor::exchange(host, &backup)?;
            sync_directory(host.parent().context("Missing LeanCTX host parent")?)?;
            sync_directory(&archive)?;
            color_eyre::eyre::bail!("LeanCTX changed during adoption; restored {}", host.display());
        }
        println!(
            "Linked {} to the verified NPX owner. Previous directory: {}",
            host.display(),
            backup.display()
        );
    }
    Ok(hosts)
}

pub(super) fn verify(home: &Path, entry: &Value, hosts: &[PathBuf]) -> Result<()> {
    if !hosts.is_empty() {
        let owner = directory(home, Path::new(OWNER), false)?;
        ensure!(name_matches(&owner, "lean-ctx")?, "Invalid LeanCTX owner identity");
        verify_tree(
            &owner,
            locked_hash(entry).context("LeanCTX lock owner changed during NPX update")?,
        )?;
        for host in hosts {
            ensure!(
                lean_ctx_link(&owner, host)?,
                "LeanCTX host no longer links directly to the NPX owner: {}",
                host.display()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::symlink;

    // From "git add --all && git write-tree" in independent synthetic repositories.
    const OLD: &str = "708f11c9217883004ced96be5bf820315f8fa516";
    const NEW: &str = "b4cc875385a47c480717ff0e3175501a31ad7278";

    fn content(dir: &Path, version: &str) -> Result<()> {
        fs::create_dir_all(dir.join("scripts"))?;
        fs::write(dir.join("SKILL.md"), format!("---\nname: lean-ctx\n---\n{version}\n"))?;
        let script = dir.join("scripts/install.sh");
        fs::write(&script, format!("fixture {version}\n"))?;
        fs::set_permissions(script, fs::Permissions::from_mode(0o755))?;
        Ok(())
    }

    fn entry(hash: &str) -> Value {
        json!({"sourceType": "github", "source": "yvgude/lean-ctx", "sourceUrl": "https://github.com/yvgude/lean-ctx.git",
               "skillPath": "skills/lean-ctx/SKILL.md", "skillFolderHash": hash})
    }

    #[test]
    fn adoption_survives_owner_directory_replacement_and_repeated_runs() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().canonicalize()?;
        let owner = home.join(OWNER);
        content(&owner, "one")?;
        let root = home.join(".local/state/topgrade-skill-lock");
        for relative in LEAN_CTX_HOSTS {
            let host = home.join(relative);
            content(&host, "one")?;
            fs::write(host.join("SKILL.md.bak"), relative)?;
            fs::write(host.join("SKILL.md.v4-migration.lock"), [])?;
        }
        let hosts = prepare(&home, &entry(OLD))?;
        assert_eq!(hosts.len(), 3);
        let backups: std::collections::BTreeSet<_> = fs::read_dir(&root)?
            .map(|dir| fs::read_to_string(dir?.path().join("lean-ctx/SKILL.md.bak")))
            .collect::<io::Result<_>>()?;
        assert_eq!(backups, LEAN_CTX_HOSTS.iter().map(|s| s.to_string()).collect());
        let candidate = home.join("new-owner");
        content(&candidate, "two")?;
        cursor::exchange(&owner, &candidate)?;
        verify(&home, &entry(NEW), &hosts)?;
        for host in &hosts {
            assert_eq!(fs::read_to_string(host.join("scripts/install.sh"))?, "fixture two\n");
        }
        assert!(verify(&home, &entry(OLD), &hosts).is_err());
        assert_eq!(prepare(&home, &entry(NEW))?, hosts);
        assert_eq!(fs::read_dir(&root)?.count(), 3);
        fs::remove_file(&hosts[0])?;
        assert!(verify(&home, &entry(NEW), &hosts).is_err());
        Ok(())
    }

    #[test]
    fn local_edit_wrong_owner_pin_or_redirect_blocks_the_whole_adoption() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().canonicalize()?;
        let owner = home.join(OWNER);
        let first = home.join(LEAN_CTX_HOSTS[0]);
        let last = home.join(LEAN_CTX_HOSTS[2]);
        for dir in [&owner, &first, &last] {
            content(dir, "one")?;
        }
        for (key, value) in [
            ("source", "other/repo"),
            ("sourceUrl", "https://example.org/repo"),
            ("pinnedRef", "v1"),
            ("skillPath", "other/SKILL.md"),
        ] {
            let mut foreign = entry(OLD);
            foreign[key] = value.into();
            assert!(prepare(&home, &foreign).is_err());
        }
        fs::set_permissions(last.join("scripts/install.sh"), fs::Permissions::from_mode(0o644))?;
        assert!(prepare(&home, &entry(OLD)).is_err());
        content(&last, "two")?;
        assert!(prepare(&home, &entry(OLD)).is_err());
        assert_eq!(fs::read_to_string(last.join("scripts/install.sh"))?, "fixture two\n");
        fs::rename(&last, home.join("redirected"))?;
        symlink(&owner, home.join("intermediate"))?;
        symlink(home.join("intermediate"), &last)?;
        assert!(prepare(&home, &entry(OLD)).is_err());
        assert!(!fs::symlink_metadata(&first)?.is_symlink());
        assert!(!home.join(".local/state/topgrade-skill-lock").exists());
        fs::remove_file(&last)?;
        symlink("../../../.agents/skills/lean-ctx", &last)?;
        assert!(audit(&home, &entry(OLD))?.contains(&last));
        fs::write(owner.join("SKILL.md.v4-migration.lock"), "busy")?;
        assert!(audit(&home, &entry(OLD)).is_err());
        Ok(())
    }
}
