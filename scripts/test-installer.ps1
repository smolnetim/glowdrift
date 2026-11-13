<#
.SYNOPSIS
  Smoke-tests the Windows setup file against the real registry and file system of the machine it runs on.

.DESCRIPTION
  Runs these scenarios in order and prints PASS or FAIL for every assertion:
    1. Silent first install into a custom folder: files, Uninstall entry, remembered folder, shortcuts, and
       an existing autostart entry rewritten to the new folder.
    2. Silent uninstall: files, registry entries, autostart entry and shortcuts removed, app data kept.
    3. Silent install without /D goes to the default folder (the earlier folder is forgotten); no autostart
       entry is created when none existed; uninstalling removes the empty vendor folder.
    4. Silent install into a custom folder with an unquoted autostart entry, rewritten and then removed.
    5. Update: the setup file of the latest published release (or -OldSetupUrl) is installed silently into a
       custom folder with a space in its name, then the new setup file runs as "/P /UPDATE" without /D.
       The folder is reused, the version changes, the remembered folder and the autostart entry survive.
  Finally it asserts that nothing of Glowdrift remains. The exit code is 1 when any assertion failed.

  The script installs and uninstalls Glowdrift for the current user. It refuses to run (exit 2) when a
  Glowdrift install, remembered folder, autostart entry or app data folder already exists, unless the
  environment variable CI is 'true'.

  Not covered: the interactive folder page (manual checks listed in docs).

.PARAMETER SetupFile
  The setup file under test. It must carry a version higher than the latest published release.

.PARAMETER OldSetupUrl
  URL of the setup file to install before the update scenario. Defaults to the Windows setup file named in
  the latest.json of the latest GitHub release.

.PARAMETER Workdir
  Parent folder for the folders the test creates. Defaults to RUNNER_TEMP, then TEMP.

.PARAMETER TimeoutSeconds
  How long a single setup or uninstall run may take before it is killed and counted as a failure.

.EXAMPLE
  pwsh -File scripts/test-installer.ps1 -SetupFile target/release/bundle/nsis/Glowdrift_99.0.0_x64-setup.exe
#>
[CmdletBinding()]
param(
    [string]$SetupFile,
    [string]$OldSetupUrl,
    [string]$Workdir,
    [int]$TimeoutSeconds = 180
)

$ErrorActionPreference = 'Stop'

$ProductName = 'Glowdrift'
$Vendor = 'T1M'
$ExeName = 'glowdrift-desktop.exe'
$BundleId = 'gg.t1m.glowdrift'
$LatestJsonUrl = 'https://github.com/smolnetim/glowdrift/releases/latest/download/latest.json'

$UninstKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$ProductName"
$VendorKey = "HKCU:\Software\$Vendor"
$ProductKey = "HKCU:\Software\$Vendor\$ProductName"
$RunKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$StartMenuShortcut = Join-Path ([Environment]::GetFolderPath('Programs')) "$ProductName.lnk"
$DesktopShortcut = Join-Path ([Environment]::GetFolderPath('Desktop')) "$ProductName.lnk"
$AppDataFolder = Join-Path $env:APPDATA $BundleId
$AppConfigFile = Join-Path $AppDataFolder 'config.json'

