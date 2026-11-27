# Dot-source this file; it does not change proxy or certificate settings.
function Test-DotnetupChannelReachable([string]$Url) {
    Add-Type -AssemblyName System.Net.Http
    $handler = [System.Net.Http.HttpClientHandler]::new()
    $handler.UseProxy = $true
    # The public channel probe never needs the user's credentials at a redirect target.
    $handler.UseDefaultCredentials = $false
    $handler.AllowAutoRedirect = $false
    $client = [System.Net.Http.HttpClient]::new($handler)
    try {
        for ($attempt = 0; $attempt -lt 2; $attempt++) {
            $uri = [uri]$Url
            $deadline = [Threading.CancellationTokenSource]::new([TimeSpan]::FromSeconds(15))
            try {
                for ($hop = 0; $hop -le 10; $hop++) {
                    $request = [System.Net.Http.HttpRequestMessage]::new([System.Net.Http.HttpMethod]::Head, $uri)
                    try {
                        $response = $client.SendAsync($request, [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead, $deadline.Token).GetAwaiter().GetResult()
                        try {
                            if ([int]$response.StatusCode -in 301,302,303,307,308) {
                                if (-not $response.Headers.Location) { throw 'Missing redirect location' }
                                $next = [uri]::new($uri, $response.Headers.Location)
                                if ($next.Scheme -ne 'https' -or $next.UserInfo) { throw 'Unexpected redirect location' }
                                $uri = $next
                                continue
                            }
                            if ($hop -gt 0 -and $response.IsSuccessStatusCode -and $uri.Host -ne 'aka.ms' -and
                                $response.Content.Headers.ContentType.MediaType -ne 'text/html') { return $true }
                            Write-Warning ("dotnetup channel probe at {0} returned HTTP {1} or unexpected content." -f $uri.Host, [int]$response.StatusCode)
                            break
                        } finally { $response.Dispose() }
                    } finally { $request.Dispose() }
                }
            } catch {
                $exception = $_.Exception
                while ($exception) {
                    $detail = $exception.Message -replace '(?i)(?:https?|socks5h?)://\S+', '[address]'
                    Write-Warning ("dotnetup channel probe at {0}: {1}: {2}" -f $uri.Host, $exception.GetType().Name, $detail)
                    $exception = $exception.InnerException
                }
            } finally { $deadline.Dispose() }
            if ($attempt -eq 0) { Start-Sleep -Seconds 2 }
        }
        return $false
    } finally { $client.Dispose() }
}

function Invoke-DotnetupUpdate {
    [CmdletBinding(DefaultParameterSetName='Daily')]
    param(
        [Parameter(Mandatory)][string]$Manager,
        [Parameter(Mandatory, ParameterSetName='Preview')][switch]$Preview,
        [Parameter(Mandatory, ParameterSetName='Daily')][string]$InstallPath,
        [Parameter(Mandatory, ParameterSetName='Daily')][string]$ManifestPath
    )

    if ($Preview) {
        $arguments = @('self','update','--channel','preview','--no-progress')
        $pattern = '\AError: Failed to resolve preview channel via (https://aka\.ms/dotnet/dotnetup/preview/dotnetup-win-x64\.exe): The SSL connection could not be established, see inner exception\.\z'
    } else {
        $arguments = @('sdk','update','--install-path',$InstallPath,'--manifest-path',$ManifestPath,'--no-progress')
        $pattern = '\AFailed to update \.NET SDK daily\. Error: Failed to resolve daily channel via (https://aka\.ms/dotnet/[0-9]+\.[0-9]+/daily/dotnet-sdk-win-x64\.zip): The SSL connection could not be established, see inner exception\.\z'
        $manifestText = [IO.File]::ReadAllText($ManifestPath)
        $roots = @(($manifestText | ConvertFrom-Json).dotnetRoots)
        $singleExplicitDaily = $roots.Count -eq 1 -and $roots[0].path -ieq $InstallPath -and
            $roots[0].architecture -eq 'x64' -and @($roots[0].installSpecs).Count -eq 1 -and
            $roots[0].installSpecs[0].component -eq 'SDK' -and $roots[0].installSpecs[0].versionOrChannel -eq 'daily' -and
            $roots[0].installSpecs[0].installSource -eq 'Explicit'
    }

    $firstFailure = $null
    $initialHash = (Get-FileHash -LiteralPath $Manager -Algorithm SHA256 -ErrorAction Stop).Hash
    while ($true) {
        $saved = $ErrorActionPreference
        try {
            $ErrorActionPreference = 'Continue'
            $global:LASTEXITCODE = $null
            $output = @(& $Manager @arguments 2>&1 | ForEach-Object { $_.ToString() })
            $code = $global:LASTEXITCODE
        } finally { $ErrorActionPreference = $saved }
        if ($null -ne $code -and $code -eq 0) { return $output }

        $message = ($output -join "`n").Trim()
        $failure = "dotnetup failed ($code): $message"
        if ($firstFailure) { throw "$firstFailure`nRetry: $failure" }
        if (($message -replace '\s+', ' ') -cnotmatch $pattern) { throw $failure }

        $url = $Matches[1]
        Write-Warning $failure
        try {
            $reachable = Test-DotnetupChannelReachable $url
            # A global.json or multi-spec update may have already changed installation state.
            $canRetry = $reachable -and ($Preview -or ($singleExplicitDaily -and
                [string]::Equals($manifestText, [IO.File]::ReadAllText($ManifestPath), [StringComparison]::Ordinal))) -and
                (Get-FileHash -LiteralPath $Manager -Algorithm SHA256 -ErrorAction Stop).Hash -eq $initialHash
        } catch {
            Write-Warning ('dotnetup channel verification failed: ' + $_.Exception.Message)
            throw $failure
        }
        if (-not $canRetry) { throw $failure }
        $firstFailure = $failure
        Write-Warning 'Channel resolution is reachable again. Retrying the original dotnetup command once.'
    }
}
