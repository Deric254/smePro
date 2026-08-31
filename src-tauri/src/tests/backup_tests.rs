use rusqlite::Connection;
use std::path::PathBuf;

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

fn cleanup(path: &PathBuf) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(crate::db::key_path_for(path));
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}

#[test]
fn test_backup_and_restore_round_trip_recovers_real_data() {
    let (mut conn, path) = open_real_test_db();
    let business_id = crate::business_panel::create_business(&mut conn, "Backup Test Biz", "USD", "UTC").unwrap();

    let backup = crate::backup::create_backup(&conn, "a-real-passphrase-123").unwrap();
    drop(conn);

    // A genuinely fresh, separate database -- proving this isn't just
    // reading back the same connection's own cached state.
    let (restore_conn, restore_path) = open_real_test_db();
    let input = crate::backup::RestoreInput {
        database_base64: backup.database_base64.clone(),
        wrapped_key_base64: backup.wrapped_key_base64.clone(),
        passphrase: "a-real-passphrase-123".to_string(),
    };
    crate::backup::stage_restore(&restore_conn, input).unwrap();
    drop(restore_conn);

    // The real, intended flow: db::open() on the same path again is
    // exactly what a real app restart does, and it's what actually
    // applies a staged restore -- not a hand-reconstructed version of
    // that same internal logic.
    let reopened = crate::db::open(restore_path.to_str().unwrap()).expect("reopen should apply the staged restore");
    let recovered_name: String = reopened
        .query_row("SELECT name FROM businesses WHERE id = ?1", [&business_id], |r| r.get(0))
        .unwrap();
    assert_eq!(recovered_name, "Backup Test Biz");

    cleanup(&path);
    cleanup(&restore_path);
}

#[test]
fn test_wrong_passphrase_is_rejected_cleanly() {
    let (mut conn, path) = open_real_test_db();
    crate::business_panel::create_business(&mut conn, "Wrong Pass Biz", "USD", "UTC").unwrap();
    let backup = crate::backup::create_backup(&conn, "the-real-passphrase").unwrap();

    let (restore_conn, restore_path) = open_real_test_db();
    let input = crate::backup::RestoreInput {
        database_base64: backup.database_base64,
        wrapped_key_base64: backup.wrapped_key_base64,
        passphrase: "totally-wrong-passphrase".to_string(),
    };
    let result = crate::backup::stage_restore(&restore_conn, input);
    assert!(result.is_err(), "a wrong passphrase must be rejected, not silently accepted");

    cleanup(&path);
    cleanup(&restore_path);
}

#[test]
fn test_weak_passphrase_refused_at_backup_time() {
    let (mut conn, path) = open_real_test_db();
    crate::business_panel::create_business(&mut conn, "Weak Pass Biz", "USD", "UTC").unwrap();

    let result = crate::backup::create_backup(&conn, "short");
    assert!(result.is_err(), "a passphrase under 8 characters must be refused, not silently accepted");

    cleanup(&path);
}

#[test]
fn test_backup_file_alone_does_not_reveal_the_real_key() {
    // The actual point of this whole fix: possessing database_base64
    // and wrapped_key_base64 together -- everything in the exported
    // file -- is genuinely not enough without the passphrase. This
    // tries every plausible "guess" a naive attacker with just the
    // file might try, and confirms all of them fail.
    let (mut conn, path) = open_real_test_db();
    crate::business_panel::create_business(&mut conn, "Security Test Biz", "USD", "UTC").unwrap();
    let backup = crate::backup::create_backup(&conn, "the-actual-real-passphrase").unwrap();

    for guess in ["", "password", "the-actual-real-passphras", "THE-ACTUAL-REAL-PASSPHRASE"] {
        let (restore_conn, restore_path) = open_real_test_db();
        let input = crate::backup::RestoreInput {
            database_base64: backup.database_base64.clone(),
            wrapped_key_base64: backup.wrapped_key_base64.clone(),
            passphrase: guess.to_string(),
        };
        let result = crate::backup::stage_restore(&restore_conn, input);
        assert!(result.is_err(), "guess '{guess}' must not unlock the backup");
        cleanup(&restore_path);
    }

    cleanup(&path);
}
