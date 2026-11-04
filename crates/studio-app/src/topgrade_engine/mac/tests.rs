use super::*;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;

const OLD: &str = "1111111111111111111111111111111111111111";
const NEW: &str = "2222222222222222222222222222222222222222";

fn release(home: &Path, tree: &str, config: &[u8]) -> Result<PathBuf> {
    let release = home
        .join(".local/share/topgrade-rustup-head/releases")
        .join(tree);
    let binary = release.join("bin/topgrade");
    fs::create_dir_all(binary.parent().unwrap())?;
    fs::write(
        &binary,
        format!("#!/bin/sh\n# {tree}\nprintf 'topgrade 17.15.0\\n'\n"),
    )?;
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))?;
    let profile = release.join("profile");
    let mut assets = Vec::new();
    for destination in ASSETS {
        let source = format!("config/{}", destination.strip_prefix(".config/").unwrap());
        let path = profile.join(&source);
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(
            &path,
            if destination == ASSETS[0] {
                config
            } else {
                b"unchanged"
            },
        )?;
        assets.push(json!({
            "source": source,
            "destination": destination,
            "sha256": sha256_file(&path)?,
            "mode": 0o644,
        }));
    }
    let manifest = release.join("profile-manifest.json");
    write_json(&manifest, &json!({"version": 1, "files": assets}))?;
    fs::copy(&manifest, profile.join("manifest.json"))?;
    write_json(
        &release.join("studio-engine.json"),
        &Receipt {
            schema_version: 1,
            studio_commit: tree.to_owned(),
            component_tree: tree.to_owned(),
            binary_sha256: sha256_file(&binary)?,
            profile_sha256: sha256_file(&manifest)?,
            assets: asset_map(&manifest)?,
        },
    )?;
    Ok(release)
}

#[test]
fn same_version_rollback_restores_verified_binary_profile_and_receipt() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let home = fixture.path().canonicalize()?;
    let previous = release(&home, OLD, b"original")?;
    let current = release(&home, NEW, b"updated")?;
    let receipt = home.join(".local/state/topgrade-macos-profile/installed.json");
    let prior = profile(&previous, &home, &["--apply"])?;
    let original_receipt = fs::read(&receipt)?;
    let applied = profile(&current, &home, &["--apply"])?;
    let id = applied["transaction"].as_str().unwrap();
    let link = home.join(".local/bin/topgrade");
    fs::create_dir_all(link.parent().unwrap())?;
    let target = current.join("bin/topgrade");
    let original = previous.join("bin/topgrade");
    symlink(&target, &link)?;
    let base = home.join(".local/share/topgrade-rustup-head");
    let journal = base.join(format!("studio-activation-{NEW}.json"));
    let pending = base.join("studio-activation.pending.json");
    write_json(
        &journal,
        &Activation {
            schema_version: 1,
            previous: original.clone(),
            target: target.clone(),
            profile_transaction: Some(id.to_owned()),
        },
    )?;
    let active = verified_engine_release(&home, &target)?;
    let rollback = verified_engine_release(&home, &original)?;
    assert_eq!(active.upstream_version, rollback.upstream_version);
    assert_ne!(active.binary_sha256, rollback.binary_sha256);
    assert_ne!(active.profile_sha256, rollback.profile_sha256);
    assert!(restore_activation(&home, NEW, Some(NEW)).is_err());
    assert!(!pending.exists());

    let backup = receipt
        .parent()
        .unwrap()
        .join(format!("transactions/{id}/backup/0"));
    let original_backup = fs::read(&backup)?;
    fs::write(&backup, b"tampered")?;
    assert!(
        format!(
            "{:#}",
            restore_activation(&home, NEW, Some(OLD)).unwrap_err()
        )
        .contains("Profile backup checksum mismatch")
    );
    assert!(!pending.exists());
    assert_eq!(fs::read_link(&link)?, target);
    assert_eq!(fs::read(home.join(ASSETS[0]))?, b"updated");
    fs::write(&backup, original_backup)?;

    restore_activation(&home, NEW, Some(OLD))?;
    assert_eq!(fs::read_link(&link)?, original);
    assert_eq!(sha256_file(&link.canonicalize()?)?, rollback.binary_sha256);
    assert_eq!(fs::read(home.join(ASSETS[0]))?, b"original");
    assert_eq!(fs::read(&receipt)?, original_receipt);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&original_receipt)?["transaction"],
        prior["transaction"]
    );
    assert!(!journal.exists());
    assert!(!pending.exists());
    assert!(restore_activation(&home, NEW, Some(OLD)).is_err());
    Ok(())
}
