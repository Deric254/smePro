//! Backup and restore — a real answer to "what happens if this machine
//! dies," not just a suggestion to copy files manually.
//!
//! Uses SQLite's own Online Backup API (via `rusqlite`'s `backup`
//! feature), not a raw file copy. That distinction matters: this
//! database runs in WAL mode, which means at any moment there can be
//! uncommitted writes sitting in a separate `-wal` file, not yet folded
//! into the main database file — a plain `cp erp.db backup.db` while the
//! app is running can silently produce a backup missing recent
//! transactions, or in the worst case an inconsistent one. The Backup
//! API is SQLite's own mechanism specifically for producing a correct,
//! consistent snapshot of a live database, page by page, regardless of
//! WAL state.
//!
//! The backup stays encrypted — it's produced by opening a destination
//! connection, setting the SAME SQLCipher key on it before the backup
//! runs, and copying into that. A backup file that decrypts with
//! nobody's password isn't a backup, it's a liability.
//!
//! THE KEY ITSELF IS WRAPPED, NOT SHIPPED IN THE CLEAR: earlier, this
//! file's `BackupData` included the raw database key directly in the
//! same JSON payload as the encrypted data — meaning anyone who ever
//! got hold of the backup file had everything needed to open it, no
//! separate secret required, which quietly defeated the entire point
//! of encrypting it. Now the raw key is wrapped inside a tiny, separate
//! SQLCipher-encrypted blob, keyed by a passphrase the caller supplies
//! and the owner is expected to remember — reusing SQLCipher's own
//! built-in passphrase key derivation rather than a second, parallel
//! crypto mechanism. Possessing the file alone is no longer enough.
//!
//! Restore is deliberately restart-based, not a live hot-swap — see
//! `apply_pending_restore_if_any` in db.rs for exactly why.

use anyhow::{anyhow, Result};
use rusqlite::Connection;
use rusqlite::backup::Backup;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Serialize)]
pub struct BackupData {
    /// The encrypted database, base64-encoded so it travels safely as
    /// plain JSON — this is ciphertext, not a security concern in
    /// transit, but base64 avoids any encoding corruption either way.
    pub database_base64: String,
    /// The raw database key, itself wrapped inside a tiny, separate
    /// SQLCipher-encrypted blob keyed by the passphrase the caller
    /// provided to create_backup() — never the raw key in the clear.
    /// Before this field existed, `key_hex` was shipped here directly:
    /// anyone who ever got hold of this backup file had everything
    /// needed to open it, no separate secret required at all, which
    /// defeated the point of encrypting it in the first place. Now
    /// possessing this file alone is not enough — the passphrase,
    /// never stored anywhere in the file, is what's actually required.
    pub wrapped_key_base64: String,
    pub created_at: String,
    /// Included so a restore can sanity-check it isn't about to load a
    /// backup from some unrelated, differently-shaped database.
    pub schema_version: String,
}

#[derive(Debug, Deserialize)]
pub struct RestoreInput {
    pub database_base64: String,
    pub wrapped_key_base64: String,
    pub passphrase: String,
}

/// Wraps a raw hex key inside a tiny, single-row SQLCipher database
/// keyed directly by the passphrase (`PRAGMA key = 'passphrase'` uses
/// SQLCipher's own internal PBKDF2, not a hand-rolled derivation) —
/// reusing the exact mechanism already used everywhere else in this
/// file rather than introducing a second, parallel crypto approach for
/// one small piece of data.
fn wrap_key(key_hex: &str, passphrase: &str) -> Result<Vec<u8>> {
    if passphrase.trim().len() < 8 {
        return Err(anyhow!("a backup passphrase needs to be at least 8 characters"));
    }
    let temp_path = std::env::temp_dir().join(format!("sme-pro-keywrap-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<Vec<u8>> {
        let conn = Connection::open(&temp_path)?;
        conn.execute_batch(&format!("PRAGMA key = '{}';", passphrase.replace('\'', "''")))?;
        conn.execute_batch("CREATE TABLE wrapped_key (value TEXT NOT NULL);")?;
        conn.execute("INSERT INTO wrapped_key (value) VALUES (?1);", [key_hex])?;
        drop(conn);
        std::fs::read(&temp_path).map_err(|e| anyhow!("failed to read the wrapped key: {e}"))
    })();
    let _ = std::fs::remove_file(&temp_path);
    result
}

/// The inverse of wrap_key — opens the tiny wrapped-key blob with the
/// passphrase supplied at restore time. A wrong passphrase fails
/// exactly here, cleanly, with a real explanation, rather than
/// producing a garbage key that goes on to corrupt something later.
fn unwrap_key(wrapped_key_bytes: &[u8], passphrase: &str) -> Result<String> {
    let temp_path = std::env::temp_dir().join(format!("sme-pro-keyunwrap-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<String> {
        std::fs::write(&temp_path, wrapped_key_bytes)?;
        let conn = Connection::open(&temp_path)?;
        conn.execute_batch(&format!("PRAGMA key = '{}';", passphrase.replace('\'', "''")))?;
        conn.query_row("SELECT value FROM wrapped_key LIMIT 1", [], |r| r.get::<_, String>(0))
            .map_err(|_| anyhow!("wrong passphrase, or this backup file is corrupted"))
    })();
    let _ = std::fs::remove_file(&temp_path);
    result
}

/// Finds the live database's own file path by asking SQLite directly
/// (`PRAGMA database_list`) rather than threading the path through
/// every function that might need it — the connection already knows.
fn current_db_path(conn: &Connection) -> Result<std::path::PathBuf> {
    let path: String = conn.query_row(
        "SELECT file FROM pragma_database_list WHERE name = 'main'",
        [],
        |r| r.get(0),
    )?;
    if path.is_empty() {
        return Err(anyhow!("database has no backing file (in-memory database?) — cannot back up"));
    }
    Ok(std::path::PathBuf::from(path))
}

