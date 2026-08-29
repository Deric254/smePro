//! Android background execution — this file's name and doc comment
//! used to promise more than the code actually does, and that gap is
//! the single most important thing to understand about it.
//!
//! WHAT THIS FILE ACTUALLY DOES TODAY:
//! On every platform, including Android, `start_server_platform` just
//! spawns the HTTP server on a plain `std::thread::spawn` (or a named
//! thread, on the Android build target — the name is only for logcat
//! debugging, it changes nothing about how Android schedules it). This
//! Rust code itself is unchanged and still has no JNI bridge and no
//! direct awareness of the service described below.
//!
//! WHY THAT USED TO MATTER (and why it's addressed, separately, now):
//! Android 8+ (API 26+) enforces background execution limits that
//! reclaim exactly this pattern — a plain background thread with no
//! foreground service and no persistent notification — typically
//! within minutes of the app leaving the foreground. A real Kotlin
//! `Service` now exists to close this gap: see
//! `src-tauri/android/com/smepro/app/SmeProForegroundService.kt` and
//! `SmeProApplication.kt`, wired into the generated Android project by
//! a CI step in `.github/workflows/release.yml` (which runs after
//! `tauri android init` scaffolds `gen/android`, since that directory
//! doesn't exist in this repo and is regenerated fresh on every
//! build). The service calls `startForeground()` with a persistent,
//! low-priority notification, which is what raises the whole
//! process's — including this file's plain background thread's —
//! scheduling priority so Android's background limits stop applying
//! to it. It does NOT talk to this Rust code directly (no JNI): it
//! doesn't need to, since raising the process's priority is enough to
//! protect a thread that's already running.
//!
//! WHAT IS AND ISN'T VERIFIED: the manifest-patching logic that wires
//! the service in was tested directly (run against a representative
//! sample manifest, confirmed to produce well-formed XML with the
//! right attributes, and confirmed idempotent on a second run) — that
//! part has real proof behind it, not just review. The Kotlin itself
//! was written against standard, documented Android platform APIs
//! (`Service`, `startForeground`, `NotificationChannel`) but has NOT
//! been compiled or run — no Kotlin compiler was available in the
//! environment this was written in. Most importantly, NOTHING here has
//! been confirmed against a real device or emulator actually
//! surviving extended backgrounding — that requires installing a real
//! built APK (this repo's GitHub Actions pipeline does build and sign
//! one for real) and testing it by hand. Don't treat this as a closed
//! issue until that device test has actually been done.

/// Starts the HTTP server. Identical behavior on every platform right
/// now — see the module doc comment above for why the Android branch
/// existing separately doesn't currently mean anything functionally
/// different happens there.
///
/// # Arguments
/// * `conn` — SQLite connection (already opened)
/// * `addr` — bind address (e.g. "127.0.0.1:8080")
/// * `app_data_dir` — path for the readiness marker file (see below)
#[allow(unused_variables)]
pub fn start_server_platform(conn: rusqlite::Connection, addr: &'static str, app_data_dir: &std::path::Path) {
    #[cfg(target_os = "android")]
    {
        start_android_thread(conn, addr, app_data_dir);
    }
    #[cfg(not(target_os = "android"))]
    {
        // `serve()` used to be `.expect()`-ed at the bind step inside
        // http_api.rs itself: a bind failure (most commonly: another
        // instance of this app, or something else, already holding
        // port 8080) panicked this background thread. A panic here
        // doesn't crash the app — the Tauri window keeps running
        // normally — it just silently kills the server thread, and
        // with `#![windows_subsystem = "windows"]` suppressing the
        // console on Windows, that panic message went nowhere anyone
        // could see it. The frontend was left permanently retrying a
        // server that would never come up, with no way to tell why.
        //
        // Now that `serve()` returns a `Result` instead of panicking,
        // capture the real error here and write it to a plain-text
        // log file in the app's own data directory — the one place
        // guaranteed to exist and be readable without a console.
        let app_data_dir = app_data_dir.to_path_buf();
        std::thread::spawn(move || {
            if let Err(e) = crate::http_api::serve(conn, addr) {
                eprintln!("[api] {e}");
                let _ = std::fs::write(app_data_dir.join("server_error.log"), &e);
            }
        });
    }
}

#[cfg(target_os = "android")]
fn start_android_thread(conn: rusqlite::Connection, addr: &'static str, app_data_dir: &std::path::Path) {
    // This marker file records that the Rust side has started the
    // server, for whatever Android-side code eventually wants to
    // check readiness — it is NOT part of a foreground-service
    // handshake, since no such handshake exists. Kept because
    // something on the Android side may already depend on this
    // file's presence, and removing it isn't something to do blind
    // without Android-side visibility into what reads it.
    let marker = app_data_dir.join(".rust_ready");
    let _ = std::fs::write(&marker, addr.as_bytes());

    // A named thread only for logcat readability — this is NOT a
    // foreground service, and Android's background execution limits
    // apply to it exactly as they would to any other thread.
    let error_dir = app_data_dir.to_path_buf();
    std::thread::Builder::new()
        .name("smepro-api".into())
        .spawn(move || {
            if let Err(e) = crate::http_api::serve(conn, addr) {
                eprintln!("[api] {e}");
                let _ = std::fs::write(error_dir.join("server_error.log"), &e);
            }
        })
        .expect("failed to spawn API server thread");
}
