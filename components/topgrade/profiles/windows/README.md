# Windows dotnetup updates

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
