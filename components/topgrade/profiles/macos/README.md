# This Mac's maintained Topgrade profile

This Apple Silicon macOS profile configures the managed Rust Topgrade tasks. The manifest covers the configuration, compatibility helpers, and reviewed npm patch assets. The managed Homebrew step updates both the official LLVM bottle in its installed stable series and the separately installed LLVM preview.

DevEnv Studio builds the tracked Studio HEAD, verifies the matching profile, and atomically replaces the executable at `~/.local/bin/topgrade`. The built-in Helm and GitHub Skills steps use their native validators. The older standalone helpers remain available for legacy entry points and recovery.

The Homebrew formula step refreshes catalogs before checking for upgrades. When a tap has local changes, the step preserves that checkout and fast-forwards clean tracked tap checkouts, including `homebrew/core`. The log names deferred taps and reports refresh failures. A tap with no tracking branch whose sole remote points to itself is reported as local-only. A failed core refresh stops formula upgrades. The step checks the requested formulae again after upgrades. Deferred catalogs or formulae still reported as outdated produce `PARTIAL` with a nonzero exit status. Intentional pin and separately managed formula exclusions remain visible.

The official LLVM bottle updates before its consumers. The stable pin advances after the Homebrew Rust runtime checks pass. The installed preview follows the exact official main commit after the ordinary formula updates. Explicit `--only` runs skip global pre/post hooks.

Topgrade reports incomplete checks separately from completed steps. Dry runs and post-commands appear in the summary. If a selected configuration or its includes cannot be loaded, the CLI stops before executing updates.

HEAD-sync also checks for consumers still linked to an old JPEG XL ABI after a bottle upgrade, even when JPEG XL itself is current. It rebuilds only the affected consumers from source, verifies the active links and sample codec operations, and uses the same journal to recover a failed repair. BetterDisplay's verified check uses the official Sparkle feed with the three configured channels. The app manages its own update preference.

## Review and install

Use the Studio maintenance CLI against the existing tracked checkout:

```sh
devenv-studio topgrade-engine plan /absolute/studio-checkout
devenv-studio topgrade-engine update /absolute/studio-checkout
devenv-studio topgrade-engine verify-current /absolute/studio-checkout
```

The updater fetches the branch already configured in Git and accepts only a clean checkout and a fast-forward. It validates the candidate before replacing the one installed executable. The engine receipt is stored on that executable. The installed helper at `~/.local/bin/topgrade-rustup-headsync` delegates `plan` and `update` to `~/.local/bin/devenv-studio` using the supplied absolute checkout path.

Profile writes still use their existing lock and journal. Initial adoption checks an existing file against the known preimage. Later installs check the last recorded bytes and stop on local edits or a pending transaction. Existing profile journals and backups remain in `~/.local/state/topgrade-macos-profile/transactions/`.

The standalone legacy fixtures use Python 3.15. The Rust SQLite integration still verifies the Homebrew Python 3.15 extension against its target interpreter. Other tools, such as Conda, retain their own runtimes.

## Existing local prerequisites

This is a profile for the already maintained Mac, not a bootstrap of every tool from an empty computer. Keep these machine-owned settings and assets outside this public repository:

- Homebrew, its taps, exact pins, installed formula options, Rustup Nightly, Xcode and macOS permissions.
- `~/.config/project-deps.json` (local repository roots and policy), `~/.agents/.skill-lock.json`, Cursor's bundled skills, Codex/Claude settings, Git and npm configuration, tokens and Keychain credentials.
- Conda environments, MAS account authorization, cloud components and vendor application preferences.
- The SQLite source checkout at `~/Developer/sqlite-git`, generated SQLite runtime/shims, Gradle runtime and caches, and the existing Sparkle CLI at `~/.local/opt/sparkle-cli/sparkle.app/Contents/MacOS/sparkle`.
- Prior journals, backups, package receipts, application state, downloaded archives and logs.

Generated runtime files stay with their existing state directories. Studio installs the standard `~/.config/topgrade.toml` and its fragments. Tool paths target Apple Silicon Homebrew's `/opt/homebrew`.

Skills remain updated by their existing owners and independent coverage checks. A literal empty disable list would duplicate owners for HEAD-sync, Skills, MAS, gcloud and Conda. There is no `ignore_failures` setting.

## Upstream maintenance and Helm

Review upstream revisions against the imported Topgrade ancestry. Make component changes in Studio, then run Topgrade Rust checks and the profile tests independently. Use Studio engine maintenance to build and switch the reviewed component and its verified profile.

Helm remains an enabled built-in step. The optional updater downloads the entire public Bitnami index through official HTTPS with compression and compares its complete bytes to the regular local index. Only an identical body that also passes native Helm index parsing is reported current, without writing Helm's cache. Special files are opened without blocking and rejected. Changed/absent indexes and other repositories use native Helm, and native failure remains nonzero. TLS verification, official redirect-host checks, bounded downloads, local configuration drift checks and rejected cache symlinks remain active. This addresses the unchanged large-index transfer case; it does not promise that future changed-index transfers will succeed.

## GitHub CLI Skills ownership

The enabled GitHub CLI Skills step uses `[github_cli].skills_native`. It validates installed metadata and registered ownership before updating anything. GitHub-managed directories are grouped by parent and selected by directory basename with `gh skill update --dir ... --all ...`.

