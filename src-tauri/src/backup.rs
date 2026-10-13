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
//! consistent snapshot of a live database, regardless of WAL state.
//!
//! The backup stays encrypted — it is produced by opening a destination
//! connection, setting the SAME SQLCipher key on it before the backup
//! runs, and copying into that. A backup file that decrypts with
//! nobody's password isn't a backup, it's a liability.
//!
//! THE KEY ITSELF IS WRAPPED, NOT SHIPPED IN THE CLEAR: the raw database
//! key is wrapped inside a tiny, separate SQLCipher-encrypted blob,
//! keyed by a passphrase the caller supplies and the owner is expected
//! to remember — reusing SQLCipher's own built-in passphrase key
//! derivation rather than a second, parallel crypto mechanism.
//! Possessing the file alone is never enough.
//!
//! SIZE: nothing here holds the database in memory.
//! * The snapshot is taken on its OWN connection, in one step, so it
//!   never holds the lock the rest of the app's requests queue on and
//!   never restarts because the app wrote in the middle of it (the old
//!   code ran page-by-page on the shared connection with that lock held,
//!   freezing the whole app for the length of the backup).
//! * The file format is binary and streamed — see `MAGIC` — not the
//!   old base64-in-JSON (33% bigger, and the whole thing in memory three
//!   or four times over on both the Rust and the browser side). Restore
//!   used to be rejected outright above the server's 10 MB body limit,
//!   which in practice meant any backup over ~7 MB could be created but
//!   never restored.
//! * A SHA-256 of the database bytes closes the file, so a truncated or
//!   damaged download is caught before anything is staged.
//!
//! Restore is deliberately restart-based, not a live hot-swap — see
//! `apply_pending_restore_if_any` in db.rs for exactly why. Older
//! base64-in-JSON backup files (first byte `{`) are still accepted.

use anyhow::{anyhow, Result};
use rusqlite::backup::{Backup, StepResult};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufWriter, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// First 16 bytes of every backup file. Layout:
/// `MAGIC | header_len (u32 LE) | header (JSON) | database bytes | SHA-256 of the database bytes`.
pub const MAGIC: &[u8; 16] = b"SMEPRO-BACKUP-2\n";
const FORMAT: u32 = 2;
const MAX_HEADER_BYTES: u32 = 64 * 1024;
const DIGEST_LEN: usize = 32;
const IO_CHUNK: usize = 1024 * 1024;

/// Older backups hold the whole database as base64 inside one JSON
/// document, so restoring one means holding that document in memory.
/// Capped so a huge or hostile upload can't exhaust memory.
const LEGACY_MAX_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
struct Header {
    format: u32,
    created_at: String,
    /// The raw database key, wrapped inside a tiny SQLCipher-encrypted
    /// blob keyed by the passphrase — never the raw key in the clear.
    wrapped_key_base64: String,
    db_len: u64,
}

/// Only the one small field this app needs out of an older JSON backup;
/// serde skips the (huge) `database_base64` field without copying it.
#[derive(Deserialize)]
struct LegacyMeta {
    wrapped_key_base64: String,
}

/// A finished backup, ready to stream: the header, then the snapshot
/// file, then its digest. The snapshot is deleted when this is dropped.
pub struct BackupFile {
    header: Vec<u8>,
    snapshot: PathBuf,
    db_len: u64,
    digest: [u8; DIGEST_LEN],
}

impl BackupFile {
    pub fn total_len(&self) -> u64 {
        self.header.len() as u64 + self.db_len + DIGEST_LEN as u64
    }

    pub fn into_reader(mut self) -> Result<BackupReader> {
        let file = File::open(&self.snapshot).map_err(|e| anyhow!("failed to read the completed backup: {e}"))?;
        let header = std::mem::take(&mut self.header);
        let inner = Cursor::new(header).chain(file).chain(Cursor::new(self.digest));
        Ok(BackupReader { inner: Some(inner), snapshot: std::mem::take(&mut self.snapshot) })
    }
}

