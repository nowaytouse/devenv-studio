use super::*;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

const OLD: &str = "1111111111111111111111111111111111111111";
const NEW: &str = "2222222222222222222222222222222222222222";
const CONFIG: &[u8] = b"# keep local topgrade settings\n";

fn test_home() -> Result<tempfile::TempDir> {
    let home = tempfile::tempdir()?;
    for destination in ASSETS {
        let path = home.path().join(destination);
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(&path, CONFIG)?;
    }
    Ok(home)
}

fn candidate(stage: &Path, home: &Path, revision: &str, valid: bool) -> Result<Prepared> {
    let binary = stage.join("candidate");
    let script = format!(
        "#!/bin/sh\n# {revision}\ncase \"$1\" in\n--version) printf 'topgrade 17.16.0\\n';;\n--config-reference) printf '[misc]\\n'; exit {};;\n*) exit 2;;\nesac\n",
        i32::from(!valid),
    );
    fs::write(&binary, script)?;
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))?;
    let assets = ASSETS
        .iter()
        .map(|destination| {
            let digest = sha256_file(&home.join(destination))?;
            Ok(((*destination).to_owned(), digest))
        })
        .collect::<Result<_>>()?;
    let receipt = Receipt {
        schema_version: 1,
        studio_commit: revision.to_owned(),
        component_tree: revision.to_owned(),
        binary_sha256: sha256_file(&binary)?,
        profile_sha256: "a".repeat(64),
        profile_version: 1,
        assets,
    };
    Ok(Prepared {
        binary,
        receipt,
        profile: None,
    })
}

fn assert_clean_install(home: &Path) -> Result<()> {
    let bin = home.join(".local/bin");
    let entries: Vec<_> = fs::read_dir(&bin)?.collect::<std::io::Result<_>>()?;
    assert_eq!(entries.len(), 1, "temporary files or extra executables");
    assert_eq!(entries[0].file_name(), "topgrade");
    assert!(entries[0].path().symlink_metadata()?.is_file());
    assert!(
        !home
            .join(".local/share/topgrade-rustup-head/releases")
            .exists()
    );
    for destination in ASSETS {
        assert_eq!(fs::read(home.join(destination))?, CONFIG);
    }
    Ok(())
}

#[test]
fn first_install_and_repeat_keep_one_regular_topgrade() -> Result<()> {
    let fixture = test_home()?;
    let home = fixture.path().canonicalize()?;
    let executable = home.join(".local/bin/topgrade");
    assert!(!executable.exists());

    assert!(install_rolling(&home, OLD, OLD, |stage| candidate(
        stage, &home, OLD, true
    ))?);
    let identity = studio_deployment::maintenance::verified_engine(&home, &executable)?;
    assert_eq!(identity.binary, executable);
    assert_eq!(identity.studio_commit, OLD);
    assert_eq!(identity.component_tree, OLD);
    let original = fs::metadata(&executable)?;
    assert!(!install_rolling(&home, OLD, OLD, |_| panic!(
        "current executable must not be rebuilt"
    ))?);
    assert_eq!(fs::metadata(&executable)?.ino(), original.ino());
    assert_clean_install(&home)
}

#[test]
fn rolling_replacement_accepts_existing_regular_file_and_symlink() -> Result<()> {
    for symlink_install in [false, true] {
        let fixture = test_home()?;
        let home = fixture.path().canonicalize()?;
        let executable = home.join(".local/bin/topgrade");
        fs::create_dir_all(executable.parent().unwrap())?;
        let source = home.join("user-file");
        if symlink_install {
            fs::write(&source, b"user data")?;
            symlink(&source, &executable)?;
        } else {
            fs::write(&executable, b"original")?;
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))?;
        }

        assert!(install_rolling(&home, NEW, NEW, |stage| candidate(
            stage, &home, NEW, true
        ))?);
        assert!(fs::read_to_string(&executable)?.contains(NEW));
        assert_clean_install(&home)?;
        if symlink_install {
            assert_eq!(fs::read(&source)?, b"user data");
        }
    }
    Ok(())
}

