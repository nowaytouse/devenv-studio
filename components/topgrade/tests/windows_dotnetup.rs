#![cfg(windows)]

use std::{fs, process::Command};

#[test]
fn daily_and_preview_run_once_without_the_legacy_scripts() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("topgrade.toml");
    fs::write(
        &config,
        r#"
[commands]
daily = 'pwsh.exe -NoProfile -File "$env:APPDATA\topgrade\Update-DotnetDaily.ps1"'
preview = 'pwsh.exe -NoProfile -File "$env:APPDATA\topgrade\Update-DotnetupPreview.ps1"'
"#,
    )
    .unwrap();

    for selection in [
        &["--only", "custom_commands", "--custom-commands", "daily", "preview"][..],
        &["--only", "custom_commands", "dotnet_daily", "dotnetup_preview"][..],
    ] {
        #[expect(
            clippy::disallowed_methods,
            reason = "integration test launches the Topgrade executable"
        )]
        let output = Command::new(env!("CARGO_BIN_EXE_topgrade"))
            .current_dir(dir.path())
            .env("APPDATA", dir.path())
            .args([
                "--dry-run",
                "--no-self-update",
                "--no-ask-retry",
                "--notify-end",
                "never",
                "--config",
            ])
            .arg(&config)
            .args(selection)
            .output()
            .unwrap();
        let log = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.status.success(), "{log}");
        assert_eq!(
            log.matches("Would update and verify the isolated daily SDK").count(),
            1,
            "{log}"
        );
        assert_eq!(
            log.matches("Would update and verify dotnetup preview").count(),
            1,
            "{log}"
        );
        assert!(
            !log.contains("Update-DotnetDaily.ps1") && !log.contains("Update-DotnetupPreview.ps1"),
            "{log}"
        );
    }
}