impl Drop for BackupFile {
    fn drop(&mut self) {
        if !self.snapshot.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&self.snapshot);
        }
    }
}

type BackupChain = std::io::Chain<std::io::Chain<Cursor<Vec<u8>>, File>, Cursor<[u8; DIGEST_LEN]>>;

/// Streams a [`BackupFile`] and deletes its temporary snapshot once the
/// response is finished or abandoned.
pub struct BackupReader {
    inner: Option<BackupChain>,
    snapshot: PathBuf,
}

impl Read for BackupReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self.inner.as_mut() {
            Some(r) => r.read(buf),
            None => Ok(0),
        }
    }
}

impl Drop for BackupReader {
    fn drop(&mut self) {
        // Close the file first — Windows won't delete a file that's open.
        self.inner = None;
        let _ = std::fs::remove_file(&self.snapshot);
    }
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
    let key_hex = result?.trim().to_string();
    if key_hex.len() != 64 || !key_hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(anyhow!("invalid key recovered from backup — this doesn't look like a backup from this app"));
    }
    Ok(key_hex)
}

/// Finds the live database's own file path by asking SQLite directly
/// (`PRAGMA database_list`) rather than threading the path through
/// every function that might need it — the connection already knows.
pub fn db_file_path(conn: &Connection) -> Result<PathBuf> {
    let path: String = conn.query_row(
        "SELECT file FROM pragma_database_list WHERE name = 'main'",
        [],
        |r| r.get(0),
    )?;
    if path.is_empty() {
        return Err(anyhow!("database has no backing file (in-memory database?) — cannot back up"));
    }
    Ok(PathBuf::from(path))
}

/// Deletes a path on drop — keeps every early `?` return in a function
/// that creates a large temporary file from leaving it behind.
struct RemoveOnDrop<'a>(&'a Path);

impl Drop for RemoveOnDrop<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0);
    }
}

fn key_pragma(key_hex: &str) -> String {
    format!("PRAGMA key = \"x'{key_hex}'\";")
}

/// Copies the live database into `snapshot`, encrypted with the same
/// key, as one consistent point-in-time image.
///
/// A dedicated source connection + a single all-pages step: the copy is
/// one read transaction, so (in WAL mode) the app keeps writing normally
/// while it runs and the result is exactly the database as of the moment
/// it began. Stepping a few pages at a time instead (the old approach)
/// lets other connections' writes land between steps, which makes SQLite
/// restart the copy from page one — on a busy database it may never finish.
fn snapshot_database(db_path: &Path, key_hex: &str, snapshot: &Path) -> Result<()> {
    let source = Connection::open(db_path)?;
    source.execute_batch(&key_pragma(key_hex))?;
    source.busy_timeout(Duration::from_secs(30))?;
    source
        .query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0))
        .map_err(|_| anyhow!("could not read the database to back it up"))?;

    let mut dest = Connection::open(snapshot)?;
    // The destination must be keyed BEFORE the backup runs, so every
    // page written to it is encrypted as it's written — not written
    // plain and encrypted after, which would briefly leave an
    // unencrypted copy of real business data on disk.
    dest.execute_batch(&key_pragma(key_hex))?;

    let backup = Backup::new(&source, &mut dest)?;
    // `step(-1)` copies every page in one go. (`run_to_completion` insists
    // on a positive page count, i.e. several steps, which is exactly what
    // this avoids.) Busy/Locked only mean another connection holds a lock
    // at this instant — wait and retry, for up to a minute.
    let mut waited = Duration::ZERO;
    loop {
        match backup.step(-1)? {
            StepResult::Done => break,
            StepResult::More => {}
            _ => {
                if waited >= Duration::from_secs(60) {
                    return Err(anyhow!("the database stayed busy for a minute — try the backup again"));
                }
                std::thread::sleep(Duration::from_millis(100));
                waited += Duration::from_millis(100);
            }
        }
    }
    drop(backup);
    drop(dest);
    Ok(())
}

fn hash_file(path: &Path) -> Result<(u64, [u8; DIGEST_LEN])> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; IO_CHUNK];
    let mut len = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        len += n as u64;
    }
    Ok((len, hasher.finalize().into()))
}

