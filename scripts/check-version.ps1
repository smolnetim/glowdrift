<#
.SYNOPSIS
  Verifies that the version is consistent across the workspace and, optionally, matches a release tag.

.PARAMETER Tag
  Release tag to compare against (for example v0.2.0). When omitted, only consistency is checked.

.EXAMPLE
  ./scripts/check-version.ps1 -Tag v0.2.0
#>
[CmdletBinding()]
param(
    [string]$Tag
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

function Read-Text([string]$relative) {
    [System.IO.File]::ReadAllText((Join-Path $root $relative))
}

$versions = [ordered]@{}

$cargo = Read-Text 'Cargo.toml'
$match = [regex]::Match($cargo, '(?ms)^\[workspace\.package\]\s*$.*?^version\s*=\s*"([^"]+)"')
if (-not $match.Success) { throw 'Cargo.toml: no version found in [workspace.package].' }
$versions['Cargo.toml (workspace.package)'] = $match.Groups[1].Value

$package = Read-Text 'apps/desktop/package.json'
$match = [regex]::Match($package, '(?m)^\s{2}"version"\s*:\s*"([^"]+)"')
if (-not $match.Success) { throw 'apps/desktop/package.json: no top-level version found.' }
$versions['apps/desktop/package.json'] = $match.Groups[1].Value

$tauriConf = Read-Text 'apps/desktop/src-tauri/tauri.conf.json'
$match = [regex]::Match($tauriConf, '(?m)^\s{2}"version"\s*:\s*"([^"]+)"')
if ($match.Success) { $versions['tauri.conf.json'] = $match.Groups[1].Value }

$lockPath = Join-Path $root 'Cargo.lock'
if (Test-Path $lockPath) {
    $lock = [System.IO.File]::ReadAllText($lockPath)
    $match = [regex]::Match($lock, '(?m)^name = "glowdrift-desktop"\s*\r?\nversion = "([^"]+)"')
    if ($match.Success) { $versions['Cargo.lock (glowdrift-desktop)'] = $match.Groups[1].Value }
}

$expected = $versions['Cargo.toml (workspace.package)']
if ($Tag) {
    $expected = $Tag -replace '^v', ''
    $versions['tag'] = $expected
}

$failed = $false
foreach ($entry in $versions.GetEnumerator()) {
    $state = 'ok'
    if ($entry.Value -ne $expected) { $failed = $true; $state = 'MISMATCH' }
    Write-Host ('{0,-9} {1,-34} {2}' -f $state, $entry.Key, $entry.Value)
}

if ($failed) {
    throw "Version mismatch (expected $expected). Run scripts/bump-version.ps1 to make them consistent."
}
Write-Host "Version $expected is consistent."
