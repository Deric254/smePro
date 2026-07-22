#!/usr/bin/env bash
# Builds a real, installable .dmg (macOS) or .AppImage/.deb (Linux).
# The finished installer will be in:
#   src-tauri/target/release/bundle/
#
# Run with: ./build-installer.sh

cd "$(dirname "$0")"
chmod +x scripts/setup.sh
./scripts/setup.sh --build
echo
echo "Done. Look in src-tauri/target/release/bundle/ for the installer."
