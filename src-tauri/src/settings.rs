//! Generic per-business settings — theme, locale, date format, and
//! anything else that should be configurable without a schema migration
//! every time a new preference is needed.
//!
//! Deliberately a plain key/value store rather than named columns on
//! `businesses`: the frontend can introduce a new setting (a new theme
//! name, a display density toggle, whatever) without any backend change
//! at all. The engine doesn't know or care what "theme" means — it's
//! just a string the frontend interprets, which is exactly what keeps
//! this from becoming another hardcoded list.

use anyhow::Result;
use rusqlite::{params, Connection};
use serde_json::{json, Value};

/// Every stored setting EXCEPT API keys — those are write-only from
/// this function's point of view, on purpose. GET /settings (the HTTP
/// route this feeds) has no admin gate, by design, since ordinary
/// settings like the theme need to be readable by every signed-in
/// role, not just admins. That's fine for a theme; it would be a real
/// secret leak for an AI provider's API key, readable back in plain
/// text by any Staff-tier account. Filtering here, at the source,
/// means every current and future caller of get_all() is safe by
/// construction — not dependent on each one remembering to redact.
pub fn get_all(conn: &Connection, business_id: &str) -> Result<Value> {
    let mut stmt = conn.prepare("SELECT key, value FROM business_settings WHERE business_id = ?1")?;
    let rows: Vec<(String, String)> = stmt
        .query_map(params![business_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();
    let mut map = serde_json::Map::new();
    for (k, v) in rows {
        if k.ends_with("_api_key") {
            continue;
        }
        map.insert(k, json!(v));
    }
    Ok(Value::Object(map))
}

/// Reads a single setting, or None if it was never set. Used by
/// ai_assistant.rs to check for a stored API key before falling back
/// to an environment variable — the actual reachable path for a real
/// customer, who has no way to set an OS environment variable before
/// launching a desktop app from an icon.
pub fn get(conn: &Connection, business_id: &str, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM business_settings WHERE business_id = ?1 AND key = ?2",
        params![business_id, key],
        |r| r.get(0),
    )
    .ok()
}

/// The unfiltered version of get_all() — includes API keys. NEVER
/// wire this to an unauthenticated or non-admin-gated route; it exists
/// specifically for the already-admin-gated GET /ai/settings endpoint,
/// which itself only ever reports whether a key is set, not its value.
pub fn get_all_including_keys(conn: &Connection, business_id: &str) -> Result<Value> {
    let mut stmt = conn.prepare("SELECT key, value FROM business_settings WHERE business_id = ?1")?;
    let rows: Vec<(String, String)> = stmt
        .query_map(params![business_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();
    let mut map = serde_json::Map::new();
    for (k, v) in rows {
        map.insert(k, json!(v));
    }
    Ok(Value::Object(map))
}

pub fn set(conn: &Connection, business_id: &str, key: &str, value: &str) -> Result<()> {
    let key = key.trim();
    if key.is_empty() {
        anyhow::bail!("setting key cannot be empty");
    }
    if key.len() > 128 {
        anyhow::bail!("setting key is too long (max 128 characters)");
    }
    conn.execute(
        "INSERT INTO business_settings (business_id, key, value, updated_at) VALUES (?1, ?2, ?3, datetime('now'))
         ON CONFLICT(business_id, key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        params![business_id, key, value],
    )?;
    Ok(())
}
