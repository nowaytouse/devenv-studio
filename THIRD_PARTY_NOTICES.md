# Third-party notices

DevEnv Studio is an independent MIT-licensed Rust application. Selected registry behavior is mapped from the following MIT projects, with the original copyright and full license notices preserved in this distribution.

| Project | Reviewed revision | License copy |
| --- | --- | --- |
| [ChrisTitusTech/winutil](https://github.com/ChrisTitusTech/winutil) | `7514faf0d0f4bff9b6c5d9213426ab52520cfac4` | [WinUtil MIT](third-party/winutil-LICENSE) |
| [farag2/Sophia-Script-for-Windows](https://github.com/farag2/Sophia-Script-for-Windows) | `eeef532fc5f37129fe77ee58342f112f3bb71d39` | [Sophia MIT](third-party/sophia-script-LICENSE) |
| [Raphire/Win11Debloat](https://github.com/Raphire/Win11Debloat) | `a2d63c7c1b9c7cb6554d4fce3aa7f377567f7f9e` | [Win11Debloat MIT](third-party/win11debloat-LICENSE) |

The registry integration does not download or execute WinUtil, Sophia Script or Win11Debloat scripts. Per-action evidence and intentionally narrower behavior are documented in [INTEGRATIONS.md](docs/INTEGRATIONS.md). The merged deployment engine can invoke reviewed package providers and installers when explicitly selected. All selected original mapping and notice information remains attributed to its respective author.

[LTSC_Tools_Backup](https://github.com/nowaytouse/LTSC_Tools_Backup/tree/a8523abbe08147b4d5de8746f26d99d056057ab0) is the owner's predecessor project. With the owner's explicit permission, seven Rust backend modules are adapted into the internal `studio-legacy` crate, and its visual layout informs the native GUI. Its reviewed main history is retained in DevEnv Studio; ongoing maintenance moves to this application. The predecessor has no standalone license declaration and is not represented as a third-party MIT dependency. Bundled fonts, personal inventory, skills and plugins are not imported or relicensed. Its previous Windows-only GUI constraints do not apply to this cross-platform project. See [LEGACY_MERGE.md](docs/LEGACY_MERGE.md) for provenance and execution boundaries.

## Development-environment integrations

The following projects supply documented native formats, command contracts or design references. DevEnv Studio invokes supported tools already present on the user's machine; it does not bundle their executables or copy their source code. These references do not imply that every upstream feature is integrated.

| Project | Reviewed revision | Upstream license | Integration |
| --- | --- | --- | --- |
| [Homebrew/brew](https://github.com/Homebrew/brew/tree/9811f64e2cccd62fd8a981c1fed5248b9705fea4) | `9811f64e2cccd62fd8a981c1fed5248b9705fea4` | BSD-2-Clause | Inventory, controlled Brewfile, Bundle, rolling and formula HEAD operations |
| [microsoft/winget-cli](https://github.com/microsoft/winget-cli/tree/bc1e9ff8b982798ab346b61416b76ae738038e57) | `bc1e9ff8b982798ab346b61416b76ae738038e57` | MIT | Trusted Windows deployment and native packages manifest |
| [rust-lang/rustup](https://github.com/rust-lang/rustup/tree/5c8ff3f907c37688d20d9e7038a2f826042cd763) | `5c8ff3f907c37688d20d9e7038a2f826042cd763` | MIT OR Apache-2.0 | Rust channel inventory, native deployment and toolchain manifest |
| [astral-sh/uv](https://github.com/astral-sh/uv/tree/e96aa6932bb860f3b950ddb46d1646d45e09315d) | `e96aa6932bb860f3b950ddb46d1646d45e09315d` | MIT OR Apache-2.0 | Cross-platform uv tool inventory and selected PyPI command-tool redeployment; executable not bundled |
| [pypa/pipx](https://github.com/pypa/pipx/tree/9a49f1957fad930860189421f2cd29d5a5604fd4) | `9a49f1957fad930860189421f2cd29d5a5604fd4` | MIT | Evaluated as an alternative; no parallel backend or automatic migration |
| [jdx/mise](https://github.com/jdx/mise/tree/d2e9ff8b10a98571740042aa2d2321df1b4b1817) | `d2e9ff8b10a98571740042aa2d2321df1b4b1817` | MIT | Rust toolchain manifest plus reviewed cross-platform core runtime inventory, locked deployment and readback; no arbitrary config/plugins/tasks |
| [twpayne/chezmoi](https://github.com/twpayne/chezmoi/tree/da3406f7aa2a114322334578639812684016d352) | `da3406f7aa2a114322334578639812684016d352` | MIT | Review-before-apply design reference only |

The rustup and uv revisions were checked against pinned repository metadata and licenses on 2026-10-23/24. See the rustup [Cargo.toml](https://github.com/rust-lang/rustup/blob/5c8ff3f907c37688d20d9e7038a2f826042cd763/Cargo.toml), [MIT](https://github.com/rust-lang/rustup/blob/5c8ff3f907c37688d20d9e7038a2f826042cd763/LICENSE-MIT), and [Apache-2.0](https://github.com/rust-lang/rustup/blob/5c8ff3f907c37688d20d9e7038a2f826042cd763/LICENSE-APACHE) files, plus uv [Cargo.toml](https://github.com/astral-sh/uv/blob/e96aa6932bb860f3b950ddb46d1646d45e09315d/Cargo.toml), [MIT](https://github.com/astral-sh/uv/blob/e96aa6932bb860f3b950ddb46d1646d45e09315d/LICENSE-MIT), and [Apache-2.0](https://github.com/astral-sh/uv/blob/e96aa6932bb860f3b950ddb46d1646d45e09315d/LICENSE-APACHE) files. pipx is an evaluated alternative, not redistributed; its [MIT license](https://github.com/pypa/pipx/blob/9a49f1957fad930860189421f2cd29d5a5604fd4/LICENSE) remains with upstream. Attribution and implementation boundaries are detailed in [ENVIRONMENT_SOURCES.md](docs/ENVIRONMENT_SOURCES.md) and [REDEPLOYMENT.md](docs/REDEPLOYMENT.md).

Rust dependencies retain their own licenses. The dependency versions are fixed in Cargo.lock; package metadata includes each crate's license information. eframe/egui is available under MIT or Apache-2.0. The application uses system fonts, so it does not redistribute macOS or Windows font files.
