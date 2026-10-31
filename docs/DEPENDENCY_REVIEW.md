# Dependency review — 2026-10-23

## Topgrade environment refresh and workspace review — 2026-10-31

Reviewed the complete existing dependency delta against `2ada60b`. The dirty manifest had changed the exact `windows-collections` pin from `=0.3.2` to `=0.100.0`, while `windows` remained 0.62.2. A fresh Windows GNU check reproduced six E0277 errors in `winget_bootstrap.rs`: the two versions use incompatible `windows-core` projection types. The original dirty files are preserved privately outside the repository. Restoring the compatible exact pin removed the five unused 0.100.0 packages; the manifest now matches the committed baseline.

The final lockfile retains all ten compatible existing-package updates and adds one required transitive dependency, `utf_types 1.0.0`, through `utf8_iter 1.1.0`. It contains 434 package entries, versus 433 in the baseline; package sources are unchanged.

| Package | Previous | Reviewed | Reviewed Rust minimum | License |
| --- | --- | --- | --- | --- |
| js-sys | 0.3.106 | 0.3.107 | 1.81 | MIT OR Apache-2.0 |
| log | 0.4.34 | 0.4.35 | 1.71.0 | MIT OR Apache-2.0 |
| utf8_iter | 1.0.4 | 1.1.0 | 1.83 | Apache-2.0 OR MIT |
| utf_types | not present | 1.0.0 | 1.83 | Apache-2.0 OR MIT |
| wasm-bindgen | 0.2.129 | 0.2.130 | 1.81 | MIT OR Apache-2.0 |
| wasm-bindgen-futures | 0.4.79 | 0.4.80 | 1.81 | MIT OR Apache-2.0 |
| wasm-bindgen-macro | 0.2.129 | 0.2.130 | 1.81 | MIT OR Apache-2.0 |
| wasm-bindgen-macro-support | 0.2.129 | 0.2.130 | 1.81 | MIT OR Apache-2.0 |
| wasm-bindgen-shared | 0.2.129 | 0.2.130 | 1.81 | MIT OR Apache-2.0 |
| web-sys | 0.3.106 | 0.3.107 | 1.81 | MIT OR Apache-2.0 |
| zcheapstr | 1.1.0 | 1.2.0 | 1.87 | MIT |

Independent SHA-256 checks of all 21 old/new cached published `.crate` archives matched their corresponding lockfile checksums. Their published manifests introduce no new license family and no declared Rust minimum above the workspace's 1.95 requirement. `utf8_iter 1.0.4` had no declared minimum; its reviewed replacement and the new `utf_types` both declare 1.83. The Wasm support crates move as a coordinated family; this static review and the desktop checks do not prove WebAssembly runtime behavior. The actual local compiler is Rust 1.99.1, not a direct test of the 1.95 minimum.

`cargo audit --json` examined the corrected 434-package lockfile using RustSec database commit `2295466c4e9df26647fd67d8fa1c705ac2fc7f81`, updated 2026-10-30. No matching vulnerability advisory was found. The three existing Linux D-Bus unmaintained notices remain: zbus_names 4.3.5 (RUSTSEC-2026-0371), zvariant 5.15.1 (RUSTSEC-2026-0370), and zvariant_derive 5.15.1 (RUSTSEC-2026-0372). These notices and the previously documented upstream XML-complexity follow-up are not resolved by this update.

After correction, `cargo clippy -p studio-legacy --all-targets --target x86_64-pc-windows-gnu --locked -- -D warnings` passed. Final workspace checks are recorded separately in VALIDATION.md. This review did not execute Windows binaries, inspect CI, test a Windows/LTSC device, or perform a complete binary distribution-notice audit.

## Workspace retirement and dependency review — 2026-10-28

Reviewed the complete pre-existing dependency delta against `467998c`. Twelve compatible patch updates are retained; no package is added or removed relative to that committed baseline, and all 433 locked package entries remain from their existing sources. Independently hashing the 24 old/new cached published `.crate` archives matched their corresponding lockfile checksums.

