# LTSC Studio — design and first delivery

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
