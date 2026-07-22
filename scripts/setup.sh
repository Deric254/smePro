#!/usr/bin/env bash
# Automated setup for SME Pro — installs everything needed to
# build the app, then optionally builds it.
#
# Usage:
#   ./scripts/setup.sh            # install prerequisites only
#   ./scripts/setup.sh --build    # install prerequisites AND build the installer
#   ./scripts/setup.sh --dev      # install prerequisites AND launch dev mode
#
# Safe to re-run: every step checks whether it's already done before
# doing anything, so running this again after a partial failure won't
# reinstall things that already succeeded.

set -euo pipefail

RUST_MIN_MAJOR=1
RUST_MIN_MINOR=77

c_green() { printf '\033[0;32m%s\033[0m\n' "$1"; }
c_yellow() { printf '\033[0;33m%s\033[0m\n' "$1"; }
c_red() { printf '\033[0;31m%s\033[0m\n' "$1"; }
step() { printf '\n\033[1;36m==> %s\033[0m\n' "$1"; }

fail() {
    c_red "ERROR: $1"
    exit 1
}

detect_os() {
    case "$(uname -s)" in
        Linux*)  echo "linux" ;;
        Darwin*) echo "macos" ;;
        *) fail "Unsupported OS for this script — Windows users should run scripts/setup.ps1 instead." ;;
    esac
}

detect_linux_pkg_manager() {
    if command -v apt-get >/dev/null 2>&1; then echo "apt"
    elif command -v dnf >/dev/null 2>&1; then echo "dnf"
    elif command -v pacman >/dev/null 2>&1; then echo "pacman"
    else fail "No supported package manager found (expected apt, dnf, or pacman)."
    fi
}

rust_version_ok() {
    if ! command -v rustc >/dev/null 2>&1; then
        return 1
    fi
    local ver major minor
    ver=$(rustc --version | awk '{print $2}')
    major=$(echo "$ver" | cut -d. -f1)
    minor=$(echo "$ver" | cut -d. -f2)
    if [ "$major" -gt "$RUST_MIN_MAJOR" ]; then return 0; fi
    if [ "$major" -eq "$RUST_MIN_MAJOR" ] && [ "$minor" -ge "$RUST_MIN_MINOR" ]; then return 0; fi
    return 1
}

install_rust() {
    step "Checking Rust toolchain"
    if rust_version_ok; then
        c_green "Rust $(rustc --version | awk '{print $2}') already installed and current enough."
        return
    fi

    if command -v rustc >/dev/null 2>&1; then
        c_yellow "Found Rust $(rustc --version | awk '{print $2}'), but Tauri needs >= ${RUST_MIN_MAJOR}.${RUST_MIN_MINOR}."
        c_yellow "This is exactly the trap this project's own build hit in an environment whose Rust"
        c_yellow "came from the OS package manager instead of rustup — updating now via rustup."
    else
        c_yellow "Rust not found. Installing via rustup..."
    fi

    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    # shellcheck disable=SC1090
    source "$HOME/.cargo/env"

    if ! rust_version_ok; then
        fail "Rust installation did not produce a version >= ${RUST_MIN_MAJOR}.${RUST_MIN_MINOR}. Check the rustup output above."
    fi
    c_green "Rust $(rustc --version | awk '{print $2}') installed."
}

check_node() {
    step "Checking Node.js"
    if ! command -v node >/dev/null 2>&1; then
        fail "Node.js not found. Install it from https://nodejs.org (v18 or newer), then re-run this script."
    fi
    local major
    major=$(node --version | sed 's/v//' | cut -d. -f1)
    if [ "$major" -lt 18 ]; then
        fail "Node.js $(node --version) found, but v18+ is required. Update from https://nodejs.org."
    fi
    c_green "Node.js $(node --version) found."
}

install_linux_deps() {
    step "Installing Linux system dependencies (Tauri + SQLCipher)"
    local pm
    pm=$(detect_linux_pkg_manager)

    case "$pm" in
        apt)
            sudo apt-get update
            sudo apt-get install -y \
                libwebkit2gtk-4.1-dev \
                libjavascriptcoregtk-4.1-dev \
                libsoup-3.0-dev \
                librsvg2-dev \
                libgtk-3-dev \
                libayatana-appindicator3-dev \
                libsqlcipher-dev \
                pkg-config \
                build-essential \
                curl \
                wget \
                file
            ;;
        dnf)
            sudo dnf install -y \
                webkit2gtk4.1-devel \
                openssl-devel \
                curl \
                wget \
                file \
                libappindicator-gtk3-devel \
                librsvg2-devel \
                sqlcipher-devel \
                gcc gcc-c++ make
            ;;
        pacman)
            sudo pacman -Sy --needed --noconfirm \
                webkit2gtk-4.1 \
                base-devel \
                curl \
                wget \
                file \
                openssl \
                appmenu-gtk-module \
                librsvg \
                sqlcipher
            ;;
    esac
    c_green "Linux system dependencies installed."
}

install_macos_deps() {
    step "Installing macOS dependencies"
    if ! xcode-select -p >/dev/null 2>&1; then
        c_yellow "Installing Xcode Command Line Tools (a system dialog will appear)..."
        xcode-select --install || true
        c_yellow "Re-run this script after the Xcode Command Line Tools install finishes."
        exit 0
    fi
    if ! command -v brew >/dev/null 2>&1; then
        fail "Homebrew not found. Install it from https://brew.sh, then re-run this script."
    fi
    brew install sqlcipher
    c_green "macOS dependencies installed."
}

install_npm_deps() {
    step "Installing frontend dependencies (npm install)"
    npm install
    c_green "Frontend dependencies installed."
}

main() {
    local os
    os=$(detect_os)
    c_green "Detected OS: $os"

    install_rust
    check_node

    if [ "$os" = "linux" ]; then
        install_linux_deps
    elif [ "$os" = "macos" ]; then
        install_macos_deps
    fi

    install_npm_deps

    step "Setup complete"
    c_green "Everything needed to build SME Pro is installed."

    case "${1:-}" in
        --build)
            step "Building the installer (npm run tauri build)"
            npm run tauri build
            c_green "Build complete — check src-tauri/target/release/bundle/ for the installer."
            ;;
        --dev)
            step "Launching dev mode (npm run tauri dev)"
            npm run tauri dev
            ;;
        "")
            echo ""
            echo "Next steps:"
            echo "  npm run tauri dev     # run the app in development mode"
            echo "  npm run tauri build   # build a real installer"
            echo "  ./scripts/setup.sh --build   # do both in one go, next time"
            ;;
        *)
            fail "Unknown option '$1'. Use --build, --dev, or no argument."
            ;;
    esac
}

main "$@"