| Package | Previous | Reviewed | Declared Rust minimum | License |
| --- | --- | --- | --- | --- |
| accesskit | 0.25.1 | 0.25.2 | 1.87 | MIT OR Apache-2.0 |
| accesskit_atspi_common | 0.21.0 | 0.21.1 | 1.87 | MIT OR Apache-2.0 |
| accesskit_consumer | 0.39.1 | 0.39.2 | 1.87 | MIT OR Apache-2.0 |
| accesskit_ios | 0.2.1 | 0.2.2 | 1.87 | MIT OR Apache-2.0 |
| accesskit_macos | 0.27.1 | 0.27.2 | 1.87 | MIT OR Apache-2.0 |
| accesskit_unix | 0.24.0 | 0.24.1 | 1.87 | MIT OR Apache-2.0 |
| accesskit_windows | 0.35.1 | 0.35.2 | 1.87 | MIT OR Apache-2.0 |
| accesskit_winit | 0.34.1 | 0.34.2 | 1.87 | Apache-2.0 |
| ordered-float | 5.5.0 | 5.5.1 | 1.90 | MIT |
| rustix | 1.1.5 | 1.1.6 | 1.65 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| unicode-width | 0.2.2 | 0.2.3 | 1.66 | MIT OR Apache-2.0 |
| wl-clipboard-rs | 0.9.4 | 0.9.5 | not declared | MIT OR Apache-2.0 |

