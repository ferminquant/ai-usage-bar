[CmdletBinding()]
param(
    [string]$Version = "",
    [string]$OutputDirectory = "",
    [string]$TargetDirectory = "",
    [switch]$SkipBuild,
    [string]$SignToolPath = "",
    [string]$CertificateThumbprint = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = (Resolve-Path (Join-Path $scriptRoot "..")).Path
if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = Join-Path $repoRoot "dist"
}
if ([string]::IsNullOrWhiteSpace($TargetDirectory)) {
    $TargetDirectory = Join-Path $repoRoot "target\release"
}

function Get-CargoVersion {
    $cargoToml = Join-Path $repoRoot "Cargo.toml"
    $versionLine = Get-Content -LiteralPath $cargoToml |
        Where-Object { $_ -match '^\s*version\s*=\s*"([^"]+)"' } |
        Select-Object -First 1
    if ($null -eq $versionLine -or $versionLine -notmatch '^\s*version\s*=\s*"([^"]+)"') {
        throw "Cargo.toml does not contain a package version"
    }
    return $matches[1]
}

function Get-CommitId {
    if (-not [string]::IsNullOrWhiteSpace($env:GITHUB_SHA)) {
        return $env:GITHUB_SHA
    }
    $gitCommand = Get-Command git -ErrorAction SilentlyContinue
    if ($null -eq $gitCommand) {
        return "unknown"
    }
    try {
        $output = & $gitCommand.Source -C $repoRoot rev-parse --short=12 HEAD 2>$null
        if ($LASTEXITCODE -eq 0 -and $null -ne $output) {
            return ($output | Select-Object -First 1).Trim()
        }
    } catch {
        return "unknown"
    }
    return "unknown"
}

function Get-FileRecord {
    param(
        [string]$Root,
        [string]$RelativePath
    )
    $path = Join-Path $Root ($RelativePath -replace "/", "\")
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Package file is missing: $RelativePath"
    }
    $item = Get-Item -LiteralPath $path
    $hash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    [ordered]@{
        path = $RelativePath
        bytes = [int64]$item.Length
        sha256 = $hash
    }
}

if ([string]::IsNullOrWhiteSpace($Version)) {
    $Version = Get-CargoVersion
}
$packageVersion = ($Version -replace "[^0-9A-Za-z._-]", "-")
if ([string]::IsNullOrWhiteSpace($packageVersion)) {
    throw "Version must contain at least one package-safe character"
}

if (($SignToolPath -and -not $CertificateThumbprint) -or
    ($CertificateThumbprint -and -not $SignToolPath)) {
    throw "SignToolPath and CertificateThumbprint must be supplied together"
}
$signingThumbprint = ($CertificateThumbprint -replace "\s", "").ToLowerInvariant()
$signingThumbprintOption = $null
if (-not [string]::IsNullOrWhiteSpace($signingThumbprint)) {
    if ($signingThumbprint -match "^[0-9a-f]{40}$") {
        $signingThumbprintOption = "/sha1"
    } elseif ($signingThumbprint -match "^[0-9a-f]{64}$") {
        $signingThumbprintOption = "/sha256"
    } else {
        throw "CertificateThumbprint must be a 40-character SHA-1 or 64-character SHA-256 thumbprint"
    }
}

if (-not $SkipBuild) {
    Push-Location $repoRoot
    try {
        & cargo build --release --locked --bin ai-usage-bar --bin ai-usage-bar-shell
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build failed with exit code $LASTEXITCODE"
        }
    } finally {
        Pop-Location
    }
}

New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
$stageRoot = Join-Path $OutputDirectory ("ai-usage-bar-{0}-windows-x64" -f $packageVersion)
if (Test-Path -LiteralPath $stageRoot) {
    Remove-Item -LiteralPath $stageRoot -Recurse -Force
}
New-Item -ItemType Directory -Path $stageRoot -Force | Out-Null

