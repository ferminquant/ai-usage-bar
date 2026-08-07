[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$PackagePath,
    [string]$SandboxPath = "",
    [switch]$KeepSandbox
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $PackagePath)) {
    throw "Package path does not exist: $PackagePath"
}
$PackagePath = (Resolve-Path -LiteralPath $PackagePath).Path

if ([string]::IsNullOrWhiteSpace($SandboxPath)) {
    $SandboxPath = Join-Path $env:TEMP ("ai-usage-bar-smoke-{0}" -f ([Guid]::NewGuid().ToString("N")))
}
$SandboxPath = [IO.Path]::GetFullPath($SandboxPath)
if (Test-Path -LiteralPath $SandboxPath) {
    Remove-Item -LiteralPath $SandboxPath -Recurse -Force
}
New-Item -ItemType Directory -Path $SandboxPath -Force | Out-Null

$packageRoot = $PackagePath
$expandedPackage = $false
if ((Get-Item -LiteralPath $PackagePath).PSIsContainer -eq $false) {
    if ([IO.Path]::GetExtension($PackagePath) -ine ".zip") {
        throw "PackagePath must be a package directory or .zip archive"
    }
    $packageRoot = Join-Path $SandboxPath "package"
    Expand-Archive -LiteralPath $PackagePath -DestinationPath $packageRoot -Force
    $expandedPackage = $true
}
$packageRoot = (Resolve-Path -LiteralPath $packageRoot).Path

$installRoot = Join-Path $SandboxPath "install"
$testAppData = Join-Path $SandboxPath "appdata"
$testLocalAppData = Join-Path $SandboxPath "localappdata"
$testProfile = Join-Path $SandboxPath "profile"
$startupValueName = "AI Usage Bar Smoke $PID"
$configPath = Join-Path $testAppData "AI Usage Bar\config.json"
$credentialSentinel = Join-Path $testProfile "provider-credentials-sentinel.txt"
$installScript = Join-Path $packageRoot "install.ps1"
$uninstallScript = Join-Path $packageRoot "uninstall.ps1"
$shellPath = Join-Path $installRoot "ai-usage-bar-shell.exe"
$runKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"

foreach ($directory in @(
    $testAppData,
    $testLocalAppData,
    $testProfile,
    (Split-Path -Parent $configPath),
    (Split-Path -Parent $credentialSentinel)
)) {
    New-Item -ItemType Directory -Path $directory -Force | Out-Null
}
$configContents = "{`"version`":1,`"providers`":{}}`n"
$credentialContents = "smoke-test-provider-data`n"
Set-Content -LiteralPath $configPath -Value $configContents -Encoding ASCII
Set-Content -LiteralPath $credentialSentinel -Value $credentialContents -Encoding ASCII

function Get-RunValue {
    param([string]$Name)
    if (-not (Test-Path -LiteralPath $runKey)) {
        return $null
    }
    $property = Get-ItemProperty -LiteralPath $runKey -Name $Name -ErrorAction SilentlyContinue
    if ($null -eq $property) {
        return $null
    }
    return [string]$property.$Name
}

function Invoke-PackageScript {
    param(
        [string]$ScriptPath,
        [hashtable]$Parameters
    )
    & $ScriptPath @Parameters
    $exitCode = 0
    if (Test-Path variable:LASTEXITCODE) {
        $exitCode = [int]$LASTEXITCODE
    }
    if ($exitCode -ne 0) {
        throw "$([IO.Path]::GetFileName($ScriptPath)) failed with exit code $exitCode"
    }
}

$environmentNames = @("APPDATA", "LOCALAPPDATA", "USERPROFILE", "HOME", "XDG_CONFIG_HOME")
$savedEnvironment = @{}
foreach ($name in $environmentNames) {
    $savedEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, "Process")
}

$runningProcess = $null
$summary = [ordered]@{
    package = $PackagePath
    install_root = $installRoot
    startup_value_name = $startupValueName
    checks = [ordered]@{}
}