The dirty `windows-collections 0.100.0` update was incompatible with the existing [`windows 0.62.2` projection](https://docs.rs/crate/windows/0.62.2), which requires collections 0.3.2 and core 0.62.2. Reproduced six E0277 errors in `winget_bootstrap.rs` on `x86_64-pc-windows-gnu`: its `Uri` and `IIterable<Uri>` came from different `windows-core` families. The direct dependency is now pinned to `=0.3.2`, with a comment requiring coordinated future upgrades. Cargo removed the five unused 0.100.0 packages while preserving every compatible patch update above. This corrects the reviewed update; a larger Windows API migration is outside this checkpoint.

The current published manifests declare no Rust minimum above the workspace's 1.95 requirement; the compiler used for validation is Rust 1.99.1, so this is not a direct Rust 1.95 build. License identifiers remain permissive; a complete binary distribution-notice audit was not performed.

`cargo audit --json` fetched RustSec database commit `40f5be6fb12ed93fc591bf4ab23b7d793b3f2239` (updated 2026-10-28), reporting zero matching vulnerability advisories. A second audit after the correction examined 433 packages and again found none, with the same three pre-existing unmaintained notices: `zbus_names` (RUSTSEC-2026-0371), `zvariant` (RUSTSEC-2026-0370), and `zvariant_derive` (RUSTSEC-2026-0372). They enter through the Linux accessibility/D-Bus stack; migrating that stack to zbus 6 is an upstream integration follow-up, not part of these patch updates.

Validation after the correction:

- `cargo clippy -p studio-legacy --all-targets --target x86_64-pc-windows-gnu --locked -- -D warnings`: passed.
- `cargo check --workspace --all-targets --target x86_64-pc-windows-gnu --locked`: passed with 26 pre-existing platform-specific unused import/dead-code/mutability warnings.
- Full workspace Windows strict Clippy: failed on those warnings, including the existing Topgrade directory builder; do not present the workspace as warning-free on Windows.
- `git diff --check`: passed. Native workspace checks and any subsequent warning repairs are recorded separately in VALIDATION.md.

No Windows program was linked or run, no LTSC device was tested, and no CI run was inspected for this dependency review. The older review sections below are historical evidence for their stated snapshots.

## Current compatible-version and security check — 2026-10-26

`cargo outdated --workspace --depth 3 --aggressive` reported only Windows-rs support crates whose newest versions move to the 0.100.0 family or are removed; no compatible update was offered. The app uses the coordinated `windows 0.62.2` family, while current `eframe 0.37.0` and `jsonc-parser 0.34.0` are already the latest published versions at this check. No dependency or lockfile changes are needed for the current UI and editor-target work.

`cargo audit` scanned 433 locked packages: no vulnerability advisory matched; three Linux D-Bus/variant crates were flagged as unmaintained. The previously identified `roxmltree` high-attribute-count complexity report remains an upstream release follow-up and is not resolved by this audit. This review is not a Windows device or runtime acceptance result.

## Unreleased legacy-engine integration

Relative to `3e0741b`, the lockfile adds the internal `studio-legacy 0.10.0` crate and only three external packages: `chrono 0.4.45`, `iana-time-zone 0.1.65` and `iana-time-zone-haiku 0.1.2`. Existing external versions, sources, checksums and dependency arrays are unchanged. These additions support the retained deployment engine's timestamps; they are not a general dependency upgrade.

All three declare MIT OR Apache-2.0. Chrono and iana-time-zone require Rust 1.62, below the workspace's 1.95 minimum. The Haiku helper has no declared MSRV and is selected only on Haiku, not macOS or Windows. Three cached published archives match Cargo.lock and the cached sparse index; all 97 extracted files match archive bytes. Cached records are not yanked; no online current-status or RustSec claim is made.

| Added package | Archive and lock SHA-256 |
| --- | --- |
| chrono 0.4.45 | `1aa79e62e7697b8e29b513a68abacf485adcd1fe8284a4316c5ae868e6633327` |
| iana-time-zone 0.1.65 | `e31bc9ad994ba00e440a8aa5c9ef0ec67d5cb5e5cb0cc7f8b744a35b389cc470` |
| iana-time-zone-haiku 0.1.2 | `f31827a206f56af32e590ba56d5d2d085f558508192593743f16b2306495269f` |

The checksum-verified `windows 0.62.2` manifest requires collections 0.3.2, core 0.62.2, future 0.3.2 and numerics 0.3.1. The merged crate uses this compatible family, all MIT OR Apache-2.0 with Rust 1.82 minimum. These five cached archives and extracted sources match. The incompatible uncommitted collections 0.100.0 experiment in the old checkout remains preserved outside the new build. Compilation and runtime evidence are recorded separately in VALIDATION.md.

## Unreleased GUI checkpoint: cc 1.9.0 → 1.10.0

The existing external lockfile update was reviewed separately from the GUI changes. Only the cc package version and checksum change; other external package entries remain identical. The official [crates.io record](https://crates.io/api/v1/crates/cc/1.10.0) was not yanked at review time. Its checksum `86e055fa6e3e40ad6187ba25b6855eeb635d5271f8e0f0e1e9a5826b8d75bea0` matches both Cargo.lock and the cached published archive. All 39 archive files match the extracted source cache, and the [upstream tag](https://github.com/rust-lang/cc-rs/tree/cc-v1.10.0) resolves to `8d06fcb73f6938c35c9bc59edbdcb9fb56f04d71`, matching the archive VCS metadata.

The normalized manifests differ only in version: dependencies, features, MIT OR Apache-2.0 license and Rust 1.65 minimum remain unchanged. The inbound android-activity, blake3 and wayland-backend constraints permit 1.10.0. The upstream changelog covers libtool archive support, parallel child-status debug output, target metadata and codegen-flag caching. No static compatibility blocker was found; local native and Windows-target checks remain separate validation evidence. This review does not claim a RustSec audit or Windows device acceptance.

## Previous 0.9 dependency checkpoint

Reviewed the existing `Cargo.lock` delta against `636a5e3d9b3a1e93622ab3b19f8014f5672efcb4`. Reviewed lockfile SHA-256: `bf1c1868f77dc3ffc78c29a68c2b298bd96d8634228a50e58c5aa2f22c046be1`.

**Result:** no checksum, declared dependency-constraint, declared MSRV, or license-identifier-change blocker found in this delta. This is a static dependency review; repository checks and Windows LTSC runtime acceptance remain separate evidence.

The 0.9 feature commit additionally changes four local workspace package versions from 0.8.0 to 0.9.0. The hash above identifies the reviewed external-update snapshot before that mechanical version bump; the 15 external version/checksum pairs below remain the reviewed delta.

## Exact scope

- **15 package entries updated**, all from crates.io; only `version` and `checksum` fields change.
- Lockfile format remains 4; package count remains **428**; no additions, removals, source changes or dependency-array changes.
- Parsed both TOML snapshots and checked all **1,306** current dependency references resolve to one package entry.
- Checked **31** declared inbound dependency requirements involving the changed packages against the selected versions; all satisfy their existing caret/tilde ranges.
- Compared every old/new checksum with its live [crates.io sparse-index record](https://doc.rust-lang.org/cargo/reference/registry-index.html), and independently hashed all **30** cached published `.crate` archives; all matched. All 15 selected versions were not yanked at review time.
- License identifiers below were read from the checksum-verified old/new published archive manifests; every identifier is unchanged.

| Package / primary registry record | Old | New | New declared Rust minimum | Unchanged license |
| --- | --- | --- | --- | --- |
| [clipboard-win](https://index.crates.io/cl/ip/clipboard-win) | 5.4.1 | 5.4.3 | not declared | BSL-1.0 |
| [icu_collections](https://index.crates.io/ic/u_/icu_collections) | 2.3.0 | 2.4.0 | 1.88 | Unicode-3.0 |
| [icu_locale_core](https://index.crates.io/ic/u_/icu_locale_core) | 2.3.0 | 2.4.0 | 1.88 | Unicode-3.0 |
| [icu_normalizer](https://index.crates.io/ic/u_/icu_normalizer) | 2.3.0 | 2.4.0 | 1.88 | Unicode-3.0 |
| [icu_normalizer_data](https://index.crates.io/ic/u_/icu_normalizer_data) | 2.3.0 | 2.4.0 | 1.88 | Unicode-3.0 |
| [icu_properties](https://index.crates.io/ic/u_/icu_properties) | 2.3.0 | 2.4.0 | 1.88 | Unicode-3.0 |
| [icu_properties_data](https://index.crates.io/ic/u_/icu_properties_data) | 2.3.0 | 2.4.0 | 1.88 | Unicode-3.0 |
| [icu_provider](https://index.crates.io/ic/u_/icu_provider) | 2.3.1 | 2.4.0 | 1.88 | Unicode-3.0 |
| [memmap2](https://index.crates.io/me/mm/memmap2) | 0.9.11 | 0.9.12 | 1.65 | MIT OR Apache-2.0 |
| [tinystr](https://index.crates.io/ti/ny/tinystr) | 0.8.4 | 0.8.5 | 1.82 | Unicode-3.0 |
| [tokio](https://index.crates.io/to/ki/tokio) | 1.54.0 | 1.54.1 | 1.85 | MIT |
| [webbrowser](https://index.crates.io/we/bb/webbrowser) | 1.2.4 | 1.2.5 | 1.85 | MIT OR Apache-2.0 |
| [yoke](https://index.crates.io/yo/ke/yoke) | 0.8.3 | 0.8.4 | 1.82 | Unicode-3.0 |
| [zerovec](https://index.crates.io/ze/ro/zerovec) | 0.11.8 | 0.11.9 | 1.83 | Unicode-3.0 |
| [zerovec-derive](https://index.crates.io/ze/ro/zerovec-derive) | 0.11.6 | 0.11.7 | 1.83 | Unicode-3.0 |

## Compatibility and material changes

The workspace declares Rust **1.95**, edition 2024, resolver 2, with `stable` in `rust-toolchain.toml`. Every declared minimum above is within that requirement. Most declared minima are unchanged; `zerovec-derive` now explicitly declares 1.83 where the old index record had no declaration. `clipboard-win` still has no declared MSRV, so its compiler compatibility needs compilation evidence.

The changed crates are transitive GUI dependencies: `arboard 3.7.0 → clipboard-win ^5.3.1` on Windows; `egui-winit 0.37.0 → webbrowser ^1.2.4 → url → idna → idna_adapter 1.2.2 → icu_normalizer/icu_properties ^2.2`; and font/system/window libraries consuming `memmap2 ^0.9`. Application manifests need no change.

- **ICU4X family:** all seven selected ICU entries move together to 2.4.0. Component/data requirements such as `~2.4.0`, and provider/locale requirements `^2.4.0`, match. The utility minimums are also present: `tinystr 0.8.5`, `yoke 0.8.4`, `zerovec 0.11.9`, `zerovec-derive 0.11.7`; `yoke-derive 0.8.4` was already selected. The [release-source workspace manifest](https://github.com/unicode-org/icu4x/blob/031d9e89f6a3fd26a14491e090729d46094328fd/Cargo.toml) confirms the family constraints and Rust 1.88 requirement.
- **ICU behavior/security:** the [pinned 2.4 changelog](https://github.com/unicode-org/icu4x/blob/031d9e89f6a3fd26a14491e090729d46094328fd/CHANGELOG.md) records Unicode 18 / CLDR 49 data updates, the normalizer UTF-8 soundness fix, `zerovec` panic-safety/soundness improvements, and a `zerovec-derive` layout soundness improvement. Updated normalization/property data can change handling of newly assigned Unicode characters through URL/IDNA processing. [GHSA-3jjw-63vg-v3hh](https://github.com/unicode-org/icu4x/security/advisories/GHSA-3jjw-63vg-v3hh) describes invalid UTF-8 and undefined behavior in `normalize_utf8` / `split_normalized_utf8`; its displayed affected/patched ranges overlap at review time, so those fields do not establish a reliable vulnerability-range verdict. No application exploitability claim is made. Upstream still qualifies `zerovec::cast_box` soundness pending stable pointer-metadata APIs.
- **Clipboard:** the [published 5.4.3 source revision](https://github.com/DoumanAsh/clipboard-win/tree/ed27008ae5d1f10cfbb266de039f0fb35fd05a56) replaces HTML-fragment pointer copying with checked slicing and adds a malformed-HTML regression. It also changes retry sleeps from `Sleep(0)` to `Sleep(5)`, which can add delay under clipboard contention. No public API removal or new required dependency was found in the 5.4.1-to-5.4.3 source diff. The optional monitoring dependency is not added to this lock graph.
- **Memory mapping:** [memmap2 0.9.12](https://github.com/RazrFalcon/memmap2-rs/blob/v0.9.12/CHANGELOG.md) adds optional safe-mapping/no-probe APIs and caches Windows allocation granularity. These APIs require no application changes; existing mapping safety requirements still apply.
- **Browser opening on Windows:** [webbrowser 1.2.5](https://github.com/amodm/webbrowser-rs/releases/tag/v1.2.5) changes the `shlwapi` declaration to `raw-dylib` to fix GNU/LLD linking. [Upstream PR #123](https://github.com/amodm/webbrowser-rs/pull/123) documents x64 GNU and i686 GNU/MSVC link smoke tests, including Rust 1.85; those tests do not exercise browser launching. Local Windows build/runtime proof remains necessary.
- **Tokio:** [1.54.1](https://github.com/tokio-rs/tokio/releases/tag/tokio-1.54.1) reverts timer ownership work; [upstream PR #8692](https://github.com/tokio-rs/tokio/pull/8692) describes a data race/use-after-free. In this graph its only incoming dependency is `wasm-bindgen-futures` under `cfg(all(target_os = "emscripten", wasm_bindgen_unstable_tokio))`; this lock entry does not demonstrate exposure on native Windows/macOS/Linux builds.

## Licenses and remaining evidence

No new license family or identifier change was introduced by these 15 updates. MIT / Apache-2.0, [Boost Software License 1.0](https://www.boost.org/LICENSE_1_0.txt), and [the pinned ICU4X Unicode license](https://github.com/unicode-org/icu4x/blob/031d9e89f6a3fd26a14491e090729d46094328fd/LICENSE) retain their redistribution terms. Unicode copyright/permission notices must accompany copies or associated documentation; package metadata alone does not prove a distributed executable bundle includes those notices. A complete distribution-notice audit was not performed.

No Cargo commands, upgrades, builds, tests, RustSec audit, CI queries, or device checks were run for this review. The security findings above are targeted upstream evidence, not a claim that the full graph is free of advisories. Native checks and actual clipboard/browser behavior on **Windows 10 Enterprise LTSC 2021** and **Windows 11 (IoT) Enterprise LTSC 2024** must be reported separately.

## Exact SHA-256 changes

| Package | Old archive / lock checksum | New archive / lock checksum |
| --- | --- | --- |
| clipboard-win | `bde03770d3df201d4fb868f2c9c59e66a3e4e2bd06692a0fe701e7103c7e84d4` | `052af7878466feccda12ba7a6a67d019d712f296809559b47c12c8cc196d3a9a` |
| icu_collections | `fa68d21081c4a05d5a901a1c62add574c77048b6a1c67be3b50ce0b60d4ca513` | `fe090a42a0d3140fed8c63263ed1fecc02422cb2f4bfe55cd8de2de1b8e105dd` |
| icu_locale_core | `d56e28588da92eee5c3201a6eff33fabdd49b62269c8938d4ff050ce4d900deb` | `25d29b57c17c6528d0dc4c2141cfa2d08200465acb537a3909c8b9b8a4a984c0` |
| icu_normalizer | `12f9cf5f235641ed274641dd81c3f28d870e276763d0797aeeab72317b1c646f` | `404a3b8c486f205c87fe7f23ab6894d84a4ad76d6e4671ae1981ba7def87c599` |
| icu_normalizer_data | `1563da1ed3e0b3bf3d74c9b85917ac9c56464d2f57242270c09c9e752f8021a0` | `20f0088bfb7441f7a37fa0c9e20cb1e9cba1cac6b9ce8cd2a6dc44294f8e8cbc` |
| icu_properties | `7e7ca276ad3145661a65914e6daf131ca5120cd3dcee8f8f3214b8875184a148` | `2ea83d991c56158f7cf898bf86436c1dfe7c07ee82fe88951f2695c9f37a379f` |
| icu_properties_data | `e590f038c1464a96894fd6d10127e90a8be4509f56ff7ecef851b15cee0b7caa` | `b98f29af82d02b163fe50161388350e743bb3107cc8e8c2f7314cb505faddfcf` |
| icu_provider | `d27bbb9d3abbefac45d55f647c9de1d44aafcd1186eb91879afef17c396c3e73` | `413099315636c588ee8f011190b329b971a09a8fae9d157e01dcae8379468514` |
| memmap2 | `d1219ed1b7f229ee7104d281dd01d6802fe28bb6e95d292942c4daacdeb798c0` | `6b0ca1155a9ecc25aa12e8313a119f32edf0255fea2f44839c4ac0a12b2ac157` |
| tinystr | `b1e27c91459209c2986af3dcf603a5a74a4368754ce37414f59acc971167f643` | `abc840c238154d58f9e86b2792261edfde565a17696e1cda1a449de27979801a` |
| tokio | `a0c168a474b95bfecfe908a65d4bdffc523144b649a74c09c286afb35d53e0bc` | `b52fd3255258d26fac7264ab5a76b53c13367136dc011d3d6ad4f8ed070396a3` |
| webbrowser | `62c35be770821a214dbc362fc26908c853e776c0004294d0b10b8a6bad582f94` | `d2c8ddfbbf66e501434657400268cefe51b95140a0266236415cda7627732df7` |
| yoke | `709fe23a0424b6a435d82152b1bd3fdfb0833487d5fa90d05d42762a9891fef5` | `417ef3fcf6c300b7dbd25c85531a746931c02ad536f3cf4786ed1e88223340b3` |
| zerovec | `bb0464e17806c1d976d5cba29399c7f08e516e279e2ba493f63123b5fca67dd8` | `f58a2799cfe9a1670c7ea8edbd52ef8edb93cb37a7f83bf31ba9e35d9dae6a40` |
| zerovec-derive | `34df6fc39dbd26ddc9c10e6a2984476e13acce22e64e4487636ef494369225da` | `c3743ddbf47738d929cd1ae2e199d40d0162979dfe31aef8ae646d2bf00bf97a` |

## Historical freshness and RustSec check — 2026-10-24

- `cargo outdated --workspace --depth 3` found no newer compatible dependency set; only the Windows binding family has newer incompatible-generation packages: `windows-collections 0.3.2 → 0.100.0`, `windows-core 0.62.2 → 0.100.0`, `windows-implement 0.60.2` and `windows-interface 0.59.3` are removed, plus newer `windows-link`, `windows-result`, and `windows-strings` 0.100.0. The workspace directly pairs `windows 0.62.2` with `windows-collections 0.3.2`. The [upstream 0.100 migration notes](https://github.com/microsoft/windows-rs/issues/4867) describe breaking collection iterator changes, so these packages must move together and need Windows-target validation; no Windows target is installed on this host. They remain at the latest compatible family.
- `cargo audit` scanned 432 locked dependencies and reported no vulnerability advisories. It did report three informational unmaintained notices: [`zbus_names`](https://rustsec.org/advisories/RUSTSEC-2026-0371.html) 4.3.5, [`zvariant`](https://rustsec.org/advisories/RUSTSEC-2026-0370.html) 5.15.1, and [`zvariant_derive`](https://rustsec.org/advisories/RUSTSEC-2026-0372.html) 5.15.1. `cargo tree --target all` traces them through `eframe → egui-winit → accesskit_winit → accesskit_unix` and the Linux accessibility / D-Bus stack. Upstream has merged these standalone crates into `zbus` v6.0; its [official release page](https://github.com/z-galaxy/zbus/releases) lists v6.1.0 (2026-10-23) as latest. The locked AccessKit chain still depends on zbus 5.x and the standalone crates (`cargo tree --target all`); `cargo outdated` found no semver-compatible upgrade. Do not force a major override: wait for a coordinated AccessKit/egui migration, preserve Linux accessibility, then build and run the Linux GUI before adopting it.
- At the previous Git-profile checkpoint, no new external packages or Cargo.toml requirements had been added. `cargo update` moved only `smallvec 1.16.3 → 1.16.4` (MIT OR Apache-2.0; no declared MSRV) and `uuid 1.28.0 → 1.29.0` (Apache-2.0 OR MIT; Rust 1.89), both within existing compatible constraints. Their Cargo.lock checksums are `d0d5d4b1981b8bbbb7aed204cae5726d683d5d6ac7e74ec54122ea28f315e7c8` and `4a0e2d11620c50695fe12bbbfde96b3477fe606e7284c22d505c12ce51214514`. `cargo outdated --workspace --depth 0` reports all direct dependencies current; depth 3 reports only the incompatible Windows API generation noted above. Earlier 0.9, legacy-engine and cc reviews remain historical snapshots; this dependency freshness and RustSec snapshot was collected on 2026-10-24.

## JSONC settings parser — 2026-10-26

- Added `jsonc-parser 0.34.0` to `studio-deployment`. Docs.rs lists it as the newest published release on the review date (released 2026-09-27); the crate metadata identifies MIT and does not declare a Rust version. The project MSRV remains 1.95. See [release metadata](https://docs.rs/crate/jsonc-parser/0.34.0) and the pinned [upstream MIT license](https://github.com/dprint/jsonc-parser/blob/0.34.0/LICENSE).
- Default features are disabled; only the explicit `cst` and `serde` features are enabled. CST edits preserve JSONC comments and unrelated settings; Serde converts the validated allowlist to profile values. The parser's sole locked dependency is the already-used `serde` crate. The same lock refresh advances transitive `roxmltree` 0.21.1 → 0.21.2 (MIT OR Apache-2.0, Rust 1.60), pulled through `fontique → egui_system_font_provider → eframe`; its 0.21.2 changelog fixes stack overflow on deeply nested XML. This is the smallest reviewed parser path for safe JSONC merge; a handwritten comment-preserving parser would add more maintenance risk than the single dependency.
- The separate upstream [roxmltree issue #153](https://github.com/RazrFalcon/roxmltree/issues/153) describes quadratic checks for many attributes and namespace declarations. The maintainer merged [PR #157](https://github.com/RazrFalcon/roxmltree/pull/157) on 2026-10-24 to reject elements above `u16::MAX` attributes, but the locked 0.21.2 source still contains the quadratic `resolve_attributes` scan and does not include that cap. The [GitHub advisory](https://github.com/advisories/ghsa-5fxw-8768-v85x) has unknown affected/patched versions; RustSec `cargo audit` does not report it. No crates.io release containing the merged cap was available in this check, so we did not pin unreleased Git source. Reassess the fontique input boundary when a release containing the fix is available.

## Current dependency and RustSec check — 2026-10-26

- `cargo outdated --workspace --depth 3 --aggressive` reports no newer compatible package set outside the Windows binding family. It shows `windows-collections 0.100.0` and `windows-core 0.100.0` as a newer incompatible API generation, plus related 0.100.0 packages; keep the paired `windows 0.62.2` / `windows-collections 0.3.2` family until a coordinated migration can be validated on Windows. A collection-only 0.100.0 experiment was removed before commit.
- `cargo audit` loaded the current advisory database and scanned 433 locked crate dependencies: no RustSec vulnerability advisories, three allowed unmaintained notices (`zbus_names 4.3.5`, `zvariant 5.15.1`, `zvariant_derive 5.15.1`). The prior review traced them to the Linux accessibility/D-Bus stack; no compatible update candidate appeared in the current freshness check. This audit result does not resolve the separate upstream roxmltree report above.
