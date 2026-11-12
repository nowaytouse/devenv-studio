#Requires -Version 7.0
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
if (-not $IsWindows) {
    throw 'The MSVC development shell requires Windows.'
}

if (-not ${env:ProgramFiles(x86)}) {
    throw 'The Visual Studio Installer path is unavailable. Run this script on 64-bit Windows.'
}
$installer = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
if (-not (Test-Path -LiteralPath $installer -PathType Leaf)) {
    throw 'Visual Studio Installer was not found. Install Visual Studio or Build Tools with the x64 C++ tools and Windows SDK.'
}

$installations = @(& $installer -latest -prerelease -products '*' -utf8 -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)
if ($LASTEXITCODE -ne 0 -or $installations.Count -ne 1 -or -not [IO.Path]::IsPathFullyQualified($installations[0])) {
    throw 'Could not identify a Visual Studio installation with the x64 C++ tools.'
}

$launch = Join-Path $installations[0] 'Common7/Tools/Launch-VsDevShell.ps1'
if (-not (Test-Path -LiteralPath $launch -PathType Leaf)) {
    throw 'The selected Visual Studio installation is missing Launch-VsDevShell.ps1. Repair it in Visual Studio Installer.'
}

$before = [Environment]::GetEnvironmentVariables('Process')
try {
    & $launch -Arch amd64 -HostArch amd64 -SkipAutomaticLocation
    if (-not $?) {
        throw 'Visual Studio Developer PowerShell initialization failed.'
    }

    $toolchain = [IO.Path]::GetFullPath((Join-Path $installations[0] 'VC/Tools/MSVC')) + [IO.Path]::DirectorySeparatorChar
    foreach ($tool in @('cl.exe', 'link.exe')) {
        $resolved = (Get-Command $tool -CommandType Application | Select-Object -First 1).Source
        if (-not $resolved.StartsWith($toolchain, [StringComparison]::OrdinalIgnoreCase) -or
            -not $resolved.EndsWith("\bin\Hostx64\x64\$tool", [StringComparison]::OrdinalIgnoreCase)) {
            throw "The current shell resolves $tool outside the selected x64 toolchain. Open a fresh PowerShell and retry."
        }
    }

    $sdkFiles = @{ LIB = 'kernel32.lib'; INCLUDE = 'windows.h' }
    foreach ($name in @('LIB', 'LIBPATH', 'INCLUDE')) {
        $value = [Environment]::GetEnvironmentVariable($name, 'Process')
        $entries = @($value -split ';' | Where-Object { $_ })
        if ($entries.Count -eq 0) {
            if ($name -ne 'LIBPATH') {
                throw "The selected Visual Studio installation did not initialize $name. Check its Windows SDK."
            }
            continue
        }
        $normalized = foreach ($entry in $entries) {
            if (-not [IO.Path]::IsPathFullyQualified($entry)) {
                throw "The selected Visual Studio installation returned a relative $name path. Check its toolchain and Windows SDK."
            }
            [IO.Path]::GetFullPath($entry)
        }
        $sdkFile = $sdkFiles[$name]
        if ($sdkFile -and -not ($normalized | Where-Object { Test-Path -LiteralPath (Join-Path $_ $sdkFile) -PathType Leaf })) {
            throw "The Windows SDK file $sdkFile was not found on $name. Check the SDK installation."
        }
        [Environment]::SetEnvironmentVariable($name, ($normalized -join ';'), 'Process')
    }
}
catch {
    $after = [Environment]::GetEnvironmentVariables('Process')
    foreach ($key in $after.Keys) {
        if (-not $before.Contains($key)) {
            [Environment]::SetEnvironmentVariable($key, $null, 'Process')
        }
    }
    foreach ($key in $before.Keys) {
        if (-not $after.Contains($key) -or $after[$key] -cne $before[$key]) {
            [Environment]::SetEnvironmentVariable($key, $before[$key], 'Process')
        }
    }
    throw
}

Write-Host 'Windows x64 MSVC development environment is ready in this PowerShell.'
