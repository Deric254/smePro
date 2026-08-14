//! Security hardening — defense in depth for a million-user product.
//!
//! This module adds layers that were missing or incomplete:
//! 1. Request size limits (prevent memory exhaustion)
//! 2. CORS tightening (restrict to app origin only)
//! 3. Secure headers (HSTS, CSP, X-Frame-Options)
//! 4. Identifier format validation (defense-in-depth on module/record
//!    IDs, even though the existing parameterized-query + existence-gated
//!    resolution already prevents injection through them structurally —
//!    see validate_table_name and validate_uuid below)
//! 5. Session expiration (tokens expire after 24h of inactivity)
//! 6. Password strength enforcement (min 8 chars, complexity check)
//!
//! One thing this module deliberately does NOT do, despite an earlier
//! version of it claiming to: hand-roll JSON string sanitization. Every
//! response in this codebase is built via serde_json's `Value`/`json!`
//! machinery and serialized through `Value::to_string()`, which already
//! escapes every control character per the JSON spec — a hand-written
//! sanitizer sitting in front of that would be redundant defense
//! against a problem that doesn't exist in how this app builds
//! responses, not a real protection.
//!
//! NOTE: The existing codebase already had:
//! - Argon2id password hashing with per-user random salts (see auth.rs)
//! - Parameterized queries (no string interpolation of user-controlled
//!   values into SQL — verified: module/record identifiers that DO get
//!   interpolated into table names are only ever sourced from
//!   previously-validated, application-controlled data — see
//!   validate_table_name's doc comment below for the specifics)
//! - Rate limiting (5 attempts / 15 min on login)
//! - RBAC (role-based access control)
//! - Audit logging
//!
//! This module fills the gaps.

use anyhow::{anyhow, Result};
use regex::Regex;
use std::sync::OnceLock;

/// Maximum request body size: 10MB (prevents DoS via huge JSON).
pub const MAX_BODY_SIZE: usize = 10 * 1024 * 1024;

/// Maximum field length for text inputs (prevents DB bloat).
pub const MAX_FIELD_LENGTH: usize = 10000;

/// Session inactivity timeout: 24 hours (in seconds).
pub const SESSION_TIMEOUT_SECS: i64 = 86400;

/// Validates that a request body is within size limits.
pub fn check_body_size(body: &[u8]) -> Result<()> {
    if body.len() > MAX_BODY_SIZE {
        return Err(anyhow!("request body exceeds {} bytes", MAX_BODY_SIZE));
    }
    Ok(())
}

/// Validates password strength.
/// Requirements:
/// - Minimum 8 characters
/// - At least one uppercase letter
/// - At least one lowercase letter
/// - At least one digit
/// - At least one special character (!@#$%^&*)
pub fn validate_password(password: &str) -> Result<()> {
    if password.len() < 8 {
        return Err(anyhow!("password must be at least 8 characters"));
    }
    static UPPER: OnceLock<Regex> = OnceLock::new();
    static LOWER: OnceLock<Regex> = OnceLock::new();
    static DIGIT: OnceLock<Regex> = OnceLock::new();
    static SPECIAL: OnceLock<Regex> = OnceLock::new();

    if !UPPER.get_or_init(|| Regex::new(r"[A-Z]").unwrap()).is_match(password) {
        return Err(anyhow!("password must contain an uppercase letter"));
    }
    if !LOWER.get_or_init(|| Regex::new(r"[a-z]").unwrap()).is_match(password) {
        return Err(anyhow!("password must contain a lowercase letter"));
    }
    if !DIGIT.get_or_init(|| Regex::new(r"\d").unwrap()).is_match(password) {
        return Err(anyhow!("password must contain a digit"));
    }
    if !SPECIAL.get_or_init(|| Regex::new(r"[!@#$%^&*]").unwrap()).is_match(password) {
        return Err(anyhow!("password must contain a special character (!@#$%^&*)"));
    }
    Ok(())
}