/// Creates a backup of the database at `db_path`. Does not need the
/// application's database connection (or its lock) — see
/// `snapshot_database`.
pub fn create_backup(db_path: &Path, passphrase: &str) -> Result<BackupFile> {
    let key_path = crate::db::key_path_for(db_path);
    let key_hex = std::fs::read_to_string(&key_path)
        .map_err(|e| anyhow!("could not read the database key at {}: {e}", key_path.display()))?
        .trim()
        .to_string();

    let wrapped_key_bytes = wrap_key(&key_hex, passphrase)?;

    // Next to the database (same drive, where the space is known to
    // exist), named so db.rs can sweep it up if the app dies mid-backup.
    let snapshot = db_path.with_file_name(format!(
        "{}.backup-{}.tmp",
        db_path.file_name().and_then(|s| s.to_str()).unwrap_or("erp.db"),
        uuid::Uuid::new_v4()
    ));
    let cleanup = RemoveOnDrop(&snapshot);
    snapshot_database(db_path, &key_hex, &snapshot)?;
    let (db_len, digest) = hash_file(&snapshot)?;

    use base64::Engine;
    let header_json = serde_json::to_vec(&Header {
        format: FORMAT,
        created_at: chrono::Utc::now().to_rfc3339(),
        wrapped_key_base64: base64::engine::general_purpose::STANDARD.encode(&wrapped_key_bytes),
        db_len,
    })?;
    let mut header = Vec::with_capacity(MAGIC.len() + 4 + header_json.len());
    header.extend_from_slice(MAGIC);
    header.extend_from_slice(&(header_json.len() as u32).to_le_bytes());
    header.extend_from_slice(&header_json);

    std::mem::forget(cleanup); // ownership of the snapshot passes to the BackupFile
    Ok(BackupFile { header, snapshot, db_len, digest })
}

/// Percent-decodes a header value into UTF-8 (the passphrase may contain
/// any character, and HTTP header values can't carry them raw).
pub fn decode_passphrase_header(raw: &str) -> Result<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = raw.get(i + 1..i + 3).ok_or_else(|| anyhow!("malformed passphrase header"))?;
            out.push(u8::from_str_radix(hex, 16).map_err(|_| anyhow!("malformed passphrase header"))?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| anyhow!("malformed passphrase header"))
}

/// Receives a backup from `reader` (the HTTP request body, streamed),
/// verifies it thoroughly, and stages it to be applied on the next start.
/// Everything is checked BEFORE anything real is touched — wrong
/// passphrase, truncated or corrupted data, a file from something else
/// entirely, or a database from a newer app version all fail loudly now,
/// not at the next launch.
///
/// `body_len` is the request's declared length when it has one.
pub fn stage_restore(db_path: &Path, reader: &mut dyn Read, body_len: Option<usize>, passphrase: &str) -> Result<()> {
    let incoming = crate::db::restore_incoming_path(db_path);
    // Exclusive creation doubles as the "one restore at a time" lock: two
    // would share the same incoming/pending files. A file left by a crash
    // is swept at startup (db.rs), so this can't stay locked.
    let out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&incoming)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                anyhow!("another restore is already being prepared — wait for it to finish")
            } else {
                anyhow!("failed to stage restore: {e}")
            }
        })?;
    let _cleanup = RemoveOnDrop(&incoming); // a no-op once the file has been promoted

    let mut first = [0u8; 16];
    reader
        .read_exact(&mut first)
        .map_err(|_| anyhow!("this doesn't look like an SME Pro backup file"))?;

    let key_hex = if &first == MAGIC {
        receive_streamed(reader, body_len, passphrase, out)?
    } else if first[0] == b'{' {
        receive_legacy(reader, first, passphrase, out)?
    } else {
        return Err(anyhow!("this doesn't look like an SME Pro backup file"));
    };

    verify_database(&incoming, &key_hex)?;
    commit_stage(db_path, &incoming, &key_hex)
}

