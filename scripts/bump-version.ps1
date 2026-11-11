<#
.SYNOPSIS
  Sets the app version consistently across the repository and prints the git commands to release it.

.DESCRIPTION
  Updates the workspace version in Cargo.toml, the version in apps/desktop/package.json
  and package-lock.json, and the version in tauri.conf.json when that file defines one.
  Refreshes the workspace entries in Cargo.lock and moves the Unreleased section of
  CHANGELOG.md under the new version heading. Does not run any git command.

.PARAMETER Version
  New semantic version, with or without a leading "v" (for example 0.2.0 or 1.0.0-rc.1).

.PARAMETER SkipLock
  Do not run cargo to refresh Cargo.lock.

.EXAMPLE
  ./scripts/bump-version.ps1 0.2.0
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$Version,
    [switch]$SkipLock
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$Version = $Version -replace '^v', ''

$semver = '^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?(\+[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$'
if ($Version -notmatch $semver) { throw "'$Version' is not a valid semantic version." }

$utf8 = New-Object System.Text.UTF8Encoding($false)

# Applies $transform to the file text. Returns $false when the file is missing or the
# transform returns $null (nothing to do), otherwise writes the result and returns $true.
function Update-File([string]$relative, [scriptblock]$transform) {
    $path = Join-Path $root $relative
    if (-not (Test-Path $path)) { return $false }
    $text = [System.IO.File]::ReadAllText($path)
    $updated = & $transform $text
    if ($null -eq $updated) { return $false }
    if ($updated -ne $text) { [System.IO.File]::WriteAllText($path, $updated, $utf8) }
    return $true
}

function Replace-First([string]$text, [string]$pattern, [string]$replacement, [string]$what) {
    $regex = New-Object System.Text.RegularExpressions.Regex($pattern, 'Multiline, Singleline')
    if (-not $regex.IsMatch($text)) { throw "Could not find $what." }
    $regex.Replace($text, $replacement, 1)
}

$null = Update-File 'Cargo.toml' {
    param($t)
    Replace-First $t '(^\[workspace\.package\]\s*$.*?^version\s*=\s*")[^"]*(")' ('${1}' + $Version + '${2}') 'workspace version in Cargo.toml'
}
Write-Host "Cargo.toml                     -> $Version"

$null = Update-File 'apps/desktop/package.json' {
    param($t)
    Replace-First $t '(^  "version"\s*:\s*")[^"]*(")' ('${1}' + $Version + '${2}') 'version in package.json'
}
Write-Host "apps/desktop/package.json      -> $Version"

$lockUpdated = Update-File 'apps/desktop/package-lock.json' {
    param($t)
    $t = Replace-First $t '(^  "version"\s*:\s*")[^"]*(")' ('${1}' + $Version + '${2}') 'version in package-lock.json'
    Replace-First $t '("packages"\s*:\s*\{\s*""\s*:\s*\{.*?"version"\s*:\s*")[^"]*(")' ('${1}' + $Version + '${2}') 'root package version in package-lock.json'
}
if ($lockUpdated) { Write-Host "apps/desktop/package-lock.json -> $Version" }

$confUpdated = Update-File 'apps/desktop/src-tauri/tauri.conf.json' {
    param($t)
    if ($t -notmatch '(?m)^  "version"\s*:') { return $null }
    Replace-First $t '(^  "version"\s*:\s*")[^"]*(")' ('${1}' + $Version + '${2}') 'version in tauri.conf.json'
}
if ($confUpdated) {
    Write-Host "tauri.conf.json                -> $Version"
} else {
    Write-Host 'tauri.conf.json                has no version field (inherits from Cargo.toml)'
}

$changelogUpdated = Update-File 'CHANGELOG.md' {
    param($t)
    if ($t -match "(?m)^## \[$([regex]::Escape($Version))\]") { return $null }
    if ($t -notmatch '(?m)^## \[Unreleased\]') { return $null }
    $date = (Get-Date).ToString('yyyy-MM-dd')
    $regex = New-Object System.Text.RegularExpressions.Regex('(?m)^## \[Unreleased\][ \t]*$')
    $regex.Replace($t, "## [Unreleased]`n`n## [$Version] - $date", 1)
}
if ($changelogUpdated) { Write-Host "CHANGELOG.md                   -> [$Version]" }

if (-not $SkipLock) {
    $cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
    if ((Test-Path $cargoBin) -and ($env:Path -notlike "*$cargoBin*")) { $env:Path = "$cargoBin;$env:Path" }
    Push-Location $root
    try {
        & cargo update --workspace
        if ($LASTEXITCODE -ne 0) { throw 'cargo update --workspace failed.' }
    } finally {
        Pop-Location
    }
    Write-Host 'Cargo.lock                     refreshed'
}

Write-Host ''
Write-Host 'Review the changes, then release with:'
Write-Host ''
Write-Host '  git add Cargo.toml Cargo.lock CHANGELOG.md apps/desktop/package.json apps/desktop/package-lock.json apps/desktop/src-tauri/tauri.conf.json'
Write-Host "  git commit -m `"chore(release): v$Version`""
Write-Host "  git tag -a v$Version -m `"Glowdrift v$Version`""
Write-Host "  git push origin main v$Version"