try {
    $env:APPDATA = $testAppData
    $env:LOCALAPPDATA = $testLocalAppData
    $env:USERPROFILE = $testProfile
    $env:HOME = $testProfile
    $env:XDG_CONFIG_HOME = (Join-Path $SandboxPath "xdg-config")

    Invoke-PackageScript -ScriptPath $installScript -Parameters @{
        PackageRoot = $packageRoot
        InstallRoot = $installRoot
        StartupValueName = $startupValueName
    }

    if (-not (Test-Path -LiteralPath $shellPath -PathType Leaf) -or
        -not (Test-Path -LiteralPath (Join-Path $installRoot "ai-usage-bar.exe") -PathType Leaf)) {
        throw "Installed package is missing one or more binaries"
    }
    $startupValue = Get-RunValue -Name $startupValueName
    if ([string]::IsNullOrWhiteSpace($startupValue) -or
        $startupValue.Trim('"') -ine $shellPath.Trim('"')) {
        throw "Startup registration does not point to the installed shell"
    }
    $summary.checks.install_and_startup = "passed"

    $runningProcess = Start-Process -FilePath $shellPath -PassThru -WindowStyle Hidden
    Start-Sleep -Seconds 4
    if ($runningProcess.HasExited) {
        throw "Installed shell exited during startup smoke test with code $($runningProcess.ExitCode)"
    }
    Stop-Process -Id $runningProcess.Id -Force
    $runningProcess = $null
    $summary.checks.shell_startup = "passed"

    $beforeUpgradeConfig = (Get-Content -LiteralPath $configPath -Raw).Trim()
    Invoke-PackageScript -ScriptPath $installScript -Parameters @{
        PackageRoot = $packageRoot
        InstallRoot = $installRoot
        StartupValueName = $startupValueName
    }
    $afterUpgradeConfig = (Get-Content -LiteralPath $configPath -Raw).Trim()
    if ($afterUpgradeConfig -cne $beforeUpgradeConfig) {
        throw "Upgrade changed the user configuration"
    }
    $summary.checks.upgrade_preserves_config = "passed"

    Invoke-PackageScript -ScriptPath $uninstallScript -Parameters @{
        InstallRoot = $installRoot
        StartupValueName = $startupValueName
    }
    if (Test-Path -LiteralPath $installRoot) {
        throw "Uninstall left the install directory behind"
    }
    if ($null -ne (Get-RunValue -Name $startupValueName)) {
        throw "Uninstall left the startup registration behind"
    }
    if ((Get-Content -LiteralPath $configPath -Raw).Trim() -cne $configContents.Trim()) {
        throw "Uninstall removed or changed the user configuration"
    }
    if ((Get-Content -LiteralPath $credentialSentinel -Raw).Trim() -cne $credentialContents.Trim()) {
        throw "Uninstall removed or changed provider-owned data"
    }
    $summary.checks.uninstall_preserves_user_data = "passed"
    $summary.result = "passed"
    $summary | ConvertTo-Json -Depth 8
} finally {
    if ($null -ne $runningProcess) {
        try {
            Stop-Process -Id $runningProcess.Id -Force -ErrorAction SilentlyContinue
        } catch {
            # Best effort cleanup after a failed startup assertion.
        }
    }
    $startupValue = Get-RunValue -Name $startupValueName
    if ($null -ne $startupValue -and $startupValue.Trim('"') -ieq $shellPath.Trim('"')) {
        Remove-ItemProperty -LiteralPath $runKey -Name $startupValueName -ErrorAction SilentlyContinue
    }
    foreach ($name in $environmentNames) {
        [Environment]::SetEnvironmentVariable($name, $savedEnvironment[$name], "Process")
    }
    if (-not $KeepSandbox -and (Test-Path -LiteralPath $SandboxPath)) {
        Remove-Item -LiteralPath $SandboxPath -Recurse -Force
    }
}
