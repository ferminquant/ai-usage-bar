[CmdletBinding()]
param(
    [string]$PackageRoot = "",
    [string]$InstallRoot = "",
    [string]$StartupValueName = "AI Usage Bar",
    [switch]$SkipStartup,
    [switch]$Force,
    # Test-only fault injection used by smoke-test.ps1 to exercise rollback.
    [string]$TestFailureMode = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
if ([string]::IsNullOrWhiteSpace($PackageRoot)) {
    $PackageRoot = $scriptRoot
}
$PackageRoot = (Resolve-Path -LiteralPath $PackageRoot).Path

if ([string]::IsNullOrWhiteSpace($InstallRoot)) {
    $localAppData = [Environment]::GetFolderPath("LocalApplicationData")
    if ([string]::IsNullOrWhiteSpace($localAppData)) {
        $localAppData = $env:LOCALAPPDATA
    }
    if ([string]::IsNullOrWhiteSpace($localAppData)) {
        throw "LOCALAPPDATA is not available; provide -InstallRoot explicitly"
    }
    $InstallRoot = Join-Path $localAppData "AI Usage Bar"
}
$InstallRoot = [IO.Path]::GetFullPath($InstallRoot)

if ([string]::IsNullOrWhiteSpace($StartupValueName)) {
    throw "StartupValueName cannot be empty"
}
if ($TestFailureMode -notin @(
        "",
        "after-swap",
        "after-startup",
        "after-swap-cleanup-blocked",
        "after-quarantine-blocked",
        "after-restore-blocked"
    )) {
    throw "Unsupported TestFailureMode: $TestFailureMode"
}

$manifestPath = Join-Path $PackageRoot "package-manifest.json"
if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    throw "package-manifest.json is missing from $PackageRoot"
}
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
if ($manifest.product -ne "AI Usage Bar" -or [int]$manifest.schema_version -ne 1) {
    throw "Unsupported AI Usage Bar package manifest"
}

function Test-SafeRelativePath {
    param([string]$RelativePath)
    if ([string]::IsNullOrWhiteSpace($RelativePath)) {
        return $false
    }
    return -not ([IO.Path]::IsPathRooted($RelativePath) -or
        $RelativePath -match "(^|[\\/])\.\.([\\/]|$)")
}

function Stop-InstalledShell {
    param([string]$Root)
    $shellPath = [IO.Path]::GetFullPath((Join-Path $Root "ai-usage-bar-shell.exe"))
    foreach ($process in @(Get-Process -Name "ai-usage-bar-shell" -ErrorAction SilentlyContinue)) {
        $processPath = ""
        try {
            $processPath = $process.Path
        } catch {
            $processPath = ""
        }
        if (-not [string]::IsNullOrWhiteSpace($processPath) -and
            [IO.Path]::GetFullPath($processPath) -ieq $shellPath) {
            Stop-Process -Id $process.Id -Force
            if (-not $process.WaitForExit(10000)) {
                throw "The installed shell did not exit within 10 seconds"
            }
        }
    }
}

function Get-RunValue {
    param([string]$Name)
    $runKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
    if (-not (Test-Path -LiteralPath $runKey)) {
        return $null
    }
    $property = Get-ItemProperty -LiteralPath $runKey -Name $Name -ErrorAction SilentlyContinue
    if ($null -eq $property) {
        return $null
    }
    return [string]$property.$Name
}

function Set-RunValue {
    param(
        [string]$Name,
        [string]$Value
    )
    $runKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
    if (-not (Test-Path -LiteralPath $runKey)) {
        New-Item -Path $runKey -Force | Out-Null
    }
    New-ItemProperty -LiteralPath $runKey -Name $Name -Value $Value -PropertyType String -Force | Out-Null
}

function Remove-RunValue {
    param([string]$Name)
    $runKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
    if (Test-Path -LiteralPath $runKey) {
        Remove-ItemProperty -LiteralPath $runKey -Name $Name -ErrorAction SilentlyContinue
    }
}

$startupPreferenceKey = "HKCU:\Software\AI Usage Bar"
$startupPreferenceValueName = "StartupEnabled"

function Get-StartupPreference {
    if (-not (Test-Path -LiteralPath $startupPreferenceKey)) {
        return $null
    }
    $property = Get-ItemProperty -LiteralPath $startupPreferenceKey -Name $startupPreferenceValueName -ErrorAction SilentlyContinue
    if ($null -eq $property) {
        return $null
    }
    $raw = $property.$startupPreferenceValueName
    if ($raw -is [bool]) {
        return $raw
    }
    if ($raw -eq 0 -or $raw -eq 1) {
        return ([int]$raw -eq 1)
    }
    return $null
}

