[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [string]$InstallRoot = "",
    [string]$StartupValueName = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

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

if (-not (Test-Path -LiteralPath $InstallRoot -PathType Container)) {
    Write-Output "AI Usage Bar is not installed at $InstallRoot"
    return
}

$statePath = Join-Path $InstallRoot "install-state.json"
$manifestPath = Join-Path $InstallRoot "package-manifest.json"
if (-not (Test-Path -LiteralPath $statePath -PathType Leaf) -and
    -not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    throw "Refusing to remove a directory without an AI Usage Bar installation marker: $InstallRoot"
}

$manifest = $null
if (Test-Path -LiteralPath $manifestPath -PathType Leaf) {
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    if ($manifest.product -ne "AI Usage Bar" -or [int]$manifest.schema_version -ne 1) {
        throw "Package manifest does not belong to AI Usage Bar"
    }
}

$state = $null
if (Test-Path -LiteralPath $statePath -PathType Leaf) {
    $state = Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json
    if ($state.product -ne "AI Usage Bar") {
        throw "Installation marker does not belong to AI Usage Bar"
    }
    if ([string]::IsNullOrWhiteSpace($StartupValueName) -and $state.startup_value_name) {
        $StartupValueName = [string]$state.startup_value_name
    }
}
if ([string]::IsNullOrWhiteSpace($StartupValueName)) {
    $StartupValueName = "AI Usage Bar"
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

$runKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
$expectedStartup = '"' + (Join-Path $InstallRoot "ai-usage-bar-shell.exe") + '"'
$registeredStartup = $null
if (Test-Path -LiteralPath $runKey) {
    $property = Get-ItemProperty -LiteralPath $runKey -Name $StartupValueName -ErrorAction SilentlyContinue
    if ($null -ne $property) {
        $registeredStartup = [string]$property.$StartupValueName
    }
}

if ($PSCmdlet.ShouldProcess($InstallRoot, "Stop AI Usage Bar and remove installed files")) {
    Stop-InstalledShell -Root $InstallRoot

    # Remove only our startup value. If a user repointed it, leave their newer
    # choice intact instead of deleting an unrelated startup command.
    if ($null -ne $registeredStartup -and
        $registeredStartup.Trim('"') -ieq $expectedStartup.Trim('"') -and
        (Test-Path -LiteralPath $runKey)) {
        Remove-ItemProperty -LiteralPath $runKey -Name $StartupValueName -ErrorAction SilentlyContinue
    }

    Remove-Item -LiteralPath $InstallRoot -Recurse -Force
    Write-Output "Removed AI Usage Bar from $InstallRoot"
    Write-Output "Preserved provider configuration and credentials outside the install directory"
}
