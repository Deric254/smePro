use rusqlite::Connection;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

/// Backup needs a real, file-backed, SQLCipher-keyed database --
/// unlike the rest of this crate's tests, which use an in-memory
/// connection that has no file path at all for create_backup() to
/// find a key file next to.
fn temp_db_path() -> PathBuf {
    std::env::temp_dir().join(format!("sme-pro-backup-test-{}.db", uuid::Uuid::new_v4()))
}

fn open_real_test_db() -> (Connection, PathBuf) {
    let path = temp_db_path();
    let conn = crate::db::open(path.to_str().unwrap()).expect("open real db");
    (conn, path)
}

fn cleanup(path: &Path) {
    let (db_pending, key_pending) = crate::db::pending_restore_paths(path);
    let (db_previous, key_previous) = crate::db::previous_paths(path);
    for p in [
        path.to_path_buf(),
        crate::db::key_path_for(path),
        path.with_extension("db-wal"),
        path.with_extension("db-shm"),
        db_pending,
        key_pending,
        db_previous,
        key_previous,
        crate::db::restore_incoming_path(path),
    ] {
        let _ = std::fs::remove_file(p);
    }
}

/// A complete backup file as bytes, taken while `path`'s own connection
/// is still open — the real situation (the app is running).
fn backup_bytes(path: &Path, passphrase: &str) -> Vec<u8> {
    let file = crate::backup::create_backup(path, passphrase).unwrap();
    let total = file.total_len();
    let mut bytes = Vec::new();
    file.into_reader().unwrap().read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes.len() as u64, total, "the declared length must be exact");
    bytes
}

fn stage(path: &Path, bytes: &[u8], passphrase: &str) -> anyhow::Result<()> {
    crate::backup::stage_restore(path, &mut Cursor::new(bytes.to_vec()), Some(bytes.len()), passphrase)
}

fn nothing_staged(path: &Path) -> bool {
    let (db_pending, key_pending) = crate::db::pending_restore_paths(path);
    !db_pending.exists() && !key_pending.exists() && !crate::db::restore_incoming_path(path).exists()
}

#[test]
fn test_backup_and_restore_round_trip_recovers_real_data() {
    let (mut conn, path) = open_real_test_db();
    let business_id = crate::business_panel::create_business(&mut conn, "Backup Test Biz", "USD", "UTC").unwrap();

    // Taken with the source connection still open and in use.
    let bytes = backup_bytes(&path, "a-real-passphrase-123");
    drop(conn);

    // A genuinely fresh, separate database -- proving this isn't just
    // reading back the same connection's own cached state.
    let (restore_conn, restore_path) = open_real_test_db();
    drop(restore_conn);
    stage(&restore_path, &bytes, "a-real-passphrase-123").unwrap();

    // The real, intended flow: db::open() on the same path again is
    // exactly what a real app restart does, and it's what actually
    // applies a staged restore -- not a hand-reconstructed version of
    // that same internal logic.
    let reopened = crate::db::open(restore_path.to_str().unwrap()).expect("reopen should apply the staged restore");
    let recovered_name: String = reopened
        .query_row("SELECT name FROM businesses WHERE id = ?1", [&business_id], |r| r.get(0))
        .unwrap();
    assert_eq!(recovered_name, "Backup Test Biz");

    // What the restore replaced is kept, not destroyed.
    let (db_previous, key_previous) = crate::db::previous_paths(&restore_path);
    assert!(db_previous.exists() && key_previous.exists(), "the previous database and key must be kept");
    assert!(nothing_staged(&restore_path), "applying must consume the staged files");

    drop(reopened);
    cleanup(&path);
    cleanup(&restore_path);
}

#[test]
fn test_backup_includes_committed_data_written_while_connection_is_open() {
    let (mut conn, path) = open_real_test_db();
    crate::business_panel::create_business(&mut conn, "First", "USD", "UTC").unwrap();
    crate::business_panel::create_business(&mut conn, "Second", "USD", "UTC").unwrap();
    // WAL mode: these commits may still be in the -wal file, not the
    // main file. A raw file copy would miss them; the backup must not.
    let bytes = backup_bytes(&path, "wal-passphrase-123");

    let (restore_conn, restore_path) = open_real_test_db();
    drop(restore_conn);
    stage(&restore_path, &bytes, "wal-passphrase-123").unwrap();
    let reopened = crate::db::open(restore_path.to_str().unwrap()).unwrap();
    let count: i64 = reopened.query_row("SELECT count(*) FROM businesses", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 2);

    drop(reopened);
    drop(conn);
    cleanup(&path);
    cleanup(&restore_path);
}

