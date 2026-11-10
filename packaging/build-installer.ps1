#Requires -Version 5.1
<#
.SYNOPSIS
    Build the typors Windows installer (Inno Setup).

.DESCRIPTION
    Compiles the typors crate in release mode, then compiles packaging\typors.iss
    with ISCC into <OutDir>.

    Signing is opt-in via environment variables. When TYPORS_SIGN_CERT is unset
    (the default) the produced installer is unsigned and no signing tool is
    required. When it is set, the installer is signed with signtool.exe.

.PARAMETER SkipPayloadBuild
    Do not run `cargo build --release -p typors-app`; use the existing typors.exe.

.PARAMETER Version
    Override the version. Defaults to `version` in the workspace Cargo.toml.

.PARAMETER OutDir
    Output directory. Defaults to packaging\dist.

.PARAMETER IsccPath
    Path to ISCC.exe. Defaults to auto-discovery (Inno Setup 6 standard locations).

.PARAMETER PayloadPath
    Payload executable to package. Defaults to typors\target\release\typors.exe.

.EXAMPLE
    pwsh -File packaging\build-installer.ps1

.EXAMPLE
    pwsh -File packaging\build-installer.ps1 -SkipPayloadBuild -Version 0.2.0
#>
[CmdletBinding()]
param(
    [switch]$SkipPayloadBuild,
    [string]$Version,
    [string]$OutDir,
    [string]$IsccPath,
    [string]$PayloadPath
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$scriptDir = $PSScriptRoot
$repoRoot = Split-Path -Parent $scriptDir
$crateDir = $repoRoot
$manifest = Join-Path $crateDir 'Cargo.toml'
$payload = Join-Path $repoRoot 'target\release\typors.exe'
$issFile = Join-Path $scriptDir 'typors.iss'

if (-not $OutDir) { $OutDir = Join-Path $scriptDir 'dist' }
if (-not (Test-Path $OutDir)) { New-Item -ItemType Directory -Path $OutDir -Force | Out-Null }
$OutDir = (Resolve-Path -LiteralPath $OutDir).Path

# An explicit payload makes the crate build pointless, so it wins over the default.
if ($PayloadPath) {
    if (-not (Test-Path -LiteralPath $PayloadPath)) {
        throw "Payload not found at -PayloadPath '$PayloadPath'."
    }
    $payload = (Resolve-Path -LiteralPath $PayloadPath).Path
    $SkipPayloadBuild = [switch]$true
}

function Invoke-Native {
    <#
        Runs an external tool and returns its exit code.
        Windows PowerShell turns a native command's stderr into a *terminating*
        error while $ErrorActionPreference is Stop, so a harmless cargo warning
        would abort the build. Relax it for the call and judge by exit code.
    #>
    param(
        [Parameter(Mandatory)][string]$FilePath,
        [string[]]$Arguments = @()
    )

    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & $FilePath @Arguments | Out-Host
        return $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previous
    }
}

