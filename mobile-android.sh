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

    c_cyan "Adding FileProvider (so the downloaded APK can actually reach the installer)"
    # THE FIX for AndroidUpdateChecker.tsx's "downloads to 100%, then
    # fails to install" bug — see installer.rs's doc comment for the
    # full story. Short version: Android has refused to let one app
    # hand another a raw file:// path since API 24, so the downloaded
    # APK has to be wrapped in a content:// URI through a FileProvider
    # this app declares itself, scoped to wherever
    # AndroidUpdateChecker.tsx's appCacheDir() actually resolves to on
    # Android. NOT independently verified against a real device which
    # of Android's cache dirs that is (internal cacheDir vs.
    # externalCacheDir), so file_paths.xml below declares BOTH — an
    # unused <path> entry is harmless, an APK sitting in the one NOT
    # declared here is a silent "no provider" failure, so this errs
    # toward covering both rather than guessing.
    XML_RES_DIR="src-tauri/gen/android/app/src/main/res/xml"
    mkdir -p "$XML_RES_DIR"
    cat > "$XML_RES_DIR/file_paths.xml" <<'EOF'
<?xml version="1.0" encoding="utf-8"?>
<paths xmlns:android="http://schemas.android.com/apk/res/android">
    <cache-path name="update_apk_internal" path="." />
    <external-cache-path name="update_apk_external" path="." />
</paths>
EOF
    if [ -f "$MANIFEST" ] && ! grep -q "fileprovider" "$MANIFEST"; then
        python3 - "$MANIFEST" <<'PYEOF'
import re, sys
path = sys.argv[1]
with open(path) as f:
    content = f.read()
provider = (
    '    <provider\n'
    '        android:name="androidx.core.content.FileProvider"\n'
    '        android:authorities="${applicationId}.fileprovider"\n'
    '        android:exported="false"\n'
    '        android:grantUriPermissions="true">\n'
    '        <meta-data\n'
    '            android:name="android.support.FILE_PROVIDER_PATHS"\n'
    '            android:resource="@xml/file_paths" />\n'
    '    </provider>\n'
    '</application>'
)
new_content = content.replace('</application>', provider, 1)
if new_content != content:
    with open(path, 'w') as f:
        f.write(new_content)
    print("Added FileProvider")
else:
    print("WARNING: could not find </application> tag to patch — add the <provider> block manually, see MOBILE.md")
PYEOF
    fi

    c_cyan "Wiring in SmeProApplication (foreground service + edge-to-edge insets) and the installer plugin"
    # Same file, same wiring, same reasoning as the matching step in
    # .github/workflows/release.yml — kept in sync here so a locally
    # built dev/debug APK behaves the same as a CI-built release one,
    # rather than only the release pipeline ever getting the
    # foreground-service protection and the edge-to-edge inset fix
    # (see SmeProApplication.kt's own doc comments for what each does
    # and why it lives there instead of a patched MainActivity.kt).
    # InstallerPlugin.kt rides along in the same copy step — see its
    # own doc comment, and installer.rs on the Rust side that registers
    # it, for what it's for.
    JAVA_DIR="src-tauri/gen/android/app/src/main/java/com/smepro/app"
    mkdir -p "$JAVA_DIR"
    cp src-tauri/android/com/smepro/app/SmeProForegroundService.kt "$JAVA_DIR/"
    cp src-tauri/android/com/smepro/app/SmeProApplication.kt "$JAVA_DIR/"
    cp src-tauri/android/com/smepro/app/InstallerPlugin.kt "$JAVA_DIR/"

    if [ -f "$MANIFEST" ]; then
        python3 - "$MANIFEST" <<'PYEOF'
import re, sys
path = sys.argv[1]
with open(path) as f:
    content = f.read()

if 'FOREGROUND_SERVICE' not in content:
    content = re.sub(
        r'(<manifest\b[^>]*>)',
        r'\1\n    <uses-permission android:name="android.permission.FOREGROUND_SERVICE" />'
        r'\n    <uses-permission android:name="android.permission.FOREGROUND_SERVICE_DATA_SYNC" />',
        content, count=1,
    )

if 'android:name=".SmeProApplication"' not in content:
    content = re.sub(
        r'(<application\b)',
        r'\1 android:name=".SmeProApplication"',
        content, count=1,
    )

if 'SmeProForegroundService' not in content:
    service_tag = (
        '    <service android:name=".SmeProForegroundService" '
        'android:enabled="true" android:exported="false" '
        'android:foregroundServiceType="dataSync" />\n'
        '</application>'
    )
    content = content.replace('</application>', service_tag, 1)

with open(path, 'w') as f:
    f.write(content)
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
