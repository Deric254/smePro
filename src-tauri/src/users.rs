//! User lifecycle management. Before this, the ONLY way a user account
//! ever got created was `POST /setup/create-business` (the very first
//! Owner, once, at install time) — there was no way for an Owner to add
//! a Cashier, a second Staff account, or anyone else afterward through
//! the API. Every custom role this app can now create would have been
//! unusable without this: a role nobody can be assigned to isn't a real
//! feature.

use crate::auth;
use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use uuid::Uuid;

pub fn list_users(conn: &Connection, business_id: &str) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT u.id, u.username, r.name, u.active, u.created_at
         FROM users u JOIN roles r ON r.id = u.role_id
         WHERE u.business_id = ?1 ORDER BY u.created_at",
    )?;
    let rows = stmt.query_map(params![business_id], |r| {
        Ok(json!({
            "id": r.get::<_, String>(0)?,
            "username": r.get::<_, String>(1)?,
            "role": r.get::<_, String>(2)?,
            "active": r.get::<_, i64>(3)? == 1,
            "created_at": r.get::<_, String>(4)?,
        }))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// Creates a new user under an existing role, with the same forced
/// security-question setup the first-run Owner account requires — a
/// staff account with no recovery path is a support ticket waiting to
/// happen the first time someone forgets their password.
pub fn create_user(
    conn: &Connection,
    business_id: &str,
    username: &str,
    password: &str,
    role_id: &str,
    security_q1: &str,
    security_a1: &str,
    security_q2: &str,
    security_a2: &str,
) -> Result<String> {
    let username = username.trim();
    if username.is_empty() {
        return Err(anyhow!("username cannot be empty"));
    }
    if password.len() < 8 {
        return Err(anyhow!("password must be at least 8 characters"));
    }
    if security_q1.is_empty() || security_a1.is_empty() || security_q2.is_empty() || security_a2.is_empty() {
        return Err(anyhow!("both security questions and answers are required"));
    }
    let _: String = conn
        .query_row(
            "SELECT id FROM roles WHERE id = ?1 AND business_id = ?2",
            params![role_id, business_id],
            |r| r.get(0),
        )
        .map_err(|_| anyhow!("role not found"))?;

    let password_hash = auth::hash_secret(password)?;
    let user_id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO users (id, business_id, username, password_hash, role_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
        params![user_id, business_id, username, password_hash, role_id],
    )
    .map_err(|e| {
        if e.to_string().to_uppercase().contains("UNIQUE") {
            anyhow!("a user named '{username}' already exists")
        } else {
            anyhow!(e)
        }
    })?;
    auth::set_security_questions(conn, &user_id, security_q1, security_a1, security_q2, security_a2)?;
    Ok(user_id)
}

/// Reassigns a user to a different role. Refuses to move the LAST active
/// user out of the Owner role — a business with zero Owners is bricked
/// (nobody left who can manage roles, activate licenses, or add more
/// users), so this is checked directly rather than trusted to the
/// caller's judgment.
pub fn set_role(conn: &Connection, business_id: &str, user_id: &str, new_role_id: &str) -> Result<()> {
    let current_role_id: String = conn
        .query_row(
            "SELECT role_id FROM users WHERE id = ?1 AND business_id = ?2",
            params![user_id, business_id],
            |r| r.get(0),
        )
        .map_err(|_| anyhow!("user not found"))?;

    let _: String = conn
        .query_row(
            "SELECT id FROM roles WHERE id = ?1 AND business_id = ?2",
            params![new_role_id, business_id],
            |r| r.get(0),
        )
        .map_err(|_| anyhow!("target role not found"))?;

    let current_is_owner: i64 = conn.query_row(
        "SELECT is_system FROM roles WHERE id = ?1",
        params![current_role_id],
        |r| r.get(0),
    )?;
    if current_is_owner == 1 {
        let other_owners: i64 = conn.query_row(
            "SELECT COUNT(*) FROM users u JOIN roles r ON r.id = u.role_id
             WHERE u.business_id = ?1 AND r.is_system = 1 AND u.active = 1 AND u.id != ?2",
            params![business_id, user_id],
            |r| r.get(0),
        )?;
        if other_owners == 0 {
            return Err(anyhow!("cannot reassign the last active Owner — a business must always have at least one"));
        }
    }

    conn.execute("UPDATE users SET role_id = ?1 WHERE id = ?2 AND business_id = ?3", params![new_role_id, user_id, business_id])?;
    // Role changed — force re-login everywhere so stale permission
    // assumptions in an already-open session can't linger.
    conn.execute("DELETE FROM sessions WHERE user_id = ?1", params![user_id])?;
    Ok(())
}

/// Deactivates (never hard-deletes) a user — soft-delete, same principle
/// as every other record in this app: the audit trail should always be
/// able to say who did what, even for someone no longer employed. Also
/// immediately revokes every session, so a deactivated account can't
/// keep acting through a token issued before the deactivation.
/// Same last-Owner protection as `set_role`.
pub fn deactivate_user(conn: &Connection, business_id: &str, user_id: &str) -> Result<()> {
    let role_id: String = conn
        .query_row(
            "SELECT role_id FROM users WHERE id = ?1 AND business_id = ?2 AND active = 1",
            params![user_id, business_id],
            |r| r.get(0),
        )
        .map_err(|_| anyhow!("active user not found"))?;
    let is_owner: i64 = conn.query_row("SELECT is_system FROM roles WHERE id = ?1", params![role_id], |r| r.get(0))?;
    if is_owner == 1 {
        let other_owners: i64 = conn.query_row(
            "SELECT COUNT(*) FROM users u JOIN roles r ON r.id = u.role_id
             WHERE u.business_id = ?1 AND r.is_system = 1 AND u.active = 1 AND u.id != ?2",
            params![business_id, user_id],
            |r| r.get(0),
        )?;
        if other_owners == 0 {
            return Err(anyhow!("cannot deactivate the last active Owner — a business must always have at least one"));
        }
    }
    conn.execute("UPDATE users SET active = 0 WHERE id = ?1 AND business_id = ?2", params![user_id, business_id])?;
    conn.execute("DELETE FROM sessions WHERE user_id = ?1", params![user_id])?;
    Ok(())
}
