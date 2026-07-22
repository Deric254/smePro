//! Vendor-issued license key redemption — a SEPARATE mechanism from the
//! recurring payment license in `license.rs` (which is about SME
//! subscription billing). This one is the classic "one key, one device"
//! software-license model: the vendor (you) issues a key out-of-band via
//! the `vendor_authority` service, gives it to a customer, and this
//! module is what that customer's install calls, exactly once, to
//! redeem it.
//!
//! Single-device enforcement genuinely lives on the vendor's server (see
//! `vendor-authority/`), because that's the only place that can see
//! every install at once — this local module can't enforce anything by
//! itself, it can only ask. What it CAN do locally: generate a stable
//! per-install device_id, persist the successful activation once so the
//! app never has to phone home again, and refuse to silently re-run
//! redemption against a different key once one is already bound here.

use anyhow::{anyhow, Result};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use std::time::Duration;

/// Reads (or creates, on first run) a stable random device identifier.
/// Stored as a plain UUID in the same encrypted SQLite database as
/// everything else — not a separate plaintext file — so it inherits the
/// app's existing at-rest encryption instead of being a second, weaker
/// place secrets live.
pub fn device_id(conn: &Connection) -> Result<String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS vendor_device (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            device_id TEXT NOT NULL
        );",
    )?;
    let existing: Option<String> = conn
        .query_row("SELECT device_id FROM vendor_device WHERE id = 1", [], |r| r.get(0))
        .optional()?;
    if let Some(id) = existing {
        return Ok(id);
    }
    let new_id = uuid::Uuid::new_v4().to_string();
    conn.execute("INSERT INTO vendor_device (id, device_id) VALUES (1, ?1)", [&new_id])?;
    Ok(new_id)
}

/// Local cache of a successful redemption. Presence of this row IS the
/// license, from the app's point of view — checked on every launch
/// without any network call. `redeem()` is the only thing that writes it.
fn ensure_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS vendor_license (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            key_id TEXT NOT NULL,
            device_id TEXT NOT NULL,
            activated_at TEXT NOT NULL
        );",
    )?;
    Ok(())
}

pub fn status(conn: &Connection) -> Result<Value> {
    ensure_table(conn)?;
    let row: Option<(String, String, String)> = conn
        .query_row(
            "SELECT key_id, device_id, activated_at FROM vendor_license WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    match row {
        Some((key_id, device_id, activated_at)) => {
            Ok(json!({"licensed": true, "key_id": key_id, "device_id": device_id, "activated_at": activated_at}))
        }
        None => Ok(json!({"licensed": false})),
    }
}

/// Calls the vendor's authority server exactly once to redeem `key` for
/// this device, then persists the result locally forever. Safe to call
/// again later with the SAME key (idempotent on the server side, and a
/// no-op here if we're already licensed) — but refuses to bind a second,
/// different key over an existing local activation, since that almost
/// always means "the user mistyped and is retrying," not "replace my
/// license," and silently allowing it would make it easy to paper over a
/// real error.
pub fn redeem(conn: &Connection, vendor_url: &str, key: &str) -> Result<Value> {
    ensure_table(conn)?;
    if let Some((existing_key_id, _, activated_at)) = conn
        .query_row("SELECT key_id, device_id, activated_at FROM vendor_license WHERE id = 1", [], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })
        .optional()?
    {
        return Ok(json!({
            "ok": true,
            "key_id": existing_key_id,
            "activated_at": activated_at,
            "note": "this device is already licensed; ignoring redemption attempt"
        }));
    }

    let id = device_id(conn)?;
    let payload = json!({"key": key, "device_id": id}).to_string();

    let response = ureq::post(&format!("{}/redeem", vendor_url.trim_end_matches('/')))
        .set("Content-Type", "application/json")
        .timeout(Duration::from_secs(10))
        .send_string(&payload);

    let resp = match response {
        Ok(r) => r,
        Err(ureq::Error::Status(code, r)) => {
            let body: Value = r.into_json().unwrap_or_else(|_| json!({}));
            let msg = body.get("error").and_then(|v| v.as_str()).unwrap_or("license key rejected");
            return Err(anyhow!("{msg} (status {code})"));
        }
        Err(e) => return Err(anyhow!("could not reach the license server: {e}")),
    };

    let body: Value = resp.into_json().map_err(|e| anyhow!("invalid response from license server: {e}"))?;
    let key_id = body.get("key_id").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
    let activated_at = body
        .get("activated_at")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    conn.execute(
        "INSERT INTO vendor_license (id, key_id, device_id, activated_at) VALUES (1, ?1, ?2, ?3)",
        rusqlite::params![key_id, id, activated_at],
    )?;

    Ok(json!({"ok": true, "key_id": key_id, "activated_at": activated_at}))
}

/// Local-only format check, so a mistyped key gets an instant, clear
/// error instead of a network round trip. Mirrors the vendor authority's
/// own checksum — kept in sync manually since they're separate crates by
/// design (the client should never need the vendor's issuing logic).
pub fn validate_key_format(key: &str) -> Result<()> {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let stripped = key.trim().to_uppercase();
    let body: String = stripped
        .strip_prefix("LKC-")
        .ok_or_else(|| anyhow!("key must start with LKC-"))?
        .chars()
        .filter(|c| *c != '-')
        .collect();
    if body.len() != 16 {
        return Err(anyhow!("key is the wrong length"));
    }
    let idx_of = |c: char| -> Result<usize> {
        ALPHABET
            .iter()
            .position(|&a| a as char == c)
            .ok_or_else(|| anyhow!("key contains an invalid character: '{c}'"))
    };
    let mut sum = 0usize;
    for c in body[..15].chars() {
        sum += idx_of(c)?;
    }
    let expected = ALPHABET[sum % ALPHABET.len()] as char;
    let actual = body.chars().nth(15).unwrap();
    if expected != actual {
        return Err(anyhow!("key checksum does not match — likely a typo"));
    }
    Ok(())
}
