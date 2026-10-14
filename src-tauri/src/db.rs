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
    // Without this, WAL mode's default (synchronous = NORMAL) does not
    // guarantee the most recently committed transaction survives an OS
    // crash or power loss at the exact moment of a WAL checkpoint --
    // for a system recording real money, that gap is not acceptable.
    // FULL fsyncs on every commit, trading a small amount of write
    // throughput for the actual guarantee "committed means durable."
    conn.execute_batch("PRAGMA synchronous = FULL;")?;
    // Without this, SQLite's default is to fail an operation
    // immediately with "database is locked" the instant it can't
    // acquire a write lock -- a near-certainty the moment two real
    // requests land close together (two POS terminals, or the HTTP API
    // simply serving two concurrent requests). 5 seconds is enough for
    // a genuinely concurrent write to wait its turn rather than fail a
    // real sale for no reason other than bad timing.
    conn.execute_batch("PRAGMA busy_timeout = 5000;")?;
    // SQLite's default page cache is 2 MB. This database is encrypted, so
    // every page that misses the cache is read AND decrypted (and
    // authenticated) again — on a business with years of sales, report
    // queries that revisit the same pages kept paying that cost.
    // 32 MB (a negative value is KiB) keeps the hot index and table pages
    // resident; it is a ceiling that fills only as pages are used, so a
    // small business pays nothing for it.
    conn.execute_batch("PRAGMA cache_size = -32768;")?;
    // schema.sql is 16 CREATE TABLE IF NOT EXISTS statements — no-ops
    // after the first launch, since the tables already exist, but
    // still real work every single time otherwise: SQLite's default
    // autocommit mode makes each individual statement its own
    // implicit transaction, and with `synchronous = FULL` above (a
    // deliberate, correct choice for a system recording real money —
    // see that PRAGMA's own comment), every one of those 16
    // transactions means its own fsync. On Android's often-slow flash
    // storage — this app's primary target platform — 16 sequential
    // fsyncs on every app launch, before the login screen can even
    // render (this all runs inside Tauri's blocking `.setup()`), is a
    // real, measurable, and entirely avoidable source of startup
    // delay. Wrapping the whole batch in one explicit transaction
    // makes it one commit and one fsync instead of 16 — same
    // durability guarantee for what actually matters (this only runs
    // once, at startup, not on every real transaction afterward),
    // same idempotent CREATE TABLE IF NOT EXISTS semantics, just
    // committed together rather than one at a time.
    let tx = conn.transaction()?;
    tx.execute_batch(SCHEMA)?;
    tx.commit()?;
    crate::db_migrations::run(&mut conn)?;
    Ok(conn)
}

pub fn key_path_for(db_path: &Path) -> PathBuf {
    let mut p = db_path.to_path_buf();
    let file_name = format!("{}.key", db_path.file_name().and_then(|s| s.to_str()).unwrap_or("erp"));
    p.set_file_name(file_name);
    p
}

/// Sibling files `backup.rs` writes next to the live database when a
/// restore is staged — deliberately just suffixes on the real paths, not a
/// separate directory, so it's obvious at a glance in the app data folder
/// what these files are for.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let name = format!("{}{suffix}", path.file_name().and_then(|s| s.to_str()).unwrap_or("erp.db"));
    path.with_file_name(name)
}

/// The staged database (renamed into place last — its existence is what
/// marks a staged restore as complete) and the staged key.
pub(crate) fn pending_restore_paths(db_path: &Path) -> (PathBuf, PathBuf) {
    (sibling(db_path, ".restore-pending"), sibling(&key_path_for(db_path), ".restore-pending"))
}

/// Where an uploaded backup is streamed to and verified before it is
/// promoted to the pending database. Same directory (so the final rename
/// is atomic), never read by anything but `backup.rs`.
pub(crate) fn restore_incoming_path(db_path: &Path) -> PathBuf {
    sibling(db_path, ".restore-incoming")
}