function Get-RegValue([string]$Path, [string]$Name) {
    $key = Get-Item -LiteralPath $Path -ErrorAction SilentlyContinue
    if ($null -eq $key) { return $null }
    return $key.GetValue($Name, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
}

function Set-RunValue([string]$Value) {
    if (-not (Test-Path -LiteralPath $RunKey)) { New-Item -Path $RunKey -Force | Out-Null }
    New-ItemProperty -LiteralPath $RunKey -Name $ProductName -Value $Value -PropertyType String -Force | Out-Null
}

function Test-GlowdriftLeftovers {
    return (Test-Path -LiteralPath $UninstKey) -or (Test-Path -LiteralPath $ProductKey) -or
        ($null -ne (Get-RegValue $RunKey $ProductName))
}

# The guard runs before anything is installed or written.
if ($env:CI -ne 'true') {
    $existing = @()
    if (Test-Path -LiteralPath $UninstKey) { $existing += $UninstKey }
    if (Test-Path -LiteralPath $ProductKey) { $existing += $ProductKey }
    if ($null -ne (Get-RegValue $RunKey $ProductName)) { $existing += "$RunKey ($ProductName)" }
    if (Test-Path -LiteralPath $AppDataFolder) { $existing += $AppDataFolder }
    $defaultVendorFolder = Join-Path $env:LOCALAPPDATA "Programs\$Vendor"
    if (Test-Path -LiteralPath $defaultVendorFolder) { $existing += $defaultVendorFolder }
    if ($existing.Count -gt 0) {
        [Console]::Error.WriteLine("Refusing to run: $ProductName is installed or has data on this machine, and the test would uninstall it.")
        foreach ($item in $existing) { [Console]::Error.WriteLine("  found: $item") }
        [Console]::Error.WriteLine("Run it on a throwaway machine, or set CI=true to override.")
        exit 2
    }
}

if (-not $SetupFile) { throw 'Parameter -SetupFile is required.' }
if (-not (Test-Path -LiteralPath $SetupFile -PathType Leaf)) { throw "Setup file not found: $SetupFile" }
$SetupFile = (Resolve-Path -LiteralPath $SetupFile).Path

if (-not $Workdir) {
    $Workdir = $env:RUNNER_TEMP
    if (-not $Workdir) { $Workdir = $env:TEMP }
}
$Root = Join-Path $Workdir 'glowdrift-installer-smoke'
$LogFile = Join-Path $Root 'smoke.log'
if (Test-Path -LiteralPath $Root) { Remove-Item -LiteralPath $Root -Recurse -Force }
New-Item -ItemType Directory -Path $Root -Force | Out-Null

Add-Type -Namespace Glowdrift -Name KnownFolder -MemberDefinition @'
[DllImport("shell32.dll")]
public static extern int SHGetKnownFolderPath([MarshalAs(UnmanagedType.LPStruct)] Guid rfid, uint dwFlags, IntPtr hToken, out IntPtr ppszPath);
[DllImport("ole32.dll")]
public static extern void CoTaskMemFree(IntPtr pv);
'@

$script:Passed = 0
$script:Failed = New-Object System.Collections.Generic.List[string]
$script:Folders = New-Object System.Collections.Generic.List[string]

function Assert-That([string]$Name, [bool]$Condition) {
    if ($Condition) {
        $script:Passed++
        Write-Host "PASS  $Name"
    } else {
        $script:Failed.Add($Name)
        Write-Host "FAIL  $Name"
    }
}

function Get-DefaultInstallFolder {
    $programs = $null
    $pointer = [IntPtr]::Zero
    $result = [Glowdrift.KnownFolder]::SHGetKnownFolderPath([Guid]'5CD7AEE2-2219-4A67-B85D-6C9CE15660CB', 0, [IntPtr]::Zero, [ref]$pointer)
    if ($result -eq 0 -and $pointer -ne [IntPtr]::Zero) {
        $programs = [System.Runtime.InteropServices.Marshal]::PtrToStringUni($pointer)
        [Glowdrift.KnownFolder]::CoTaskMemFree($pointer)
    }
    if (-not $programs) { $programs = Join-Path $env:LOCALAPPDATA 'Programs' }
    return (Join-Path (Join-Path $programs $Vendor) $ProductName)
}

function Test-SamePath($Left, $Right) {
    if ([string]::IsNullOrEmpty([string]$Left) -or [string]::IsNullOrEmpty([string]$Right)) { return $false }
    $a = [System.IO.Path]::GetFullPath([string]$Left).TrimEnd('\')
    $b = [System.IO.Path]::GetFullPath([string]$Right).TrimEnd('\')
    return [string]::Equals($a, $b, [System.StringComparison]::OrdinalIgnoreCase)
}

function Wait-Until([scriptblock]$Condition, [int]$Seconds) {
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (& $Condition) { return $true }
        Start-Sleep -Milliseconds 500
    }
    return [bool](& $Condition)
}

# Runs a program to completion and returns its exit code, or -1 after killing it on timeout.
function Invoke-Program([string]$File, [string]$Arguments, [string]$Label) {
    Write-Host "  run: $Label -> $File $Arguments"
    $process = Start-Process -FilePath $File -ArgumentList $Arguments -PassThru
    $null = $process.Handle
    if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
        & taskkill.exe /PID $process.Id /T /F | Out-Null
        Assert-That "$Label finishes within $TimeoutSeconds seconds (a hung run was killed)" $false
        return -1
    }
    return $process.ExitCode
}

# NSIS needs /D= as the last argument and unquoted, even when the folder contains spaces.
function Install-Silent([string]$Setup, [string]$Folder, [string]$Label) {
    $arguments = '/S'
    if ($Folder) {
        $arguments = "/S /D=$Folder"
        if (-not $script:Folders.Contains($Folder)) { $script:Folders.Add($Folder) }
    }
    $code = Invoke-Program $Setup $arguments $Label
    Assert-That "$Label exits with code 0 (got $code)" ($code -eq 0)
}

function Assert-Installed([string]$Folder, [string]$Label) {
    Assert-That "$Label : $ExeName exists" (Test-Path -LiteralPath (Join-Path $Folder $ExeName) -PathType Leaf)
    Assert-That "$Label : uninstall.exe exists" (Test-Path -LiteralPath (Join-Path $Folder 'uninstall.exe') -PathType Leaf)
    Assert-That "$Label : Uninstall DisplayName is $ProductName" ((Get-RegValue $UninstKey 'DisplayName') -eq $ProductName)
    Assert-That "$Label : Uninstall Publisher is $Vendor" ((Get-RegValue $UninstKey 'Publisher') -eq $Vendor)
    Assert-That "$Label : Uninstall DisplayVersion is set" (-not [string]::IsNullOrEmpty([string](Get-RegValue $UninstKey 'DisplayVersion')))
    Assert-That "$Label : Uninstall InstallLocation is the quoted folder" ((Get-RegValue $UninstKey 'InstallLocation') -eq ('"' + $Folder + '"'))
    Assert-That "$Label : Uninstall UninstallString points at uninstall.exe in the folder" ((Get-RegValue $UninstKey 'UninstallString') -eq ('"' + (Join-Path $Folder 'uninstall.exe') + '"'))
    Assert-That "$Label : Uninstall EstimatedSize is above 0" (([int64](Get-RegValue $UninstKey 'EstimatedSize')) -gt 0)
    Assert-That "$Label : Uninstall NoModify is 1" ((Get-RegValue $UninstKey 'NoModify') -eq 1)
    Assert-That "$Label : Uninstall NoRepair is 1" ((Get-RegValue $UninstKey 'NoRepair') -eq 1)
    Assert-That "$Label : remembered folder equals the install folder" (Test-SamePath (Get-RegValue $ProductKey '') $Folder)
}

function Test-VendorKeyCleared {
    $key = Get-Item -LiteralPath $VendorKey -ErrorAction SilentlyContinue
    if ($null -eq $key) { return $true }
    return ($key.SubKeyCount -gt 0 -or $key.ValueCount -gt 0)
}

function Assert-Uninstalled([string]$Folder, [string]$Label) {
    $left = @()
    if (Test-Path -LiteralPath $Folder) {
        $left = @(Get-ChildItem -LiteralPath $Folder -Force | Where-Object { $_.Name -ne 'uninstall.exe' })
    }
    Assert-That "$Label : program files removed from the install folder" (($left.Count -eq 0) -and -not (Test-Path -LiteralPath (Join-Path $Folder $ExeName)))
    Assert-That "$Label : Uninstall entry removed" (-not (Test-Path -LiteralPath $UninstKey))
    Assert-That "$Label : remembered folder key removed" (-not (Test-Path -LiteralPath $ProductKey))
    Assert-That "$Label : empty vendor key removed" (Test-VendorKeyCleared)
    Assert-That "$Label : autostart entry removed" ($null -eq (Get-RegValue $RunKey $ProductName))
    Assert-That "$Label : Start menu shortcut removed" (-not (Test-Path -LiteralPath $StartMenuShortcut))
    Assert-That "$Label : desktop shortcut removed" (-not (Test-Path -LiteralPath $DesktopShortcut))
}

# Silent uninstall run in place with _?=, which keeps the uninstaller synchronous. That mode cannot
# delete uninstall.exe or the folder, so they are removed here after being checked.
function Uninstall-InPlace([string]$Folder, [string]$Label) {
    $uninstaller = Join-Path $Folder 'uninstall.exe'
    if (-not (Test-Path -LiteralPath $uninstaller)) {
        Assert-That "$Label : uninstall.exe is present to run" $false
        return
    }
    $code = Invoke-Program $uninstaller "/S _?=$Folder" $Label
    Assert-That "$Label exits with code 0 (got $code)" ($code -eq 0)
    Assert-Uninstalled $Folder $Label
    if (Test-Path -LiteralPath $Folder) { Remove-Item -LiteralPath $Folder -Recurse -Force }
}

function Write-Snapshot {
    Write-Host ''
    Write-Host '--- state at failure (the setup file writes no log of its own) ---'
    foreach ($path in @($UninstKey, $ProductKey, $VendorKey)) {
        if (Test-Path -LiteralPath $path) {
            Write-Host "[$path]"
            (Get-ItemProperty -LiteralPath $path | Format-List | Out-String).TrimEnd() | Write-Host
        } else {
            Write-Host "[$path] (absent)"
        }
    }
    Write-Host "[$RunKey] $ProductName = $(Get-RegValue $RunKey $ProductName)"
    Write-Host "Start menu shortcut present: $(Test-Path -LiteralPath $StartMenuShortcut)"
    Write-Host "Desktop shortcut present: $(Test-Path -LiteralPath $DesktopShortcut)"
    foreach ($folder in @($script:Folders) + @((Get-DefaultInstallFolder), (Split-Path -Parent (Get-DefaultInstallFolder)))) {
        if (Test-Path -LiteralPath $folder) {
            Write-Host "[$folder]"
            Get-ChildItem -LiteralPath $folder -Force | ForEach-Object { Write-Host "  $($_.Name)" }
        } else {
            Write-Host "[$folder] (absent)"
        }
    }
    if (Test-Path -LiteralPath $LogFile) { Write-Host "Transcript: $LogFile" }
}

function Get-OldSetupUrl {
    if ($OldSetupUrl) { return $OldSetupUrl }
    $text = Invoke-Download $LatestJsonUrl $null
    $json = $text | ConvertFrom-Json
    $script:OldVersion = $json.version
    $url = $json.platforms.'windows-x86_64'.url
    if (-not $url) { throw "No platforms.windows-x86_64.url in $LatestJsonUrl" }
    return $url
}

# Downloads with a few retries. With a target path the body is saved there, otherwise it is returned as text.
function Invoke-Download([string]$Url, [string]$OutFile) {
    $lastError = $null
    for ($attempt = 1; $attempt -le 4; $attempt++) {
        try {
            $parameters = @{ Uri = $Url; Headers = @{ Accept = 'application/octet-stream' }; UseBasicParsing = $true }
            if ($OutFile) {
                Invoke-WebRequest @parameters -OutFile $OutFile | Out-Null
                return
            }
            $response = Invoke-WebRequest @parameters
            if ($response.Content -is [byte[]]) { return [System.Text.Encoding]::UTF8.GetString($response.Content) }
            return [string]$response.Content
        } catch {
            $lastError = $_
            Start-Sleep -Seconds (5 * $attempt)
        }
    }
    throw "Download failed after 4 attempts: $Url ($lastError)"
}

$SetupVersion = [string](Get-Item -LiteralPath $SetupFile).VersionInfo.ProductVersion
if (-not $SetupVersion) { throw "The setup file has no ProductVersion: $SetupFile" }
$DefaultFolder = Get-DefaultInstallFolder
$VendorFolder = Split-Path -Parent $DefaultFolder
$VendorFolderExisted = Test-Path -LiteralPath $VendorFolder
$AppDataExisted = Test-Path -LiteralPath $AppDataFolder
$script:OldVersion = $null
$script:Folders.Add($DefaultFolder)

try { Start-Transcript -Path $LogFile -Force | Out-Null } catch { }

Write-Host "Setup file:      $SetupFile"
Write-Host "Setup version:   $SetupVersion"
Write-Host "Work folder:     $Root"
Write-Host "Default folder:  $DefaultFolder"
Write-Host ''

try {
    Assert-That 'start state: no Glowdrift install, remembered folder or autostart entry' (-not (Test-GlowdriftLeftovers))

    # Scenario 1 and 2: silent first install into a custom folder with an autostart entry pointing elsewhere.
    Write-Host '== Scenario 1: silent first install into a custom folder'
    $inst1 = Join-Path $Root 'inst1'
    New-Item -ItemType Directory -Path $AppDataFolder -Force | Out-Null
    Set-Content -LiteralPath $AppConfigFile -Value '{}' -Encoding ascii
    Set-RunValue '"C:\old\place\glowdrift-desktop.exe" --minimized'
    Install-Silent $SetupFile $inst1 'scenario 1 install'
    Assert-Installed $inst1 'scenario 1'
    Assert-That 'scenario 1 : Start menu shortcut exists' (Test-Path -LiteralPath $StartMenuShortcut -PathType Leaf)
    Assert-That 'scenario 1 : desktop shortcut exists (silent installs create it)' (Test-Path -LiteralPath $DesktopShortcut -PathType Leaf)
    Assert-That 'scenario 1 : quoted autostart entry points at the new folder and keeps its arguments' `
        ((Get-RegValue $RunKey $ProductName) -eq ('"' + (Join-Path $inst1 $ExeName) + '" --minimized'))
    Assert-That 'scenario 1 : app data is untouched by the install' (Test-Path -LiteralPath $AppConfigFile)

    Write-Host '== Scenario 2: silent uninstall keeps app data'
    Uninstall-InPlace $inst1 'scenario 2 uninstall'
    Assert-That 'scenario 2 : app data config.json still exists after a silent uninstall' (Test-Path -LiteralPath $AppConfigFile)

    # Scenario 3: the remembered folder is forgotten, so a silent install without /D uses the default folder.
    Write-Host '== Scenario 3: silent install without /D uses the default folder'
    Install-Silent $SetupFile $null 'scenario 3 install'
    Assert-That 'scenario 3 : installed into the default folder' (Test-Path -LiteralPath (Join-Path $DefaultFolder $ExeName) -PathType Leaf)
    Assert-That 'scenario 3 : nothing was installed into the earlier custom folder' (-not (Test-Path -LiteralPath $inst1))
    Assert-Installed $DefaultFolder 'scenario 3'
    Assert-That 'scenario 3 : no autostart entry is created when none existed' ($null -eq (Get-RegValue $RunKey $ProductName))

    # A real uninstall started from the install folder copies itself to TEMP and returns at once, which is
    # the only flow that also removes uninstall.exe, the install folder and the empty vendor folder.
    $code = Invoke-Program (Join-Path $DefaultFolder 'uninstall.exe') '/S' 'scenario 3 uninstall'
    Assert-That "scenario 3 uninstall exits with code 0 (got $code)" ($code -eq 0)
    $finished = Wait-Until {
        -not (Test-Path -LiteralPath $UninstKey) -and -not (Test-Path -LiteralPath $ProductKey) -and
        -not (Test-Path -LiteralPath $DefaultFolder) -and ($null -eq (Get-Process -Name 'Au_' -ErrorAction SilentlyContinue))
    } $TimeoutSeconds
    Assert-That 'scenario 3 : the uninstall completes' $finished
    Assert-Uninstalled $DefaultFolder 'scenario 3 uninstall'
    Assert-That 'scenario 3 : install folder removed including uninstall.exe' (-not (Test-Path -LiteralPath $DefaultFolder))
    if ($VendorFolderExisted) {
        Write-Host "SKIP  scenario 3 : vendor folder $VendorFolder existed before the test"
    } else {
        $vendorGone = Wait-Until { -not (Test-Path -LiteralPath $VendorFolder) } 30
        Assert-That 'scenario 3 : empty vendor folder removed' $vendorGone
    }

    # Scenario 4: an autostart entry in the unquoted form is rewritten too.
    Write-Host '== Scenario 4: unquoted autostart entry'
    $inst4 = Join-Path $Root 'inst4'
    Set-RunValue 'C:\old place\glowdrift-desktop.exe --minimized'
    Install-Silent $SetupFile $inst4 'scenario 4 install'
    Assert-Installed $inst4 'scenario 4'
    Assert-That 'scenario 4 : unquoted autostart entry is rewritten with its arguments' `
        ((Get-RegValue $RunKey $ProductName) -eq ('"' + (Join-Path $inst4 $ExeName) + '" --minimized'))
    Uninstall-InPlace $inst4 'scenario 4 uninstall'

    # Scenario 5: update of an older release in a custom folder with a space in its name.
    Write-Host '== Scenario 5: update with /P /UPDATE'
    $oldSetup = Join-Path $Root 'old-setup.exe'
    $url = Get-OldSetupUrl
    Write-Host "  old setup file: $url"
    Invoke-Download $url $oldSetup
    $oldFileVersion = [string](Get-Item -LiteralPath $oldSetup).VersionInfo.ProductVersion
    Write-Host "  old version:    $oldFileVersion"
    $inst5 = Join-Path $Root 'inst update'
    Install-Silent $oldSetup $inst5 'scenario 5 old install'
    Assert-That 'scenario 5 : old version installed in the custom folder' (Test-Path -LiteralPath (Join-Path $inst5 $ExeName) -PathType Leaf)
    Assert-That 'scenario 5 : old setup file differs from the new one' ($oldFileVersion -ne $SetupVersion)
    $oldDisplayVersion = [string](Get-RegValue $UninstKey 'DisplayVersion')
    $oldExeHash = (Get-FileHash -LiteralPath (Join-Path $inst5 $ExeName) -Algorithm SHA256).Hash
    $runEntry = '"' + (Join-Path $inst5 $ExeName) + '" --minimized'
    Set-RunValue $runEntry

    $code = Invoke-Program $SetupFile '/P /UPDATE' 'scenario 5 update'
    Assert-That "scenario 5 update exits with code 0 (got $code)" ($code -eq 0)
    Assert-Installed $inst5 'scenario 5 after update'
    Assert-That "scenario 5 : DisplayVersion changed from $oldDisplayVersion to $SetupVersion" ((Get-RegValue $UninstKey 'DisplayVersion') -eq $SetupVersion)
    Assert-That 'scenario 5 : the executable was replaced' ((Get-FileHash -LiteralPath (Join-Path $inst5 $ExeName) -Algorithm SHA256).Hash -ne $oldExeHash)
    Assert-That 'scenario 5 : no second install in the default folder' (-not (Test-Path -LiteralPath $DefaultFolder))
    Assert-That 'scenario 5 : autostart entry survived the update' ((Get-RegValue $RunKey $ProductName) -eq $runEntry)
    Assert-That 'scenario 5 : Start menu shortcut survived the update' (Test-Path -LiteralPath $StartMenuShortcut -PathType Leaf)
    Uninstall-InPlace $inst5 'scenario 5 uninstall'

    Write-Host '== Final state'
    foreach ($folder in $script:Folders) {
        Assert-That "final : $folder does not exist" (-not (Test-Path -LiteralPath $folder))
    }
    Assert-That 'final : no Uninstall entry, remembered folder or autostart entry remains' (-not (Test-GlowdriftLeftovers))
    Assert-That 'final : empty vendor key removed' (Test-VendorKeyCleared)
    Assert-That 'final : no Start menu shortcut remains' (-not (Test-Path -LiteralPath $StartMenuShortcut))
    Assert-That 'final : no desktop shortcut remains' (-not (Test-Path -LiteralPath $DesktopShortcut))
    if (-not $VendorFolderExisted) {
        Assert-That 'final : vendor folder does not exist' (-not (Test-Path -LiteralPath $VendorFolder))
    }
} catch {
    Write-Host "FAIL  the test stopped on an error: $($_.Exception.Message)"
    Write-Host $_.ScriptStackTrace
    $script:Failed.Add("error: $($_.Exception.Message)")
} finally {
    if (Test-Path -LiteralPath $AppConfigFile) { Remove-Item -LiteralPath $AppConfigFile -Force }
    if (-not $AppDataExisted -and (Test-Path -LiteralPath $AppDataFolder)) {
        Remove-Item -LiteralPath $AppDataFolder -Force -ErrorAction SilentlyContinue
    }
}

Write-Host ''
Write-Host "$($script:Passed) passed, $($script:Failed.Count) failed"
if ($script:Failed.Count -gt 0) {
    foreach ($name in $script:Failed) { Write-Host "  FAILED: $name" }
    Write-Snapshot
}
try { Stop-Transcript | Out-Null } catch { }
if ($script:Failed.Count -gt 0) { exit 1 }
exit 0
