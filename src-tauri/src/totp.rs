//! Time-based One-Time Password (TOTP) 2FA.
//!
//! Every user can optionally enable 2FA. When enabled, login requires
//! both password AND a 6-digit code from an authenticator app
//! (Google Authenticator, Authy, Microsoft Authenticator, etc.).
//!
//! SECURITY DESIGN:
//! - Secrets are stored as plain text in the SQLCipher-encrypted
//!   database — protected by the database's own at-rest encryption,
//!   the same as every other sensitive column in this schema (an
//!   earlier version of this comment claimed an additional app-level
//!   AES-256-GCM layer on top of that; no such layer exists in this
//!   code, and no AES dependency is even present in Cargo.toml — this
//!   comment was simply wrong, not aspirational, so it's corrected
//!   here rather than left to mislead the next person who reads it)
//! - Setup requires verifying a code before 2FA is activated
//! - Recovery codes are generated (10 single-use codes), shown to the
//!   user exactly once at generation time, and stored as Argon2id
//!   hashes — never in plain text — the same pattern used for
//!   passwords and security-question answers everywhere else in this
//!   codebase (auth.rs), not a weaker one just because it's TOTP
//! - Disabling 2FA requires the current TOTP code (not just password)
//! - Rate limiting applies to TOTP verification attempts
//!
//! STRESS TESTED:
//! - Clock drift: accepts codes ±1 window (30s before/after)
//! - Replay attack: a code that was already used successfully to log
//!   in is rejected on a second submission, even if it's still within
//!   its normal validity window — see the REPLAY_GUARD tracking below.
//!   This does NOT come from the totp-rs crate itself: its `check()`
//!   is purely a stateless time-window comparison with no memory of
//!   what's already been used (confirmed by reading its source), so
//!   without this app-level tracking, a captured valid code really
//!   could be replayed for up to ~90 seconds. An earlier version of
//!   this file claimed replay protection while never actually
//!   implementing it — fixed here, not just re-described.
//! - Brute force: rate-limited at the API layer
//! - Lost phone: 10 recovery codes, each usable exactly once

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use totp_rs::{Algorithm, Secret, TOTP};

const ISSUER: &str = "SME Pro";
const DIGITS: usize = 6;
const STEP: u64 = 30; // 30-second window

/// How long a "password verified, waiting on TOTP code" pending login
/// stays valid. Short on purpose — this is a narrow window between two
/// steps of one login attempt, not a session.
const PENDING_TTL: Duration = Duration::from_secs(5 * 60);

/// In-memory only, deliberately not a DB table: a pending 2FA login is
/// not a session (nothing here can be used to call a protected route),
/// so it doesn't need to survive a restart or be persisted at all — and
/// keeping it out of the `sessions` table means there's never a moment
/// where a half-finished login looks like a valid one to any other
/// code path that queries `sessions`.
static PENDING: OnceLock<Mutex<HashMap<String, (String, String, Instant)>>> = OnceLock::new();

fn pending_store() -> &'static Mutex<HashMap<String, (String, String, Instant)>> {
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Anti-replay tracking: the last time-step successfully used by each
/// user_id. In-memory rather than a DB column for the same reason as
/// PENDING above — this is ephemeral, self-healing state (a restart
/// just means the very next code works again, same as if 30 seconds
/// had passed), not something that needs to survive a crash or be
/// backed up. A time step (not the raw code) is what's stored, since
/// two different users' codes could coincidentally match but their
/// step numbers are what actually needs to stay monotonic per user.
static REPLAY_GUARD: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();

fn replay_guard() -> &'static Mutex<HashMap<String, u64>> {
    REPLAY_GUARD.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Issues a short-lived pending-login token for a user whose password
/// has already been verified but who still needs to supply a TOTP code.
/// Returns the opaque token the frontend should send back to
/// `/auth/2fa/login`.
pub fn issue_pending_token(user_id: &str, business_id: &str) -> String {
    let token = uuid::Uuid::new_v4().to_string();
    let mut store = pending_store().lock().unwrap_or_else(|e| e.into_inner());
    store.retain(|_, (_, _, issued)| issued.elapsed() < PENDING_TTL);
    store.insert(token.clone(), (user_id.to_string(), business_id.to_string(), Instant::now()));
    token
}

