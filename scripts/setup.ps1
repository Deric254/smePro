# Automated setup for SME Pro on Windows.
#
# Usage (run in PowerShell, ideally as Administrator for the vcpkg/
# winget install steps):
#   .\scripts\setup.ps1              # install prerequisites only
#   .\scripts\setup.ps1 -Build       # install prerequisites AND build the installer
#   .\scripts\setup.ps1 -Dev         # install prerequisites AND launch dev mode
#
# Safe to re-run: every step checks whether it's already done first.

param(
    [switch]$Build,
    [switch]$Dev
)

$ErrorActionPreference = "Stop"
$RustMinMajor = 1
$RustMinMinor = 77

function Step($msg) { Write-Host "`n==> $msg" -ForegroundColor Cyan }
function Ok($msg) { Write-Host $msg -ForegroundColor Green }
function Warn($msg) { Write-Host $msg -ForegroundColor Yellow }
function Fail($msg) { Write-Host "ERROR: $msg" -ForegroundColor Red; exit 1 }

function Test-RustVersion {
    if (-not (Get-Command rustc -ErrorAction SilentlyContinue)) { return $false }
    $verString = (rustc --version) -split " " | Select-Object -Index 1
    $parts = $verString -split "\."
    $major = [int]$parts[0]
    $minor = [int]$parts[1]
    if ($major -gt $RustMinMajor) { return $true }
    if ($major -eq $RustMinMajor -and $minor -ge $RustMinMinor) { return $true }
    return $false
}

function Install-Rust {
    Step "Checking Rust toolchain"
    if (Test-RustVersion) {
        $v = (rustc --version) -split " " | Select-Object -Index 1
        Ok "Rust $v already installed and current enough."
        return
    }
    if (Get-Command rustc -ErrorAction SilentlyContinue) {
        $v = (rustc --version) -split " " | Select-Object -Index 1
        Warn "Found Rust $v, but Tauri needs >= $RustMinMajor.$RustMinMinor. Updating via rustup..."
        rustup update stable
    } else {
        Warn "Rust not found. Downloading rustup-init..."
        Invoke-WebRequest -Uri "https://win.rustup.rs/x86_64" -OutFile "$env:TEMP\rustup-init.exe"
        & "$env:TEMP\rustup-init.exe" -y
        $env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
    }
    if (-not (Test-RustVersion)) {
        Fail "Rust installation did not produce a version >= $RustMinMajor.$RustMinMinor."
    }
    $v = (rustc --version) -split " " | Select-Object -Index 1
    Ok "Rust $v installed."
}

function Test-Node {
    Step "Checking Node.js"
    if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
        Fail "Node.js not found. Install it from https://nodejs.org (v18 or newer), then re-run this script."
    }
    $major = [int]((node --version) -replace "v","" -split "\.")[0]
    if ($major -lt 18) {
        Fail "Node.js $(node --version) found, but v18+ is required. Update from https://nodejs.org."
    }
    Ok "Node.js $(node --version) found."
}

function Install-WindowsDeps {
    Step "Checking Windows build tools"

    $vsInstalled = Get-Command cl.exe -ErrorAction SilentlyContinue
    if (-not $vsInstalled) {
        Warn "Microsoft C++ Build Tools not detected on PATH."
        Warn "Install 'Desktop development with C++' from:"
        Warn "  https://visualstudio.microsoft.com/visual-cpp-build-tools/"
        Warn "Then restart PowerShell and re-run this script."
    } else {
        Ok "C++ Build Tools found."
    }

    Step "Checking WebView2 runtime"
    $webview2 = Get-ItemProperty "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" -ErrorAction SilentlyContinue
    if ($webview2) {
        Ok "WebView2 runtime found."
    } else {
        Warn "WebView2 runtime not detected (usually preinstalled on Windows 11 / updated Windows 10)."
        Warn "If the build fails complaining about WebView2, install it from:"
        Warn "  https://developer.microsoft.com/microsoft-edge/webview2/"
    }

    Step "Installing SQLCipher via vcpkg"
    if (-not (Get-Command vcpkg -ErrorAction SilentlyContinue)) {
        Fail "vcpkg not found. Install it first: https://github.com/microsoft/vcpkg#quick-start-windows"
    }
    vcpkg install sqlcipher:x64-windows
    $vcpkgRoot = (Get-Command vcpkg).Source | Split-Path
    $env:SQLCIPHER_LIB_DIR = "$vcpkgRoot\installed\x64-windows\lib"
    $env:SQLCIPHER_INCLUDE_DIR = "$vcpkgRoot\installed\x64-windows\include"
    Ok "SQLCipher installed. (Note: SQLCIPHER_LIB_DIR/INCLUDE_DIR are set for this session only —"
    Ok "add them to your permanent environment variables if you'll build outside this script.)"
}

function Install-NpmDeps {
    Step "Installing frontend dependencies (npm install)"
    npm install
    Ok "Frontend dependencies installed."
}

# ---- main ----
Install-Rust
Test-Node
Install-WindowsDeps
Install-NpmDeps

Step "Setup complete"
Ok "Everything needed to build SME Pro is installed."

if ($Build) {
    Step "Building the installer (npm run tauri build)"
    npm run tauri build
    Ok "Build complete — check src-tauri\target\release\bundle\ for the installer."
} elseif ($Dev) {
    Step "Launching dev mode (npm run tauri dev)"
    npm run tauri dev
} else {
    Write-Host ""
    Write-Host "Next steps:"
    Write-Host "  npm run tauri dev              # run the app in development mode"
    Write-Host "  npm run tauri build            # build a real installer"
    Write-Host "  .\scripts\setup.ps1 -Build      # do both in one go, next time"
}
