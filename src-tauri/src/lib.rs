pub mod ai_assistant;
pub mod ai_chat;
pub mod ai_context;
pub mod android_service;
pub mod audit;
pub mod auth;
pub mod backup;
pub mod business_branding;
pub mod business_panel;
pub mod business_pulse;
pub mod crash_report;
pub mod crud;
pub mod currency;
pub mod customers;
pub mod db;
pub mod db_migrations;
pub mod debt_settlement;
pub mod excel_import;
pub mod forecast;
pub mod http_api;
pub mod invoice;
pub mod module;
pub mod money;
pub mod network_mode;
pub mod notifications;
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
pub mod stock_take;
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

/// Returns the resolved modules directory. When the real app has
/// started up through `run()`, this is the actual bundled resource
/// path (see MODULES_DIR.set() below). Otherwise — `cargo test`, or
/// any other context that never runs `run()`'s setup — falls back to
/// this crate's own `modules/` folder, resolved via `CARGO_MANIFEST_DIR`
/// (a compile-time constant Cargo always sets to this crate's own root
/// directory) rather than a bare relative path. A bare `"modules"`
/// depends on the process's RUNTIME working directory matching
/// wherever `cargo test` happened to be invoked from — exactly the
/// kind of ambient assumption that broke real CI: `db_migrations.rs`'s
/// v8 migration calls this during `cargo test --lib`, and a relative
/// path failing to resolve doesn't error loudly — it silently
/// `continue`s past that module (see v8_money_to_cents's `let Ok(...)
/// else { continue }`), leaving the table exactly as unconverted as
/// before, then failing much later and less clearly when a test tries
/// to read a column that's still REAL instead of the INTEGER the
/// migration was supposed to produce. CARGO_MANIFEST_DIR is a
/// compile-time baked-in absolute path, immune to whatever the
/// runtime CWD happens to be.
pub fn modules_dir() -> std::path::PathBuf {
    MODULES_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("modules"))
}

/// Compile-time embedded module definitions — the actual fix for the bug
/// `modules_dir()` above was written to solve, which turned out not to be
/// solved at all on Android.
///
/// CONFIRMED, not guessed: hitting `GET /setup/diagnostics` (see
/// http_api.rs) on a real Android device returned
/// `"modules_dir": "asset://localhost/modules", "modules_dir_exists": false`.
/// `resource_dir()` on Android resolves to a virtual WebView asset URI,
/// not a real filesystem path — `std::fs::read`/`Path::exists()` can
/// never read through it, on any Android device, for any business. Every
/// caller that checked `modules_dir().join(...).exists()` before doing
/// anything (onboarding's module-enabling, the "available modules" list,
/// the enable-module route, and — seriously — the v8 money-to-cents
/// migration in db_migrations.rs) was silently no-op-ing on every single
/// module, for every Android install, with no error surfaced anywhere.
///
/// These JSON files are small, fixed, and shipped identically with every
/// build regardless of platform — there was never a real reason to read
/// them from a resolved runtime directory instead of baking them into the
/// compiled binary directly. `include_str!` does that: the file's
/// contents become part of the binary at compile time, so there is no
/// runtime, platform-specific "does this path resolve to something
/// std::fs can read" question left to get wrong, on Android, desktop, or
/// anywhere else this ever runs.
///
/// `modules_dir()` above is left in place, unused by real functionality
/// now, only because `/setup/diagnostics` still reports it as the
/// evidence trail for this bug — not because anything still depends on
/// it working.
pub const MODULE_DEFS: &[(&str, &str)] = &[
    ("accounting", include_str!("../modules/accounting.json")),
    ("debt_credit", include_str!("../modules/debt_credit.json")),
    ("hr", include_str!("../modules/hr.json")),
    ("inventory", include_str!("../modules/inventory.json")),
    ("invoice", include_str!("../modules/invoice.json")),
    ("purchasing", include_str!("../modules/purchasing.json")),
    ("refunds", include_str!("../modules/refunds.json")),
    ("sales", include_str!("../modules/sales.json")),
];

/// Looks up a module's embedded JSON definition by id. Returns `None`
/// for an unrecognized id — every real call site below treats that the
/// same way the old `path.exists()` check did: "this module doesn't
/// exist", not an error.
pub fn module_json(module_id: &str) -> Option<&'static str> {
    MODULE_DEFS.iter().find(|(id, _)| *id == module_id).map(|(_, json)| *json)
}
pub mod report;
pub mod roles;
pub mod settings;
pub mod users;
pub mod xlsx_export;

