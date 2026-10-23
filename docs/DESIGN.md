# LTSC Studio — design record

## 0.9 channel-aware redeployment

Homebrew policies distinguish missing-only Bundle configuration, rolling updates using local metadata, and native HEAD installation/update with independent installed-channel readback. Existing HEAD upgrades require a matching linked HEAD keg; ambiguous stable/HEAD or keg-only opt state fails before mutation. No forced unlink, overwrite, cleanup or revision-lock claim is introduced.

A separate portable Rust request accepts official rolling channels, exact releases and dated nightly. Standard rustup capture and redeployment work on macOS/Windows, with a durable baseline/request/result, a preserved preexisting default, minimal profile and independent explicit-channel rustc release/hash/date evidence. Rust requests round-trip independently from Windows workspace profiles. Native rust-toolchain.toml and Rust mise.toml exports contain only validated fields; mise execution/locking and arbitrary Git source configuration remain outside this implementation.

## 0.8 native developer inventory and configuration on macOS and Windows

macOS is now a real inventory source with an explicit collect/review/save/load workflow. Six provider categories preserve identities, installed versions and independent complete/unavailable/failed states. Bounded read-only commands use standard paths and suppress auto-update/bootstrap behavior; raw diagnostics are sanitized. A distinct versioned backup document never includes projects, credentials, configuration or environments and is published without overwriting prior files. Cancellation or collection errors preserve the current draft. Saved backups feed the same trusted Windows mapping workbench with no selected rows; failed lists never authorize suggestions and source versions do not enter executable arguments. Large snapshots are mapped in bounded batches without truncation.

Windows collection uses five fixed providers, sharing the existing Cargo/uv/NPM/Pip/VS Code discovery and parsers; platform-specific provider sets preserve the original macOS schema. A native environment workbench selects formula/cask identities from reviewed Mac backups and starter tools, generates a restricted Brewfile and delegates to Homebrew Bundle install followed by an independent presence check. No arbitrary Ruby import, automatic cleanup or uninstall is used. Configuration may change dependencies and has no rollback; loaded backups are source evidence, not proof of this device's prior state. Generated Brewfile and selected WinGet package manifests use no-overwrite publication. Upstream contracts and pins are recorded in ENVIRONMENT_SOURCES.md; mise/chezmoi are design references only.

Pip uses a fixed registered CPython interpreter and validated user layout per task, with runtime preflight and repeated identity checks. Existing visible distributions skip installation; otherwise compatible wheels install to user scope with fixed PyPI and isolated configuration. Exit0 without matching metadata fails, and metadata visibility is not a Python import or tool launch test. A separate Python data recipe joins software prerequisites, Pip libraries and editor support. Developer provider filters keep the larger mixed catalog navigable.

## 0.7 storage maintenance and notification inspection

Storage reuses the existing maintenance task, workspace v3, shared mutation lock, cancellation and durable execution history. Four exact IDs distinguish analysis/optimization and the Windows volume/all volumes; the Windows volume resolves from the shared OS directory to a validated volume GUID. Only tools actually needed by the selected plan are resolved. No drive-letter guess, shell, parallel defrag or raised priority is introduced. Exit0 means the native command completed, not verified health, performance or coverage of every volume. Optimization has no undo.

Delete-notification inspection is a separate worker and CLI query, never an implicit scan or policy write. Independent NTFS/ReFS states preserve enabled, disabled, not set and unknown; malformed, duplicate or failed output is observable. Preview does not read local disks as Windows evidence. Legacy optimize_storage maps exactly to all-volume optimization, while enable_trim remains manual because a single original value cannot represent independent or unset filesystem policy states. Sources, bounds and device gaps are recorded in STORAGE.md.

## 0.6 active-scheme AC power domain

Power choices use a separate trusted catalog of16values in5mutually exclusive groups. A native PowrProf adapter captures the active scheme once, reads all original AC values before mutation, persists a keyed and chained original-value journal, then writes, reactivates the same scheme and verifies readback. External scheme/value changes stop the operation; restoration accepts only recorded original or desired values and refuses to activate a different scheme. Power journals share recovery.key and mutation.lock with existing domains, with independent filename/path/content/bounds validation. No scheme creation, DC mutation, CPU or hibernation control is introduced.

