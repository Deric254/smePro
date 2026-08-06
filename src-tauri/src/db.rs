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

    // A restore staged by backup::stage_restore() (see that module for
    // why this is restart-based rather than a live hot-swap) gets
    // applied here, first thing, before anything else touches the
    // database — the safest possible moment, before any connection to
    // the old file exists yet.
    apply_pending_restore_if_any(db_path)?;

    let key_path = key_path_for(db_path);
    let key_hex = get_or_create_key(&key_path)?;

    let mut conn = Connection::open(db_path)?;
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

    // CRITICAL: SQLite does NOT enforce foreign key constraints by
    // default, even when they're declared in the schema — this has to
    // be turned on per-connection, every time. Without it, every
    // `REFERENCES ... ON DELETE CASCADE` in schema.sql (10 of them) is
    // silently decorative: nothing stops an orphaned row, and cascading
    // deletes just don't cascade. Real consistency gap, now closed.
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    conn.execute_batch("PRAGMA journal_mode = WAL;")?;
    conn.execute_batch(SCHEMA)?;
    crate::db_migrations::run(&mut conn)?;
    Ok(conn)
}

pub fn key_path_for(db_path: &Path) -> PathBuf {
    let mut p = db_path.to_path_buf();
    let file_name = format!("{}.key", db_path.file_name().and_then(|s| s.to_str()).unwrap_or("erp"));
    p.set_file_name(file_name);
    p
}

/// A path convention `backup.rs` writes to when a restore is staged —
/// deliberately just a suffix on the real paths, not a separate
/// directory, so it's obvious at a glance in the app data folder what
/// these files are for.
fn pending_restore_paths(db_path: &Path) -> (PathBuf, PathBuf) {
    let mut db_pending = db_path.to_path_buf();
    let db_name = format!("{}.restore-pending", db_path.file_name().and_then(|s| s.to_str()).unwrap_or("erp.db"));
    db_pending.set_file_name(db_name);

    let key_path = key_path_for(db_path);
    let mut key_pending = key_path.clone();
    let key_name = format!("{}.restore-pending", key_path.file_name().and_then(|s| s.to_str()).unwrap_or("erp.db.key"));
    key_pending.set_file_name(key_name);

    (db_pending, key_pending)
}

/// If `backup::stage_restore` left a validated backup waiting, applies
/// it now by atomically replacing the live database and key files.
///
/// Restart-based on purpose, not a shortcut: SQLite in WAL mode has
/// live file handles, a shared-memory index file, and potentially
/// uncheckpointed writes the moment any connection is open. Swapping
/// the underlying file out from under an active connection is exactly
/// the kind of operation that risks the corruption a backup feature
/// exists to protect against. This function only ever runs here, at
/// the very start of `open()`, before any connection to the target
/// path exists — the one moment this is genuinely safe.
fn apply_pending_restore_if_any(db_path: &Path) -> Result<()> {
    let (db_pending, key_pending) = pending_restore_paths(db_path);
    if !db_pending.exists() {
        return Ok(());
    }

    let key_path = key_path_for(db_path);

    // Clear WAL sidecar files from the database being replaced — stale
    // ones referencing the old file's content would be actively wrong
    // to keep around, not just unnecessary.
    for ext in ["-wal", "-shm"] {
        let sidecar = db_path.with_extension(format!("db{ext}"));
        let _ = std::fs::remove_file(sidecar);
    }

    std::fs::rename(&db_pending, db_path)
        .map_err(|e| anyhow!("failed to apply staged restore (database file): {e}"))?;
    if key_pending.exists() {
        std::fs::rename(&key_pending, &key_path)
            .map_err(|e| anyhow!("failed to apply staged restore (key file): {e}"))?;
    }
    Ok(())
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
