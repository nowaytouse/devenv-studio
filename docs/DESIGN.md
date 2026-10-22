# LTSC Studio — design record

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
