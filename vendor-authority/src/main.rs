//! Vendor Authority — a small standalone service the VENDOR (you) runs.
//!
//! This is deliberately a separate binary from the SME app itself: it is
//! the one place that knows every key ever issued, so it's the only thing
//! that can actually enforce "one device only" across independent,
//! offline-first installs that otherwise never talk to each other. The
//! SME app calls this exactly once per install, at activation time, then
//! never needs to again — see `vendor_license.rs` in core_engine.
//!
//! Source of truth: a local SQLite database (`vendor_authority.db`),
//! never the key string itself. Only a SHA-256 hash of each key is ever
//! stored — the raw key is shown to you (the admin) exactly once, at
//! issuance, the same way a password would be.
//!
//! Usage:
//!   vendor_authority issue "Customer note"      # generate + print a new key
//!   vendor_authority list                       # list all issued keys
//!   vendor_authority revoke <key_id>            # cut off a key
//!   vendor_authority serve <port> <admin_token>  # run the HTTP service

use anyhow::{anyhow, Result};
use rand::RngCore;
use rusqlite::Connection;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::sync::{Arc, Mutex};

const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ"; // Crockford base32: no I, L, O, U — unambiguous by eye

fn db_path() -> String {
    std::env::var("VENDOR_DB").unwrap_or_else(|_| "vendor_authority.db".to_string())
}

fn open_db() -> Result<Connection> {
    let conn = Connection::open(db_path())?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS license_keys (
            key_hash     TEXT PRIMARY KEY,
            key_id       TEXT NOT NULL UNIQUE,
            note         TEXT,
            status       TEXT NOT NULL DEFAULT 'unused', -- unused | active | revoked
            device_id    TEXT,
            created_at   TEXT NOT NULL,
            activated_at TEXT,
            revoked_at   TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_license_keys_status ON license_keys(status);",
    )?;
    Ok(conn)
}

fn sha256_hex(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    hex::encode(hasher.finalize())
}

/// Generates one new license key. Format: LKC-XXXX-XXXX-XXXX-XXXC
/// The final character of the last group is a checksum over the other 15,
/// so a mistyped key can be rejected instantly client-side, before ever
/// making a network call.
fn generate_key() -> String {
    let mut rng = rand::thread_rng();
    let mut chars = Vec::with_capacity(15);
    for _ in 0..15 {
        let idx = (rng.next_u32() as usize) % ALPHABET.len();
        chars.push(ALPHABET[idx]);
    }
    let checksum_idx: usize = chars
        .iter()
        .map(|&b| ALPHABET.iter().position(|&a| a == b).unwrap())
        .sum::<usize>()
        % ALPHABET.len();
    chars.push(ALPHABET[checksum_idx]);

    let body: String = chars.iter().map(|&b| b as char).collect();
    let groups: Vec<String> = body
        .as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).to_string())
        .collect();
    format!("LKC-{}", groups.join("-"))
}

/// Validates the checksum locally — catches typos without a network call.
/// This is NOT the security boundary (the server-side hash lookup is);
/// it's purely a fast, friendly "that key isn't shaped right" check.
pub fn validate_key_format(key: &str) -> Result<()> {
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
    let expected_checksum = ALPHABET[sum % ALPHABET.len()] as char;
    let actual_checksum = body.chars().nth(15).unwrap();
    if expected_checksum != actual_checksum {
        return Err(anyhow!("key checksum does not match — likely a typo"));
    }
    Ok(())
}

fn cmd_issue(note: &str) -> Result<()> {
    let conn = open_db()?;
    let key = generate_key();
    let key_hash = sha256_hex(&key);
    let key_id = &key_hash[..8];
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO license_keys (key_hash, key_id, note, status, created_at) VALUES (?1, ?2, ?3, 'unused', ?4)",
        rusqlite::params![key_hash, key_id, note, now],
    )?;
    println!("Issued a new license key. This is shown ONE TIME ONLY — copy it now:\n");
    println!("  {key}\n");
    println!("  key_id: {key_id}   note: {note}");
    Ok(())
}

fn cmd_list() -> Result<()> {
    let conn = open_db()?;
    let mut stmt = conn.prepare(
        "SELECT key_id, note, status, device_id, created_at, activated_at FROM license_keys ORDER BY created_at DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, Option<String>>(5)?,
        ))
    })?;
    println!(
        "{:<10} {:<8} {:<24} {:<38} {}",
        "key_id", "status", "note", "device_id", "activated_at"
    );
    for row in rows {
        let (key_id, note, status, device_id, _created_at, activated_at) = row?;
        println!(
            "{:<10} {:<8} {:<24} {:<38} {}",
            key_id,
            status,
            note.unwrap_or_default(),
            device_id.unwrap_or_else(|| "-".to_string()),
            activated_at.unwrap_or_else(|| "-".to_string())
        );
    }
    Ok(())
}