#[test]
fn test_wrong_passphrase_is_rejected_cleanly() {
    let (mut conn, path) = open_real_test_db();
    crate::business_panel::create_business(&mut conn, "Wrong Pass Biz", "USD", "UTC").unwrap();
    let bytes = backup_bytes(&path, "the-real-passphrase");

    let (restore_conn, restore_path) = open_real_test_db();
    drop(restore_conn);
    let result = stage(&restore_path, &bytes, "totally-wrong-passphrase");
    assert!(result.is_err(), "a wrong passphrase must be rejected, not silently accepted");
    assert!(nothing_staged(&restore_path), "a rejected restore must leave nothing behind");

    drop(conn);
    cleanup(&path);
    cleanup(&restore_path);
}

#[test]
fn test_weak_passphrase_refused_at_backup_time() {
    let (mut conn, path) = open_real_test_db();
    crate::business_panel::create_business(&mut conn, "Weak Pass Biz", "USD", "UTC").unwrap();
    let result = crate::backup::create_backup(&path, "short");
    assert!(result.is_err(), "a passphrase under 8 characters must be refused");
    drop(conn);
    cleanup(&path);
}

#[test]
fn test_backup_file_alone_does_not_reveal_the_real_key() {
    // The actual point of the passphrase: possessing the whole exported
    // file is genuinely not enough without it. This tries every
    // plausible "guess" a naive attacker with just the file might try,
    // and confirms all of them fail.
    let (mut conn, path) = open_real_test_db();
    crate::business_panel::create_business(&mut conn, "Security Test Biz", "USD", "UTC").unwrap();
    let bytes = backup_bytes(&path, "the-actual-real-passphrase");

    for guess in ["", "password", "the-actual-real-passphras", "THE-ACTUAL-REAL-PASSPHRASE"] {
        let (restore_conn, restore_path) = open_real_test_db();
        drop(restore_conn);
        assert!(stage(&restore_path, &bytes, guess).is_err(), "guess '{guess}' must not unlock the backup");
        cleanup(&restore_path);
    }

    drop(conn);
    cleanup(&path);
}

#[test]
fn test_truncated_or_corrupted_backup_is_rejected_before_anything_is_staged() {
    let (mut conn, path) = open_real_test_db();
    crate::business_panel::create_business(&mut conn, "Damage Biz", "USD", "UTC").unwrap();
    let bytes = backup_bytes(&path, "damage-passphrase-1");
    let (restore_conn, restore_path) = open_real_test_db();
    drop(restore_conn);

    // Truncated, declared length matching what arrived (a client that
    // didn't know better) and not matching (the usual case).
    let cut = &bytes[..bytes.len() - 1000];
    assert!(stage(&restore_path, cut, "damage-passphrase-1").is_err());
    assert!(crate::backup::stage_restore(&restore_path, &mut Cursor::new(cut.to_vec()), None, "damage-passphrase-1").is_err());
    assert!(nothing_staged(&restore_path));

    // One flipped bit inside the database bytes.
    let mut flipped = bytes.clone();
    let middle = flipped.len() / 2;
    flipped[middle] ^= 0x01;
    let err = stage(&restore_path, &flipped, "damage-passphrase-1").unwrap_err().to_string();
    assert!(err.contains("checksum"), "expected a checksum failure, got: {err}");
    assert!(nothing_staged(&restore_path));

    // Not a backup at all.
    assert!(stage(&restore_path, b"hello this is not a backup file", "damage-passphrase-1").is_err());
    assert!(nothing_staged(&restore_path));

    drop(conn);
    cleanup(&path);
    cleanup(&restore_path);
}

#[test]
fn test_older_json_format_backups_can_still_be_restored() {
    use base64::Engine;
    let (mut conn, path) = open_real_test_db();
    let business_id = crate::business_panel::create_business(&mut conn, "Legacy Biz", "USD", "UTC").unwrap();
    let bytes = backup_bytes(&path, "legacy-passphrase-1");

    // Rebuild the old file layout from the new one: the same database
    // bytes and wrapped key, as base64 inside JSON.
    let header_len = u32::from_le_bytes(bytes[16..20].try_into().unwrap()) as usize;
    let header: serde_json::Value = serde_json::from_slice(&bytes[20..20 + header_len]).unwrap();
    let db_bytes = &bytes[20 + header_len..bytes.len() - 32];
    let legacy = serde_json::json!({
        "database_base64": base64::engine::general_purpose::STANDARD.encode(db_bytes),
        "wrapped_key_base64": header["wrapped_key_base64"],
        "created_at": header["created_at"],
        "schema_version": "sme-pro-v1",
    })
    .to_string();

    let (restore_conn, restore_path) = open_real_test_db();
    drop(restore_conn);
    stage(&restore_path, legacy.as_bytes(), "legacy-passphrase-1").unwrap();
    let reopened = crate::db::open(restore_path.to_str().unwrap()).unwrap();
    let name: String = reopened.query_row("SELECT name FROM businesses WHERE id = ?1", [&business_id], |r| r.get(0)).unwrap();
    assert_eq!(name, "Legacy Biz");

    // And a wrong passphrase is still refused for the old format.
    drop(reopened);
    assert!(stage(&restore_path, legacy.as_bytes(), "wrong-passphrase-123").is_err());

    drop(conn);
    cleanup(&path);
    cleanup(&restore_path);
}