#[test]
fn build_validation_and_interruption_leave_current_binary_and_config_untouched() -> Result<()> {
    let fixture = test_home()?;
    let home = fixture.path().canonicalize()?;
    let executable = home.join(".local/bin/topgrade");
    assert!(install_rolling(&home, OLD, OLD, |stage| candidate(
        stage, &home, OLD, true
    ))?);
    let expected = fs::read(&executable)?;
    let inode = fs::metadata(&executable)?.ino();

    let build = install_rolling(&home, NEW, NEW, |stage| {
        fs::write(stage.join("incomplete"), b"partial build")?;
        bail!("synthetic build failure");
    });
    assert!(format!("{:#}", build.unwrap_err()).contains("synthetic build failure"));
    assert_clean_install(&home)?;

    let validation = install_rolling(&home, NEW, NEW, |stage| candidate(stage, &home, NEW, false));
    assert!(validation.is_err());
    assert_clean_install(&home)?;

    let interrupted = install_rolling(&home, NEW, NEW, |stage| {
        candidate(stage, &home, NEW, true)?;
        Err(std::io::Error::from(std::io::ErrorKind::Interrupted).into())
    });
    assert_eq!(
        interrupted
            .unwrap_err()
            .downcast_ref::<std::io::Error>()
            .map(std::io::Error::kind),
        Some(std::io::ErrorKind::Interrupted),
    );
    assert_eq!(fs::metadata(&executable)?.ino(), inode);
    assert_eq!(fs::read(&executable)?, expected);
    assert_clean_install(&home)
}

#[test]
fn rolling_update_refuses_to_overwrite_a_concurrent_launcher_change() -> Result<()> {
    let fixture = test_home()?;
    let home = fixture.path().canonicalize()?;
    let executable = home.join(".local/bin/topgrade");
    assert!(install_rolling(&home, OLD, OLD, |stage| candidate(
        stage, &home, OLD, true
    ))?);

    let result = install_rolling(&home, NEW, NEW, |stage| {
        let prepared = candidate(stage, &home, NEW, true)?;
        fs::write(&executable, b"independent change during build")?;
        Ok(prepared)
    });

    assert!(result.unwrap_err().to_string().contains("entry changed"));
    assert_eq!(fs::read(&executable)?, b"independent change during build");
    assert_clean_install(&home)
}

fn commit_fixture(checkout: &Path, contents: &str) -> Result<String> {
    let path = "components/topgrade/fixture";
    fs::create_dir_all(checkout.join("components/topgrade"))?;
    fs::write(checkout.join(path), contents)?;
    git(checkout, &["add", path])?;
    git(
        checkout,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "-m",
            contents,
        ],
    )?;
    git(checkout, &["rev-parse", "HEAD"])
}

#[test]
fn source_export_includes_workspace_inputs_at_the_reviewed_revision() -> Result<()> {
    let checkout = tempfile::tempdir()?;
    let stage = tempfile::tempdir()?;
    let root = checkout.path();
    git(root, &["init", "-b", "main"])?;
    fs::create_dir(root.join("data"))?;
    let catalog = root.join("data/catalog.json");
    fs::write(&catalog, b"reviewed")?;
    git(root, &["add", "data/catalog.json"])?;
    let revision = commit_fixture(root, "initial")?;
    fs::write(&catalog, b"local draft")?;
    fs::write(root.join("local-only"), b"private")?;
    let source = export_tracked_source(root, &revision, stage.path())?;
    assert_eq!(fs::read(source.join("data/catalog.json"))?, b"reviewed");
    assert!(!source.join("local-only").exists());
    Ok(())
}

#[test]
fn fetch_and_fast_forward_preserve_dirty_or_diverged_checkouts() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let root = fixture.path().canonicalize()?;
    let origin = root.join("origin");
    let checkout = root.join("checkout");
    fs::create_dir(&origin)?;
    git(&origin, &["init", "-b", "main"])?;
    let original = commit_fixture(&origin, "initial")?;
    git(&root, &["clone", "--no-hardlinks", "origin", "checkout"])?;
    let latest = commit_fixture(&origin, "remote update")?;

    let local_edit = checkout.join("keep-this-file");
    fs::write(&local_edit, b"local")?;
    assert!(tracked(&checkout, true).is_err());
    assert_eq!(git(&checkout, &["rev-parse", "HEAD"])?, original);
    assert_eq!(fs::read(&local_edit)?, b"local");
    fs::remove_file(local_edit)?;

    let (revision, tree) = tracked(&checkout, true)?;
    assert_eq!(revision, latest);
    assert_eq!(git(&checkout, &["rev-parse", "HEAD"])?, revision);
    assert_eq!(
        git(&checkout, &["rev-parse", "HEAD:components/topgrade"])?,
        tree
    );

    let local = commit_fixture(&checkout, "local branch")?;
    commit_fixture(&origin, "remote branch")?;
    assert!(tracked(&checkout, true).is_err());
    assert_eq!(git(&checkout, &["rev-parse", "HEAD"])?, local);
    assert_eq!(
        fs::read_to_string(checkout.join("components/topgrade/fixture"))?,
        "local branch"
    );
    Ok(())
}
