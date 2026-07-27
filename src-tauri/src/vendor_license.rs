//! Vendor-issued license key redemption — offline, cryptographically
//! verified, no server required.
//!
//! Earlier version of this module called out to a separate always-on
//! server (`vendor-authority/`) to check whether a key was genuine.
//! That design gave real cross-device enforcement, but it costs money
//! to run and something has to keep it online forever. This version
//! trades that away for zero ongoing cost: every key is signed offline
//! with a private key only the vendor holds (see `offline_keygen.rs`),
//! and this module verifies that signature using nothing but the
//! matching public key baked into the app. No network call, ever.
//!
//! WHAT THIS DOES guarantee: a key is genuine — it was actually signed
//! by whoever holds the private key, not guessed or forged. Tampering
//! with even one character invalidates the signature.
//!
//! WHAT THIS DOES NOT guarantee, stated plainly: that a key is used on
//! only one device. Without a central server watching every install,
//! nothing here can see whether the same key has already been redeemed
//! somewhere else. What this module CAN still do locally: once a key is
//! bound to this device, it refuses to silently swap to a different key
//! (see `redeem()` below) — so at minimum, one install can't quietly
//! rotate through multiple keys, and the same key pasted twice on the
//! SAME device is a safe no-op, not an error.

use anyhow::{anyhow, Result};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};

/// The vendor's PUBLIC signing key. Safe to embed — this is not a
/// secret, it's what makes verification possible without one. Generate
/// your own real keypair before shipping:
///
///   cargo build --release --bin offline_keygen --features dev-tools
///   ./target/release/offline_keygen generate-keypair
///
/// then replace this placeholder with the public key it prints. Until
/// you do, every key will correctly fail to verify — this is a
/// deliberately invalid placeholder, not a working demo key (shipping a
/// real demo key that touched a shared session would be exactly the
/// "must be treated as compromised" mistake this project has already
/// caught itself making once before, with the desktop updater key).
const LICENSE_PUBLIC_KEY_HEX: &str = "0000000000000000000000000000000000000000000000000000000000000000";

const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ"; // Crockford base32
const NONCE_LEN: usize = 8;
const SIGNATURE_LEN: usize = 64;
const PAYLOAD_LEN: usize = NONCE_LEN + SIGNATURE_LEN;
/// ceil(PAYLOAD_LEN * 8 / 5) — the exact base32 character count for a
/// 72-byte payload. Used to reject an obviously-wrong-length key
/// instantly, before ever attempting to decode or verify it.
const ENCODED_LEN: usize = 116;

fn public_key() -> Result<VerifyingKey> {
    let bytes = hex::decode(LICENSE_PUBLIC_KEY_HEX).map_err(|e| anyhow!("invalid embedded public key: {e}"))?;
    let arr: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("embedded public key must be exactly 32 bytes"))?;
    VerifyingKey::from_bytes(&arr).map_err(|e| anyhow!("embedded public key is not a valid Ed25519 key: {e}"))
}

fn base32_decode(s: &str) -> Result<Vec<u8>> {
    let mut bits: u32 = 0;
    let mut bit_count: u32 = 0;
    let mut out = Vec::new();
    for c in s.chars() {
        let val = ALPHABET
            .iter()
            .position(|&a| a as char == c)
            .ok_or_else(|| anyhow!("key contains an invalid character: '{c}'"))? as u32;
        bits = (bits << 5) | val;
        bit_count += 5;
        if bit_count >= 8 {
            bit_count -= 8;
            out.push(((bits >> bit_count) & 0xFF) as u8);
        }
    }
    Ok(out)
}

/// Local-only format check — rejects an obviously malformed or
/// mistyped key instantly, before the (still local, but slightly more
/// expensive) actual cryptographic verification.
pub fn validate_key_format(key: &str) -> Result<()> {
    let stripped = key.trim().to_uppercase();
    let body: String = stripped
        .strip_prefix("SPK-")
        .ok_or_else(|| anyhow!("key must start with SPK-"))?
        .chars()
        .filter(|c| *c != '-')
        .collect();
    if body.len() != ENCODED_LEN {
        return Err(anyhow!("key is the wrong length — check for a missing or extra character"));
    }
    Ok(())
}

/// Reads (or creates, on first run) a stable random device identifier.
/// Stored as a plain UUID in the same encrypted SQLite database as
/// everything else — not a separate plaintext file — so it inherits
/// the app's existing at-rest encryption instead of being a second,
/// weaker place secrets live.
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

/// Verifies `key`'s signature entirely offline against the embedded
/// public key, then persists the result locally forever — no network
/// call anywhere in this function. Safe to call again later with the
/// SAME key (a no-op if we're already licensed) — but refuses to bind
/// a second, different key over an existing local activation, since
/// that almost always means "the user mistyped and is retrying," not
/// "replace my license."
pub fn redeem(conn: &Connection, key: &str) -> Result<Value> {
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

    validate_key_format(key)?;
    let stripped = key.trim().to_uppercase();
    let body: String = stripped.strip_prefix("SPK-").unwrap().chars().filter(|c| *c != '-').collect();
    let payload = base32_decode(&body)?;
    if payload.len() != PAYLOAD_LEN {
        return Err(anyhow!("key is malformed"));
    }

    let nonce = &payload[..NONCE_LEN];
    let sig_bytes: [u8; SIGNATURE_LEN] = payload[NONCE_LEN..]
        .try_into()
        .map_err(|_| anyhow!("key is malformed"))?;
    let signature = Signature::from_bytes(&sig_bytes);

    let pubkey = public_key()?;
    pubkey
        .verify(nonce, &signature)
        .map_err(|_| anyhow!("this license key isn't valid — double check it was copied correctly, with nothing missing"))?;

    // Genuinely signed by the vendor's private key. Bind it to this
    // device, permanently, right now.
    let key_id = hex::encode(nonce);
    let id = device_id(conn)?;
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO vendor_license (id, key_id, device_id, activated_at) VALUES (1, ?1, ?2, ?3)",
        rusqlite::params![key_id, id, now],
    )?;

    Ok(json!({"ok": true, "key_id": key_id, "activated_at": now}))
}