fn receive_streamed(reader: &mut dyn Read, body_len: Option<usize>, passphrase: &str, file: File) -> Result<String> {
    let mut len_bytes = [0u8; 4];
    reader.read_exact(&mut len_bytes).map_err(|_| anyhow!("the backup file is truncated"))?;
    let header_len = u32::from_le_bytes(len_bytes);
    if header_len == 0 || header_len > MAX_HEADER_BYTES {
        return Err(anyhow!("this doesn't look like an SME Pro backup file"));
    }
    let mut header_bytes = vec![0u8; header_len as usize];
    reader.read_exact(&mut header_bytes).map_err(|_| anyhow!("the backup file is truncated"))?;
    let header: Header = serde_json::from_slice(&header_bytes)
        .map_err(|_| anyhow!("this doesn't look like an SME Pro backup file"))?;
    if header.format != FORMAT {
        return Err(anyhow!("this backup was made by a different version of the app and can't be read by this one"));
    }

    if let Some(body_len) = body_len {
        let expected = (MAGIC.len() as u64 + 4 + header_len as u64)
            .saturating_add(header.db_len)
            .saturating_add(DIGEST_LEN as u64);
        if body_len as u64 != expected {
            return Err(anyhow!("the backup file is truncated or damaged (its size doesn't match what it declares)"));
        }
    }

    // Fail on a wrong passphrase now, before receiving gigabytes.
    use base64::Engine;
    let wrapped = base64::engine::general_purpose::STANDARD
        .decode(&header.wrapped_key_base64)
        .map_err(|_| anyhow!("could not decode the backup file's key section — it may be corrupted"))?;
    let key_hex = unwrap_key(&wrapped, passphrase)?;

    let mut out = BufWriter::with_capacity(IO_CHUNK, file);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; IO_CHUNK];
    let mut remaining = header.db_len;
    while remaining > 0 {
        let want = remaining.min(IO_CHUNK as u64) as usize;
        let n = reader.read(&mut buf[..want]).map_err(|e| anyhow!("the upload was interrupted: {e}"))?;
        if n == 0 {
            return Err(anyhow!("the backup file is truncated"));
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n]).map_err(|e| anyhow!("failed to stage restore (is the disk full?): {e}"))?;
        remaining -= n as u64;
    }
    let mut trailer = [0u8; DIGEST_LEN];
    reader.read_exact(&mut trailer).map_err(|_| anyhow!("the backup file is truncated"))?;
    let digest: [u8; DIGEST_LEN] = hasher.finalize().into();
    if trailer != digest {
        return Err(anyhow!("the backup file is damaged (its checksum doesn't match) — download or copy it again"));
    }
    let file = out.into_inner().map_err(|e| anyhow!("failed to stage restore: {e}"))?;
    file.sync_all().map_err(|e| anyhow!("failed to stage restore: {e}"))?;
    Ok(key_hex)
}

