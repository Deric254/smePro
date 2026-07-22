use anyhow::{anyhow, Result};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use rand_core::OsRng;
use rusqlite::{params, Connection};
use uuid::Uuid;

const SESSION_LIFETIME_HOURS: i64 = 12;

/// Hashes a plaintext secret (password OR security-question answer) with
/// Argon2id + a random salt. Every place in the codebase that stores a
/// credential must go through this — never store plaintext, ever.
pub fn hash_secret(plain: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(plain.as_bytes(), &salt)
        .map_err(|e| anyhow!("hashing failed: {e}"))?;
    Ok(hash.to_string())
}

/// Verifies a plaintext secret against a stored Argon2 hash.
pub fn verify_secret(plain: &str, hash: &str) -> bool {
    let parsed = match PasswordHash::new(hash) {
        Ok(p) => p,
        Err(_) => return false,
    };
    Argon2::default().verify_password(plain.as_bytes(), &parsed).is_ok()
}

/// Normalizes a security-question answer before hashing/comparing
/// (case/whitespace shouldn't lock someone out over "Blue" vs "blue ").
fn normalize_answer(a: &str) -> String {
    a.trim().to_lowercase()
}

/// Verifies username/password and, on success, issues a new session
/// token. Deliberately gives the same generic error whether the username
/// doesn't exist or the password is wrong, so login can't be used to
/// enumerate valid usernames.
pub fn login(conn: &Connection, business_id: &str, username: &str, password: &str) -> Result<String> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT id, password_hash FROM users WHERE business_id = ?1 AND username = ?2 AND active = 1",
            params![business_id, username],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();

    let (user_id, hash) = row.ok_or_else(|| anyhow!("invalid username or password"))?;
    if !verify_secret(password, &hash) {
        return Err(anyhow!("invalid username or password"));
    }

    let token = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO sessions (token, user_id, business_id, created_at, expires_at)
         VALUES (?1, ?2, ?3, datetime('now'), datetime('now', ?4))",
        params![token, user_id, business_id, format!("+{SESSION_LIFETIME_HOURS} hours")],
    )?;
    Ok(token)
}

/// Resolves a bearer token to (user_id, business_id), rejecting expired
/// or unknown tokens. This is what every protected API route calls first.
pub fn current_user(conn: &Connection, token: &str) -> Result<(String, String)> {
    conn.query_row(
        "SELECT user_id, business_id FROM sessions
         WHERE token = ?1 AND expires_at > datetime('now')",
        params![token],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .map_err(|_| anyhow!("session expired or invalid, please log in again"))
}

pub fn logout(conn: &Connection, token: &str) -> Result<()> {
    conn.execute("DELETE FROM sessions WHERE token = ?1", params![token])?;
    Ok(())
}

/// Sets (or replaces) a user's two security questions/answers. Called
/// from account setup, not from the recovery flow itself.
pub fn set_security_questions(
    conn: &Connection,
    user_id: &str,
    q1: &str,
    a1: &str,
    q2: &str,
    a2: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE users SET security_q1 = ?1, security_a1_hash = ?2,
                           security_q2 = ?3, security_a2_hash = ?4
         WHERE id = ?5",
        params![
            q1,
            hash_secret(&normalize_answer(a1))?,
            q2,
            hash_secret(&normalize_answer(a2))?,
            user_id
        ],
    )?;
    Ok(())
}

/// Step 1 of forgot-password: security questions. Both answers must be
/// correct — a single question is too easy to guess/social-engineer.
pub fn recover_via_security_questions(
    conn: &Connection,
    business_id: &str,
    username: &str,
    answer1: &str,
    answer2: &str,
    new_password: &str,
) -> Result<()> {
    let row: (String, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT id, security_a1_hash, security_a2_hash FROM users
             WHERE business_id = ?1 AND username = ?2",
            params![business_id, username],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|_| anyhow!("account not found"))?;

    let (user_id, h1, h2) = row;
    let (h1, h2) = match (h1, h2) {
        (Some(h1), Some(h2)) => (h1, h2),
        _ => return Err(anyhow!("security questions not set up for this account")),
    };

    if !verify_secret(&normalize_answer(answer1), &h1) || !verify_secret(&normalize_answer(answer2), &h2) {
        return Err(anyhow!("security answers did not match"));
    }

    conn.execute(
        "UPDATE users SET password_hash = ?1 WHERE id = ?2",
        params![hash_secret(new_password)?, user_id],
    )?;
    // Invalidate any existing sessions — a password reset should force
    // re-login everywhere, including on a device an attacker was using.
    conn.execute("DELETE FROM sessions WHERE user_id = ?1", params![user_id])?;
    Ok(())
}

/// Step 2 (last resort): the admin master recovery code. This is the
/// fallback when security questions are forgotten too — e.g. the owner
/// lost the phone with the answers. The code itself is generated once
/// per install and only its hash is ever stored.
pub fn recover_via_admin_code(
    conn: &Connection,
    business_id: &str,
    admin_code: &str,
    username: &str,
    new_password: &str,
) -> Result<()> {
    let code_hash: String = conn
        .query_row(
            "SELECT admin_code_hash FROM admin_recovery WHERE business_id = ?1",
            params![business_id],
            |r| r.get(0),
        )
        .map_err(|_| anyhow!("no admin recovery code has been set up for this business"))?;

    if !verify_secret(admin_code, &code_hash) {
        return Err(anyhow!("invalid admin recovery code"));
    }

    let user_id: String = conn
        .query_row(
            "SELECT id FROM users WHERE business_id = ?1 AND username = ?2",
            params![business_id, username],
            |r| r.get(0),
        )
        .map_err(|_| anyhow!("account not found"))?;

    conn.execute(
        "UPDATE users SET password_hash = ?1 WHERE id = ?2",
        params![hash_secret(new_password)?, user_id],
    )?;
    conn.execute("DELETE FROM sessions WHERE user_id = ?1", params![user_id])?;
    Ok(())
}
