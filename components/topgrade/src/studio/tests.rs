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
    let busy = lock(&state).unwrap_err();
    assert!(busy.to_string().contains("maintenance operation is running"));
    // Parallel tests spawn processes. A fork can briefly inherit the descriptor
    // before close-on-exec, so explicitly unlock before asserting reacquisition.
    first.unlock().unwrap();
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
fn studio_pending_activation_blocks_admission() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let state = state_dir(&home).unwrap();
    let _lock = lock(&state).unwrap();
    check_admission(&home, &state).unwrap();

    let pending = home.join(ACTIVATION_PENDING);
    fs::create_dir_all(pending.parent().unwrap()).unwrap();
    private_file(&pending, b"preserve");
    assert!(check_admission(&home, &state).is_err());
    assert_eq!(fs::read(&pending).unwrap(), b"preserve");

    fs::remove_file(&pending).unwrap();
    symlink(home.join("absent"), &pending).unwrap();
    assert!(check_admission(&home, &state).is_err());
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

    let routine: Vec<_> = Step::ALL.into_iter().filter(|step| !step.manual_llvm()).collect();
    data["steps"] = serde_json::json!(routine);
    fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
    assert_eq!(request(&dir).unwrap().steps, routine);
    for steps in [
        serde_json::json!(["helm", "helm"]),
        serde_json::json!(["helm", "github_cli_skills"]),
        serde_json::json!(["brew_formula"]),
        serde_json::json!(["llvm_head"]),
        serde_json::json!(["unknown"]),
    ] {
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
fn studio_llvm_requires_an_exclusive_candidate_and_manual_entry() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().canonicalize().unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.join("request.json");
    let mut data = serde_json::json!({
        "schema_version": 1,
        "run_id": "reviewed-llvm",
        "steps": ["llvm_stable"],
        "review_digest": "f".repeat(64),
        "llvm_plan_digest": "a".repeat(64),
    });
    private_file(&path, &serde_json::to_vec(&data).unwrap());
    assert_eq!(request(&dir).unwrap().steps, [Step::LlvmStable]);
    for ids in [
        &["llvm_head", "llvm_stable"][..],
        &["helm", "llvm_stable"][..],
        &["helm"][..],
    ] {
        data["steps"] = serde_json::json!(ids);
        fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
        assert!(request(&dir).is_err());
    }
    assert!(run_native_profile("llvm_stable", RunType::Dry).is_err());
    assert!(run_native_profile("llvm_head", RunType::Dry).is_err());
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

    let asset = REQUIRED_ASSETS[0];
    fs::write(home.join(asset), b"unexpected config").unwrap();
    let error = verify_release(&home, &executable, &reviewed).err().unwrap();
    assert!(error.to_string().contains("Managed file changed"));
    fs::write(home.join(asset), asset).unwrap();
    fs::write(&receipt, b"{}").unwrap();
    let error = verify_release(&home, &executable, &reviewed).err().unwrap();
    assert!(error.to_string().contains("Reviewed engine receipt has changed"));
}

#[test]
fn studio_clean_child_exit_clears_marker_but_signal_retains_it() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().canonicalize().unwrap();
    let held = lock(&dir).unwrap();
    let executable = dir.join("mock-engine");
    let check = concat!(
        "#!/bin/sh\n",
        "test \"$1\" = --studio-worker && test -e \"/dev/fd/$4\" || exit 42\n",
        "/bin/cat >/dev/null || exit 43\n",
        "/usr/bin/grep -Fq \"\\\"child_pgid\\\":$$\" \"$2/maintenance-active.json\" || exit 44\n",
    );
    private_file(&executable, format!("{check}exit 0\n").as_bytes());
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let worker = || worker_command(&dir, &executable, &held, &dir, Step::Helm);
    assert_eq!(run_step(&dir, "clean", Step::Helm, worker()).unwrap(), Outcome::Success);
    check_admission(&dir, &dir).unwrap();

    fs::write(&executable, format!("{check}kill -TERM $$\n")).unwrap();
    assert!(run_step(&dir, "interrupted", Step::Helm, worker()).is_err());
    assert!(check_admission(&dir, &dir).is_err());
    let marker: Interruption = serde_json::from_slice(&fs::read(dir.join(MARKER)).unwrap()).unwrap();
    assert_eq!(marker.run_id, "interrupted");
    assert!(!signal_group(marker.child_pgid.unwrap(), None).unwrap());
}

#[test]
fn studio_inventory_uses_topgrade_step_ids_and_only_controls_reviewed_operations() {
    use clap::ValueEnum;
    let rows = inventory::entries();
    assert_eq!(rows.len(), crate::step::Step::value_variants().len() + 4);
    assert!(rows.windows(2).all(|pair| pair[0].id < pair[1].id));
    let ids: Vec<_> = rows
        .iter()
        .filter(|entry| entry.mode == "controlled")
        .map(|entry| entry.id.as_str())
        .collect();
    assert_eq!(ids, Step::ALL.map(Step::id));
    for step in Step::ALL {
        assert_eq!(serde_json::to_value(step).unwrap(), step.id());
    }
    for id in ["brew_formula", "mas", "self_update", "custom_commands"] {
        let row = rows.iter().find(|entry| entry.id == id).unwrap();
        assert_eq!(row.mode, "terminal");
        assert!(row.tools.is_empty());
    }
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