#[test]
fn test_backup_from_a_newer_app_version_is_refused_at_restore_time() {
    let (mut conn, path) = open_real_test_db();
    crate::business_panel::create_business(&mut conn, "Future Biz", "USD", "UTC").unwrap();
    conn.execute("INSERT INTO _schema_version (version) VALUES (9999)", []).unwrap();
    let bytes = backup_bytes(&path, "future-passphrase-1");

    let (restore_conn, restore_path) = open_real_test_db();
    drop(restore_conn);
    let err = stage(&restore_path, &bytes, "future-passphrase-1").unwrap_err().to_string();
    assert!(err.contains("newer version"), "got: {err}");
    assert!(nothing_staged(&restore_path), "the live data must not be replaced by something this build cannot open");

    drop(conn);
    cleanup(&path);
    cleanup(&restore_path);
}

#[test]
fn test_interrupted_apply_is_finished_on_next_launch() {
    let (mut conn, path) = open_real_test_db();
    let business_id = crate::business_panel::create_business(&mut conn, "Crash Biz", "USD", "UTC").unwrap();
    let bytes = backup_bytes(&path, "crash-passphrase-12");
    let (restore_conn, restore_path) = open_real_test_db();
    drop(restore_conn);
    stage(&restore_path, &bytes, "crash-passphrase-12").unwrap();

    // Simulate dying right after the key was swapped but before the
    // database was: the live key is already the restored one, and no
    // pending key remains. (The old code never looked for a pending
    // database in this state's mirror image and left key and database
    // mismatched forever.)
    let (_, key_pending) = crate::db::pending_restore_paths(&restore_path);
    std::fs::rename(&key_pending, crate::db::key_path_for(&restore_path)).unwrap();

    let reopened = crate::db::open(restore_path.to_str().unwrap()).expect("the interrupted restore must complete");
    let count: i64 = reopened.query_row("SELECT count(*) FROM businesses WHERE id = ?1", [&business_id], |r| r.get(0)).unwrap();
    assert_eq!(count, 1);

    drop(reopened);
    drop(conn);
    cleanup(&path);
    cleanup(&restore_path);
}

#[test]
fn test_a_lone_pending_key_from_interrupted_staging_is_discarded() {
    let (conn, path) = open_real_test_db();
    drop(conn);
    let (_, key_pending) = crate::db::pending_restore_paths(&path);
    std::fs::write(&key_pending, "0".repeat(64)).unwrap();
    let before = std::fs::read_to_string(crate::db::key_path_for(&path)).unwrap();

    let reopened = crate::db::open(path.to_str().unwrap()).expect("a stray pending key must not affect the live database");
    assert!(!key_pending.exists());
    assert_eq!(std::fs::read_to_string(crate::db::key_path_for(&path)).unwrap(), before);

    drop(reopened);
    cleanup(&path);
}

#[test]
fn test_leftover_temporary_copies_are_swept_on_startup() {
    let (conn, path) = open_real_test_db();
    drop(conn);
    let stale_backup = path.with_file_name(format!("{}.backup-deadbeef.tmp", path.file_name().unwrap().to_str().unwrap()));
    let stale_incoming = crate::db::restore_incoming_path(&path);
    std::fs::write(&stale_backup, b"x").unwrap();
    std::fs::write(&stale_incoming, b"x").unwrap();

    let reopened = crate::db::open(path.to_str().unwrap()).unwrap();
    assert!(!stale_backup.exists() && !stale_incoming.exists());

    drop(reopened);
    cleanup(&path);
}

#[test]
fn test_streamed_backup_file_is_removed_once_the_reader_is_dropped() {
    let (conn, path) = open_real_test_db();
    let file = crate::backup::create_backup(&path, "cleanup-passphrase-1").unwrap();
    let dir = path.parent().unwrap().to_path_buf();
    let prefix = format!("{}.backup-", path.file_name().unwrap().to_str().unwrap());
    let leftovers = || {
        std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with(&prefix)).count()
    };
    assert_eq!(leftovers(), 1, "the snapshot exists while the backup is being served");
    let mut reader = file.into_reader().unwrap();
    let mut first = [0u8; 16];
    reader.read_exact(&mut first).unwrap();
    assert_eq!(&first, crate::backup::MAGIC);
    drop(reader); // an abandoned download
    assert_eq!(leftovers(), 0, "an abandoned download must not leave a full copy of the database behind");

    drop(conn);
    cleanup(&path);
}

#[test]
fn test_passphrase_header_decoding() {
    use crate::backup::decode_passphrase_header as d;
    assert_eq!(d("plain-pass").unwrap(), "plain-pass");
    assert_eq!(d("p%C3%A9ss%20word%21%25").unwrap(), "péss word!%");
    assert!(d("bad%zz").is_err());
    assert!(d("trunc%4").is_err());
    assert!(d("%FF%FE").is_err(), "invalid UTF-8 must be refused, not mangled");
}