/// The frontend needs to know this device's network mode BEFORE it can
/// make its first HTTP call at all (see main.tsx) — a client device's
/// entire API base URL depends on it (see api.ts's setApiBase). That's
/// a genuine chicken-and-egg problem for an app that otherwise talks
/// to its backend purely over HTTP (see http_api.rs's own doc
/// comments): there's no HTTP server to ask yet when a client device
/// hasn't even started one. Tauri's IPC bridge is the one channel that
/// works regardless — these three are this app's first-ever
/// `#[tauri::command]`s, deliberately kept to exactly the bootstrap
/// question "how should this device even reach its data," nothing more.
#[tauri::command]
fn get_network_mode(app: tauri::AppHandle) -> Result<network_mode::NetworkModeConfig, String> {
    use tauri::Manager;
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(network_mode::load(&dir))
}

#[tauri::command]
fn set_network_mode(app: tauri::AppHandle, mode: String, host_address: Option<String>) -> Result<(), String> {
    use tauri::Manager;
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let config = network_mode::NetworkModeConfig { mode, host_address };
    network_mode::save(&dir, &config).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_lan_address() -> Option<String> {
    network_mode::local_lan_ip()
}

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
///
/// `#[cfg_attr(mobile, tauri::mobile_entry_point)]` MUST sit directly
/// above THIS function — an attribute attaches to the very next real
/// item, and doc comments (`///`) don't count as that item. A previous
/// version of this file had three `#[tauri::command]` functions'
/// worth of doc comments and attributes sitting between this attribute
/// and `run()`, which silently attached the mobile entry point to
/// `get_network_mode` instead — a real compile error on Android
/// (`mobile_entry_point` requires a zero-argument function;
/// `get_network_mode` takes one), caught by a real Android CI build
/// after every desktop platform had already compiled and passed
/// cleanly, since desktop builds never evaluate this `cfg_attr` at
/// all (`mobile` is false there).
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default().invoke_handler(tauri::generate_handler![
        get_network_mode,
        set_network_mode,
        get_lan_address
    ]);

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

            // Network mode (see network_mode.rs) decides everything
            // from here on. Reading it this early, before the database
            // is even opened, is what makes "client" mode possible at
            // all — a client device genuinely has no local database
            // and never starts a local server; it exists purely to
            // point the frontend (via api.ts's setApiBase, driven by
            // the get_network_mode Tauri command above) at some OTHER
            // device's server instead. Anything that isn't "client"
            // falls through to the exact same startup every install
            // has always had — a missing or corrupt config file reads
            // back as "standalone" (see network_mode::load's own doc
            // comment on why that's the safe default), so an install
            // that has never touched Admin → Network is completely
            // unaffected by any of this existing.
            let net_config = network_mode::load(&app_data_dir);
            if net_config.mode == "client" {
                return Ok(());
            }

            let db_path = app_data_dir.join("erp.db").to_string_lossy().to_string();

            // See lib.rs's MODULES_DIR doc comment: this was originally
            // written as "the fix" for module JSON files never
            // resolving on a real installed app, on the assumption that
            // resource_dir() always points at a real, std::fs-readable
            // filesystem location. That's true on desktop. It is NOT
            // true on Android, where this resolves to a virtual
            // `asset://localhost/...` WebView URI instead — confirmed
            // directly against a real device, not assumed. Real module
            // loading no longer depends on this at all (see
            // `MODULE_DEFS`); this call is kept only so
            // `/setup/diagnostics` can keep reporting what this used to
            // resolve to, as the evidence trail for that bug.
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

            // "host" binds to every network interface (0.0.0.0) so
            // other devices on the same WiFi can reach this one — see
            // network_mode.rs's module doc comment for the full
            // standalone/host/client picture. Anything other than
            // "host" (i.e. "standalone", and any unrecognized value —
            // fails safe toward the more restrictive option) keeps the
            // original 127.0.0.1-only binding every install has always
            // had. Both are 'static string literals, which is what
            // start_server_platform's signature requires — this is
            // choosing between two fixed addresses, not building one
            // at runtime, so there's no lifetime issue picking between
            // them here.
            let bind_addr: &'static str = if net_config.mode == "host" { "0.0.0.0:8080" } else { "127.0.0.1:8080" };
            crate::android_service::start_server_platform(conn, bind_addr, &app_data_dir);

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running the Tauri application");
}
