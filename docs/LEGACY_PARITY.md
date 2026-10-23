# Legacy capability parity checkpoint

This is a read-only comparison of the LTSC_Tools_Backup source tree with LTSC Studio 0.8. The setting catalog now includes the legacy diagnostic-data and consumer-experience policies; shipped historical snapshots remain available for recovery. A matching label does not mean the old and new implementations behave identically.

## What the legacy application actually provides

The inspected legacy GUI and profile model expose nine execution targets: full setup, developer tools, network, storage, Windows features, VS Code extensions, agent skills, system tweaks, and latest rollback. Its setup profile covers multiple providers (WinGet, Scoop, Cargo, NPM, Pip, and UV), Mac-inventory-to-Windows package mapping, optional curated software, VS Code extensions, agent skills/plugins, Git and shell profile configuration, NPM configuration, and optional Ollama models.

Separate options configure network/TCP behavior, storage TRIM and Windows media-aware optimization, power settings, telemetry/consumer-content preferences, and Windows optional features. Its journal can restore recorded registry, TCP, TRIM, and power-plan values. It does not uninstall software or reverse enabled Windows components. Its Mac inventory UI supports refresh, HTTPS source configuration, local JSON import/export, and profile editing.

Evidence: the legacy `src/app.rs` target-specific workspace controls and `src/config.rs` profile/provider model; the legacy `README.md` and `Docs/READINESS.md` describe their documented limits. The old repository was read-only during this audit. It had pre-existing modifications to `Cargo.toml` and `Cargo.lock`; those were left untouched.

## What Studio covers in the current catalogs

The 0.5 registry catalog has 58 DWORD setting options in 40 independent settings/choice groups, with privacy, Explorer, Start, taskbar, general experience, personalization, and accessibility categories. These are user-selected, target-gated, reviewable preferences with original-value recovery. They are not a replacement for the legacy system-tuning profile: they do not cover TCP/network profiles or TRIM policy writes. Storage analysis/optimization is now modeled separately as maintenance. A separate active-scheme AC power domain now provides16choices across5groups with its own authenticated originals and recovery; it does not create or switch Ultimate Performance schemes or change CPU/hibernation/DC values. The two exact legacy diagnostic-data and consumer-experience registry policies now map into the trusted catalog; this does not claim all Windows telemetry has stopped.

The 0.5 deployment catalog contains 234 WinGet package tasks, 8 Cargo tools, 4 uv tools, 37 Pip distributions, 6 NPM tools, 8 Microsoft VS Code extensions, 7 opt-in Windows feature enables, 4 system maintenance commands, and 4 storage analysis/optimization tasks. Six mixed recipes combine these domains. Legacy SetupProfile2.2.0 and MacInventory1 JSON imports propose exact trusted mappings for explicit review, retaining manual and unsupported items. The desktop workbench keeps settings and deployment in separate pages and execution confirmations; selected tasks remain explicit. Feature enablement and repair commands have separate effect boundaries from registry recovery.

Evidence: `data/catalog.json`, `data/deployment.json`, and `docs/DESIGN.md`. Catalog presence documents available choices, not successful execution on a Windows device. The task runner and UI still require the release's Windows and LTSC acceptance checks.

## Functional gaps that remain

- Explicit native macOS inventory collection and local versioned backup now cover six provider categories; saved backups flow into the same opt-in migration review. There are31curated formula/cask mappings. Binaries, projects, configuration, virtual environments, network refresh and broad name-level equivalence remain outside this inventory backup.
- Cargo, uv, Pip and NPM execution now exist for trusted entries. All26fixed legacy Pip identities map to reviewed Pip tasks; seven reviewed Homebrew aliases map to Pip while Ruff remains uv. Scoop, arbitrary requirements and custom packages remain manual; imported arguments never become executable tasks.
- Microsoft VS Code extension deployment now exists for reviewed IDs. Cursor/editor settings merge, agent-skill/plugin payload provisioning, Git or shell profile merge, NPM mirror configuration and Ollama model setup remain unmigrated.
- No network/TCP profile or recoverable TRIM policy writes. Storage now offers explicit Windows-volume/all-volume analysis and media-aware optimization with execution history, plus read-only NTFS/ReFS notification-policy inspection. The old optimize_storage field maps only to all-volume optimization; enable_trim stays manual because independent original and unset states need a separate recovery contract. Active-scheme AC power choices now exist; legacy Ultimate Performance setup remains manual because it has different scheme/CPU/hibernation semantics. The legacy device policies now map exactly, while numeric tuning and other unsupported changes remain explicit report items.
- The new setting journal restores its recorded registry values; it does not undo installed packages, Windows components, or completed maintenance. The legacy journal also does not uninstall software or turn off enabled Windows features, but it records additional TCP, TRIM, and power state.
- Windows host behavior, applicable LTSC builds, network prerequisites, administrator prompts, and recovery still need the documented device acceptance. macOS preview and compilation are not Windows execution proof.

This checkpoint is a feature comparison, not a migration-complete claim. Legacy project files and history remain separate from LTSC Studio.
