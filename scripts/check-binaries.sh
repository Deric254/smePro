#!/usr/bin/env bash
# Fails fast if the crate would ship anything other than the app itself.
#
# Tauri's packager scans src-tauri/src/bin/ on its own and tries to bundle
# every program it finds there that is not guarded by a Cargo
# `required-features`. A stray file (for example a leftover dev tool whose
# [[bin]] entry was removed) therefore breaks every platform's installer
# build at the very last step, after 10+ minutes of compiling. This check
# catches that in seconds, with a message that says what to fix.
set -euo pipefail

CARGO_TOML="${1:-src-tauri/Cargo.toml}"
BIN_DIR="$(dirname "$CARGO_TOML")/src/bin"

python3 - "$CARGO_TOML" "$BIN_DIR" <<'PY'
import os, sys, tomllib

cargo_toml, bin_dir = sys.argv[1], sys.argv[2]
with open(cargo_toml, "rb") as f:
    manifest = tomllib.load(f)

errors = []
base = os.path.dirname(cargo_toml)
bins = manifest.get("bin", [])
declared = {os.path.normpath(os.path.join(base, b.get("path", ""))): b for b in bins}

# 1. Every file in src/bin/ must be a declared, dev-tools-guarded program.
if os.path.isdir(bin_dir):
    for name in sorted(os.listdir(bin_dir)):
        if not name.endswith(".rs"):
            continue
        entry = declared.get(os.path.normpath(os.path.join(bin_dir, name)))
        if entry is None:
            errors.append(
                f"{bin_dir}/{name} is not declared in Cargo.toml. Delete it "
                f"(git rm), or declare it with required-features = [\"dev-tools\"]."
            )
        elif "dev-tools" not in entry.get("required-features", []):
            errors.append(f"{bin_dir}/{name} must have required-features = [\"dev-tools\"].")

# 2. Exactly one unguarded program may ship, and it must be the app.
unguarded = [b["name"] for b in bins if "dev-tools" not in b.get("required-features", [])]
if unguarded != ["sme-pro"]:
    errors.append(f"Shipped programs must be exactly ['sme-pro'], found {unguarded}.")

# 3. Cargo must know which program is the default.
if manifest.get("package", {}).get("default-run") != "sme-pro":
    errors.append('[package] must set default-run = "sme-pro".')

if errors:
    print("Binary check FAILED:", *errors, sep="\n  - ")
    sys.exit(1)
print(f"Binary check passed: only 'sme-pro' ships ({len(bins) - 1} dev tools guarded by dev-tools).")
PY
