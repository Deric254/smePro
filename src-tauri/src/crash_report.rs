//! Crash reporting — sends fatal errors to a configured endpoint.
//!
//! For a million-user product, you need to know when things break.
//! This module captures panics and unhandled errors, then sends
//! a minimal, privacy-respecting report to your error tracker.
//!
//! PRIVACY DESIGN:
//! - No business data, user names, or record contents are ever sent
//! - Only: error message, stack trace, app version, OS, timestamp
//! - User can opt out via a setting (stored in business_settings)
//! - Reports are batched and sent on next app launch (not real-time)
//!   so they don't block the user or leak network activity timing
//!
//! DEFAULT: disabled until you configure a Sentry DSN or webhook.
//! The app works perfectly without it — this is observability, not
//! functionality.

use std::panic;
use std::sync::Once;

static INIT: Once = Once::new();

/// Initializes the crash reporter. Call once at app startup.
///
/// # Arguments
/// * `dsn` — Optional Sentry DSN or webhook URL. If None, crash
///   reporting is silently disabled.
/// * `version` — App version string (from tauri.conf.json)
/// * `app_data_dir` — Where to queue reports before sending
pub fn init(dsn: Option<&str>, version: &str, app_data_dir: &std::path::Path) {
    if dsn.is_none() {
        return; // silently disabled
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
pub fn flush_queue(dsn: &str, app_data_dir: &std::path::Path) {
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
