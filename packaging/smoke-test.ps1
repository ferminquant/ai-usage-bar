[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$PackagePath,
    [string]$SandboxPath = "",
    [string]$SummaryPath = "",
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
if ((Get-Item -LiteralPath $PackagePath).PSIsContainer -eq $false) {
    if ([IO.Path]::GetExtension($PackagePath) -ine ".zip") {
        throw "PackagePath must be a package directory or .zip archive"
    }
    $packageRoot = Join-Path $SandboxPath "package"
    Expand-Archive -LiteralPath $PackagePath -DestinationPath $packageRoot -Force
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
$cliPath = Join-Path $installRoot "ai-usage-bar.exe"
$shellPath = Join-Path $installRoot "ai-usage-bar-shell.exe"
$cliStdoutPath = Join-Path $SandboxPath "cli.stdout.log"
$cliStderrPath = Join-Path $SandboxPath "cli.stderr.log"
$shellStdoutPath = Join-Path $SandboxPath "shell.stdout.log"
$shellStderrPath = Join-Path $SandboxPath "shell.stderr.log"
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
    $output = @(& $ScriptPath @Parameters)
    Write-Verbose ("{0}: {1}" -f [IO.Path]::GetFileName($ScriptPath), ($output -join [Environment]::NewLine))
}

function Invoke-ExpectedPackageFailure {
    param(
        [string]$ScriptPath,
        [hashtable]$Parameters,
        [string]$ExpectedMessage,
        [string]$ExpectedWarning = ""
    )
    $failure = $null
    $scriptWarnings = @()
    try {
        $output = @(& $ScriptPath @Parameters -WarningVariable scriptWarnings)
        Write-Verbose ("{0}: {1}" -f [IO.Path]::GetFileName($ScriptPath), ($output -join [Environment]::NewLine))
    } catch {
        $failure = $_
    }
    if ($null -eq $failure) {
        throw "Expected package script to fail: $ExpectedMessage"
    }
    if ($failure.Exception.Message -notmatch [regex]::Escape($ExpectedMessage)) {
        throw "Package script failed for an unexpected reason: $($failure.Exception.Message)"
    }
    if (-not [string]::IsNullOrWhiteSpace($ExpectedWarning) -and
        (($scriptWarnings | ForEach-Object { $_.Message }) -join [Environment]::NewLine) -notmatch [regex]::Escape($ExpectedWarning)) {
        throw "Package script did not report the expected recovery warning: $ExpectedWarning"
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

    Set-Content -LiteralPath $configPath -Value "{`n" -Encoding ASCII
    Remove-Item -LiteralPath $cliStdoutPath, $cliStderrPath -Force -ErrorAction SilentlyContinue
    $cliProcess = Start-Process -FilePath $cliPath -PassThru -Wait `
        -RedirectStandardOutput $cliStdoutPath -RedirectStandardError $cliStderrPath
    $cliError = if (Test-Path -LiteralPath $cliStderrPath) {
        (Get-Content -LiteralPath $cliStderrPath -Raw).Trim()
    } else {
        ""
    }
    if ($cliProcess.ExitCode -eq 0 -or $cliError -notmatch "provider config is invalid JSON") {
        throw "The installed CLI did not read the isolated config path"
    }
    Set-Content -LiteralPath $configPath -Value $configContents -Encoding ASCII
    $summary.checks.config_path_is_read = "passed"

    Remove-Item -LiteralPath $shellStdoutPath, $shellStderrPath -Force -ErrorAction SilentlyContinue
    $runningProcess = Start-Process -FilePath $shellPath -PassThru -WindowStyle Hidden `
        -RedirectStandardOutput $shellStdoutPath -RedirectStandardError $shellStderrPath
    Start-Sleep -Seconds 4
    if ($runningProcess.HasExited) {
        $stderr = if (Test-Path -LiteralPath $shellStderrPath) {
            (Get-Content -LiteralPath $shellStderrPath -Raw).Trim()
        } else {
            ""
        }
        throw "Installed shell exited during startup smoke test with code $($runningProcess.ExitCode); stderr: $stderr"
    }
    Stop-Process -Id $runningProcess.Id -Force
    $runningProcess = $null
    $summary.checks.shell_startup = "passed"

    $beforeUpgradeConfig = (Get-Content -LiteralPath $configPath -Raw).Trim()
    $beforeUpgradeState = Get-Content -LiteralPath (Join-Path $installRoot "install-state.json") -Raw |
        ConvertFrom-Json
    $beforeUpgradeStartup = Get-RunValue -Name $startupValueName
    $installParent = Split-Path -Parent $installRoot
    $installLeaf = Split-Path -Leaf $installRoot
    $upgradePackageRoot = Join-Path $SandboxPath "upgrade-package"
    Copy-Item -LiteralPath $packageRoot -Destination $upgradePackageRoot -Recurse -Force
    $upgradeManifestPath = Join-Path $upgradePackageRoot "package-manifest.json"
    $upgradeManifest = Get-Content -LiteralPath $upgradeManifestPath -Raw | ConvertFrom-Json
    $upgradeVersion = "{0}-upgrade" -f $upgradeManifest.version
    $upgradeManifest.version = $upgradeVersion
    $upgradeManifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $upgradeManifestPath -Encoding UTF8
    $upgradeManifestHash = (Get-FileHash -LiteralPath $upgradeManifestPath -Algorithm SHA256).Hash.ToLowerInvariant()
    $upgradeChecksumsPath = Join-Path $upgradePackageRoot "checksums.sha256"
    $upgradeChecksumLines = foreach ($line in Get-Content -LiteralPath $upgradeChecksumsPath) {
        if ($line -match "^\s*[0-9a-fA-F]{64}\s+package-manifest\.json\s*$") {
            "{0}  package-manifest.json" -f $upgradeManifestHash
        } else {
            $line
        }
    }
    $upgradeChecksumLines | Set-Content -LiteralPath $upgradeChecksumsPath -Encoding ASCII

    Invoke-ExpectedPackageFailure -ScriptPath (Join-Path $upgradePackageRoot "install.ps1") -Parameters @{
        PackageRoot = $upgradePackageRoot
        InstallRoot = $installRoot
        StartupValueName = $startupValueName
        TestFailureMode = "after-swap"
    } -ExpectedMessage "Synthetic install failure after swap"
    $afterRollbackState = Get-Content -LiteralPath (Join-Path $installRoot "install-state.json") -Raw |
        ConvertFrom-Json
    if ($afterRollbackState.version -cne $beforeUpgradeState.version) {
        throw "Rollback did not restore the previous package version"
    }
    $summary.checks.rollback_recovery = "passed"

    Invoke-ExpectedPackageFailure -ScriptPath (Join-Path $upgradePackageRoot "install.ps1") -Parameters @{
        PackageRoot = $upgradePackageRoot
        InstallRoot = $installRoot
        StartupValueName = $startupValueName
        TestFailureMode = "after-startup"
    } -ExpectedMessage "Synthetic install failure after startup registration"
    $afterStartupRollbackValue = Get-RunValue -Name $startupValueName
    if ([string]::IsNullOrWhiteSpace($afterStartupRollbackValue) -or
        $afterStartupRollbackValue.Trim('"') -ine $beforeUpgradeStartup.Trim('"')) {
        throw "Rollback did not restore the previous startup registration"
    }
    $summary.checks.startup_rollback_recovery = "passed"

    Invoke-ExpectedPackageFailure -ScriptPath (Join-Path $upgradePackageRoot "install.ps1") -Parameters @{
        PackageRoot = $upgradePackageRoot
        InstallRoot = $installRoot
        StartupValueName = $startupValueName
        TestFailureMode = "after-quarantine-blocked"
    } -ExpectedMessage "Synthetic install failure after swap" `
        -ExpectedWarning "Could not quarantine the partial installation"
    $blockedQuarantineBackup = @(Get-ChildItem -LiteralPath $installParent -Directory -Filter ("{0}.__backup_*" -f $installLeaf))
    if ($blockedQuarantineBackup.Count -ne 1) {
        throw "Expected one preserved backup after quarantine failure"
    }
    Remove-Item -LiteralPath $installRoot -Recurse -Force
    Move-Item -LiteralPath $blockedQuarantineBackup[0].FullName -Destination $installRoot
    $summary.checks.quarantine_failure_warning = "passed"

    Invoke-ExpectedPackageFailure -ScriptPath (Join-Path $upgradePackageRoot "install.ps1") -Parameters @{
        PackageRoot = $upgradePackageRoot
        InstallRoot = $installRoot
        StartupValueName = $startupValueName
        TestFailureMode = "after-restore-blocked"
    } -ExpectedMessage "Synthetic install failure after swap" `
        -ExpectedWarning "Could not restore the previous installation"
    $blockedRestoreBackup = @(Get-ChildItem -LiteralPath $installParent -Directory -Filter ("{0}.__backup_*" -f $installLeaf))
    if ($blockedRestoreBackup.Count -ne 1) {
        throw "Expected one preserved backup after restore failure"
    }
    Move-Item -LiteralPath $blockedRestoreBackup[0].FullName -Destination $installRoot
    $summary.checks.restore_failure_warning = "passed"

    Invoke-ExpectedPackageFailure -ScriptPath (Join-Path $upgradePackageRoot "install.ps1") -Parameters @{
        PackageRoot = $upgradePackageRoot
        InstallRoot = $installRoot
        StartupValueName = $startupValueName
        TestFailureMode = "after-swap-cleanup-blocked"
    } -ExpectedMessage "Synthetic install failure after swap"
    $afterQuarantineState = Get-Content -LiteralPath (Join-Path $installRoot "install-state.json") -Raw |
        ConvertFrom-Json
    if ($afterQuarantineState.version -cne $beforeUpgradeState.version) {
        throw "Quarantine recovery did not restore the previous package version"
    }
    $failedRoots = @(Get-ChildItem -LiteralPath $installParent -Directory -Filter ("{0}.__failed_*" -f $installLeaf))
    if ($failedRoots.Count -ne 1) {
        throw "Expected one quarantined failed install directory"
    }
    Remove-Item -LiteralPath $failedRoots[0].FullName -Recurse -Force
    $summary.checks.quarantine_recovery = "passed"

    Invoke-PackageScript -ScriptPath (Join-Path $upgradePackageRoot "install.ps1") -Parameters @{
        PackageRoot = $upgradePackageRoot
        InstallRoot = $installRoot
        StartupValueName = $startupValueName
    }
    $afterUpgradeConfig = (Get-Content -LiteralPath $configPath -Raw).Trim()
    if ($afterUpgradeConfig -cne $beforeUpgradeConfig) {
        throw "Upgrade changed the user configuration"
    }
    $afterUpgradeState = Get-Content -LiteralPath (Join-Path $installRoot "install-state.json") -Raw |
        ConvertFrom-Json
    if ($afterUpgradeState.version -ne $upgradeVersion -or
        $afterUpgradeState.version -eq $beforeUpgradeState.version -or
        -not (Test-Path -LiteralPath (Join-Path $installRoot "ai-usage-bar-shell.exe") -PathType Leaf) -or
        -not (Test-Path -LiteralPath (Join-Path $installRoot "ai-usage-bar.exe") -PathType Leaf)) {
        throw "Upgrade did not install the new package version"
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
    $summaryJson = $summary | ConvertTo-Json -Depth 8
    if (-not [string]::IsNullOrWhiteSpace($SummaryPath)) {
        $summaryParent = Split-Path -Parent $SummaryPath
        if (-not [string]::IsNullOrWhiteSpace($summaryParent)) {
            New-Item -ItemType Directory -Path $summaryParent -Force | Out-Null
        }
        $summaryJson | Set-Content -LiteralPath $SummaryPath -Encoding UTF8
    }
    $summaryJson
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
