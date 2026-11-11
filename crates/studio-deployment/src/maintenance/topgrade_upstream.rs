use crate::process::{Invocation, run_configured};
use anyhow::{Context, Result, ensure};
use std::{sync::atomic::AtomicBool, time::Duration};

const OFFICIAL_REPOSITORY: &str = "https://github.com/topgrade-rs/topgrade.git";

/// Read only the public upstream refs, with no credentials or local Git configuration.
pub fn official_tag_refs() -> Result<String> {
    let invocation = Invocation {
        program: "/usr/bin/git".into(),
        args: [
            "-c",
            "http.followRedirects=false",
            "-c",
            "http.lowSpeedLimit=1",
            "-c",
            "http.lowSpeedTime=8",
            "ls-remote",
            "--tags",
            OFFICIAL_REPOSITORY,
            "refs/tags/v*",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        vscode_node: false,
        npm_node: false,
    };
    let output = run_configured(
        &invocation,
        &AtomicBool::new(false),
        Duration::from_secs(20),
        |command| {
            command
                .env_clear()
                .current_dir("/")
                .env("HOME", "/var/empty")
                .env("PATH", "/usr/bin:/bin")
                .env("LC_ALL", "C")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_TERMINAL_PROMPT", "0");
        },
    )
    .context("Read official Topgrade tags over HTTPS")?;
    ensure!(!output.timed_out, "Official Topgrade tag lookup timed out");
    ensure!(
        output.code == 0,
        "Official Topgrade tag lookup failed (exit {})",
        output.code
    );
    Ok(output.stdout)
}
