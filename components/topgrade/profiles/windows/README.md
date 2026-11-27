# Windows .NET rolling updates

Use the existing daily SDK and preview manager scripts with the Studio build of Topgrade. Put `bin/Dotnetup-NetworkRecovery.ps1` beside the two scripts, in their existing directory. In each script load the helper before its update call:

```powershell
. (Join-Path $PSScriptRoot 'Dotnetup-NetworkRecovery.ps1')
```

In the daily script use `$update = Invoke-DotnetupUpdate -Manager $manager -InstallPath $install -ManifestPath $manifest` for the SDK update. In the preview script use `Invoke-DotnetupUpdate -Manager $manager -Preview` for self-update. Keep the existing signature, inventory, executable version and record checks.

For the normal Windows configuration search, copy `topgrade.d/dotnetup.toml` into `%APPDATA%\topgrade.d\`. When starting Topgrade with an explicit `--config`, merge the file's key into that configuration's existing `[misc]` table. The two entries must match the names in `[commands]` exactly. This gives each script sole control over automatic retries.

On a failure confined to resolving the official daily or preview URL, the helper checks the public HTTPS redirect chain with HEAD, normal certificate validation and the process's default proxy. Two probes are allowed. If connectivity returns and the manager executable is unchanged, the helper retries that same command once. A daily retry also requires an unchanged manifest with one root and one explicitly requested daily SDK. Other failures retain the original error. PowerShell and dotnetup can use different networking runtimes. The final result comes from dotnetup and the calling script's installation checks.

Run `pwsh -NoProfile -File tests/Dotnetup-NetworkRecovery.Tests.ps1` to exercise the recovery decisions with a temporary manifest and a simulated manager.
