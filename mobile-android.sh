#!/usr/bin/env bash
# Sets up and runs/builds the Android version of SME Pro.
#
# Usage:
#   ./mobile-android.sh --dev     # run on a connected device/emulator
#   ./mobile-android.sh --build   # build a release APK/AAB
#
# REQUIRED FIRST (see MOBILE.md for full detail):
#   - Android Studio + SDK + NDK installed
#   - rustup target add aarch64-linux-android armv7-linux-androideabi \
#       i686-linux-android x86_64-linux-android
#   - ANDROID_HOME and NDK_HOME environment variables set (Android
#     Studio's SDK Manager shows you these paths)
#
# This script cannot install the Android SDK/NDK for you — that's a
# multi-GB interactive install through Android Studio's own installer,
# not something safe to script. It DOES handle everything after that.

set -euo pipefail
cd "$(dirname "$0")"

c_cyan() { printf '\033[1;36m==> %s\033[0m\n' "$1"; }
c_red() { printf '\033[0;31mERROR: %s\033[0m\n' "$1"; }

MODE="${1:-}"
if [ "$MODE" != "--dev" ] && [ "$MODE" != "--build" ]; then
    echo "Usage: ./mobile-android.sh --dev | --build"
    exit 1
fi

if [ -z "${ANDROID_HOME:-}" ]; then
    c_red "ANDROID_HOME is not set. Install Android Studio first, then set"
    echo "ANDROID_HOME to the SDK path it shows you (Settings > Languages &"
    echo "Frameworks > Android SDK). See MOBILE.md for the full checklist."
    exit 1
fi

if ! command -v rustc >/dev/null 2>&1; then
    c_red "Rust isn't installed. Run ./run.sh once first (it installs Rust)."
    exit 1
fi

c_cyan "Checking Android Rust targets"
for target in aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android; do
    if ! rustup target list --installed | grep -q "$target"; then
        echo "Installing missing target: $target"
        rustup target add "$target"
    fi
done

if [ ! -f package.json ]; then
    c_red "Run this from the project root (where package.json is)."
    exit 1
fi

if [ ! -d node_modules ]; then
    c_cyan "Installing frontend dependencies"
    npm install
fi

if [ ! -d src-tauri/gen/android ]; then
    c_cyan "Scaffolding the Android project (first time only)"
    npm run tauri android init

    c_cyan "Applying the network security config (127.0.0.1 loopback exception)"
    # Android blocks cleartext HTTP by default on API 28+. Our backend
    # talks plain HTTP to 127.0.0.1 (loopback inside the app's own
    # sandboxed process, never real network traffic) — this is exactly
    # the manual step MOBILE.md flags; automated here so it can't be
    # silently forgotten.
    XML_DIR="src-tauri/gen/android/app/src/main/res/xml"
    mkdir -p "$XML_DIR"
    cat > "$XML_DIR/network_security_config.xml" <<'EOF'
<?xml version="1.0" encoding="utf-8"?>
<network-security-config>
    <domain-config cleartextTrafficPermitted="true">
        <domain includeSubdomains="false">127.0.0.1</domain>
    </domain-config>
</network-security-config>
EOF

    MANIFEST="src-tauri/gen/android/app/src/main/AndroidManifest.xml"
    if [ -f "$MANIFEST" ] && ! grep -q "networkSecurityConfig" "$MANIFEST"; then
        # Insert the attribute into the <application ...> opening tag.
        python3 - "$MANIFEST" <<'PYEOF'
import re, sys
path = sys.argv[1]
with open(path) as f:
    content = f.read()
new_content = re.sub(
    r'(<application\b)',
    r'\1 android:networkSecurityConfig="@xml/network_security_config"',
    content,
    count=1,
)
if new_content == content:
    print("WARNING: could not find <application> tag to patch — add the attribute manually, see MOBILE.md")
else:
    with open(path, 'w') as f:
        f.write(new_content)
    print("Patched AndroidManifest.xml")
PYEOF
    fi

    c_cyan "Adding REQUEST_INSTALL_PACKAGES permission (needed for in-app updates)"
    # Lets the app trigger Android's own package installer on itself —
    # required for AndroidUpdateChecker.tsx's download-then-install flow.
    # NOTE: whether tauri-plugin-opener's own Android library already
    # declares its own FileProvider <provider> entry (needed to hand the
    # downloaded APK to the installer as a content:// URI, since Android
    # blocks raw file:// URIs across app boundaries) was NOT verified —
    # built without a real Android SDK/emulator available to check
    # against. If the in-app update button fails with a
    # FileUriExposedException or a "no provider" style error, that's the
    # first thing to check: you likely need to add your own <provider>
    # block manually (search "FileProvider Android Tauri" for the
    # current canonical snippet).
    if [ -f "$MANIFEST" ] && ! grep -q "REQUEST_INSTALL_PACKAGES" "$MANIFEST"; then
        python3 - "$MANIFEST" <<'PYEOF'
import re, sys
path = sys.argv[1]
with open(path) as f:
    content = f.read()
new_content = re.sub(
    r'(<manifest\b[^>]*>)',
    r'\1\n    <uses-permission android:name="android.permission.REQUEST_INSTALL_PACKAGES" />',
    content,
    count=1,
)
if new_content != content:
    with open(path, 'w') as f:
        f.write(new_content)
    print("Added REQUEST_INSTALL_PACKAGES permission")
PYEOF
    fi
fi

if [ "$MODE" = "--dev" ]; then
    c_cyan "Launching on a connected device or emulator"
    echo "(Reminder: this is Tauri's known risk area on Android — if the app"
    echo "goes unresponsive after being backgrounded, see the note at the"
    echo "bottom of MOBILE.md about the background HTTP server thread.)"
    npm run tauri android dev
else
    c_cyan "Building a release APK/AAB"
    npm run tauri android build
    echo
    echo "Done. Look in src-tauri/gen/android/app/build/outputs/ for the APK/AAB."
fi
