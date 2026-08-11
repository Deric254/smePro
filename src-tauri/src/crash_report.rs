//! Crash reporting — sends fatal errors to a configured endpoint.
//!
//! For a million-user product, you need to know when things break.
//! This module captures panics and unhandled errors, then sends
//! a minimal, privacy-respecting report to your error tracker.
//!
//! PRIVACY DESIGN:
//! - No business data, user names, or record contents are DELIBERATELY
//!   captured — only error message, stack trace location, app version,
//!   OS, and timestamp are gathered. One honest caveat this file
//!   previously glossed over: the panic MESSAGE itself is whatever the
//!   panicking code produced, and if some far-flung `.unwrap()` in this
//!   codebase ever panics on a `Result` whose error message happened to
//!   embed record content (several `anyhow!()` call sites elsewhere in
//!   this codebase do include field values via `{:?}` for debugging),
//!   that fragment would ride along in the message text. This isn't a
//!   gap to paper over with an absolute claim; it's the inherent
//!   tension in any crash reporter that shows you the actual panic
//!   message (which is also what makes it useful for debugging at
//!   all) — the true guarantee here is narrower than "never": nothing
//!   is ever deliberately included beyond what the panic already says.
//! - User can opt out via a setting (`crash_reporting_enabled`,
//!   stored in business_settings, checked below) — this really is
//!   checked now; an earlier version of this comment claimed it while
//!   no such setting existed anywhere in the codebase.
//! - Reports are batched and sent on next app launch (not real-time)
//!   so they don't block the user or leak network activity timing
//!
//! DEFAULT: disabled until you configure a Sentry DSN or webhook.
//! The app works perfectly without it — this is observability, not
//! functionality.

use std::panic;
use std::sync::Once;

static INIT: Once = Once::new();

const OPT_OUT_SETTING_KEY: &str = "crash_reporting_enabled";

/// Whether crash reporting is currently allowed to run, per the
/// opt-out setting — defaults to enabled (this is opt-OUT, not
/// opt-in) until a business explicitly sets it to "false".
///
/// This is checked against the FIRST business found in the install,
/// not a specific one — crash reporting installs its panic hook once,
/// globally, at process startup, before any user has necessarily
/// logged into any particular business, so there's no single
/// business context to check against at that point. This matches the
/// architecture's own stated norm of "normally exactly one active
/// business per install" (see schema.sql); on the rare multi-business
/// install, the first business's preference governs process-wide
/// reporting, which is a reasonable default rather than a silent gap
/// — documented here so it's a known, deliberate choice.
pub(crate) fn reporting_allowed(conn: &rusqlite::Connection) -> bool {
    let business_id: Option<String> = conn
        .query_row("SELECT id FROM businesses ORDER BY created_at LIMIT 1", [], |r| r.get(0))
        .ok();
    let Some(business_id) = business_id else {
        return true; // no business exists yet (fresh install) — nothing to opt out of yet either
    };
    crate::settings::get(conn, &business_id, OPT_OUT_SETTING_KEY)
        .map(|v| v != "false")
        .unwrap_or(true)
}

/// Initializes the crash reporter. Call once at app startup.
///
/// # Arguments
/// * `conn` — used only to check the opt-out setting before installing
///   anything; never touched again after this call returns
/// * `dsn` — Optional Sentry DSN or webhook URL. If None, crash
///   reporting is silently disabled.
/// * `version` — App version string (from tauri.conf.json)
/// * `app_data_dir` — Where to queue reports before sending
pub fn init(conn: &rusqlite::Connection, dsn: Option<&str>, version: &str, app_data_dir: &std::path::Path) {
    if dsn.is_none() {
        return; // silently disabled
    }
    if !reporting_allowed(conn) {
        return; // explicitly opted out via business_settings
    }
    let version = version.to_string();
    let queue_dir = app_data_dir.join("crash_reports");
    let _ = std::fs::create_dir_all(&queue_dir);

    INIT.call_once(|| {
        let default_hook = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let payload = format!("{}", info);
            let location = info.location()
                .map(|l| format!("{}:{}", l.file(), l.line()))
                .unwrap_or_else(|| "unknown".to_string());

            let report = serde_json::json!({
                "timestamp": chrono::Utc::now().to_rfc3339(),
                "version": version,
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
                "error": payload,
                "location": location,
            });

            // Queue to disk — don't block the panic handler with network
            let filename = format!("crash_{}.json", chrono::Utc::now().timestamp());
            let _ = std::fs::write(queue_dir.join(&filename), report.to_string());

            // Still call the default hook so the panic message prints
            default_hook(info);
        }));
    });
}

/// Sends all queued crash reports. Call on app startup (after network
/// is available) so crashes from the previous session get reported.
///
/// Checks the opt-out setting again here, independently of `init()` —
/// if someone turned reporting off after a crash was already queued
/// from a previous session, that queued file is intentionally left
/// alone on disk rather than sent, honoring the opt-out even for
/// reports captured before it was set.
pub fn flush_queue(conn: &rusqlite::Connection, dsn: &str, app_data_dir: &std::path::Path) {
    if !reporting_allowed(conn) {
        return;
    }
    let queue_dir = app_data_dir.join("crash_reports");
    let Ok(entries) = std::fs::read_dir(&queue_dir) else { return };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else { continue };

        // Best-effort send. If it fails, leave the file for next time.
        let sent = if dsn.starts_with("https://") && dsn.contains("sentry.io") {
            send_to_sentry(dsn, &content)
        } else {
            send_to_webhook(dsn, &content)
        };

        if sent {
            let _ = std::fs::remove_file(&path);
        }
    }
}

fn send_to_sentry(dsn: &str, payload: &str) -> bool {
    // Sentry's envelope format is complex. For a minimal implementation,
    // we POST to their store endpoint. A production app should use
    // the official sentry-rust crate instead.
    let client = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(10)).build();
    match client.post(dsn).set("Content-Type", "application/json").send_string(payload) {
        Ok(r) => r.status() == 200,
        Err(_) => false,
    }
}

fn send_to_webhook(url: &str, payload: &str) -> bool {
    let client = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(10)).build();
    match client.post(url).set("Content-Type", "application/json").send_string(payload) {
        Ok(r) => r.status() >= 200 && r.status() < 300,
        Err(_) => false,
    }
}