function Resolve-Iscc {
    param([string]$Explicit)

    if ($Explicit) {
        if (-not (Test-Path -LiteralPath $Explicit)) {
            throw "ISCC.exe not found at -IsccPath '$Explicit'."
        }
        return (Resolve-Path -LiteralPath $Explicit).Path
    }

    $candidates = [System.Collections.Generic.List[string]]::new()
    if ($env:LOCALAPPDATA) { $candidates.Add((Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe')) }
    $pf86 = [Environment]::GetEnvironmentVariable('ProgramFiles(x86)')
    if ($pf86) { $candidates.Add((Join-Path $pf86 'Inno Setup 6\ISCC.exe')) }
    if ($env:ProgramFiles) { $candidates.Add((Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe')) }

    foreach ($c in $candidates) {
        if (Test-Path -LiteralPath $c) { return (Resolve-Path -LiteralPath $c).Path }
    }
    $cmd = Get-Command -Name 'ISCC.exe' -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }

    throw 'ISCC.exe not found. Install Inno Setup 6 (e.g. winget install --id JRSoftware.InnoSetup -e --scope user) or pass -IsccPath.'
}

function Get-ManifestVersion {
    param([string]$ManifestPath)

    if (-not (Test-Path -LiteralPath $ManifestPath)) { return $null }
    $line = Select-String -LiteralPath $ManifestPath -Pattern '^\s*version\s*=\s*"([^"]+)"' |
        Select-Object -First 1
    if ($line) { return $line.Matches[0].Groups[1].Value }
    return $null
}

function Resolve-SignTool {
    $cmd = Get-Command -Name 'signtool.exe' -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }

    $roots = @()
    if ($env:ProgramFiles) { $roots += (Join-Path $env:ProgramFiles 'Windows Kits\10\bin') }
    $pf86 = [Environment]::GetEnvironmentVariable('ProgramFiles(x86)')
    if ($pf86) { $roots += (Join-Path $pf86 'Windows Kits\10\bin') }

    foreach ($root in $roots) {
        if (-not (Test-Path -LiteralPath $root)) { continue }
        $hit = Get-ChildItem -LiteralPath $root -Directory -ErrorAction SilentlyContinue |
            Sort-Object Name -Descending |
            ForEach-Object { Join-Path $_.FullName 'x64\signtool.exe' } |
            Where-Object { Test-Path -LiteralPath $_ } |
            Select-Object -First 1
        if ($hit) { return (Resolve-Path -LiteralPath $hit).Path }
    }
    return $null
}

Write-Host "== typors installer build ==" -ForegroundColor Cyan
Write-Host "repo root : $repoRoot"
Write-Host "script    : $issFile"
Write-Host "out dir   : $OutDir"

if (-not (Test-Path -LiteralPath $issFile)) { throw "Installer script missing: $issFile" }

# --- 1. payload build -------------------------------------------------------
if ($SkipPayloadBuild) {
    Write-Host "`n-- payload: build skipped (-SkipPayloadBuild)" -ForegroundColor Yellow
} else {
    if (-not (Test-Path -LiteralPath $manifest)) {
        throw "Product crate not found: $manifest. The typors crate must exist before an installer can be built (or pass -SkipPayloadBuild once typors.exe exists)."
    }
    Write-Host "`n-- payload: cargo build --release (in $crateDir)" -ForegroundColor Cyan
    Push-Location -LiteralPath $crateDir
    try {
        $cargoExit = Invoke-Native -FilePath 'cargo' -Arguments @('build', '--release', '-p', 'typors-app')
        if ($cargoExit -ne 0) {
            throw "cargo build --release failed with exit code $cargoExit in $crateDir."
        }
    } finally {
        Pop-Location
    }
}

if (-not (Test-Path -LiteralPath $payload)) {
    throw "Payload missing: $payload. Build the typors crate (omit -SkipPayloadBuild) or produce a release binary first."
}
$payloadSize = (Get-Item -LiteralPath $payload).Length
Write-Host ("-- payload ok: {0} ({1:N2} MiB)" -f $payload, ($payloadSize / 1MB))

# --- 2. version ------------------------------------------------------------
if (-not $Version) {
    $Version = Get-ManifestVersion -ManifestPath $manifest
    if (-not $Version) {
        throw "Cannot determine version: pass -Version, or make sure $manifest declares a [package] version."
    }
    Write-Host "-- version: $Version (from Cargo.toml)"
} else {
    Write-Host "-- version: $Version (from -Version)"
}

# VersionInfoVersion is a fixed 4-part numeric resource; strip any pre-release
# suffix so a version like 0.2.0-rc1 does not break the resource compiler.
$versionQuad = $Version
if ($versionQuad -notmatch '^\d+(\.\d+){0,3}$') {
    $numeric = [regex]::Match($Version, '^\d+(\.\d+){0,3}').Value
    if (-not $numeric) { throw "Version '$Version' does not start with a numeric version; cannot build the file version resource." }
    $versionQuad = $numeric
}

# --- 3. compile the installer ---------------------------------------------
$iscc = Resolve-Iscc -Explicit $IsccPath
Write-Host "`n-- ISCC: $iscc" -ForegroundColor Cyan

$isccArgs = @(
    "/DAppVersion=$Version",
    "/DVersionQuad=$versionQuad",
    "/DPayloadFile=$payload",
    "/O$OutDir",
    $issFile
)
$resultingExe = Join-Path $OutDir ("typors-{0}-setup.exe" -f $Version)

$isccExit = Invoke-Native -FilePath $iscc -Arguments $isccArgs
if ($isccExit -ne 0) {
    throw "ISCC failed with exit code $isccExit. See the compiler output above."
}

if (-not (Test-Path -LiteralPath $resultingExe)) {
    throw "ISCC reported success but the installer was not produced at $resultingExe."
}
$installer = Get-Item -LiteralPath $resultingExe

# --- 4. optional signing ---------------------------------------------------
$signCert = $env:TYPORS_SIGN_CERT
if (-not $signCert) {
    Write-Host "`n-- signing: skipped (TYPORS_SIGN_CERT not set)" -ForegroundColor Yellow
} else {
    Write-Host "`n-- signing: enabled" -ForegroundColor Cyan
    $signtool = Resolve-SignTool
    if (-not $signtool) {
        throw 'TYPORS_SIGN_CERT is set but signtool.exe was not found. Install the Windows SDK, or unset TYPORS_SIGN_CERT to build an unsigned installer.'
    }
    if (-not (Test-Path -LiteralPath $signCert)) {
        throw "TYPORS_SIGN_CERT is set but the certificate file does not exist: $signCert"
    }

    $signArgs = @('sign', '/fd', 'sha256', '/f', $signCert)
    if ($env:TYPORS_SIGN_PASSWORD) { $signArgs += @('/p', $env:TYPORS_SIGN_PASSWORD) }
    if ($env:TYPORS_TIMESTAMP_URL) {
        $signArgs += @('/tr', $env:TYPORS_TIMESTAMP_URL, '/td', 'sha256')
    } else {
        Write-Warning 'TYPORS_TIMESTAMP_URL not set: the signature will not be timestamped and expires with the certificate.'
    }
    $signArgs += $installer.FullName

    Write-Host "-- signtool: $signtool"
    $signExit = Invoke-Native -FilePath $signtool -Arguments $signArgs
    if ($signExit -ne 0) {
        throw "signtool failed with exit code $signExit."
    }
    $installer = Get-Item -LiteralPath $resultingExe
}

# --- 5. report -------------------------------------------------------------
$hash = (Get-FileHash -LiteralPath $installer.FullName -Algorithm SHA256).Hash
Write-Host "`n== build succeeded ==" -ForegroundColor Green
Write-Host ("installer : {0}" -f $installer.FullName)
Write-Host ("size      : {0:N2} MiB ({1:N0} bytes)" -f ($installer.Length / 1MB), $installer.Length)
Write-Host ("version   : {0}" -f $Version)
Write-Host ("sha256    : {0}" -f $hash)