function Set-StartupPreference {
    param([bool]$Enabled)
    if (-not (Test-Path -LiteralPath $startupPreferenceKey)) {
        New-Item -Path $startupPreferenceKey -Force | Out-Null
    }
    New-ItemProperty -LiteralPath $startupPreferenceKey `
        -Name $startupPreferenceValueName -Value ([int]$Enabled) -PropertyType DWord -Force | Out-Null
}

function Remove-StartupPreference {
    if (Test-Path -LiteralPath $startupPreferenceKey) {
        Remove-ItemProperty -LiteralPath $startupPreferenceKey -Name $startupPreferenceValueName -ErrorAction SilentlyContinue
    }
}

function Restore-StartupPreference {
    param($Preference)
    if ($null -eq $Preference) {
        Remove-StartupPreference
    } else {
        Set-StartupPreference -Enabled ([bool]$Preference)
    }
}

$manifestFiles = @()
foreach ($entry in @($manifest.files)) {
    $relativePath = [string]$entry.path
    if (-not (Test-SafeRelativePath $relativePath)) {
        throw "Package manifest contains an unsafe path: $relativePath"
    }
    $sourcePath = Join-Path $PackageRoot ($relativePath -replace "/", "\")
    if (-not (Test-Path -LiteralPath $sourcePath -PathType Leaf)) {
        throw "Package file is missing: $relativePath"
    }
    $expectedHash = ([string]$entry.sha256).ToLowerInvariant()
    if ($expectedHash -notmatch "^[0-9a-f]{64}$") {
        throw "Package manifest has an invalid SHA-256 for $relativePath"
    }
    $actualHash = (Get-FileHash -LiteralPath $sourcePath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualHash -ne $expectedHash) {
        throw "Package hash mismatch for $relativePath"
    }
    $manifestFiles += $relativePath
}
$copyFiles = @($manifestFiles + "package-manifest.json", "checksums.sha256") | Select-Object -Unique
foreach ($relativePath in $copyFiles) {
    $sourcePath = Join-Path $PackageRoot ($relativePath -replace "/", "\")
    if (-not (Test-Path -LiteralPath $sourcePath -PathType Leaf)) {
        throw "Package file is missing: $relativePath"
    }
}
$checksumPath = Join-Path $PackageRoot "checksums.sha256"
$expectedChecksumPaths = @{}
foreach ($entry in @($manifest.files)) {
    $relativePath = ([string]$entry.path).Replace("\", "/")
    if ($expectedChecksumPaths.ContainsKey($relativePath)) {
        throw "Package manifest contains duplicate file entries: $relativePath"
    }
    $expectedChecksumPaths[$relativePath] = $true
}
$expectedChecksumPaths["package-manifest.json"] = $true
$checksumByPath = @{}
foreach ($line in Get-Content -LiteralPath $checksumPath) {
    if ($line -notmatch "^\s*([0-9a-fA-F]{64})\s+(.+?)\s*$") {
        throw "checksums.sha256 contains a malformed line"
    }
    $relativePath = $matches[2].Replace("\", "/")
    if (-not $expectedChecksumPaths.ContainsKey($relativePath)) {
        throw "checksums.sha256 contains an unexpected file: $relativePath"
    }
    if ($checksumByPath.ContainsKey($relativePath)) {
        throw "checksums.sha256 contains a duplicate file: $relativePath"
    }
    $checksumByPath[$relativePath] = $matches[1].ToLowerInvariant()
}
foreach ($relativePath in $expectedChecksumPaths.Keys) {
    if (-not $checksumByPath.ContainsKey($relativePath)) {
        throw "checksums.sha256 is missing: $relativePath"
    }
}
foreach ($entry in @($manifest.files)) {
    $relativePath = ([string]$entry.path).Replace("\", "/")
    if (-not $checksumByPath.ContainsKey($relativePath) -or
        $checksumByPath[$relativePath] -ne ([string]$entry.sha256).ToLowerInvariant()) {
        throw "checksums.sha256 does not match manifest entry: $relativePath"
    }
}
$manifestHash = (Get-FileHash -LiteralPath $manifestPath -Algorithm SHA256).Hash.ToLowerInvariant()
if (-not $checksumByPath.ContainsKey("package-manifest.json") -or
    $manifestHash -ne $checksumByPath["package-manifest.json"]) {
    throw "package-manifest.json does not match checksums.sha256"
}

$existingManifest = Join-Path $InstallRoot "package-manifest.json"
$existingInstallation = Test-Path -LiteralPath $existingManifest -PathType Leaf
$existingState = $null
$existingStatePath = Join-Path $InstallRoot "install-state.json"
if ($existingInstallation -and (Test-Path -LiteralPath $existingStatePath -PathType Leaf)) {
    try {
        $candidateState = Get-Content -LiteralPath $existingStatePath -Raw | ConvertFrom-Json
        if ($candidateState.product -eq "AI Usage Bar" -and [int]$candidateState.schema_version -eq 1) {
            $existingState = $candidateState
        }
    } catch {
        # The manifest remains the authoritative installation marker. A
        # malformed state file should not prevent a repair install.
    }
}
if (Test-Path -LiteralPath $InstallRoot) {
    $existingChildren = @(Get-ChildItem -LiteralPath $InstallRoot -Force)
    if ($existingChildren.Count -gt 0 -and -not (Test-Path -LiteralPath $existingManifest -PathType Leaf)) {
        throw "Install root is not an AI Usage Bar installation: $InstallRoot"
    }
}

$expectedStartup = '"' + (Join-Path $InstallRoot "ai-usage-bar-shell.exe") + '"'
$previousStartup = Get-RunValue -Name $StartupValueName
$previousPreference = Get-StartupPreference
if ($null -eq $previousPreference -and $null -ne $existingState) {
    if ($existingState.PSObject.Properties.Name -contains "startup_enabled") {
        $previousPreference = [bool]$existingState.startup_enabled
    } elseif ($existingState.PSObject.Properties.Name -contains "startup_value_name" -and
        -not [string]::IsNullOrWhiteSpace([string]$existingState.startup_value_name)) {
        # Migrate pre-preference installations that recorded startup as
        # enabled even though the Run value was later lost.
        $previousPreference = $true
    } else {
        $previousPreference = $false
    }
}
if ($null -eq $previousPreference -and $existingInstallation) {
    # A manifest-only/legacy installation has no durable preference. Preserve
    # the old behavior: an existing Run value means enabled; no value means
    # the user opted out.
    $previousPreference = -not [string]::IsNullOrWhiteSpace($previousStartup)
}
$preserveStartupDisabled = $existingInstallation -and
    $null -ne $previousPreference -and -not [bool]$previousPreference -and -not $Force
$registerStartup = -not $SkipStartup -and -not $preserveStartupDisabled
$startupEnabledAfterInstall = $registerStartup
if ($registerStartup -and -not $Force -and
    -not [string]::IsNullOrWhiteSpace($previousStartup) -and
    $previousStartup.Trim('"') -ine $expectedStartup.Trim('"')) {
    throw "Startup entry '$StartupValueName' already points to another application; use -Force to replace it"
}

$parentRoot = Split-Path -Parent $InstallRoot
New-Item -ItemType Directory -Path $parentRoot -Force | Out-Null
$runId = [Guid]::NewGuid().ToString("N")
$stagingRoot = "$InstallRoot.__staging_$runId"
$backupRoot = "$InstallRoot.__backup_$runId"
$failedRoot = "$InstallRoot.__failed_$runId"
$oldInstallMoved = $false
$newInstallMoved = $false
$startupChanged = $false
$startupPreferenceChanged = $false
$rollbackRestored = $false
$backupCleanupAllowed = $false
$installSucceeded = $false

try {
    Stop-InstalledShell -Root $InstallRoot
    New-Item -ItemType Directory -Path $stagingRoot -Force | Out-Null
    foreach ($relativePath in $copyFiles) {
        $destinationPath = Join-Path $stagingRoot ($relativePath -replace "/", "\")
        $destinationParent = Split-Path -Parent $destinationPath
        New-Item -ItemType Directory -Path $destinationParent -Force | Out-Null
        Copy-Item -LiteralPath (Join-Path $PackageRoot ($relativePath -replace "/", "\")) -Destination $destinationPath -Force
    }

    if (Test-Path -LiteralPath $InstallRoot) {
        Move-Item -LiteralPath $InstallRoot -Destination $backupRoot
        $oldInstallMoved = $true
    }
    Move-Item -LiteralPath $stagingRoot -Destination $InstallRoot
    $newInstallMoved = $true
    if ($TestFailureMode -in @(
            "after-swap",
            "after-swap-cleanup-blocked",
            "after-quarantine-blocked",
            "after-restore-blocked"
        )) {
        throw "Synthetic install failure after swap: $TestFailureMode"
    }

    if ($registerStartup) {
        Set-RunValue -Name $StartupValueName -Value $expectedStartup
        $startupChanged = $true
    }
    Set-StartupPreference -Enabled $startupEnabledAfterInstall
    $startupPreferenceChanged = $true
    if ($TestFailureMode -eq "after-startup") {
        throw "Synthetic install failure after startup registration"
    }

    $state = [ordered]@{
        schema_version = 1
        product = "AI Usage Bar"
        version = [string]$manifest.version
        installed_at_utc = (Get-Date).ToUniversalTime().ToString("o")
        install_root = $InstallRoot
        startup_value_name = if ($registerStartup) { $StartupValueName } else { $null }
        startup_enabled = [bool]$startupEnabledAfterInstall
        config_path = if ($env:APPDATA) { Join-Path $env:APPDATA "AI Usage Bar\config.json" } else { $null }
        provider_data_is_outside_install_root = $true
    }
    $state | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $InstallRoot "install-state.json") -Encoding UTF8

    $installSucceeded = $true
    Write-Output ("Installed AI Usage Bar {0} to {1}" -f $manifest.version, $InstallRoot)
    if ($registerStartup) {
        Write-Output ("Startup registration: HKCU Run/{0}" -f $StartupValueName)
    } elseif ($preserveStartupDisabled) {
        Write-Output "Startup registration: preserved disabled preference"
    }
} catch {
    $originalError = $_
    if ($startupPreferenceChanged) {
        try {
            Restore-StartupPreference -Preference $previousPreference
        } catch {
            Write-Warning ("Could not restore startup preference; manual recovery may be required: {0}" -f $_.Exception.Message)
        }
    }
    if ($startupChanged) {
        try {
            if ([string]::IsNullOrWhiteSpace($previousStartup)) {
                Remove-RunValue -Name $StartupValueName
            } else {
                Set-RunValue -Name $StartupValueName -Value $previousStartup
            }
        } catch {
            Write-Warning ("Could not restore startup registration; manual recovery may be required: {0}" -f $_.Exception.Message)
        }
    }
    $newInstallGone = -not $newInstallMoved -or -not (Test-Path -LiteralPath $InstallRoot)
    if (-not $newInstallGone) {
        try {
            if ($TestFailureMode -in @("after-swap-cleanup-blocked", "after-quarantine-blocked")) {
                throw "Synthetic locked partial install"
            }
            Remove-Item -LiteralPath $InstallRoot -Recurse -Force
            $newInstallGone = -not (Test-Path -LiteralPath $InstallRoot)
        } catch {
            # Keep the backup below if a transient lock prevents cleanup. It
            # is the only recoverable copy of the previous installation.
            try {
                if ($TestFailureMode -eq "after-quarantine-blocked") {
                    throw "Synthetic quarantine lock"
                }
                Move-Item -LiteralPath $InstallRoot -Destination $failedRoot -Force
                $newInstallGone = -not (Test-Path -LiteralPath $InstallRoot)
                if (-not $newInstallGone) {
                    $backupMessage = if ($oldInstallMoved) {
                        "The previous-version backup remains at $backupRoot"
                    } else {
                        "This was a fresh install; no previous-version backup exists"
                    }
                    Write-Warning ("Could not quarantine the partial installation at {0}; {1}" -f $InstallRoot, $backupMessage)
                }
            } catch {
                # If the quarantine move is also blocked, leave both the
                # partial install and backup in place for manual recovery.
                $newInstallGone = $false
                $backupMessage = if ($oldInstallMoved) {
                    "The previous-version backup remains at $backupRoot"
                } else {
                    "This was a fresh install; no previous-version backup exists"
                }
                Write-Warning ("Could not quarantine the partial installation at {0}; {1}: {2}" -f $InstallRoot, $backupMessage, $_.Exception.Message)
            }
        }
    }
    if ($oldInstallMoved -and $newInstallGone -and (Test-Path -LiteralPath $backupRoot)) {
        try {
            if ($TestFailureMode -eq "after-restore-blocked") {
                throw "Synthetic restore lock"
            }
            Move-Item -LiteralPath $backupRoot -Destination $InstallRoot
            $rollbackRestored = $true
        } catch {
            # Leave backupRoot in place for manual recovery rather than
            # deleting the previous installation in finally.
            $rollbackRestored = $false
            Write-Warning ("Could not restore the previous installation from {0}; manual recovery is required: {1}" -f $backupRoot, $_.Exception.Message)
        }
    } elseif (-not $oldInstallMoved) {
        $rollbackRestored = $true
    }
    $backupCleanupAllowed = -not $oldInstallMoved -or $rollbackRestored
    throw $originalError
} finally {
    if (Test-Path -LiteralPath $stagingRoot) {
        try {
            Remove-Item -LiteralPath $stagingRoot -Recurse -Force
        } catch {
            Write-Warning ("Could not remove staging directory; manual cleanup may be required: {0}" -f $_.Exception.Message)
        }
    }
    if ($installSucceeded -and $oldInstallMoved -and (Test-Path -LiteralPath $backupRoot)) {
        try {
            Remove-Item -LiteralPath $backupRoot -Recurse -Force
        } catch {
            Write-Warning ("Install succeeded but the previous-version backup could not be removed: {0}" -f $_.Exception.Message)
        }
    } elseif (-not $installSucceeded -and $backupCleanupAllowed -and
        (Test-Path -LiteralPath $backupRoot)) {
        try {
            Remove-Item -LiteralPath $backupRoot -Recurse -Force
        } catch {
            Write-Warning ("Could not remove temporary backup; manual cleanup may be required: {0}" -f $_.Exception.Message)
        }
    }
}
