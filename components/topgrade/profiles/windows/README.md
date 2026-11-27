# Windows maintenance profiles

## Microsoft daily SDK and preview manager

The Studio build of Topgrade can update the isolated x64 daily SDK and the dotnetup preview manager directly. Enable the steps in the existing Windows configuration:

~~~toml
[windows]
dotnet_daily = true
dotnetup_preview = true
~~~

A one-time run also works without these settings:

~~~powershell
topgrade --only dotnet_daily dotnetup_preview -y
~~~

To check the installed SDK and save its report without updating the SDK or querying its release feed:

~~~powershell
topgrade --only dotnet_daily --dotnet-daily-check-only
~~~

The steps expect the existing Microsoft manager at `%USERPROFILE%\.local\bin\dotnetup.exe`. The daily SDK uses `D:\DevTools\DotnetDaily\managed` and the adjacent `dotnetup-manifest.json` in its parent directory. Topgrade verifies the manager's Authenticode signature and publisher. The daily check compares the resolved SDK version to the manager's inventory and atomically writes `DotnetDaily-latest.json` under the existing desktop folder `设备维护与记录\软件更新日志`. Preview self-update verifies the updated manager and runs its version command.

When migrating from the supplied scripts, updating `topgrade.exe` is sufficient for the standard `pwsh.exe -NoProfile -File "..."` entries pointing to `%APPDATA%\topgrade\Update-DotnetDaily.ps1` and `Update-DotnetupPreview.ps1`. Topgrade recognizes these entries even when the scripts are absent and honors the existing custom-command name filter. After enabling the two native settings you can remove the old launcher entries and their matching `no_auto_retry_commands` entries. Customized commands with additional arguments continue to run as written.

Only an exact dotnetup TLS error resolving the Microsoft x64 daily or preview channel qualifies for recovery. Topgrade probes the public HTTPS redirect with HEAD at most twice. It retries the same update once if connectivity has returned, the manager is unchanged and, for the SDK, the unchanged manifest has one explicitly requested daily SDK in one root. The outer step does not automatically retry. A failed verification or second update is reported as a failure.

## Rolling applications and Python launcher

The Windows x64 profile can maintain seven additional steps. Add the selected names to the existing Windows section. Remove the corresponding old commands and Python post-command when enabling their native equivalents.

~~~toml
[windows]
profile_steps = [
    "windows_krita_next", "windows_power_toys_preview", "windows_scoop_rolling",
    "windows_nushell_plugins", "windows_ollama_models", "windows_python_preview",
    "windows_python_launcher",
]
~~~

| Step | Managed installation and checks |
| --- | --- |
| Krita Next | Official KDE CI x64 build, signed installer, build revision, closed application and preference backup. Retains the verified installer on the staging drive. |
| PowerToys preview | Official per-user x64 release, preview policy, release checksum, Microsoft signature and module preferences. |
| Scoop rolling | Existing, unheld versions-bucket Node nightly and Bun canary, package checksums, downgrade guard and runtime readback. |
| Nushell plugins | Registers the five plugins bundled with Scoop Nushell and reads the registry back. |
| Local Ollama models | Pulls only models already present on the loopback server. Rechecks inventory before each pull and releases a temporary server after use. Select this profile instead of the general Ollama updater to avoid duplicate pulls. |
| Python preview | Selects an Astral Windows x64 CPython prerelease from uv metadata and verifies the default runtime, standard library and launcher. |
| Python launcher | Checks the uv registration and any existing Scoop python-beta installation. Repairs only its owned version key after saving the prior registry state. |

Run one or more steps with `topgrade --only <step> -y`. Add `--windows-profile-check-only` to inspect version, inventory or registration and save a local report. The Ollama inventory check may start a temporary loopback server. Use `--dry-run` to preview the selected steps without accessing services or writing reports. Normal runs write status and errors to the desktop maintenance folders. The Scoop, uv and Nushell installations retain ownership of their files.
