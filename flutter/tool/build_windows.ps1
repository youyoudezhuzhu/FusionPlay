#requires -Version 7.0
<#
.SYNOPSIS
    Builds the complete FusionPlay Windows release bundle locally (PowerShell 7).

.DESCRIPTION
    End-to-end local build, mirroring .github/workflows/windows.yml:

      1. reads VERSION (display version) and INSTALLER_VERSION (4-part numeric
         version, used by VERSIONINFO and Inno Setup) from the repository root;
      2. cargo build --release for the Rust sidecar (windows/backend);
      3. flutter pub get --enforce-lockfile + flutter build windows --release;
      4. copies FusionPlayCore.exe next to FusionPlay.exe in the Flutter release
         folder (the sidecar must live next to the UI executable);
      5. optionally builds installer/fusionplay.iss with Inno Setup 6;
      6. produces dist/FusionPlay-Windows-<ver>-x64.zip (Flutter release folder
         contents at the zip root, no extra wrapper directory) and .sha256 files
         for the zip and setup executable.

.PARAMETER RepositoryRoot
    Repository root. Defaults to two levels above flutter/tool/.

.PARAMETER Version
    Overrides the display version read from <RepositoryRoot>/VERSION.

.PARAMETER FlutterSdk
    Flutter SDK root (the folder containing bin/flutter.bat). Defaults to
    $env:FLUTTER_ROOT, then to the SDK that owns the flutter command on PATH.

.PARAMETER IsccPath
    Full path to ISCC.exe. Defaults to `iscc` on PATH and the standard Inno
    Setup 6 install locations.

.PARAMETER SkipInstaller
    Skips the Inno Setup step (zip/portable build only).

.PARAMETER OutputDirectory
    Where the zip/setup/sha256 artifacts are written. Defaults to <root>/dist.

.EXAMPLE
    pwsh -File flutter/tool/build_windows.ps1

.EXAMPLE
    pwsh -File flutter/tool/build_windows.ps1 -SkipInstaller