fn cmd_revoke(key_id: &str) -> Result<()> {
    let conn = open_db()?;
    let now = chrono::Utc::now().to_rfc3339();
    let n = conn.execute(
        "UPDATE license_keys SET status = 'revoked', revoked_at = ?1 WHERE key_id = ?2",
        rusqlite::params![now, key_id],
    )?;
    if n == 0 {
        return Err(anyhow!("no key found with key_id '{key_id}'"));
    }
    println!("Revoked key_id {key_id}. Any device using it will be locked out immediately on its next check.");
    Ok(())
}

/// The one endpoint every SME install calls, exactly once (per device),
/// to redeem a key. This is the actual single-device enforcement point:
/// it's the only place that can see every key across every install.
fn redeem(conn: &Connection, key: &str, device_id: &str) -> (u16, serde_json::Value) {
    if let Err(e) = validate_key_format(key) {
        return (400, json!({"error": format!("invalid key: {e}")}));
    }
    let key_hash = sha256_hex(&key.trim().to_uppercase());
    let row: Option<(String, String, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT key_id, status, device_id, activated_at FROM license_keys WHERE key_hash = ?1",
            [&key_hash],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .ok();

    let Some((key_id, status, bound_device, activated_at)) = row else {
        return (404, json!({"error": "unknown license key"}));
    };

    match status.as_str() {
        "revoked" => (403, json!({"error": "this license key has been revoked"})),
        "unused" => {
            let now = chrono::Utc::now().to_rfc3339();
            let updated = conn.execute(
                "UPDATE license_keys SET status = 'active', device_id = ?1, activated_at = ?2 WHERE key_hash = ?3 AND status = 'unused'",
                rusqlite::params![device_id, now, key_hash],
            );
            match updated {
                // Guards a race: two simultaneous redeem attempts on the same
                // still-unused key can't both win — only the row-affecting
                // UPDATE (WHERE status = 'unused') actually claims it.
                Ok(1) => (200, json!({"ok": true, "key_id": key_id, "activated_at": now})),
                _ => (409, json!({"error": "this key was just activated by another device — try again if that wasn't you"})),
            }
        }
        "active" if bound_device.as_deref() == Some(device_id) => {
            // Idempotent: same device re-checking in (e.g. app reinstalled
            // on the same machine) gets the same success, not an error.
            (200, json!({"ok": true, "key_id": key_id, "activated_at": activated_at, "note": "already active on this device"}))
        }
        "active" => (
            409,
            json!({"error": "this license key is already in use on a different device. Contact the vendor if you need to move it."}),
        ),
        other => (500, json!({"error": format!("unexpected key status: {other}")})),
    }
}

fn read_body(request: &mut tiny_http::Request) -> String {
    let mut body = String::new();
    let _ = request.as_reader().read_to_string(&mut body);
    body
}

fn respond_json(request: tiny_http::Request, status: u16, body: serde_json::Value) {
    let data = body.to_string();
    let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
    let response = tiny_http::Response::from_string(data)
        .with_status_code(status)
        .with_header(header);
    let _ = request.respond(response);
}

fn cmd_serve(port: u16, admin_token: String) -> Result<()> {
    let conn = Arc::new(Mutex::new(open_db()?));
    let server = tiny_http::Server::http(format!("0.0.0.0:{port}"))
        .map_err(|e| anyhow!("failed to bind: {e}"))?;
    println!("Vendor Authority listening on http://0.0.0.0:{port}");
    println!("  POST /redeem            {{key, device_id}}   — called by SME app installs, no auth");
    println!("  GET  /keys              (X-Admin-Token)      — list all issued keys");
    println!("  POST /issue             {{note}} (X-Admin-Token) — issue a new key over HTTP");
    println!("  POST /revoke            {{key_id}} (X-Admin-Token)");

    for mut request in server.incoming_requests() {
        let method = request.method().clone();
        let url = request.url().to_string();
        let is_admin = request
            .headers()
            .iter()
            .any(|h| h.field.as_str().as_str().eq_ignore_ascii_case("X-Admin-Token") && h.value.as_str() == admin_token);

        match (method, url.as_str()) {
            (tiny_http::Method::Post, "/redeem") => {
                let body = read_body(&mut request);
                let parsed: Result<serde_json::Value, _> = serde_json::from_str(&body);
                match parsed {
                    Ok(v) => {
                        let key = v.get("key").and_then(|x| x.as_str()).unwrap_or("");
                        let device_id = v.get("device_id").and_then(|x| x.as_str()).unwrap_or("");
                        if key.is_empty() || device_id.is_empty() {
                            respond_json(request, 400, json!({"error": "key and device_id are required"}));
                            continue;
                        }
                        let conn = conn.lock().unwrap();
                        let (status, resp) = redeem(&conn, key, device_id);
                        respond_json(request, status, resp);
                    }
                    Err(_) => respond_json(request, 400, json!({"error": "invalid JSON body"})),
                }
            }
            (tiny_http::Method::Get, "/keys") => {
                if !is_admin {
                    respond_json(request, 401, json!({"error": "missing or invalid X-Admin-Token"}));
                    continue;
                }
                let conn = conn.lock().unwrap();
                let mut stmt = conn
                    .prepare("SELECT key_id, note, status, device_id, created_at, activated_at FROM license_keys ORDER BY created_at DESC")
                    .unwrap();
                let rows: Vec<serde_json::Value> = stmt
                    .query_map([], |r| {
                        Ok(json!({
                            "key_id": r.get::<_, String>(0)?,
                            "note": r.get::<_, Option<String>>(1)?,
                            "status": r.get::<_, String>(2)?,
                            "device_id": r.get::<_, Option<String>>(3)?,
                            "created_at": r.get::<_, String>(4)?,
                            "activated_at": r.get::<_, Option<String>>(5)?,
                        }))
                    })
                    .unwrap()
                    .filter_map(|r| r.ok())
                    .collect();
                respond_json(request, 200, json!({"keys": rows}));
            }
            (tiny_http::Method::Post, "/issue") => {
                if !is_admin {
                    respond_json(request, 401, json!({"error": "missing or invalid X-Admin-Token"}));
                    continue;
                }
                let body = read_body(&mut request);
                let note = serde_json::from_str::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|v| v.get("note").and_then(|n| n.as_str()).map(|s| s.to_string()))
                    .unwrap_or_default();
                let key = generate_key();
                let key_hash = sha256_hex(&key);
                let key_id = key_hash[..8].to_string();
                let now = chrono::Utc::now().to_rfc3339();
                let conn = conn.lock().unwrap();
                let inserted = conn.execute(
                    "INSERT INTO license_keys (key_hash, key_id, note, status, created_at) VALUES (?1, ?2, ?3, 'unused', ?4)",
                    rusqlite::params![key_hash, key_id, note, now],
                );
                match inserted {
                    Ok(_) => respond_json(request, 200, json!({"key": key, "key_id": key_id})),
                    Err(e) => respond_json(request, 500, json!({"error": e.to_string()})),
                }
            }
            (tiny_http::Method::Post, "/revoke") => {
                if !is_admin {
                    respond_json(request, 401, json!({"error": "missing or invalid X-Admin-Token"}));
                    continue;
                }
                let body = read_body(&mut request);
                let key_id = serde_json::from_str::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|v| v.get("key_id").and_then(|n| n.as_str()).map(|s| s.to_string()))
                    .unwrap_or_default();
                let now = chrono::Utc::now().to_rfc3339();
                let conn = conn.lock().unwrap();
                let n = conn
                    .execute(
                        "UPDATE license_keys SET status = 'revoked', revoked_at = ?1 WHERE key_id = ?2",
                        rusqlite::params![now, key_id],
                    )
                    .unwrap_or(0);
                if n == 0 {
                    respond_json(request, 404, json!({"error": "no such key_id"}));
                } else {
                    respond_json(request, 200, json!({"revoked": true}));
                }
            }
            _ => respond_json(request, 404, json!({"error": "not found"})),
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("issue") => {
            let note = args.get(2).cloned().unwrap_or_default();
            cmd_issue(&note)
        }
        Some("list") => cmd_list(),
        Some("revoke") => {
            let key_id = args.get(2).cloned().ok_or_else(|| anyhow!("usage: vendor_authority revoke <key_id>"))?;
            cmd_revoke(&key_id)
        }
        Some("serve") => {
            let port: u16 = args.get(2).map(|s| s.parse()).transpose()?.unwrap_or(9090);
            let admin_token = args
                .get(3)
                .cloned()
                .or_else(|| std::env::var("VENDOR_ADMIN_TOKEN").ok())
                .ok_or_else(|| anyhow!("usage: vendor_authority serve <port> <admin_token>  (or set VENDOR_ADMIN_TOKEN)"))?;
            cmd_serve(port, admin_token)
        }
        _ => {
            println!("Vendor Authority — license key issuing & device-lock enforcement\n");
            println!("Usage:");
            println!("  vendor_authority issue \"note about this customer\"");
            println!("  vendor_authority list");
            println!("  vendor_authority revoke <key_id>");
            println!("  vendor_authority serve <port> <admin_token>");
            Ok(())
        }
    }
}
