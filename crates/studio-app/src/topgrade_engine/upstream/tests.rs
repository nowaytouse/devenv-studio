use super::*;
use std::os::unix::fs::symlink;

const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const C: &str = "cccccccccccccccccccccccccccccccccccccccc";
const D: &str = "dddddddddddddddddddddddddddddddddddddddd";

fn pinned() -> Release {
    Release {
        tag: "v17.9.0".into(),
        tag_object: A.into(),
        commit: B.into(),
    }
}

#[test]
fn matching_pin_and_numeric_stable_update_detection() -> Result<()> {
    let pin = pinned();
    let original = format!("{A}\trefs/tags/v17.9.0\n{B}\trefs/tags/v17.9.0^{{}}\n");
    assert_eq!(compare_refs(&pin, &original)?, (pinned(), Status::Current));
    let next = format!(
        "{C}\trefs/tags/v17.10.0\n{D}\trefs/tags/v17.10.0^{{}}\n{A}\trefs/tags/v18.0.0-rc.1\n"
    );
    let (latest, status) = compare_refs(&pin, &format!("{original}{next}"))?;
    assert_eq!(status, Status::NewRelease);
    assert_eq!(latest.tag, "v17.10.0");
    assert_eq!(latest.tag_object, C);
    assert_eq!(latest.commit, D);
    assert_eq!(compare_refs(&pin, &next)?.1, Status::PinMissing);
    Ok(())
}

#[test]
fn edited_tags_and_incomplete_ref_outputs_do_not_match() -> Result<()> {
    let pin = pinned();
    let moved = format!("{A}\trefs/tags/v17.9.0\n{D}\trefs/tags/v17.9.0^{{}}\n");
    assert_eq!(compare_refs(&pin, &moved)?.1, Status::PinChanged);
    let retagged = format!("{C}\trefs/tags/v17.9.0\n{B}\trefs/tags/v17.9.0^{{}}\n");
    assert_eq!(compare_refs(&pin, &retagged)?.1, Status::PinChanged);
    assert!(compare_refs(&pin, &format!("{A}\trefs/tags/v17.9.0\n")).is_err());
    assert!(compare_refs(&pin, &format!("{moved}{moved}")).is_err());
    assert!(compare_refs(&pin, "not Git refs").is_err());
    Ok(())
}

#[test]
fn pin_requires_full_ids_and_manifest_agreement() -> Result<()> {
    let pin = pinned();
    let json = serde_json::to_string(&pin)?;
    assert_eq!(
        reviewed_pin(&json, "[package]\nversion = \"17.9.0\"\n[dependencies]\n")?,
        pin
    );
    assert!(reviewed_pin(&json, "[package]\nversion = \"17.8.0\"").is_err());
    assert!(reviewed_pin(&json, "[dependencies]\nversion = \"17.9.0\"").is_err());
    assert!(reviewed_pin(&json.replace(B, "123"), "[package]\nversion = \"17.9.0\"").is_err());
    for tag in ["v17.09.0", "v17.9.0-rc.1", "v17.9.0+stable", "v17.9.0/../x"] {
        assert!(stable_version(tag).is_none());
    }
    Ok(())
}

#[test]
fn launch_agent_has_escaped_absolute_arguments_and_read_only_action() -> Result<()> {
    let xml = launch_agent(Path::new("/path/Studio&Tools"), Path::new("/path/<Studio>"))?;
    assert!(xml.contains("<string>/path/Studio&amp;Tools</string>"));
    assert!(xml.contains("<string>/path/&lt;Studio&gt;</string>"));
    assert!(xml.contains("<string>record-upstream</string>"));
    assert!(xml.contains("<integer>86400</integer>"));
    assert!(!xml.contains("prepare"));
    assert!(!xml.contains("StandardOutPath"));
    assert!(xml_path(Path::new("/path/\ninvalid")).is_err());
    Ok(())
}

#[test]
fn recorded_status_is_replaced_in_private_directory_and_symlinks_are_rejected() -> Result<()> {
    let home = tempfile::tempdir()?;
    let home = home.path().canonicalize()?;
    fs::create_dir_all(home.join("Library/Application Support"))?;
    let mut report = Report {
        schema_version: 1,
        checked_at_unix_seconds: 1,
        studio_commit: D.into(),
        pinned: pinned(),
        latest: pinned(),
        status: Status::Current,
    };
    let path = record(&report, &home)?;
    report.checked_at_unix_seconds = 2;
    assert_eq!(record(&report, &home)?, path);
    let written: serde_json::Value = serde_json::from_slice(&fs::read(&path)?)?;
    assert_eq!(written["checked_at_unix_seconds"], 2);
    assert_eq!(written["status"], "current");
    assert_eq!(path.symlink_metadata()?.mode() & 0o777, 0o600);
    fs::remove_file(&path)?;
    let other = home.join("original");
    fs::write(&other, "original")?;
    symlink(&other, path)?;
    assert!(record(&report, &home).is_err());
    assert_eq!(fs::read_to_string(other)?, "original");
    Ok(())
}
