use super::*;
use std::os::unix::fs::{PermissionsExt, symlink};

fn private_file(path: &Path, data: &[u8]) {
    create_file(path).unwrap().write_all(data).unwrap();
}

fn digest(path: &Path) -> String {
    sha256(open_file(path, false, MAX_ASSET).unwrap()).unwrap()
}

#[test]
fn studio_shared_lock_refuses_contention_and_linked_files() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().canonicalize().unwrap();
    let first = lock(&state).unwrap();
    assert!(lock(&state).is_err());
    drop(first);
    drop(lock(&state).unwrap());

    let lock_path = state.join("maintenance.lock");
    fs::remove_file(&lock_path).unwrap();
    let target = state.join("other");
    private_file(&target, b"preserve");
    symlink(&target, &lock_path).unwrap();
    assert!(lock(&state).is_err());
    assert_eq!(fs::read(target).unwrap(), b"preserve");
}

#[test]
fn studio_request_requires_private_directory_and_distinct_known_steps() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().canonicalize().unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.join("request.json");
    let mut data = serde_json::json!({
        "schema_version": 1,
        "run_id": "run-42",
        "steps": ["github_cli_skills", "helm"],
        "review_digest": "f".repeat(64),
    });
    private_file(&path, &serde_json::to_vec(&data).unwrap());
    assert_eq!(request(&dir).unwrap().steps, [Step::GithubCliSkills, Step::Helm]);

    for steps in [serde_json::json!(["helm", "helm"]), serde_json::json!(["shell"])] {
        data["steps"] = steps;
        fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
        assert!(request(&dir).is_err());
    }
    data["steps"] = serde_json::json!(["helm"]);
    fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(request(&dir).is_err());
}

#[test]
fn studio_release_rejects_asset_drift_and_receipt_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let tree = "a".repeat(40);
    let release = home.join(RELEASES).join(&tree);
    let executable = release.join("bin/topgrade");
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    private_file(&executable, b"fixture engine");
    let profile = release.join("profile-manifest.json");
    private_file(&profile, b"fixture profile");

    let mut assets = BTreeMap::new();
    for relative in REQUIRED_ASSETS {
        let path = home.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        private_file(&path, relative.as_bytes());
        assets.insert(relative, digest(&path));
    }
    let receipt = release.join("studio-engine.json");
    private_file(
        &receipt,
        &serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "studio_commit": "b".repeat(40),
            "component_tree": tree,
            "binary_sha256": digest(&executable),
            "profile_sha256": digest(&profile),
            "assets": assets,
        }))
        .unwrap(),
    );
    let reviewed = digest(&receipt);
    verify_release(&home, &executable, &reviewed).unwrap();

    fs::write(home.join(SKILLS), b"unexpected script").unwrap();
    let error = verify_release(&home, &executable, &reviewed).err().unwrap();
    assert!(error.to_string().contains("Managed file changed"));
    fs::write(home.join(SKILLS), SKILLS).unwrap();
    fs::write(&receipt, b"{}").unwrap();
    let error = verify_release(&home, &executable, &reviewed).err().unwrap();
    assert!(error.to_string().contains("Reviewed engine receipt has changed"));
}

#[test]
fn studio_clean_child_exit_clears_marker_but_signal_retains_it() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().canonicalize().unwrap();
    let interpreter = dir.join("mock-python");
    private_file(&interpreter, b"#!/bin/sh\nexit 0\n");
    fs::set_permissions(&interpreter, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        run_step(&dir, &dir, &dir, "clean", Step::Helm, &interpreter).unwrap(),
        Outcome::Success
    );
    check_interruption(&dir).unwrap();

    fs::write(&interpreter, b"#!/bin/sh\nkill -TERM $$\n").unwrap();
    assert!(run_step(&dir, &dir, &dir, "interrupted", Step::Helm, &interpreter).is_err());
    assert!(check_interruption(&dir).is_err());
    let marker: Interruption = serde_json::from_slice(&fs::read(dir.join(MARKER)).unwrap()).unwrap();
    assert_eq!(marker.run_id, "interrupted");
    assert!(!signal_group(marker.child_pgid.unwrap(), None).unwrap());
}

#[test]
fn studio_timeout_terminates_helper_group() {
    let mut child = command(Path::new("/bin/sleep"))
        .arg("60")
        .process_group(0)
        .spawn_checked()
        .unwrap();
    let pgid = i32::try_from(child.id()).unwrap();
    assert!(wait_until(&mut child, Instant::now()).unwrap().is_none());
    terminate_group(&mut child, pgid).unwrap();
    assert!(!signal_group(pgid, None).unwrap());
}