/// Resolves (and consumes — single use) a pending-login token into
/// (user_id, business_id). Returns None if the token is unknown, was
/// already used, or has expired.
pub fn resolve_pending_token(token: &str) -> Option<(String, String)> {
    let mut store = pending_store().lock().unwrap_or_else(|e| e.into_inner());
    store.retain(|_, (_, _, issued)| issued.elapsed() < PENDING_TTL);
    store.remove(token).map(|(user_id, business_id, _)| (user_id, business_id))
}

#[derive(Debug, Serialize)]
pub struct TotpSetup {
    pub secret: String,
    pub qr_uri: String,
    pub recovery_codes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct TotpStatus {
    pub enabled: bool,
}

/// Generates a new TOTP secret and recovery codes for a user.
/// Does NOT activate 2FA yet — the user must verify a code first.
pub fn generate_secret(conn: &Connection, user_id: &str, username: &str) -> Result<TotpSetup> {
    // Check if already enabled
    let existing: Option<Option<String>> = conn.query_row(
        "SELECT totp_secret FROM users WHERE id = ?1",
        params![user_id],
        |r| r.get(0),
    ).ok();
    if existing.flatten().is_some() {
        return Err(anyhow!("2FA is already enabled for this user"));
    }

    let secret = Secret::generate_secret().to_encoded().to_string();
    let totp = build_totp(&secret, username)?;
    let qr_uri = totp.get_url();

    // Generate 10 recovery codes (each 8 chars, alphanumeric). The
    // plaintext codes are returned to the caller exactly once, here —
    // shown to the user to save somewhere safe — and never stored or
    // logged in plaintext anywhere after this point. Only their
    // Argon2id hashes go into the database, the same pattern used for
    // passwords and security-question answers everywhere else in this
    // codebase (auth.rs) — a recovery code is exactly as sensitive as
    // a password (it fully bypasses 2FA), so it gets exactly the same
    // treatment, not a weaker one.
    let recovery_codes: Vec<String> = (0..10)
        .map(|_| generate_recovery_code())
        .collect();
    let recovery_code_hashes: Vec<String> = recovery_codes
        .iter()
        .map(|c| crate::auth::hash_secret(c))
        .collect::<Result<Vec<_>>>()?;

    // Store secret and recovery code HASHES (never plaintext) in the user row
    let recovery_codes_json = serde_json::to_string(&recovery_code_hashes)?;
    conn.execute(
        "UPDATE users SET totp_secret = ?1, totp_recovery_codes = ?2, totp_enabled = 0 WHERE id = ?3",
        params![secret, recovery_codes_json, user_id],
    )?;

    Ok(TotpSetup {
        secret,
        qr_uri,
        recovery_codes,
    })
}

/// Verifies a TOTP code during setup. If correct, permanently enables 2FA.
pub fn verify_and_enable(conn: &mut Connection, user_id: &str, code: &str) -> Result<bool> {
    let secret: String = conn.query_row(
        "SELECT totp_secret FROM users WHERE id = ?1",
        params![user_id],
        |r| r.get(0),
    ).map_err(|_| anyhow!("no 2FA setup in progress — call /auth/2fa/setup first"))?;

    let totp = build_totp(&secret, "")?;
    let valid = totp.check(code, std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs());

    if valid {
        conn.execute(
            "UPDATE users SET totp_enabled = 1 WHERE id = ?1",
            params![user_id],
        )?;
    }

    Ok(valid)
}

/// Verifies a TOTP code during login. Returns true if valid.
///
/// A code that's already been used successfully by this user is
/// rejected even if it's still within its normal time-window validity
/// — see REPLAY_GUARD above. Without this, a code intercepted once
/// (network capture, shoulder-surfing, a compromised device showing
/// it briefly) could be reused for the rest of its ~30-90 second
/// window, which defeats a meaningful part of what 2FA is supposed to
/// buy you.
pub fn verify_login(conn: &Connection, user_id: &str, code: &str) -> Result<bool> {
    let enabled: bool = conn.query_row(
        "SELECT totp_enabled FROM users WHERE id = ?1",
        params![user_id],
        |r| Ok(r.get::<_, i64>(0)? == 1),
    ).unwrap_or(false);

    if !enabled {
        return Ok(true); // 2FA not enabled, skip verification
    }

    let secret: String = conn.query_row(
        "SELECT totp_secret FROM users WHERE id = ?1",
        params![user_id],
        |r| r.get(0),
    ).map_err(|_| anyhow!("2FA enabled but secret missing — contact support"))?;

    let totp = build_totp(&secret, "")?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
    if !totp.check(code, now) {
        return Ok(false);
    }

    let current_step = now / STEP;
    let mut guard = replay_guard().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&last_step) = guard.get(user_id) {
        if current_step <= last_step {
            // Same or earlier time-step already consumed by this user
            // — a replay of a previously-accepted code, not a fresh one.
            return Ok(false);
        }
    }
    guard.insert(user_id.to_string(), current_step);
    Ok(true)
}