$binaryNames = @("ai-usage-bar.exe", "ai-usage-bar-shell.exe")
foreach ($binaryName in $binaryNames) {
    $binaryPath = Join-Path $TargetDirectory $binaryName
    if (-not (Test-Path -LiteralPath $binaryPath -PathType Leaf)) {
        throw "Release binary is missing: $binaryPath"
    }
    Copy-Item -LiteralPath $binaryPath -Destination (Join-Path $stageRoot $binaryName)
}

Copy-Item -LiteralPath (Join-Path $scriptRoot "install.ps1") -Destination (Join-Path $stageRoot "install.ps1")
Copy-Item -LiteralPath (Join-Path $scriptRoot "uninstall.ps1") -Destination (Join-Path $stageRoot "uninstall.ps1")
Copy-Item -LiteralPath (Join-Path $repoRoot "README.md") -Destination (Join-Path $stageRoot "README.md")

$signedFiles = @()
if (-not [string]::IsNullOrWhiteSpace($SignToolPath)) {
    foreach ($binaryName in $binaryNames) {
        $binaryPath = Join-Path $stageRoot $binaryName
        & $SignToolPath sign $signingThumbprintOption $signingThumbprint /fd SHA256 /tr "http://timestamp.digicert.com" /td SHA256 $binaryPath
        if ($LASTEXITCODE -ne 0) {
            throw "Authenticode signing failed for $binaryName with exit code $LASTEXITCODE"
        }
        $signedFiles += $binaryName
    }
}

$relativePayloadFiles = @(
    "ai-usage-bar.exe",
    "ai-usage-bar-shell.exe",
    "install.ps1",
    "uninstall.ps1",
    "README.md"
)
$payloadRecords = @(
    foreach ($relativePath in $relativePayloadFiles) {
        Get-FileRecord -Root $stageRoot -RelativePath $relativePath
    }
)

$manifest = [ordered]@{
    schema_version = 1
    product = "AI Usage Bar"
    version = $packageVersion
    platform = "windows"
    architecture = "x64"
    commit = Get-CommitId
    generated_at_utc = (Get-Date).ToUniversalTime().ToString("o")
    install_root = "%LOCALAPPDATA%\AI Usage Bar"
    config_path = "%APPDATA%\AI Usage Bar\config.json"
    startup_registry = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run\AI Usage Bar"
    signing = [ordered]@{
        mode = if ($signedFiles.Count -gt 0) { "authenticode" } else { "unsigned" }
        files = @($signedFiles)
        note = if ($signedFiles.Count -gt 0) {
            "Signed with the supplied certificate thumbprint."
        } else {
            "No signing certificate was supplied; verify and sign release artifacts before publication."
        }
    }
    files = $payloadRecords
}
$manifestPath = Join-Path $stageRoot "package-manifest.json"
$manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding UTF8

$checksumRecords = @($payloadRecords + (Get-FileRecord -Root $stageRoot -RelativePath "package-manifest.json"))
$checksumLines = foreach ($record in $checksumRecords) {
    "{0}  {1}" -f $record.sha256, $record.path
}
$checksumsPath = Join-Path $stageRoot "checksums.sha256"
$checksumLines | Set-Content -LiteralPath $checksumsPath -Encoding ASCII

$zipPath = Join-Path $OutputDirectory ("ai-usage-bar-{0}-windows-x64.zip" -f $packageVersion)
if (Test-Path -LiteralPath $zipPath) {
    Remove-Item -LiteralPath $zipPath -Force
}
Compress-Archive -Path (Join-Path $stageRoot "*") -DestinationPath $zipPath -CompressionLevel Optimal
$zipHash = (Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
$zipChecksumPath = "$zipPath.sha256"
"{0}  {1}" -f $zipHash, (Split-Path -Leaf $zipPath) |
    Set-Content -LiteralPath $zipChecksumPath -Encoding ASCII

[ordered]@{
    package = $zipPath
    staging_directory = $stageRoot
    manifest = $manifestPath
    checksums = $zipChecksumPath
    version = $packageVersion
    signing = if ($signedFiles.Count -gt 0) { "authenticode" } else { "unsigned" }
} | ConvertTo-Json -Depth 4
