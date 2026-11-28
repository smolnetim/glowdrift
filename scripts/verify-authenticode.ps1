<#
.SYNOPSIS
  Fails unless every matched file has a valid, timestamped Authenticode signature.

.PARAMETER Path
  One or more file paths or wildcard patterns. Each pattern must match at least one file.

.EXAMPLE
  ./scripts/verify-authenticode.ps1 -Path target/release/glowdrift-desktop.exe, target/release/bundle/nsis/*-setup.exe
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string[]]$Path
)

$ErrorActionPreference = 'Stop'

$failures = @()
foreach ($pattern in $Path) {
    $files = @(Get-ChildItem -Path $pattern -File -ErrorAction SilentlyContinue)
    if ($files.Count -eq 0) {
        $failures += "No file matches '$pattern'."
        continue
    }
    foreach ($file in $files) {
        $signature = Get-AuthenticodeSignature -FilePath $file.FullName
        if ($signature.Status -ne 'Valid') {
            $failures += "$($file.Name): signature status is $($signature.Status). $($signature.StatusMessage)"
        } elseif ($null -eq $signature.TimeStamperCertificate) {
            $failures += "$($file.Name): signature has no timestamp."
        } else {
            Write-Host "$($file.Name): valid, signed by '$($signature.SignerCertificate.Subject)'"
        }
    }
}

if ($failures.Count -gt 0) {
    $failures | ForEach-Object { Write-Error $_ -ErrorAction Continue }
    throw "Authenticode verification failed for $($failures.Count) item(s)."
}