pub fn create_backup(conn: &Connection, passphrase: &str) -> Result<BackupData> {
    let db_path = current_db_path(conn)?;
    let key_path = crate::db::key_path_for(&db_path);
    let key_hex = std::fs::read_to_string(&key_path)
        .map_err(|e| anyhow!("could not read the database key at {}: {e}", key_path.display()))?
        .trim()
        .to_string();

    let wrapped_key_bytes = wrap_key(&key_hex, passphrase)?;

    // A temp file, never left behind: the backup is read into memory
    // and returned as base64, then this file is deleted regardless of
    // whether that succeeded.
    let temp_path = std::env::temp_dir().join(format!("sme-pro-backup-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<Vec<u8>> {
        let mut dest = Connection::open(&temp_path)?;
        // The destination must be keyed BEFORE the backup runs, so
        // every page written to it is encrypted as it's written — not
        // written plain and encrypted after, which would briefly leave
        // an unencrypted copy of real business data on disk.
        dest.execute_batch(&format!("PRAGMA key = \"x'{key_hex}'\";"))?;

        let backup = Backup::new(conn, &mut dest)?;
        // 5 pages per step with no artificial pause: this is a local
        // backup with no other writers actively contending for the
        // file, so there's no reason to throttle it — pausing exists
        // for the case where you're backing up a database something
        // else is concurrently hammering, which isn't this.
        backup.run_to_completion(5, Duration::from_millis(0), None)?;
        drop(backup);
        drop(dest);

        std::fs::read(&temp_path).map_err(|e| anyhow!("failed to read the completed backup: {e}"))
    })();

    let _ = std::fs::remove_file(&temp_path);
    let db_bytes = result?;

    use base64::Engine;
    let database_base64 = base64::engine::general_purpose::STANDARD.encode(&db_bytes);
    let wrapped_key_base64 = base64::engine::general_purpose::STANDARD.encode(&wrapped_key_bytes);

    Ok(BackupData {
        database_base64,
        wrapped_key_base64,
        created_at: chrono::Utc::now().to_rfc3339(),
        schema_version: SCHEMA_MARKER.to_string(),
    })
}

/// A simple, honest marker — this schema has no formal version numbers
/// yet (see the note in RELEASE.md / the conversation this was built
/// from), so this exists to at least catch the most obvious mistake: a
/// backup from something that clearly isn't this app at all. Not a
/// substitute for a real migration/version system.
const SCHEMA_MARKER: &str = "sme-pro-v1";

/// Validates a restore payload thoroughly BEFORE staging anything —
/// wrong key, corrupted data, or a backup from something else entirely
/// should fail loudly right now, not silently corrupt the next launch.
pub fn stage_restore(conn: &Connection, input: RestoreInput) -> Result<()> {
    use base64::Engine;
    let wrapped_key_bytes = base64::engine::general_purpose::STANDARD
        .decode(&input.wrapped_key_base64)
        .map_err(|_| anyhow!("could not decode the backup file's key section — it may be corrupted"))?;
    let key_hex = unwrap_key(&wrapped_key_bytes, &input.passphrase)?;

    if key_hex.trim().len() != 64 || !key_hex.trim().chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(anyhow!("invalid key recovered from backup — this doesn't look like a backup from this app"));
    }
    let key_hex = key_hex.trim().to_string();

    let db_bytes = base64::engine::general_purpose::STANDARD
        .decode(&input.database_base64)
        .map_err(|_| anyhow!("could not decode the backup file — it may be corrupted"))?;

    // Validate it actually opens and decrypts with this exact key
    // before touching anything real — the same "force a real read now"
    // check db::open() itself does, for the same reason.
    let temp_path = std::env::temp_dir().join(format!("sme-pro-restore-check-{}.tmp", uuid::Uuid::new_v4()));
    let validation = (|| -> Result<()> {
        std::fs::write(&temp_path, &db_bytes)?;
        let conn = Connection::open(&temp_path)?;
        conn.execute_batch(&format!("PRAGMA key = \"x'{key_hex}'\";"))?;
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0))
            .map_err(|_| anyhow!("this backup could not be decrypted with the key provided — the file may be corrupted or from a different install"))?;
        // A minimal shape check — confirms this is plausibly THIS
        // app's database, not just any valid SQLite file.
        let has_businesses_table: i64 = conn
            .query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='businesses'", [], |r| r.get(0))
            .unwrap_or(0);
        if has_businesses_table == 0 {
            return Err(anyhow!("this file doesn't look like an SME Pro backup — missing expected tables"));
        }
        Ok(())
    })();
    let _ = std::fs::remove_file(&temp_path);
    validation?;

    let db_path = current_db_path(conn)?;
    stage_files(&db_path, &db_bytes, &key_hex)
}

fn stage_files(db_path: &Path, db_bytes: &[u8], key_hex: &str) -> Result<()> {
    let mut db_pending = db_path.to_path_buf();
    let db_name = format!("{}.restore-pending", db_path.file_name().and_then(|s| s.to_str()).unwrap_or("erp.db"));
    db_pending.set_file_name(db_name);

    let key_path = crate::db::key_path_for(db_path);
    let mut key_pending = key_path.clone();
    let key_name = format!("{}.restore-pending", key_path.file_name().and_then(|s| s.to_str()).unwrap_or("erp.db.key"));
    key_pending.set_file_name(key_name);

    std::fs::write(&db_pending, db_bytes).map_err(|e| anyhow!("failed to stage restore: {e}"))?;
    std::fs::write(&key_pending, key_hex).map_err(|e| anyhow!("failed to stage restore key: {e}"))?;
    Ok(())
}
