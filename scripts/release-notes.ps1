<#
.SYNOPSIS
  Generates Markdown release notes from the Conventional Commits between two tags.

.PARAMETER Tag
  The release tag. Commits up to and including this tag are considered.

.PARAMETER PreviousTag
  Start of the range (exclusive). Defaults to the closest earlier v*.*.* tag, or the first
  commit when there is none.

.PARAMETER OutFile
  Where to write the notes. When omitted, the notes are written to the pipeline.

.EXAMPLE
  ./scripts/release-notes.ps1 -Tag v0.2.0 -OutFile notes.md
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Tag,
    [string]$PreviousTag,
    [string]$OutFile
)

$ErrorActionPreference = 'Stop'

function Invoke-Git {
    $output = & git @args
    if ($LASTEXITCODE -ne 0) { throw "git $($args -join ' ') failed." }
    $output
}

if (-not $PreviousTag) {
    $tags = @(Invoke-Git tag --list 'v*.*.*' --sort=-v:refname)
    $tagCommit = (Invoke-Git rev-list -n 1 $Tag | Select-Object -First 1)
    foreach ($candidate in $tags) {
        if ($candidate -eq $Tag) { continue }
        $candidateCommit = (Invoke-Git rev-list -n 1 $candidate | Select-Object -First 1)
        # Take the newest tag that is an ancestor of this release.
        & git merge-base --is-ancestor $candidateCommit $tagCommit
        if ($LASTEXITCODE -eq 0 -and $candidateCommit -ne $tagCommit) { $PreviousTag = $candidate; break }
    }
}

$range = $Tag
if ($PreviousTag) { $range = "$PreviousTag..$Tag" }

$separator = [string][char]0x1e
$fieldSeparator = [string][char]0x1f
$raw = (Invoke-Git log $range --no-merges "--pretty=format:%h$fieldSeparator%s$fieldSeparator%b$separator") -join "`n"

$sections = [ordered]@{
    'breaking' = @{ Title = 'Breaking changes'; Items = New-Object System.Collections.ArrayList }
    'feat'     = @{ Title = 'Features'; Items = New-Object System.Collections.ArrayList }
    'fix'      = @{ Title = 'Bug fixes'; Items = New-Object System.Collections.ArrayList }
    'perf'     = @{ Title = 'Performance'; Items = New-Object System.Collections.ArrayList }
    'refactor' = @{ Title = 'Refactoring'; Items = New-Object System.Collections.ArrayList }
    'docs'     = @{ Title = 'Documentation'; Items = New-Object System.Collections.ArrayList }
    'build'    = @{ Title = 'Build and dependencies'; Items = New-Object System.Collections.ArrayList }
    'other'    = @{ Title = 'Other changes'; Items = New-Object System.Collections.ArrayList }
}
$typeToSection = @{
    'feat' = 'feat'; 'fix' = 'fix'; 'perf' = 'perf'; 'refactor' = 'refactor'; 'docs' = 'docs'
    'build' = 'build'; 'deps' = 'build'; 'ci' = 'build'
}
# Types that are left out of the notes unless the commit is a breaking change.
$hiddenTypes = @('chore', 'test', 'style', 'revert')

foreach ($record in $raw.Split([char]0x1e)) {
    $record = $record.Trim("`r", "`n", ' ')
    if (-not $record) { continue }
    $fields = $record.Split([char]0x1f)
    $hash = $fields[0].Trim()
    $subject = $fields[1].Trim()
    $body = ''
    if ($fields.Length -gt 2) { $body = $fields[2] }

    $match = [regex]::Match($subject, '^(?<type>[A-Za-z]+)(\((?<scope>[^)]*)\))?(?<bang>!)?:\s*(?<desc>.+)$')
    if (-not $match.Success) {
        [void]$sections['other'].Items.Add("- $subject ($hash)")
        continue
    }

    $type = $match.Groups['type'].Value.ToLowerInvariant()
    $scope = $match.Groups['scope'].Value
    $description = $match.Groups['desc'].Value
    $breaking = $match.Groups['bang'].Success -or ($body -match '(?m)^BREAKING[ -]CHANGE:')

    if ($type -eq 'chore' -and $scope -eq 'release') { continue }

    $line = '- '
    if ($scope) { $line += "**$scope**: " }
    $line += "$description ($hash)"

    if ($breaking) {
        [void]$sections['breaking'].Items.Add($line)
        continue
    }
    if ($hiddenTypes -contains $type) { continue }
    if ($typeToSection.ContainsKey($type)) {
        [void]$sections[$typeToSection[$type]].Items.Add($line)
    } else {
        [void]$sections['other'].Items.Add($line)
    }
}

$lines = New-Object System.Collections.ArrayList
foreach ($key in $sections.Keys) {
    $section = $sections[$key]
    if ($section.Items.Count -eq 0) { continue }
    [void]$lines.Add("### $($section.Title)")
    [void]$lines.Add('')
    foreach ($item in $section.Items) { [void]$lines.Add($item) }
    [void]$lines.Add('')
}
if ($lines.Count -eq 0) {
    [void]$lines.Add('No user-facing changes in this release.')
    [void]$lines.Add('')
}

[void]$lines.Add('### Install')
[void]$lines.Add('')
[void]$lines.Add('Download the Windows installer (`.exe`) below and run it. It installs for the current user into `%LOCALAPPDATA%\Programs\T1M\Glowdrift` and needs no administrator rights. Installed copies update themselves from later releases.')
[void]$lines.Add('')

if ($env:GITHUB_REPOSITORY) {
    if ($PreviousTag) {
        [void]$lines.Add("**Full changelog**: https://github.com/$($env:GITHUB_REPOSITORY)/compare/$PreviousTag...$Tag")
    } else {
        [void]$lines.Add("**Full changelog**: https://github.com/$($env:GITHUB_REPOSITORY)/commits/$Tag")
    }
    [void]$lines.Add('')
}

$text = ($lines -join "`n")
if ($OutFile) {
    $utf8 = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($OutFile, $text, $utf8)
    Write-Host "Release notes for $Tag written to $OutFile (range: $range)"
} else {
    $text
}
