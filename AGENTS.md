# DevEnv Studio contributor instructions

- Rust owns the catalog, plan validation, Windows mutations, recovery and GUI state.
- Windows 10 Enterprise LTSC 2021 and Windows 11 (IoT) Enterprise LTSC 2024 are distinct targets. Do not infer support from a Windows major version alone.
- macOS/Linux preview must never claim to have inspected or changed a Windows machine.
- All configuration actions are opt-in. Preserve original raw values before writing and verify results. Stop on journal, readback and unsupported-host failures.
- Do not execute downloaded scripts, disable security/update components, embed credentials or include personal data.
- Pin source revisions, keep provenance per action and preserve relevant third-party notices.
- Keep existing user work, stage explicit paths, and report local tests, target compilation, CI and device acceptance separately.
- Use cargo fmt --all -- --check, cargo test --workspace and cargo clippy --workspace --all-targets -- -D warnings for this workspace. Windows adapter changes also need a Windows target check when available.
- Do not monitor CI unless the user requests it.
- Keep changes under Unreleased and do not bump package versions, create tags or publish releases for each task or conversation turn. Choose a version only at an explicitly authorized release checkpoint; follow docs/RELEASING.md.
- DevEnv Studio is the product name and devenv-studio is the canonical package/command. Preserve intentional legacy CLI aliases, storage locations, bundle identity, schema fields, catalog IDs and authenticated history unless a separately reviewed migration requires changing them.
