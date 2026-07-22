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

pub fn get_all(conn: &Connection, business_id: &str) -> Result<Value> {
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
