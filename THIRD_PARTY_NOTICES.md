# Third-party notices

LTSC Studio is an independent MIT-licensed Rust application. Selected registry behavior is mapped from the following MIT projects, with the original copyright and full license notices preserved in this distribution.

| Project | Reviewed revision | License copy |
| --- | --- | --- |
| [ChrisTitusTech/winutil](https://github.com/ChrisTitusTech/winutil) | `7514faf0d0f4bff9b6c5d9213426ab52520cfac4` | [WinUtil MIT](third-party/winutil-LICENSE) |
| [farag2/Sophia-Script-for-Windows](https://github.com/farag2/Sophia-Script-for-Windows) | `eeef532fc5f37129fe77ee58342f112f3bb71d39` | [Sophia MIT](third-party/sophia-script-LICENSE) |
| [Raphire/Win11Debloat](https://github.com/Raphire/Win11Debloat) | `a2d63c7c1b9c7cb6554d4fce3aa7f377567f7f9e` | [Win11Debloat MIT](third-party/win11debloat-LICENSE) |

The application does not download or execute upstream scripts. Per-action evidence and intentionally narrower behavior are documented in [INTEGRATIONS.md](docs/INTEGRATIONS.md). All selected original mapping and notice information remains attributed to its respective author.

[LTSC_Tools_Backup](https://github.com/nowaytouse/LTSC_Tools_Backup) is a design predecessor. No code or personal inventory files from that repository have been copied. Its previous Windows-only GUI constraints do not apply to this newly requested cross-platform project.

Rust dependencies retain their own licenses. The dependency versions are fixed in Cargo.lock; package metadata includes each crate's license information. eframe/egui is available under MIT or Apache-2.0. The application uses system fonts, so it does not redistribute macOS or Windows font files.