#>
[CmdletBinding()]
param(
    [string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path,
    [string]$Version,
    [string]$FlutterSdk = $env:FLUTTER_ROOT,
    [string]$IsccPath,
    [switch]$SkipInstaller,
    [string]$OutputDirectory
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Write-Step([string]$Message) {
    Write-Host ''
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Invoke-Checked([string]$Description, [scriptblock]$Action) {
    Write-Step $Description
    & $Action
    if ($LASTEXITCODE -ne 0) {
        throw "$Description failed with exit code $LASTEXITCODE."
    }
}

function Write-Sha256File([string]$FilePath) {
    $hash = (Get-FileHash -LiteralPath $FilePath -Algorithm SHA256).Hash.ToLowerInvariant()
    $line = "$hash  $([System.IO.Path]::GetFileName($FilePath))"
    $hashPath = "$FilePath.sha256"
    # UTF-8 without BOM and a trailing newline: same format as sha256sum.
    [System.IO.File]::WriteAllText(
        $hashPath,
        "$line`n",
        [System.Text.UTF8Encoding]::new($false)
    )
    Write-Host "    $hashPath"
}

if ($env:OS -ne 'Windows_NT') {
    throw 'This script targets Windows. Run it with PowerShell 7 on Windows.'
}
if ($PSVersionTable.PSVersion.Major -lt 7) {
    throw "PowerShell 7 or newer is required (found $($PSVersionTable.PSVersion))."
}

$repositoryRoot = [System.IO.Path]::GetFullPath($RepositoryRoot)
if (-not (Test-Path -LiteralPath (Join-Path $repositoryRoot 'VERSION'))) {
    throw "Repository root not found or missing VERSION: $repositoryRoot"
}
if ($null -eq $OutputDirectory -or $OutputDirectory -eq '') {
    $OutputDirectory = Join-Path $repositoryRoot 'dist'
}
$OutputDirectory = [System.IO.Path]::GetFullPath($OutputDirectory)
[System.IO.Directory]::CreateDirectory($OutputDirectory) | Out-Null

# ---------------------------------------------------------------------------
# Versions (repository root is the single source of truth).
# ---------------------------------------------------------------------------
$repositoryVersion = (Get-Content -LiteralPath (Join-Path $repositoryRoot 'VERSION') -Raw).Trim()
$displayVersion = if ($null -ne $Version -and $Version -ne '') {
    $Version.Trim()
} else {
    $repositoryVersion
}
if ($displayVersion -notmatch '^\d+\.\d+\.\d+$') {
    throw "VERSION must be MAJOR.MINOR.PATCH, got '$displayVersion'."
}

# INSTALLER_VERSION is the repository-wide 4-part version. When -Version
# overrides the display version, derive the numeric version from it instead so
# the zip name, VERSIONINFO and installer never disagree.
$installerVersionPath = Join-Path $repositoryRoot 'INSTALLER_VERSION'
$installerVersion = if ($displayVersion -eq $repositoryVersion -and
    (Test-Path -LiteralPath $installerVersionPath)) {
    (Get-Content -LiteralPath $installerVersionPath -Raw).Trim()
} else {
    "$displayVersion.0"
}
if ($installerVersion -notmatch '^\d+\.\d+\.\d+\.\d+$') {
    throw "INSTALLER_VERSION must be a 4-part numeric version, got '$installerVersion'."
}

# ---------------------------------------------------------------------------
# Toolchain discovery.
# ---------------------------------------------------------------------------
if ($null -eq $FlutterSdk -or $FlutterSdk -eq '') {
    $flutterOnPath = Get-Command 'flutter.bat' -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($null -ne $flutterOnPath) {
        $FlutterSdk = Split-Path (Split-Path $flutterOnPath.Source -Parent) -Parent
    }
}
if ($null -eq $FlutterSdk -or $FlutterSdk -eq '') {
    throw 'Flutter SDK not found: set FLUTTER_ROOT or put flutter.bat on PATH.'
}
$FlutterSdk = [System.IO.Path]::GetFullPath($FlutterSdk)
$flutter = Join-Path $FlutterSdk 'bin\flutter.bat'
if (-not (Test-Path -LiteralPath $flutter)) {
    throw "Flutter SDK is incomplete (missing bin\flutter.bat): $FlutterSdk"
}

$cargoCommand = Get-Command 'cargo.exe' -ErrorAction SilentlyContinue |
    Select-Object -First 1
$cargo = if ($null -ne $cargoCommand) { $cargoCommand.Source } else { $null }
if ($null -eq $cargo) {
    throw 'cargo.exe not found on PATH; install the Rust stable toolchain first.'
}

# `cargo build` runs with the repository root as the working directory, so a
# relative CARGO_TARGET_DIR resolves against the repository root.
$cargoTargetDirectory = if ($env:CARGO_TARGET_DIR) {
    if ([System.IO.Path]::IsPathRooted($env:CARGO_TARGET_DIR)) {
        [System.IO.Path]::GetFullPath($env:CARGO_TARGET_DIR)
    } else {
        [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $env:CARGO_TARGET_DIR))
    }
} else {
    Join-Path $repositoryRoot 'windows\backend\target'
}
$sidecarExecutable = Join-Path $cargoTargetDirectory 'release\FusionPlayCore.exe'

$flutterDirectory = Join-Path $repositoryRoot 'flutter'
$releaseDirectory = Join-Path $flutterDirectory 'build\windows\x64\runner\Release'

Write-Host "Repository:        $repositoryRoot"
Write-Host "Version:           $displayVersion (VERSIONINFO $installerVersion)"
Write-Host "Flutter SDK:       $FlutterSdk"
Write-Host "Cargo:             $cargo"
Write-Host "Output directory:  $OutputDirectory"

# ---------------------------------------------------------------------------
# Rust sidecar.
# ---------------------------------------------------------------------------
Push-Location $repositoryRoot
try {
    Invoke-Checked 'Building FusionPlayCore.exe (cargo build --release)' {
        & $cargo build --release --manifest-path windows/backend/Cargo.toml
    }
    if (-not (Test-Path -LiteralPath $sidecarExecutable)) {
        throw "FusionPlayCore.exe was not produced at $sidecarExecutable."
    }
} finally {
    Pop-Location
}

# ---------------------------------------------------------------------------
# Flutter Windows application.
# ---------------------------------------------------------------------------
Push-Location $flutterDirectory
try {
    Invoke-Checked 'Resolving Flutter dependencies (flutter pub get)' {
        & $flutter pub get --enforce-lockfile
    }
    Invoke-Checked 'Building FusionPlay.exe (flutter build windows --release)' {
        & $flutter build windows --release --no-pub `
            "--dart-define=FUSIONPLAY_VERSION=$displayVersion"
    }
} finally {
    Pop-Location
}

$appExecutable = Join-Path $releaseDirectory 'FusionPlay.exe'
if (-not (Test-Path -LiteralPath $appExecutable)) {
    throw "FusionPlay.exe was not produced at $appExecutable."
}
if (-not (Test-Path -LiteralPath $releaseDirectory)) {
    throw "Flutter release directory not found: $releaseDirectory"
}

Write-Step 'Staging FusionPlayCore.exe next to FusionPlay.exe'
Copy-Item -LiteralPath $sidecarExecutable `
    -Destination (Join-Path $releaseDirectory 'FusionPlayCore.exe') -Force
if (-not (Test-Path -LiteralPath (Join-Path $releaseDirectory 'FusionPlayCore.exe'))) {
    throw 'Failed to stage FusionPlayCore.exe in the release directory.'
}

# ---------------------------------------------------------------------------
# Zip bundle: contents at the archive root (FusionPlay.exe, data\, ...).
# ---------------------------------------------------------------------------
$zipName = "FusionPlay-Windows-$displayVersion-x64.zip"
$zipPath = Join-Path $OutputDirectory $zipName
Write-Step "Creating $zipName"
if (Test-Path -LiteralPath $zipPath) {
    Remove-Item -LiteralPath $zipPath -Force
}
Compress-Archive -Path (Join-Path $releaseDirectory '*') `
    -DestinationPath $zipPath -CompressionLevel Optimal -Force

Write-Step 'Verifying zip layout'
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [System.IO.Compression.ZipFile]::OpenRead($zipPath)
try {
    $entryNames = @($zip.Entries | ForEach-Object { $_.FullName })
} finally {
    $zip.Dispose()
}
foreach ($required in @('FusionPlay.exe', 'FusionPlayCore.exe', 'flutter_windows.dll')) {
    if ($entryNames -notcontains $required) {
        throw "Zip is missing required root entry '$required'. Entries: $($entryNames -join ', ')"
    }
}
if (-not ($entryNames | Where-Object { $_ -like 'data/*' })) {
    throw 'Zip is missing the data/ directory (Flutter assets).'
}
if ($entryNames | Where-Object { $_ -like 'Release/*' -or $_ -like 'runner/*' }) {
    throw 'Zip has an unexpected wrapper directory; contents must be at the archive root.'
}
Write-Host "    $zipPath ($($entryNames.Count) entries)"
Write-Sha256File $zipPath

# ---------------------------------------------------------------------------
# Inno Setup installer (optional).
# ---------------------------------------------------------------------------
$setupExecutable = $null
if (-not $SkipInstaller) {
    $installerScript = Join-Path $repositoryRoot 'installer\fusionplay.iss'
    if (-not (Test-Path -LiteralPath $installerScript)) {
        throw "Inno Setup script not found: $installerScript (use -SkipInstaller to skip)."
    }
    if ($null -eq $IsccPath -or $IsccPath -eq '') {
        $isccCommand = Get-Command 'iscc.exe' -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if ($null -ne $isccCommand) {
            $IsccPath = $isccCommand.Source
        } else {
            $isccCandidates = @()
            if (${env:ProgramFiles(x86)}) {
                $isccCandidates += Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'
            }
            if ($env:ProgramFiles) {
                $isccCandidates += Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe'
            }
            foreach ($candidate in $isccCandidates) {
                if (Test-Path -LiteralPath $candidate) {
                    $IsccPath = $candidate
                    break
                }
            }
        }
    }
    if ($null -eq $IsccPath -or -not (Test-Path -LiteralPath $IsccPath)) {
        throw 'ISCC.exe (Inno Setup 6) not found: install Inno Setup or pass -IsccPath, or use -SkipInstaller.'
    }

    # /O<dir> keeps all setup outputs in dist/; the .iss file resolves its
    # [Files] Source relative to the script location (installer\..\flutter\...).
    Invoke-Checked 'Building the Inno Setup installer (iscc)' {
        & $IsccPath "/DMyAppVersion=$installerVersion" `
            "/DMyAppDisplayVersion=$displayVersion" `
            "/O$OutputDirectory" $installerScript
    }
    $setupExecutable = Join-Path $OutputDirectory "FusionPlay-Setup-$displayVersion-x64.exe"
    if (-not (Test-Path -LiteralPath $setupExecutable)) {
        throw "Inno Setup did not produce $setupExecutable."
    }
    Write-Sha256File $setupExecutable
}

# ---------------------------------------------------------------------------
# Summary.
# ---------------------------------------------------------------------------
Write-Host ''
Write-Host 'FusionPlay Windows build complete:' -ForegroundColor Green
Get-ChildItem -LiteralPath $OutputDirectory -File |
    Sort-Object Name |
    ForEach-Object {
        Write-Host ("  {0,-56} {1,10:N0} bytes" -f $_.Name, $_.Length)
    }
