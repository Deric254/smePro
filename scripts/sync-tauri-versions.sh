#!/usr/bin/env bash
# Keeps the Rust-side Tauri crates locked to whatever version the npm
# side already resolved to, instead of the two drifting apart (the
# actual cause of the "Found version mismatched Tauri packages" CI
# failure this replaces a hardcoded fix for). package-lock.json is the
# source of truth here — read the six @tauri-apps/* versions it
# already locked, map each to its Rust crate name, and pin Cargo.lock
# to exactly that version. Self-updating: the next time npm moves,
# this reads the new number automatically instead of needing a human
# to edit a hardcoded version again.
set -euo pipefail
cd "$(dirname "$0")/.."

node -e '
  const lock = require("./package-lock.json");
  const pkgs = lock.packages || {};
  const map = {
    "node_modules/@tauri-apps/api": "tauri",
    "node_modules/@tauri-apps/plugin-os": "tauri-plugin-os",
    "node_modules/@tauri-apps/plugin-process": "tauri-plugin-process",
    "node_modules/@tauri-apps/plugin-fs": "tauri-plugin-fs",
    "node_modules/@tauri-apps/plugin-updater": "tauri-plugin-updater",
    "node_modules/@tauri-apps/plugin-http": "tauri-plugin-http",
  };
  for (const [pkgPath, crate] of Object.entries(map)) {
    const version = pkgs[pkgPath] && pkgs[pkgPath].version;
    if (!version) {
      console.error(`Could not find a locked version for ${pkgPath} in package-lock.json`);
      process.exit(1);
    }
    console.log(`${crate} ${version}`);
  }
' | while read -r crate version; do
  echo "Pinning $crate to $version (matching npm's locked version)"
  (cd src-tauri && cargo update -p "$crate" --precise "$version")
done
