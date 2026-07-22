pub mod ai_assistant;
pub mod ai_context;
pub mod audit;
pub mod auth;
pub mod business_panel;
pub mod crud;
pub mod db;
pub mod forecast;
pub mod http_api;
pub mod license;
pub mod module;
pub mod notifications;
pub mod ocr_import;
pub mod onboarding;
pub mod payment;
pub mod rate_limit;
pub mod rbac;
pub mod reference_data;
pub mod report;
pub mod roles;
pub mod settings;
pub mod users;
pub mod vendor_license;
pub mod xlsx_export;

/// The one real entry point for the packaged app — desktop (called from
/// `main.rs`) and mobile (called automatically via the
/// `mobile_entry_point` attribute below) both run through here. This is
/// what keeps "one system" true across every target: there's no
/// separate mobile app logic anywhere, just this same function running
/// on a different OS.
///
/// MOBILE-SPECIFIC NOTE this project's build environment could not
/// verify (no Android SDK/NDK reachable — see MOBILE.md): on Android,
/// backgrounded apps can have their threads suspended or killed by the
/// OS more aggressively than a desktop OS ever would. The
/// spawn-a-thread-and-forget pattern below is exactly what worked for
/// desktop, but on Android it may need to move to a foreground service
/// (or bind the HTTP server's lifecycle to Tauri's own app lifecycle
/// events) to survive the user switching away from the app briefly.
/// Flagging this now rather than assuming desktop's threading model
/// transfers over silently.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    std::thread::spawn(|| {
        let conn = db::open("erp.db").expect("failed to open local database");
        http_api::serve(conn, "127.0.0.1:8080");
    });

    let builder = tauri::Builder::default();

    // Self-updating via tauri-plugin-updater only makes sense on
    // desktop — that plugin has no Android/iOS implementation.
    #[cfg(desktop)]
    let builder = builder
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init());

    // These four ARE cross-platform, and are what Android's own
    // in-app update flow is built from instead (see
    // AndroidUpdateChecker.tsx): `os` to detect we're on Android at
    // all, `http` to download the new APK from the GitHub release
    // (bypasses the webview's CORS restrictions, which plain fetch()
    // would hit), `fs` to write those bytes to a real file Android can
    // hand to its installer, and `opener` to actually hand it off —
    // openPath() on an .apk triggers Android's package installer via
    // FileProvider, the same "tap to confirm" screen a normal reinstall
    // would show, just launched from inside the app instead of a file
    // manager.
    let builder = builder
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_http::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init());

    builder
        .run(tauri::generate_context!())
        .expect("error while running the Tauri application");
}