Workspace schema3 adds explicit power IDs. v1/v2 load with an empty power selection, and v2 cannot carry power changes. UI and CLI build the same canonical power plan, while import analysis retains legacy Ultimate Performance setup as manual because changing only selected AC settings is not equivalent. macOS/Linux remain preview-only and never read or modify a Windows power scheme.

## 0.5 NPM and policy continuity

NPM reuses the developer task and history protocol, with a fixed official Node installation, explicit per-user global prefix and registry, configuration isolation and lifecycle scripts disabled. Installation success requires exact JSON package identity/version readback; it does not prove CLI functionality. Exact trusted IDs from SetupProfile or Mac inventory can join the same workflow; imported commands and arbitrary package specifications remain non-executable. The Web recipe explicitly selects Node prerequisites and NPM tools. Large profiles stay exportable while GUI inspection/execution uses the same session bound as the backend.

The new registry catalog preserves every prior action and ships the immutable 2026-10-21.2 snapshot alongside the first snapshot. Historical journals are validated against their own trusted revision. New device policies use the existing write-ahead original-value journal and native readback; a matching registry value does not establish that organization policy or all Windows behavior changed.

## 0.4 mixed workflows and legacy migration

Recipes compose canonical settings and deployment IDs into one v2 workspace. Merging is transactional: validate all added identities, resolve mutually exclusive setting groups, preserve unrelated selections, then validate the complete result before assigning GUI state. Migration reads only explicitly supplied SetupProfile2.2.0 or MacInventory1 JSON, rejects duplicate keys and input bounds violations, and reports every requested, false, empty, custom or unsupported field. Imports start unchecked. Only selected mapped rows enter the same merge path; private configuration values and arbitrary commands never become executable payloads. Analyzed path and target identify the report; either changing disables merge pending reanalysis.

The pinned WinUtil applications snapshot supplies234native WinGet IDs. Original task identities and historical records remain stable. Cargo, uv and VS Code task kinds use strict provider-specific IDs and the existing bounded runner, cancellation, durable history and exact readback. Plans order WinGet before developer tasks, then features and maintenance. Developer tools resolve fixed known-folder locations per task so prerequisites explicitly installed earlier can become available without restarting the app. uv automatic Python download is disabled. No package-manager bootstrap or imported script execution is introduced. Provider presence does not prove a functioning compiler/linker/SDK/Python environment. Catalog bounds grow to2048; native inspection/execution remains capped at128tasks.

## 0.3 application shell and preferences

The desktop now groups eleven pages into Workspace, Execution and Application. A compact header replaces the decorative hero. Shared tokens supply five accent palettes for light and dark appearance, including the native system preference, while interface zoom preserves display DPI and density affects spacing. Navigation and each page scroll independently.

Application preferences use a separate versioned file, with immediate preview and explicit save. Validated JSON is staged beside the destination, synced and atomically renamed over the prior valid file. Corrupt, unsafe, oversized or future-version preferences remain untouched and visible as errors. Preferences never select or execute Windows actions.

Chinese and English chrome use explicit translation pairs. English catalog descriptions are an embedded complete overlay keyed by stable IDs; only presentation clones are localized. Canonical catalogs, executable plans, profiles and recovery validation remain unchanged. Original technical output is retained. Translation validation failures remain observable in Settings and Status.

Status derives from host detection, validated catalogs, capability checks, active workers and returned reports. Dismissing a notice does not discard the last operation report. Persisted deployment history is read only from the application directory with file/event/aggregate bounds; errors remain visible, unfinished runs remain interrupted, and final counts must agree with task evidence. History is neither current-machine inspection nor restore authorization.

Help explains preparation, confirmations, prerequisites, troubleshooting and recovery boundaries. About reports the actual package version, licensing and pinned source provenance. Closing an idle app exits; closing during work requests cancellation and requires the joined report before exit. Legacy feature gaps are tracked in LEGACY_PARITY.md.

## 0.2 workbench extension

