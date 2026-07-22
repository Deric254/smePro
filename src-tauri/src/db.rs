use anyhow::{anyhow, Result};
use rand_core::{OsRng, RngCore};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

pub const SCHEMA: &str = include_str!("../schema.sql");

/// Opens (or creates) the local database file, encrypted at rest via
/// SQLCipher, and applies the core schema.
///
/// The encryption key is a per-install secret, not a user-facing
/// password: it's generated once (32 random bytes, hex-encoded) into a
/// key file next to the database, with restrictive file permissions on
/// Unix. This protects the data if the file is copied off the device or
/// backed up somewhere insecure, without adding a password prompt to
/// every app launch — that's a deliberate scope choice, not an
/// oversight. A future "encrypt with the owner's login password
/// instead" upgrade would change only `get_or_create_key`, nothing else
/// in this file or its callers.
pub fn open(path: &str) -> Result<Connection> {
    let db_path = Path::new(path);
    let key_path = key_path_for(db_path);
    let key_hex = get_or_create_key(&key_path)?;

    let conn = Connection::open(db_path)?;
    // SQLCipher's raw-key syntax (`x'...'`) avoids its own key-derivation
    // pass since we already have a high-entropy random key — no need to
    // stretch a password that doesn't exist.
    conn.execute_batch(&format!("PRAGMA key = \"x'{key_hex}'\";"))?;

    // PRAGMA key alone doesn't fail on a wrong key — SQLCipher only
    // reveals that on the first real read. Force that check now, with a
    // clear error, instead of a confusing "file is not a database" error
    // surfacing from some unrelated query later.
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0))
        .map_err(|_| anyhow!(
            "could not open the database: the encryption key at {} doesn't match this database file. \
             If this file was copied from another install, its key file must come with it.",
            key_path.display()
        ))?;

    conn.execute_batch("PRAGMA journal_mode = WAL;")?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

fn key_path_for(db_path: &Path) -> PathBuf {
    let mut p = db_path.to_path_buf();
    let file_name = format!("{}.key", db_path.file_name().and_then(|s| s.to_str()).unwrap_or("erp"));
    p.set_file_name(file_name);
    p
}

fn get_or_create_key(key_path: &Path) -> Result<String> {
    if key_path.exists() {
        let contents = std::fs::read_to_string(key_path)?;
        let trimmed = contents.trim();
        if trimmed.len() != 64 || !trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(anyhow!("key file at {} is corrupted (expected 64 hex chars)", key_path.display()));
        }
        return Ok(trimmed.to_string());
    }

    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let hex_key: String = bytes.iter().map(|b| format!("{b:02x}")).collect();

    std::fs::write(key_path, &hex_key)?;
    restrict_permissions(key_path)?;
    Ok(hex_key)
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o600); // owner read/write only
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> Result<()> {
    // Windows ACLs are a different model; the key file still isn't
    // world-readable by default in a per-user app data directory. Worth
    // revisiting with proper ACL restriction before shipping on Windows.
    Ok(())
}
