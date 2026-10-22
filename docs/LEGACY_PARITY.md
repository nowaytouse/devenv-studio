# Legacy capability parity checkpoint

This is a read-only comparison of the LTSC_Tools_Backup source tree with LTSC Studio's 0.2 setting catalog and 0.3 deployment catalog/UI work. A matching label does not mean the old and new implementations behave identically.

## What the legacy application actually provides

The inspected legacy GUI and profile model expose nine execution targets: full setup, developer tools, network, storage, Windows features, VS Code extensions, agent skills, system tweaks, and latest rollback. Its setup profile covers multiple providers (WinGet, Scoop, Cargo, NPM, Pip, and UV), Mac-inventory-to-Windows package mapping, optional curated software, VS Code extensions, agent skills/plugins, Git and shell profile configuration, NPM configuration, and optional Ollama models.

Separate options configure network/TCP behavior, storage TRIM and Windows media-aware optimization, power settings, telemetry/consumer-content preferences, and Windows optional features. Its journal can restore recorded registry, TCP, TRIM, and power-plan values. It does not uninstall software or reverse enabled Windows components. Its Mac inventory UI supports refresh, HTTPS source configuration, local JSON import/export, and profile editing.

Evidence: the legacy `src/app.rs` target-specific workspace controls and `src/config.rs` profile/provider model; the legacy `README.md` and `Docs/READINESS.md` describe their documented limits. The old repository was read-only during this audit. It had pre-existing modifications to `Cargo.toml` and `Cargo.lock`; those were left untouched.

## What Studio covers in the current catalogs

The 0.2 registry catalog has 45 DWORD setting options in 31 independent settings/choice groups, with privacy, Explorer, Start, taskbar, general experience, personalization, and accessibility categories. These are user-selected, target-gated, reviewable preferences with original-value recovery. They are not a replacement for the legacy system-tuning profile: they do not cover TCP/network profiles, TRIM/storage optimization, power plans, broad telemetry policy, or the legacy consumer-features switch.

The 0.3 deployment catalog contains 30 WinGet package tasks, 7 opt-in Windows feature enables, and 4 maintenance commands. The desktop workbench keeps settings and deployment in separate pages and execution confirmations; selected tasks remain explicit. Feature enablement and repair commands have separate effect boundaries from registry recovery.

Evidence: `data/catalog.json`, `data/deployment.json`, and `docs/DESIGN.md`. Catalog presence documents available choices, not successful execution on a Windows device. The task runner and UI still require the release's Windows and LTSC acceptance checks.

## Functional gaps that remain

- No migrated Mac inventory refresh/import/export workflow or name-level Mac-to-Windows parity report.
- No migrated Scoop, Cargo, NPM, Pip, or UV installation tasks; the new deployment catalog is WinGet-only.
- No VS Code extension deployment, agent-skill/plugin payload provisioning, Git or shell profile merge, NPM mirror configuration, or Ollama model setup.
- No network/TCP profile, storage/TRIM optimization workflow, power-plan tuning, or legacy broad telemetry/consumer-content profile.
- The new setting journal restores its recorded registry values; it does not undo installed packages, Windows components, or completed maintenance. The legacy journal also does not uninstall software or turn off enabled Windows features, but it records additional TCP, TRIM, and power state.
- Windows host behavior, applicable LTSC builds, network prerequisites, administrator prompts, and recovery still need the documented device acceptance. macOS preview and compilation are not Windows execution proof.

This checkpoint is a feature comparison, not a migration-complete claim. Legacy project files and history remain separate from LTSC Studio.