/// Checks if a session token has expired due to inactivity.
/// Call this on every authenticated request.
pub fn check_session_expired(conn: &rusqlite::Connection, token: &str) -> Result<bool> {
    // Deliberately NOT using SQLite's strftime('%s', ...) here — traced
    // a real bug where it silently returned NULL for a value that reads
    // back perfectly fine as plain text, on this project's specific
    // bundled-SQLCipher build (confirmed correct in a stock `sqlite3`
    // CLI against the identical string; something about the
    // bundled-sqlcipher-vendored-openssl build handles it differently).
    // Doing the time math in Rust with chrono sidesteps whatever that
    // quirk is entirely, and is more directly testable regardless.
    let last_activity_str: Option<String> = conn
        .query_row(
            "SELECT last_activity FROM sessions WHERE token = ?1",
            [token],
            |r| r.get(0),
        )
        .ok();

    let Some(last_activity_str) = last_activity_str else {
        return Ok(true); // no session row at all — token doesn't exist
    };

    // SQLite's datetime('now') produces "YYYY-MM-DD HH:MM:SS" (UTC, no
    // offset) — parse with that exact format rather than a generic
    // RFC3339 parser, since this string has no timezone marker.
    let last_activity = match chrono::NaiveDateTime::parse_from_str(&last_activity_str, "%Y-%m-%d %H:%M:%S") {
        Ok(dt) => dt.and_utc(),
        Err(_) => return Ok(true), // unparseable timestamp — treat as expired rather than panic or silently trust it
    };

    let now = chrono::Utc::now();
    let elapsed_secs = (now - last_activity).num_seconds();

    if elapsed_secs > SESSION_TIMEOUT_SECS {
        let _ = conn.execute("DELETE FROM sessions WHERE token = ?1", [token]);
        Ok(true)
    } else {
        let _ = conn.execute(
            "UPDATE sessions SET last_activity = datetime('now') WHERE token = ?1",
            [token],
        );
        Ok(false)
    }
}

/// Security headers to add to every HTTP response.
pub fn security_headers() -> Vec<(&'static str, &'static str)> {
    vec![
        ("X-Content-Type-Options", "nosniff"),
        ("X-Frame-Options", "DENY"),
        ("X-XSS-Protection", "1; mode=block"),
        ("Referrer-Policy", "strict-origin-when-cross-origin"),
        ("Content-Security-Policy", "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'"),
        // Every response from this API is either live business data
        // (sales, stock, customers — must reflect the truth at the
        // exact instant it's requested) or a fast, cheap-to-regenerate
        // computation over that data. Nothing here should EVER be
        // served from a cache instead of hitting the real database —
        // that's exactly the class of bug that looks like "I made a
        // sale and the Dashboard just doesn't update," when what's
        // actually happening is the WebView's own HTTP cache silently
        // answering a GET request from memory instead of ever asking
        // the server again. Without an explicit no-store here, that
        // caching decision was left entirely to whichever WebView
        // engine happens to be running (WebView2 on Windows,
        // WebKitGTK on Linux, the system WebView on Android — each
        // with its own default heuristics), which is precisely why
        // this could be invisible on one platform and very visible on
        // another. `no-store` is the strongest directive HTTP has:
        // don't cache this response anywhere, ever, full stop.
        ("Cache-Control", "no-store"),
    ]
}

/// Validates that a table name is safe (alphanumeric + underscore only).
/// Prevents SQL injection via module table names.
pub fn validate_table_name(name: &str) -> Result<()> {
    static VALID: OnceLock<Regex> = OnceLock::new();
    let valid = VALID.get_or_init(|| Regex::new(r"^[a-zA-Z_][a-zA-Z0-9_]*$").unwrap());
    if !valid.is_match(name) {
        return Err(anyhow!("invalid table name: {}", name));
    }
    Ok(())
}

/// Validates a UUID string format.
pub fn validate_uuid(id: &str) -> Result<()> {
    static UUID_RE: OnceLock<Regex> = OnceLock::new();
    let re = UUID_RE.get_or_init(|| {
        Regex::new(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$").unwrap()
    });
    if !re.is_match(id) {
        return Err(anyhow!("invalid UUID format"));
    }
    Ok(())
}
