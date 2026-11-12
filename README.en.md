# DevEnv Studio

[简体中文](README.md) · English

DevEnv Studio is a Rust desktop workbench for backing up, reviewing, and redeploying development environments on macOS and Windows. Linux supports loading, previewing, and exporting plans. The current development baseline is **0.10.0**.

## What you can do

- **Back up and inspect tools.** Capture a bounded inventory of the supported tools on the current host. The app distinguishes complete and partial inventories, unavailable tools, and failed checks. Save the resulting inventory to a private JSON file.
- **Review and redeploy.** Start from an inventory or choose individual tools. Supported actions include Homebrew and selected development tools on macOS, mise and Rust toolchains on both platforms, and reviewed WinGet, Scoop, Cargo, uv, Pip, npm, and VS Code workflows on Windows. Each action requires review and confirmation.
- **Work with Windows LTSC.** Select system settings, packages, optional features, or maintenance tasks. Native system configuration is restricted to Windows 10 Enterprise LTSC 2021 and Windows 11 (IoT) Enterprise LTSC 2024. Preview mode uses the selected Windows target.
- **Review updates and recovery evidence.** View the Topgrade update catalog and back up explicitly selected configuration files. The app preserves recorded originals for supported Windows settings and maintains separate deployment history.
- **Choose your language.** Switch between Simplified Chinese and English in application settings. Package identities, commands, and exported plans use the canonical catalog.

## Build and run

Use Rust 1.95 or newer and the repository's stable toolchain. On Windows x64, open PowerShell 7 in the repository and initialize the process-local Microsoft C++ build environment before running Cargo.

```powershell
& .\scripts\Enter-DevEnvironment.ps1
cargo run --locked -p devenv-studio
```

On macOS, start the application with Cargo.

```sh
cargo run --locked -p devenv-studio
```

The compatibility command `ltsc-studio` still starts the same implementation. The installed app continues to use the existing `LTSC Studio` data directory.

## Contribute

- Read the [integration contracts](docs/ENVIRONMENT_SOURCES.md) and [release and Windows build notes](docs/RELEASING.md).
- Edit the [GUI translation resource](data/locales/ui.json) for messages using `t(...)`. Each entry pairs the exact Simplified Chinese source text with an English translation. Preserve format placeholders. The [English catalog overlay](data/locales/en-US.json) is separate from the executable catalog.
- Keep user inventories, credentials, machine-specific files, and raw logs outside source control. Use synthetic fixtures for tests.

```sh
cargo fmt --all -- --check
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
```

The [Chinese README](README.md) contains the detailed feature guide, CLI examples, and platform constraints.
