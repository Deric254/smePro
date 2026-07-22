# Mobile (Android) — Phase 10

## What's actually done

The codebase is now genuinely **one system across desktop and mobile**,
not just desktop with mobile "planned":

- `src-tauri/src/lib.rs` has a `run()` function marked
  `#[cfg_attr(mobile, tauri::mobile_entry_point)]` — Tauri's real
  convention for sharing one codebase across desktop and Android/iOS.
  Desktop's `main.rs` calls it directly; on mobile, Tauri's build
  tooling calls it automatically as the native entry point. There is no
  separate mobile app logic anywhere.
- `Cargo.toml`'s `[lib]` section is `crate-type = ["staticlib", "cdylib", "rlib"]`
  — required so Android (via JNI) and iOS can load this as a native
  library, while `rlib` keeps it linkable by desktop's `main.rs` and the
  `demo_seed` dev binary exactly as before.
- The desktop-only plugins (auto-updater, process/relaunch) are gated
  behind `#[cfg(desktop)]` — deliberately **not** compiled into mobile
  builds, since `tauri-plugin-updater` has no Android/iOS
  implementation at all. **The self-hosted-APK-plus-in-app-checker path
  is now built**, not just planned — see `AndroidUpdateChecker.tsx`:
  it checks the GitHub release, downloads the new APK, and hands it to
  Android's own package installer, all from inside the app. One tap to
  confirm the install is unavoidable (a real Android OS security
  requirement for any app not distributed via Play Store, not a
  shortcoming of this implementation) but the user never leaves the app
  or needs a file manager/browser.
- Verified: with Tauri itself stripped out (this sandbox's standard
  workaround throughout this project), the business logic + demo binary
  still compile cleanly against this restructured `lib.rs`/`main.rs` —
  confirming the mobile-readiness changes didn't break anything that
  worked before.

## In-app updates — what's real vs. untested

`AndroidUpdateChecker.tsx` is built from four genuine, documented Tauri
v2 plugins (`os` to detect Android, `http` to download past the
webview's CORS restrictions, `fs` to write the file, `opener` to hand
it to Android's installer) — this is the real, standard pattern for
self-hosted Android app updates, the same one F-Droid-style
"in-app updater" apps use. `capabilities/default.json` scopes the
network/filesystem permissions narrowly (GitHub's API + release-asset
domains only, `$APPCACHE` only) rather than granting broad defaults.

**Not verified on a real device** — this sandbox has no Android SDK or
emulator. The specific risk points, in order of likelihood if something
doesn't work:
1. `capabilities/default.json`'s permission scopes — Tauri v2's
   capability system is the most common source of silent "works in
   code, fails at runtime with a permission error" bugs.
2. Whether `tauri-plugin-opener`'s Android implementation needs an
   additional manually-declared `FileProvider` in the manifest beyond
   what `mobile-android.sh`/CI already add (the `REQUEST_INSTALL_PACKAGES`
   permission) — genuinely unverified either way.
3. Android's per-app "install unknown apps" one-time permission prompt
   — expected native OS behavior the first time a user taps Update, not
   a bug.

## What could NOT be done in this environment

`npx tauri android init` (the command that scaffolds the actual Android
Studio project under `src-tauri/gen/android`) requires the Android SDK.
This sandbox doesn't have it, and Google's SDK download servers
(`dl.google.com`) aren't reachable here (this environment's network is
locked to a fixed domain allowlist — confirmed with a direct `403`, the
same way NVIDIA/Gemini were confirmed blocked back in the AI-provider
phase). Even with the SDK, cross-compiling Rust for Android needs the
NDK and `rustup target add aarch64-linux-android` (etc.) — `rustup`
itself isn't reachable here either (this sandbox's Rust came from
`apt`, capped at 1.75, the same ceiling that blocked the desktop Tauri
build). So this is two compounding blockers, both environment-specific,
neither a problem with the code.

## What to do on your own machine

1. Install prerequisites: https://v2.tauri.app/start/prerequisites/#android
   (Android Studio + SDK + NDK, plus `rustup target add
   aarch64-linux-android armv7-linux-androideabi i686-linux-android
   x86_64-linux-android`)
2. `npm run tauri android init` — scaffolds `src-tauri/gen/android`
3. **Apply this network security config** (Android blocks cleartext
   HTTP by default on API 28+; our backend deliberately talks
   plain HTTP to `127.0.0.1`, which is loopback-only traffic within the
   app's own sandboxed process, not real network traffic — but Android's
   default policy doesn't distinguish that automatically, so it must be
   allow-listed explicitly):

   Create `src-tauri/gen/android/app/src/main/res/xml/network_security_config.xml`:
   ```xml
   <?xml version="1.0" encoding="utf-8"?>
   <network-security-config>
       <domain-config cleartextTrafficPermitted="true">
           <domain includeSubdomains="false">127.0.0.1</domain>
       </domain-config>
   </network-security-config>
   ```
   Then reference it in `src-tauri/gen/android/app/src/main/AndroidManifest.xml`,
   inside the `<application>` tag:
   ```xml
   android:networkSecurityConfig="@xml/network_security_config"
   ```
4. `npm run tauri android dev` to run on an emulator/device, or
   `npm run tauri android build` for a release APK/AAB.

## The one architectural risk to actually test on a real device

`lib.rs::run()` spawns the HTTP API on a background OS thread and lets
it run for the app's lifetime — this is exactly what worked for
desktop. **Android is more aggressive about suspending or killing
background work than a desktop OS.** If the backend thread gets
suspended when the app is backgrounded, in-flight requests could fail
or the app could feel unresponsive when resumed. This wasn't something
that could be tested without a device/emulator. If it turns out to be a
real problem, the fix is likely either:
- Moving the HTTP server into a proper Android foreground service, or
- Binding its start/stop to Tauri's own app lifecycle events
  (`on_resume`, `on_pause`) rather than a single always-on spawned thread

Flagging this now, before it surfaces as a confusing "works on desktop,
flaky on phone" bug later.
