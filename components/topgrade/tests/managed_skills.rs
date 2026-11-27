#![cfg(target_os = "macos")]

use std::{fs, process::Command};

#[test]
fn skills_failure_reaches_the_summary_and_the_next_step() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("topgrade.toml");
    let skills = include_str!("../profiles/macos/config/topgrade.d/95-skills-unattended.toml");
    fs::write(
        &config,
        format!(
            "[misc]\nonly = [\"skills\", \"custom_commands\"]\nask_retry = false\nauto_retry = 1\nnotify_end = \"never\"\n{skills}\n[commands]\nprobe = \"echo ran-after-skills\"\n"
        ),
    )
    .unwrap();

    // A non-default state directory fails the ownership check before any user skills are touched.
    #[expect(
        clippy::disallowed_methods,
        reason = "This integration test exercises the actual CLI as a child process"
    )]
    let output = Command::new(env!("CARGO_BIN_EXE_topgrade"))
        .args(["-y", "--no-tmux", "--config"])
        .arg(config)
        .env("XDG_STATE_HOME", temp.path())
        .env("TOPGRADE_SKIP_BRKC_NOTIFY", "true")
        .env("LC_ALL", "C")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(stdout.contains("ran-after-skills"), "{stdout}");
    assert!(
        stdout.contains("Skills: FAILED") && stdout.contains("probe: OK"),
        "{stdout}"
    );
}