The second delivery adds a deployment domain alongside reversible registry preferences. There are 45 setting options in 31 independent setting groups, plus 30 WinGet packages, 7 optional feature-enables and 4 maintenance commands. GUI pages browse these domains separately; v2 workspace profiles and exports preserve one selection. Each domain has its own execution confirmation and recovery boundary.

Registry plans validate mutual-choice groups and selected-address collisions. Read-only inspection returns desired/current alignment; non-Windows inspection returns explicit preview states. Three presets select editable choices only. The first catalog snapshot remains built in for authenticated historical recovery validation; imported catalogs never authorize writes.

studio-deployment owns immutable task definitions, native argv, trusted executable resolution, bounded output/deadlines, cancellation, pre-mutation execution history and readback. It shares the mutation lock with registry apply/restore. Package installation requires configured Microsoft App Installer and source agreements. Feature-enables use DISM without automatic restart. Maintenance command completion is distinct from confirmed repair.

Execution history cannot undo software, components or repairs. Native cancellation reaps the direct child but cannot guarantee Windows servicing or installer descendants stopped. Windows device acceptance remains a separate requirement.

## Original 0.1 design and intent

User intent: a new Rust-first application combining selected capabilities from LTSC_Tools_Backup and mature Windows configuration projects. Every configuration change is chosen by the user. One native desktop GUI runs on macOS and Windows. Repository: nowaytouse/ltsc-studio, private initially.

## First integrated slice

A shared, reviewed action catalog feeds category browsing, search, selection, exact plan preview, reusable profiles and execution. Source IDs and immutable upstream revisions accompany every mapped action. Initial coverage is deliberately reversible registry configuration: privacy, Explorer and desktop experience. Unsupported features are documented, never represented as working controls. Software deployment, driver updates, Store repair and aggressive debloating remain future integrations.

The GUI uses eframe/egui 0.37 on native desktop renderers. On macOS/Linux it is a real interactive preview: selecting a Windows target, inspecting actions and exporting plans/profiles work, while Windows mutation is unavailable. On Windows the actual host edition/build determines applicability; unsupported editions/builds fail closed.

## Components and acceptance

- studio-core: catalog validation, target applicability, profile validation and plan construction, JSON exports, portable recovery engine with deterministic failure tests.
- studio-platform: native Windows registry adapter, supported-host detection, persistent journal and serialized mutation. Other platforms return explicit unsupported errors for mutation.
- studio-app: native GUI, command line plan/profile inspection, background apply/recovery, cooperative cancellation and readable outcomes.

No selected actions on first launch. No download-and-execute scripts. Each mutation saves the original raw registry value before writing, verifies readback and leaves durable recovery information. Restore protects later external changes with conflict checks and revalidates catalog addresses. Journal failures stop mutation. Cancellation waits for the current native operation and reports partial work. Recovery covers recorded values, not a full OS image; absent values are removed but newly created empty keys may remain.

Acceptance for this delivery: meaningful core failure-path tests, native macOS build and visual inspection, Windows target compile checks if available, fresh private repository with explicit staged files and verified remote parity. Windows execution/device acceptance remains a separate requirement and is not inferred from tests or cross-compilation.

## Visual direction

A quiet configuration workbench with a blue four-pane mark, spacious cool-white canvas and a persistent selection tray. Compact left navigation opens configuration, plan, recovery and sources. Main content uses readable setting rows grouped by intent, with source and applicability details at the point of selection. Hero uses a small desktop illustration rather than generic statistics.

Palette: canvas #F3F6FA, ink #202D48, primary #3B66E8, blue wash #EAF0FF, secondary #6B7890, separator #E0E7F0. Native system fonts provide Chinese coverage; clear text sizes and keyboard navigation come before ornament. The primary action describes its real outcome: review a plan, export it, or apply on a supported Windows host.

## Implementation sequence

1. Audit old project without editing it; pin upstream sources and licenses.
2. Define shared catalog/profile/plan API and implement core plus native adapter.
3. Implement GUI against the same API, CLI and honest platform states.
4. Integrate reviewed catalog/attribution, run focused tests and target checks, inspect macOS GUI.
5. Document scope and validation; create and push private GitHub repository; verify local/remote commit equality. No unsolicited CI monitoring.
