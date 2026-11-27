$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '../bin/Dotnetup-NetworkRecovery.ps1')
$WarningPreference = 'SilentlyContinue'
$temp = Join-Path ([IO.Path]::GetTempPath()) ([guid]::NewGuid().ToString())
$null = New-Item -ItemType Directory -Path $temp
$manager = Join-Path $temp 'manager.ps1'
$manifest = Join-Path $temp 'manifest.json'
$countFile = Join-Path $temp 'calls'

function Test-DotnetupChannelReachable([string]$Url) {
    $script:probes++
    if ($script:changeManifest) { [IO.File]::AppendAllText($manifest, ' ') }
    if ($script:changeManager) { [IO.File]::AppendAllText($manager, "`n# changed") }
    return $script:reachable
}

function Test-Case($Mode, $Diagnostic, $Success, $Calls, $Probes, $Source = 'Explicit', $ExtraSpec = $false) {
    $script:probes = 0
    [IO.File]::WriteAllText($countFile, '')
    $spec = @{component='SDK';versionOrChannel='daily';installSource=$Source}
    $specs = @($spec)
    if ($ExtraSpec) { $specs += $spec }
    @{dotnetRoots=@(@{path=$temp;architecture='x64';installSpecs=$specs})} |
        ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $manifest
    $options = if ($Mode -eq 'Preview') { @{Preview=$true} } else { @{InstallPath=$temp;ManifestPath=$manifest} }
    $failure = $null
    $caseOutput = $Diagnostic
    try { $result = @(Invoke-DotnetupUpdate -Manager $manager @options) } catch { $failure = $_.Exception.Message }
    $actual = @([IO.File]::ReadAllLines($countFile))
    $expected = if ($Mode -eq 'Preview') { 'self update --channel preview --no-progress' }
        else { "sdk update --install-path $temp --manifest-path $manifest --no-progress" }
    if (($null -eq $failure) -ne $Success -or $actual.Count -ne $Calls -or $script:probes -ne $Probes -or
        @($actual | Where-Object { $_ -ne $expected }).Count -gt 0) {
        throw "Unexpected $Mode result (calls=$($actual.Count), probes=$script:probes, error=$failure)"
    }
    if ($failure -and -not $failure.Contains($Diagnostic)) { throw 'Lost original failure' }
    if ($Success -and $result -notcontains 'Verified update') { throw 'Lost native output' }
}

try {
    @'
[IO.File]::AppendAllText($countFile, ($args -join ' ') + [Environment]::NewLine)
if (($caseOutput -and [IO.File]::ReadAllLines($countFile).Count -eq 1) -or $retryFails) {
    Write-Output $caseOutput
    exit 1
}
Write-Output 'Verified update'
exit 0
'@ | Set-Content -LiteralPath $manager
    $preview = "Error: Failed to resolve preview channel via https://aka.ms/dotnet/dotnetup/preview/dotnetup-win-x64.exe: The SSL`nconnection could not be established, see inner exception."
    $daily = "Failed to update .NET SDK daily.`nError: Failed to resolve daily channel via https://aka.ms/dotnet/12.0/daily/dotnet-sdk-win-x64.zip: The SSL connection could not be established, see inner exception."
    $script:reachable = $true
    Test-Case Preview '' $true 1 0
    Test-Case Preview $preview $true 2 1
    Test-Case Daily $daily $true 2 1
    Test-Case Daily $daily $false 1 1 'GlobalJson'
    Test-Case Daily $daily $false 1 1 'Explicit' $true
    Test-Case Preview "Installing...`n$preview" $false 1 0
    Test-Case Preview ($preview.Replace('aka.ms','unrelated.example')) $false 1 0
    $script:reachable = $false
    Test-Case Preview $preview $false 1 1
    $script:reachable = $true
    $script:retryFails = $true
    Test-Case Preview $preview $false 2 1
    $script:retryFails = $false
    $script:changeManifest = $true
    Test-Case Daily $daily $false 1 1
    $script:changeManifest = $false
    $script:changeManager = $true
    Test-Case Preview $preview $false 1 1
    Write-Output 'Passed 11 dotnetup recovery cases.'
} finally { Remove-Item -LiteralPath $temp -Force -Recurse }