/// Where the database and key that a restore replaced are kept, so a
/// restore of the wrong file, or one that can't be opened, never means
/// the previous data is gone.
pub(crate) fn previous_paths(db_path: &Path) -> (PathBuf, PathBuf) {
    (sibling(db_path, ".pre-restore"), sibling(&key_path_for(db_path), ".pre-restore"))
}

/// If `backup::stage_restore*` left a validated backup waiting, applies
/// it now by replacing the live database and key files.
///
/// Restart-based on purpose, not a shortcut: SQLite in WAL mode has
/// live file handles, a shared-memory index file, and potentially
/// uncheckpointed writes the moment any connection is open. Swapping
/// the underlying file out from under an active connection is exactly
/// the kind of operation that risks the corruption a backup feature
/// exists to protect against. This function only ever runs here, at
/// the very start of `open()`, before any connection to the target
/// path exists — the one moment this is genuinely safe.
///
/// Crash-safe: staging writes the key first and renames the database
/// into place last, so a pending DATABASE means "a complete restore is
/// staged"; a lone pending key is the debris of an interrupted staging
/// and is discarded. Applying swaps the key first and the database
/// second, each step only if its pending file still exists, so a crash
/// anywhere in between is finished by simply running this again on the
/// next launch (the old code renamed the database first and only looked
/// for a pending key when a pending database existed — a crash between
/// the two left a new database with the old key, which can never open).
/// The replaced files are moved aside, not deleted.
fn apply_pending_restore_if_any(db_path: &Path) -> Result<()> {
    remove_stale_temp_files(db_path);
    let (db_pending, key_pending) = pending_restore_paths(db_path);
    if !db_pending.exists() {
        let _ = std::fs::remove_file(&key_pending);
        return Ok(());
    }

    let key_path = key_path_for(db_path);
    let (db_previous, key_previous) = previous_paths(db_path);

    if key_pending.exists() {
        if key_path.exists() {
            std::fs::rename(&key_path, &key_previous)
                .map_err(|e| anyhow!("failed to apply staged restore (keeping the previous key): {e}"))?;
        }
        std::fs::rename(&key_pending, &key_path)
            .map_err(|e| anyhow!("failed to apply staged restore (key file): {e}"))?;
    }

    if db_path.exists() {
        std::fs::rename(db_path, &db_previous)
            .map_err(|e| anyhow!("failed to apply staged restore (keeping the previous database): {e}"))?;
    }
    // WAL sidecar files belong to the database that was just moved
    // aside — stale ones referencing the old file's content would be
    // actively wrong next to the restored one. The WAL may hold committed
    // transactions not yet folded into the old file, so it travels with
    // the previous database; the shared-memory index is disposable.
    let _ = std::fs::rename(sibling(db_path, "-wal"), sibling(&db_previous, "-wal"));
    let _ = std::fs::remove_file(sibling(db_path, "-shm"));
    std::fs::rename(&db_pending, db_path)
        .map_err(|e| anyhow!("failed to apply staged restore (database file): {e}"))?;
    Ok(())
}

/// Removes temp files a crashed or killed backup/restore left behind —
/// each one is a full copy of the database, so they must not pile up.
fn remove_stale_temp_files(db_path: &Path) {
    let _ = std::fs::remove_file(restore_incoming_path(db_path));
    let (Some(dir), Some(name)) = (db_path.parent(), db_path.file_name().and_then(|s| s.to_str())) else { return };
    let prefix = format!("{name}.backup-");
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if file_name.starts_with(&prefix) && file_name.ends_with(".tmp") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
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
pub(crate) fn restrict_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o600); // owner read/write only
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn restrict_permissions(_path: &Path) -> Result<()> {
    // Windows ACLs are a different model; the key file still isn't
    // world-readable by default in a per-user app data directory. Worth
    // revisiting with proper ACL restriction before shipping on Windows.
    Ok(())
}
