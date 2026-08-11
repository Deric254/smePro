pub mod ai_assistant;
pub mod ai_context;
pub mod android_service;
pub mod audit;
pub mod auth;
pub mod backup;
pub mod business_branding;
pub mod business_panel;
pub mod crash_report;
pub mod crud;
pub mod currency;
pub mod customers;
pub mod db;
pub mod db_migrations;
pub mod excel_import;
pub mod forecast;
pub mod http_api;
pub mod invoice;
pub mod module;
pub mod money;
pub mod notifications;
pub mod ocr_import;
pub mod onboarding;
pub mod pos;
pub mod rate_limit;
pub mod rbac;
pub mod receipt;
pub mod receiving;
pub mod reference_data;
pub mod refund;
pub mod repack;
pub mod security;
pub mod tax;
#[cfg(test)]
mod tests;
pub mod totp;

/// Where the bundled `modules/*.json` files actually live at runtime.
/// Resolved ONCE at startup (see `run()` below) via Tauri's own
/// resource-directory API, then read from here — the HTTP handler runs
/// on a plain thread with no direct access to the Tauri `app` handle,
/// so this is how it reaches a value that otherwise requires one.
///
/// THE BUG THIS FIXES: `onboarding.rs` used to read module definitions
/// from a bare relative path ("modules/inventory.json"), which only
/// resolves correctly when the process's current working directory
/// happens to be the source tree — true in dev (`cargo run`), never
/// true for a real installed app, whose working directory depends on
/// how the OS launched it. On every real installed copy, every module
/// silently failed to enable, business type selection did nothing, and
/// a brand new business ended up with zero modules and no visible way
/// to add any — the exact "No modules are enabled yet" dead end this
/// was traced from. Same root-cause pattern, same fix shape, as the
/// earlier database-path bug.
static MODULES_DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// Returns the resolved modules directory, or a plain relative
/// "modules" path as a fallback for contexts where the real app never
/// started up through `run()` at all (dev/test binaries like
/// `demo_seed`, which call `enable_module` directly with their own
/// paths and never go through this at all — this fallback exists for
/// completeness, not because anything currently relies on it).
pub fn modules_dir() -> std::path::PathBuf {
    MODULES_DIR.get().cloned().unwrap_or_else(|| std::path::PathBuf::from("modules"))
}
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
        .setup(|app| {
            // THE FIX: the database used to open at a bare relative
            // path ("erp.db"), which resolves against the process's
            // current working directory — for a real installed
            // desktop app, that directory is NOT guaranteed to be the
            // same every launch (it depends on how Windows happened to
            // start the process — Start Menu vs. desktop shortcut vs.
            // "Run as administrator" can all differ). That's exactly
            // what could make the app seem to intermittently "forget"
            // a business was already set up: it wasn't forgetting, it
            // was sometimes looking in a different folder for a
            // database that was never there in the first place, and
            // silently creating a new empty one.
            //
            // app.path().app_data_dir() is Tauri's own, OS-correct,
            // always-consistent answer to "where should this app keep
            // its data" — resolves to a real per-user, per-app folder
            // (e.g. %APPDATA%\com.smepro.app\ on Windows) that's the
            // same every single launch, regardless of how the app was
            // started.
            use tauri::Manager;
            let app_data_dir = app
                .path()
                .app_data_dir()
                .map_err(|e| format!("could not resolve the app data directory: {e}"))?;
            std::fs::create_dir_all(&app_data_dir)
                .map_err(|e| format!("could not create the app data directory: {e}"))?;
            let db_path = app_data_dir.join("erp.db").to_string_lossy().to_string();

            // See the MODULES_DIR doc comment above — this is the fix
            // for module JSON files never resolving correctly on a real
            // installed app. resource_dir() correctly points at wherever
            // the OS actually placed this app's bundled resources
            // (declared in tauri.conf.json's bundle.resources).
            let resource_dir = app
                .path()
                .resource_dir()
                .map_err(|e| format!("could not resolve the resource directory: {e}"))?;
            let _ = MODULES_DIR.set(resource_dir.join("modules"));

            // Some HTTP handlers (business branding logo upload/serving) run
            // on a plain thread with no Tauri app handle, same problem
            // MODULES_DIR solves above — this env var is how they reach the
            // app data directory too.
            std::env::set_var("SME_APP_DATA_DIR", app_data_dir.to_string_lossy().to_string());

            let conn = db::open(&db_path).expect("failed to open local database");

            // Crash reporting is off by default (no DSN configured) — see
            // crash_report.rs. Flip `None` to `Some("your-sentry-dsn")` once
            // you have a real endpoint to send to. Even then, a business
            // can turn it off via the crash_reporting_enabled setting.
            // Must run before conn moves into start_server_platform below
            // (that function takes ownership of it, not a borrow).
            let crash_dsn: Option<&str> = None;
            let version = app.package_info().version.to_string();
            crate::crash_report::init(&conn, crash_dsn, &version, &app_data_dir);
            // flush_queue actually SENDS whatever init's panic hook queued
            // from a previous session — init() alone only ever writes
            // reports to disk, it never transmits anything. An earlier
            // version of this file called init() but never flush_queue()
            // anywhere, so even a deployer who followed this exact
            // module's own setup instructions and configured a real DSN
            // would have crash reports pile up on disk forever and never
            // actually reach anywhere.
            if let Some(dsn) = crash_dsn {
                // Background thread, not inline: this makes real network
                // calls (10s timeout each, times however many crashes
                // queued while offline), and app launch shouldn't stall
                // on that — same "don't block" principle the panic hook
                // itself already follows for queuing.
                let dsn = dsn.to_string();
                let flush_conn = db::open(&db_path);
                let flush_dir = app_data_dir.clone();
                if let Ok(flush_conn) = flush_conn {
                    std::thread::spawn(move || {
                        crate::crash_report::flush_queue(&flush_conn, &dsn, &flush_dir);
                    });
                }
            }

            crate::android_service::start_server_platform(conn, "127.0.0.1:8080", &app_data_dir);

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running the Tauri application");
}