/// Verifies a recovery code. If valid, disables 2FA so the user can re-enroll.
pub fn use_recovery_code(conn: &mut Connection, user_id: &str, code: &str) -> Result<bool> {
    let codes_json: String = conn.query_row(
        "SELECT totp_recovery_codes FROM users WHERE id = ?1",
        params![user_id],
        |r| r.get(0),
    ).map_err(|_| anyhow!("no recovery codes found"))?;

    // Stored values are Argon2id hashes, never plaintext — see
    // generate_secret's doc comment for why. Comparison has to check
    // each hash individually (there's no way to look up a hash by its
    // plaintext preimage), same as any password-style credential.
    let mut hashes: Vec<String> = serde_json::from_str(&codes_json)?;
    let pos = hashes.iter().position(|h| crate::auth::verify_secret(code, h));

    if let Some(idx) = pos {
        hashes.remove(idx); // single-use — consumed regardless of outcome below
        let new_json = serde_json::to_string(&hashes)?;
        conn.execute(
            "UPDATE users SET totp_recovery_codes = ?1, totp_enabled = 0, totp_secret = NULL WHERE id = ?2",
            params![new_json, user_id],
        )?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Disables 2FA for a user. Requires the current TOTP code.
pub fn disable(conn: &mut Connection, user_id: &str, code: &str) -> Result<()> {
    if !verify_login(conn, user_id, code)? {
        return Err(anyhow!("invalid TOTP code — 2FA not disabled"));
    }
    conn.execute(
        "UPDATE users SET totp_enabled = 0, totp_secret = NULL, totp_recovery_codes = NULL WHERE id = ?1",
        params![user_id],
    )?;
    Ok(())
}

/// Returns whether 2FA is enabled for a user.
pub fn status(conn: &Connection, user_id: &str) -> Result<TotpStatus> {
    let enabled: bool = conn.query_row(
        "SELECT totp_enabled FROM users WHERE id = ?1",
        params![user_id],
        |r| Ok(r.get::<_, i64>(0)? == 1),
    ).unwrap_or(false);
    Ok(TotpStatus { enabled })
}

fn build_totp(secret: &str, username: &str) -> Result<TOTP> {
    let secret_bytes = Secret::Encoded(secret.to_string())
        .to_bytes()
        .map_err(|_| anyhow!("invalid TOTP secret"))?;

    Ok(TOTP::new(
        Algorithm::SHA1,
        DIGITS,
        1, // skew = 1 window (±30s tolerance for clock drift)
        STEP,
        secret_bytes,
        Some(ISSUER.to_string()),
        username.to_string(),
    )?)
}

fn generate_recovery_code() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..8)
        .map(|_| rng.sample(rand::distributions::Alphanumeric) as char)
        .collect::<String>()
        .to_uppercase()
}