Skills without GitHub metadata stay with their verified existing owners. NPX records must match the canonical shared installation; additional host copies must match its complete file tree. Cursor mirrors must match the existing fixed native-skill allowlist and vendor contents. Unknown owners, invalid metadata, changed copies or pinned entries stop this step instead of being reported current. This does not claim that NPX or Cursor updated anything: their existing independent updater and coverage checks still run.

Before updates, the helper follows native GH's unpinned reference policy: the latest published release, or the repository default branch when no release exists. Other release lookup failures remain errors. The helper walks only the selected Git trees and verifies each declared directory and regular SKILL.md. Truncated or invalid responses fail closed. After native updates, metadata must match the verified directory tree SHA and original source/path. A nonzero exit, native warning or unknown completion output remains a failure, including warnings followed by “All skills are up to date.”. Native GH writes its own installation records to the shared lock. The helper validates the lock schema before updates, rejects publisher/path/pin collisions and checks that native writes preserve unrelated entries and dismissed choices. Dry runs require the complete lock bytes to stay unchanged. The helper never injects metadata into foreign skills, independently rewrites the shared lock, unpins skills or uses force. Other installations keep the upstream native behavior unless this optional hook is configured.

The shared lock can also contain NPX's `lastSelectedAgents` and `pluginName` fields. The helper validates these fields and, when verified remote trees already match, returns without calling the native writer. Pending GitHub-owned skills are staged with a private GitHub CLI home. The helper reproduces the installed revision before replacing it and retains the previous directory after an atomic exchange. Locally edited or unreproducible copies are deferred and reported. The shared lock stays byte-identical. Coverage reports attach repository discovery failures to GitHub-tracked copies, separately from NPX copies with the same display name.

An exact previously installed Gemini copy of Google's retired `discovering-gcp-data-assets` can migrate to the official `discover-assets`. The replacement is staged and verified before publication. The old copy moves to a private recovery directory, and the shared NPX lock is left intact. Changed copies and ambiguous owners require review.

The unattended NPX step checks the installed LeanCTX directory against the Git tree ID in the global lock, including executable bits. Existing Cursor, Copilot and OpenCode copies that match the same tree are atomically linked to the canonical NPX directory before updating. The original host directories, including any local `SKILL.md.bak` and empty migration lock, are retained once in the private skill-lock state directory. A local edit, foreign source, pin or redirected link stops adoption. The updater verifies the links and the new locked tree after NPX finishes. Coverage independently checks the host paths even when GitHub CLI reports only the canonical directory.

NPX retains one private backup per distinct pre-update lock. Repeated runs reuse the verified bytes. Earlier recovery directories remain available.

## Per-repository dependency compatibility

The fork opts into its existing compatible dependency-update path with the root file `.topgrade-dependencies.json` containing exactly `{"allow_incompatible": false}`. The repository still participates in Rolling updates; Cargo uses `cargo update` within its declared requirements instead of the edge-mode `cargo upgrade --incompatible allow --pinned ignore`. Other surfaces retain their existing compatible behavior, including action updates.

The Rust rolling task respects exact Cargo requirements. Dirty checkouts are deferred before dependency snapshots and listed by repository in the summary. The task also applies compatible mode to Zig HEAD, Docker tags and Gradle. A malformed, duplicate-key, oversized, symlinked or nonregular policy file fails before that repository's update commands execute. The policy is included in dependency backups. The task uses installed ncu, disables JavaScript installation hooks, and pins the Gradle report plugin. Gradle wrapper updates require a verified distribution checksum and keep the existing scripts and JAR. Shared version references with unverified impact are reported as discovered candidates.

## npm reference data

The guard now reads versioned reference data from `~/.local/share/topgrade-macos-profile/npm-security/`. Its two vendor trees live inside `node_modules/` so the existing Rolling dependency discovery prunes them. They are verified guard-owned package data, not development projects for generic npm dependency updates; narrow Git ignore exceptions version only these two trees. Historical `patches/` destinations remain accepted solely so prior deployment journals can still roll back. It retains full before/after npm fingerprints and the original verified patch metadata. Only complete official `undici 6.29.1` and `ip-address 10.7.5` package trees are shipped, including their MIT licenses; the complete npm installation is not vendored. Exact per-package tree verification, full live-tree post-verification, original-directory rollback, declared ranges and the bounded official advisory query remain.

The validation report is a historical October 2026 probe. Its local workspace paths are replaced by neutral placeholders; its commands are evidence, not commands to execute verbatim. It does not assert that future npm releases or all installed applications are vulnerability-free. Unknown npm trees are checked without being overwritten. New guard receipts/backups live in the private profile state directory.

## Acceptance

Synthetic HOME tests cover read-only planning, installation, idempotence, exact rollback, mode-only rollback, drift, symlink/path rejection, interrupted deployment recovery, disk failure and modified patch trees. These tests never run real updates or use the user's live profile.

The real device acceptance result must be recorded separately from source checks and isolated tests. Helm transfers, plugin marketplaces, MAS authorization and CI billing can still fail independently. Preserve their errors and the original environment rather than removing update owners to manufacture a green summary.
