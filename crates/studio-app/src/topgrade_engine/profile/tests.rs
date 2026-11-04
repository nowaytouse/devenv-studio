use super::*;

struct Fixture {
    _dir: tempfile::TempDir,
    home: PathBuf,
    profile: PathBuf,
}

impl Fixture {
    fn new() -> Result<Self> {
        let dir = tempfile::tempdir()?;
        let base = dir.path().canonicalize()?;
        let home = base.join("home");
        let profile = base.join("profile");
        fs::create_dir(&home)?;
        fs::create_dir_all(profile.join("bin"))?;
        fs::write(profile.join("bin/cli-versions"), b"new profile asset")?;
        let manifest = json!({
            "version": 1,
            "files": [{
                "source": "bin/cli-versions",
                "destination": ".local/bin/cli-versions",
                "sha256": digest(b"new profile asset"),
                "adoptSha256": digest(b"legacy asset"),
                "mode": 0o755,
            }]
        });
        fs::write(
            profile.join("manifest.json"),
            serde_json::to_vec(&manifest)?,
        )?;
        Ok(Self {
            _dir: dir,
            home,
            profile,
        })
    }

    fn target(&self) -> PathBuf {
        self.home.join(".local/bin/cli-versions")
    }

    fn original(&self) -> Result<Snapshot> {
        let target = self.target();
        fs::create_dir_all(target.parent().unwrap())?;
        atomic(
            &target,
            b"legacy asset",
            0o640,
            Some(1_734_859_109_420_010_003),
        )?;
        Ok(snapshot(&target)?.unwrap())
    }
}

#[test]
fn plan_is_read_only_and_apply_is_idempotent_with_exact_rollback() -> Result<()> {
    let fixture = Fixture::new()?;
    assert_eq!(execute(&fixture.profile, &fixture.home, &[])?["changed"], 1);
    assert_eq!(fs::read_dir(&fixture.home)?.count(), 0);
    let before = fixture.original()?;
    let first = execute(&fixture.profile, &fixture.home, &["--apply"])?;
    let id = first["transaction"].as_str().unwrap();
    assert_eq!(first["changed"], 1);
    assert_eq!(
        execute(&fixture.profile, &fixture.home, &["--verify"])?["status"],
        "verified"
    );
    let applied = snapshot(&fixture.target())?;
    assert_eq!(
        execute(&fixture.profile, &fixture.home, &["--apply"])?["status"],
        "already-current"
    );
    assert!(snapshot(&fixture.target())? == applied);
    assert_eq!(
        execute(&fixture.profile, &fixture.home, &["--rollback", id])?["status"],
        "rolled-back"
    );
    assert!(snapshot(&fixture.target())? == Some(before));
    assert_eq!(
        execute(&fixture.profile, &fixture.home, &["--rollback", id])?["status"],
        "already-rolled-back"
    );
    assert!(installed(&fixture.home)?.is_none());
    Ok(())
}

#[test]
fn a_prepared_legacy_journal_blocks_mutations_and_can_restore() -> Result<()> {
    let fixture = Fixture::new()?;
    let before = fixture.original()?;
    let (state, guard) = lock(&fixture.home)?;
    let id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let folder = state.join("transactions").join(id);
    fs::create_dir_all(folder.join("backup"))?;
    fs::write(folder.join("backup/0"), &before.bytes)?;
    let journal = format!(
        r#"{{"version":1,"status":"prepared","previous":null,"files":[{{
        "source":"bin/cli-versions","destination":".local/bin/cli-versions",
        "sha256":"{}","mode":493,"adoptSha256":null,"beforeSha256":"{}",
        "beforeMode":{},"beforeMtimeNs":{},"backup":"0"}}]}}"#,
        digest(b"new profile asset"),
        digest(&before.bytes),
        before.mode,
        before.mtime_ns
    );
    fs::write(folder.join("journal.json"), journal)?;
    atomic(&fixture.target(), b"new profile asset", 0o755, None)?;
    drop(guard);
    let error = execute(&fixture.profile, &fixture.home, &["--apply"]).unwrap_err();
    assert!(format!("{error:#}").contains("Incomplete profile transaction"));
    execute(&fixture.profile, &fixture.home, &["--rollback", id])?;
    assert!(snapshot(&fixture.target())? == Some(before));
    Ok(())
}

#[test]
fn detects_external_content_edits_before_apply_or_rollback() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.original()?;
    fs::write(fixture.target(), b"external edit")?;
    assert!(execute(&fixture.profile, &fixture.home, &["--apply"]).is_err());
    assert_eq!(fs::read(fixture.target())?, b"external edit");
    fs::write(fixture.target(), b"legacy asset")?;
    let result = execute(&fixture.profile, &fixture.home, &["--apply"])?;
    let id = result["transaction"].as_str().unwrap();
    fs::write(fixture.target(), b"later edit")?;
    assert!(execute(&fixture.profile, &fixture.home, &["--rollback", id]).is_err());
    assert_eq!(fs::read(fixture.target())?, b"later edit");
    Ok(())
}

#[test]
fn rejects_symlinks_and_tampered_source_assets() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.original()?;
    let outside = fixture.profile.join("outside");
    fs::write(&outside, b"outside")?;
    fs::remove_file(fixture.target())?;
    std::os::unix::fs::symlink(&outside, fixture.target())?;
    assert!(execute(&fixture.profile, &fixture.home, &["--apply"]).is_err());
    assert_eq!(fs::read(&outside)?, b"outside");
    fs::remove_file(fixture.target())?;
    fs::write(fixture.profile.join("bin/cli-versions"), b"tampered")?;
    assert!(execute(&fixture.profile, &fixture.home, &[]).is_err());
    Ok(())
}

#[test]
fn receipt_rejects_duplicate_destination_keys() {
    let data = r#"{"version":1,"transaction":"x","files":{"path":"a","path":"b"}}"#;
    assert!(serde_json::from_str::<Installed>(data).is_err());
}

#[test]
fn released_profile_lock_is_available_even_if_a_fork_retained_the_descriptor() -> Result<()> {
    let fixture = Fixture::new()?;
    let (_, guard) = lock(&fixture.home)?;
    let inherited_descriptor = guard.0.try_clone()?;
    drop(guard);
    let (_, _next) = lock(&fixture.home)?;
    drop(inherited_descriptor);
    Ok(())
}

#[test]
fn complete_reviewed_profile_applies_and_rolls_back_in_a_fresh_home() -> Result<()> {
    let fixture = Fixture::new()?;
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../components/topgrade/profiles/macos")
        .canonicalize()?;
    let install = execute(&source, &fixture.home, &["--apply"])?;
    let count = install["changed"].as_u64().unwrap();
    assert!(count >= 200);
    assert_eq!(
        execute(&source, &fixture.home, &["--verify"])?["unchanged"],
        count
    );
    let tx = install["transaction"].as_str().unwrap();
    execute(&source, &fixture.home, &["--rollback", tx])?;
    assert_eq!(execute(&source, &fixture.home, &[])?["changed"], count);
    Ok(())
}