/// Older base64-in-JSON backups: the whole document is read (bounded by
/// `LEGACY_MAX_BYTES`), but never parsed into a second copy — the key
/// section is read by serde and the database section is base64-decoded
/// straight from the buffer to disk.
fn receive_legacy(reader: &mut dyn Read, first: [u8; 16], passphrase: &str, file: File) -> Result<String> {
    let mut doc = first.to_vec();
    reader
        .take(LEGACY_MAX_BYTES + 1)
        .read_to_end(&mut doc)
        .map_err(|e| anyhow!("the upload was interrupted: {e}"))?;
    if doc.len() as u64 > LEGACY_MAX_BYTES {
        return Err(anyhow!(
            "this is an older-format backup larger than 1 GB, which can't be restored — \
             make a new backup from the original app (new backups have no size limit)"
        ));
    }
    let meta: LegacyMeta = serde_json::from_slice(&doc)
        .map_err(|_| anyhow!("could not read the backup file — it may be corrupted"))?;

    use base64::Engine;
    let wrapped = base64::engine::general_purpose::STANDARD
        .decode(&meta.wrapped_key_base64)
        .map_err(|_| anyhow!("could not decode the backup file's key section — it may be corrupted"))?;
    let key_hex = unwrap_key(&wrapped, passphrase)?;

    // Base64 never contains a quote or a backslash, so the value is
    // exactly the bytes between the opening and the next closing quote.
    const FIELD: &[u8] = b"\"database_base64\":\"";
    let start = doc
        .windows(FIELD.len())
        .position(|w| w == FIELD)
        .map(|p| p + FIELD.len())
        .ok_or_else(|| anyhow!("this doesn't look like an SME Pro backup file"))?;
    let end = start
        + doc[start..]
            .iter()
            .position(|&b| b == b'"')
            .ok_or_else(|| anyhow!("could not read the backup file — it may be corrupted"))?;

    let mut decoder = base64::read::DecoderReader::new(&doc[start..end], &base64::engine::general_purpose::STANDARD);
    let mut out = BufWriter::with_capacity(IO_CHUNK, file);
    std::io::copy(&mut decoder, &mut out)
        .map_err(|_| anyhow!("could not decode the backup file — it may be corrupted"))?;
    let file = out.into_inner().map_err(|e| anyhow!("failed to stage restore: {e}"))?;
    file.sync_all().map_err(|e| anyhow!("failed to stage restore: {e}"))?;
    Ok(key_hex)
}

/// Proves the received file is a sound, openable database of THIS app —
/// decrypts with the recovered key, passes SQLite's own consistency
/// check, has the expected tables, and isn't from a newer app version
/// than this build can open (db_migrations refuses those at startup,
/// and by then the restore would already have replaced the live data).
fn verify_database(path: &Path, key_hex: &str) -> Result<()> {
    let conn = Connection::open(path)?;
    conn.execute_batch(&key_pragma(key_hex))?;
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0))
        .map_err(|_| anyhow!("this backup could not be decrypted with the key provided — the file may be corrupted or from a different install"))?;

    let check: String = conn
        .query_row("PRAGMA quick_check", [], |r| r.get(0))
        .map_err(|e| anyhow!("this backup failed its integrity check: {e}"))?;
    if check != "ok" {
        return Err(anyhow!("this backup failed its integrity check ({check}) — the file is damaged"));
    }

    // A minimal shape check — confirms this is plausibly THIS app's
    // database, not just any valid SQLite file.
    let has_businesses_table: i64 = conn
        .query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='businesses'", [], |r| r.get(0))
        .unwrap_or(0);
    if has_businesses_table == 0 {
        return Err(anyhow!("this file doesn't look like an SME Pro backup — missing expected tables"));
    }

    let schema: i64 = conn
        .query_row("SELECT COALESCE(MAX(version), 0) FROM _schema_version", [], |r| r.get(0))
        .unwrap_or(0);
    if schema > crate::db_migrations::CURRENT_VERSION as i64 {
        return Err(anyhow!(
            "this backup was made by a newer version of SME Pro (database version {schema}; this app understands up to {}) — update the app first, then restore it",
            crate::db_migrations::CURRENT_VERSION
        ));
    }
    Ok(())
}

/// Promotes a verified file to "staged". Crash-safe by ordering: the key
/// goes down first, and the database is renamed into place last — its
/// existence is what marks the staged restore complete (see
/// `db::apply_pending_restore_if_any`).
fn commit_stage(db_path: &Path, incoming: &Path, key_hex: &str) -> Result<()> {
    let (db_pending, key_pending) = crate::db::pending_restore_paths(db_path);
    // A previously staged, never-applied restore must not be half
    // replaced by this one.
    let _ = std::fs::remove_file(&db_pending);

    let stage_err = |e: std::io::Error| anyhow!("failed to stage restore: {e}");
    let mut key_file = File::create(&key_pending).map_err(stage_err)?;
    key_file.write_all(key_hex.as_bytes()).map_err(stage_err)?;
    key_file.sync_all().map_err(stage_err)?;
    drop(key_file);
    crate::db::restrict_permissions(&key_pending)?;

    std::fs::rename(incoming, &db_pending).map_err(stage_err)
}
