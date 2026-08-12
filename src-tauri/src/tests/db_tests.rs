/// Tests for db::open() itself — most other tests use test_db(), which
/// deliberately bypasses this function entirely (opens an in-memory
/// connection and applies schema.sql directly) for speed, so none of
/// them actually exercise db::open()'s real behavior: the SQLCipher
/// key handling, or — what this file specifically exists to prove —
/// that wrapping schema.sql's CREATE TABLE statements in one explicit
/// transaction (a real startup-performance fix; see db.rs's own
/// comment on it) still produces a fully correct, queryable database,
/// not a partially-applied one.
fn tmp_db_path() -> String {
    std::env::temp_dir()
        .join(format!("smepro_db_test_{}.db", uuid::Uuid::new_v4()))
        .to_string_lossy()
        .to_string()
}

#[test]
fn test_open_creates_a_fully_working_database() {
    let path = tmp_db_path();
    let conn = crate::db::open(&path).expect("db::open should succeed on a fresh path");

    // Prove the transaction-wrapped schema application actually
    // created every core table — not just some of them, which is
    // exactly the failure mode a broken transaction wrap could
    // produce (partial commit, or every statement silently failing
    // together instead of succeeding together).
    let core_tables = ["businesses", "users", "roles", "permissions", "modules", "sessions", "audit_log"];
    for table in core_tables {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap_or_else(|e| panic!("table '{table}' should exist and be queryable after db::open(): {e}"));
        assert_eq!(count, 0, "a freshly created table should start empty");
    }

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(crate::db::key_path_for(std::path::Path::new(&path)));
}

#[test]
fn test_open_is_idempotent_across_repeated_launches() {
    // This is the exact real-world scenario the transaction-wrapping
    // fix targets: the app opens the same database file on every
    // launch, and schema.sql's CREATE TABLE IF NOT EXISTS statements
    // must be safe no-ops every time after the first — proven here
    // by actually opening the same file three times in a row and
    // creating real data in between, confirming nothing gets wiped,
    // duplicated, or corrupted by repeated schema application.
    let path = tmp_db_path();

    {
        let conn = crate::db::open(&path).unwrap();
        conn.execute(
            "INSERT INTO businesses (id, name, currency, tax_rate, created_at, updated_at) VALUES ('b1', 'Test Biz', 'USD', 0.0, datetime('now'), datetime('now'))",
            [],
        ).unwrap();
    }
    {
        let conn = crate::db::open(&path).unwrap();
        let name: String = conn.query_row("SELECT name FROM businesses WHERE id = 'b1'", [], |r| r.get(0)).unwrap();
        assert_eq!(name, "Test Biz", "data must survive a second db::open() call on the same file");
    }
    {
        // A third open, same file — proves this isn't a one-time fluke.
        let conn = crate::db::open(&path).unwrap();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM businesses", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 1, "repeated schema application must never duplicate or lose existing rows");
    }

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(crate::db::key_path_for(std::path::Path::new(&path)));
}

#[test]
fn test_open_reuses_the_same_encryption_key_across_launches() {
    // The SQLCipher key is generated once and persisted to a sibling
    // .key file (see get_or_create_key) — a second open() on the same
    // path must reuse it, not generate a new one and lock itself out
    // of its own data.
    let path = tmp_db_path();
    crate::db::open(&path).unwrap();
    let key_path = crate::db::key_path_for(std::path::Path::new(&path));
    assert!(key_path.exists(), "a key file must be created alongside the database");
    let key_contents_first = std::fs::read_to_string(&key_path).unwrap();

    crate::db::open(&path).unwrap();
    let key_contents_second = std::fs::read_to_string(&key_path).unwrap();
    assert_eq!(key_contents_first, key_contents_second, "the key must not be regenerated on a second open");

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&key_path);
}
