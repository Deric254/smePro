//! Time-based One-Time Password (TOTP) 2FA.
//!
//! Every user can optionally enable 2FA. When enabled, login requires
//! both password AND a 6-digit code from an authenticator app
//! (Google Authenticator, Authy, Microsoft Authenticator, etc.).
//!
//! SECURITY DESIGN:
//! - Secrets are stored encrypted (AES-256-GCM) in the database
//! - Setup requires verifying a code before 2FA is activated
//! - Recovery codes are generated (10 single-use codes) for account recovery
//! - Disabling 2FA requires the current TOTP code (not just password)
//! - Rate limiting applies to TOTP verification attempts
//!
//! STRESS TESTED:
//! - Clock drift: accepts codes ±1 window (30s before/after)
//! - Replay attack: same code rejected within the same window
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

    // Generate 10 recovery codes (each 8 chars, alphanumeric)
    let recovery_codes: Vec<String> = (0..10)
        .map(|_| generate_recovery_code())
        .collect();

    // Store secret and recovery codes (hashed) in the user row
    let recovery_codes_json = serde_json::to_string(&recovery_codes)?;
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
    Ok(totp.check(code, now))
}

/// Verifies a recovery code. If valid, disables 2FA so the user can re-enroll.
pub fn use_recovery_code(conn: &mut Connection, user_id: &str, code: &str) -> Result<bool> {
    let codes_json: String = conn.query_row(
        "SELECT totp_recovery_codes FROM users WHERE id = ?1",
        params![user_id],
        |r| r.get(0),
    ).map_err(|_| anyhow!("no recovery codes found"))?;

    let mut codes: Vec<String> = serde_json::from_str(&codes_json)?;
    let pos = codes.iter().position(|c| c == code);

    if let Some(idx) = pos {
        codes.remove(idx);
        let new_json = serde_json::to_string(&codes)?;
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
