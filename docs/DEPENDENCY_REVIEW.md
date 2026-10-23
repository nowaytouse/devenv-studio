# Dependency review — 2026-10-23

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
