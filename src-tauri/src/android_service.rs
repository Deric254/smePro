//! Android background execution — this file's name and doc comment
//! used to promise more than the code actually does, and that gap is
//! the single most important thing to understand about it.
//!
//! WHAT THIS ACTUALLY IS TODAY:
//! On every platform, including Android, `start_server_platform` just
//! spawns the HTTP server on a plain `std::thread::spawn` (or a named
//! thread, on the Android build target — the name is only for logcat
//! debugging, it changes nothing about how Android schedules it).
//! There is no JNI bridge here, no `tauri-plugin-notification`
//! dependency (none exists anywhere in Cargo.toml), no Kotlin/Java
//! service class, no `AndroidManifest.xml` service declaration, and
//! no `startForeground()` call — none of that exists anywhere in this
//! repository. A previous version of this comment described that
//! entire architecture in detail and claimed it had been stress
//! tested against 30-minute backgrounding, low-memory conditions, and
//! kill/restart cycles. None of that was true; the code below was
//! never anything more than what you're reading now.
//!
//! WHY THIS MATTERS: Android 8+ (API 26+) enforces background
//! execution limits specifically to kill exactly this pattern — a
//! plain background thread with no foreground service and no
//! persistent notification is reclaimed by the OS, typically within
//! minutes of the app leaving the foreground, sometimes sooner under
//! memory pressure. That means the HTTP API this app's own frontend
//! depends on WILL stop responding once the user switches away from
//! the app on a real Android device, for exactly the reason this
//! file's original comment described as the problem — it just never
//! actually got solved.
//!
//! WHAT A REAL FIX REQUIRES (not attempted here — this needs native
//! Android platform work, verified against a real device or emulator,
//! neither of which is available in the environment these fixes were
//! written and tested in):
//! 1. A Kotlin `Service` subclass (typically registered through
//!    Tauri's Android plugin mechanism — see Tauri's mobile plugin
//!    documentation) declared in `AndroidManifest.xml` with an
//!    appropriate `foregroundServiceType`.
//! 2. A call to `startForeground()` with a persistent, low-priority
//!    notification — Android requires this for a service to be
//!    exempted from background kill limits; there's no way to keep a
//!    background HTTP server alive on modern Android without a
//!    notification the user can see.
//! 3. The Rust HTTP server thread's lifecycle bound to that service,
//!    not just spawned once and left to whatever the OS decides.
//! 4. Real device/emulator testing of the specific scenarios this
//!    file's comment used to claim were already covered (extended
//!    backgrounding, low memory, force-kill/restart) — claims about
//!    that testing should not be written again until it has actually
//!    been done.
//!
//! Until that work happens, this should be treated as a known,
//! open gap in the app's core Android reliability story — not
//! quietly-solved infrastructure.

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
