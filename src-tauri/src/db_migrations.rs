//! Schema migration system — idempotent, transactional, version-tracked.

use anyhow::Result;
use rusqlite::Connection;

const CURRENT_VERSION: i32 = 7;

pub fn run(conn: &mut Connection) -> Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS _schema_version (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
        [],
    )?;

    let current: i32 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM _schema_version",
        [],
        |r| r.get(0),
    )?;

    if current < 1 { v1_initial(conn)?; }
    if current < 2 { v2_add_slogan(conn)?; }
    if current < 3 { v3_invoice_module(conn)?; }
    if current < 4 { v4_add_totp(conn)?; }
    if current < 5 { v5_add_tax_rates(conn)?; }
    if current < 6 { v6_add_exchange_rates(conn)?; }
    if current < 7 { v7_session_security(conn)?; }
    debug_assert_eq!(CURRENT_VERSION, 7, "bump this alongside the last `if current < N` check above");

    Ok(())
}

fn v1_initial(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute("INSERT INTO _schema_version (version) VALUES (1)", [])?;
    tx.commit()?;
    Ok(())
}

fn v2_add_slogan(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute("ALTER TABLE businesses ADD COLUMN slogan TEXT", [])?;
    tx.execute("INSERT INTO _schema_version (version) VALUES (2)", [])?;
    tx.commit()?;
    Ok(())
}

fn v3_invoice_module(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute("INSERT INTO _schema_version (version) VALUES (3)", [])?;
    tx.commit()?;
    Ok(())
}

fn v4_add_totp(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute("ALTER TABLE users ADD COLUMN totp_secret TEXT", [])?;
    tx.execute("ALTER TABLE users ADD COLUMN totp_recovery_codes TEXT", [])?;
    tx.execute("ALTER TABLE users ADD COLUMN totp_enabled INTEGER NOT NULL DEFAULT 0", [])?;
    tx.execute("INSERT INTO _schema_version (version) VALUES (4)", [])?;
    tx.commit()?;
    Ok(())
}

fn v5_add_tax_rates(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "CREATE TABLE IF NOT EXISTS tax_rates (
            id TEXT PRIMARY KEY,
            business_id TEXT NOT NULL REFERENCES businesses(id) ON DELETE CASCADE,
            category TEXT NOT NULL,
            rate REAL NOT NULL DEFAULT 0.0,
            created_at TEXT NOT NULL,
            updated_at TEXT,
            UNIQUE(business_id, category)
        )",
        [],
    )?;
    tx.execute("INSERT INTO _schema_version (version) VALUES (5)", [])?;
    tx.commit()?;
    Ok(())
}

fn v6_add_exchange_rates(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "CREATE TABLE IF NOT EXISTS exchange_rates (
            id TEXT PRIMARY KEY,
            from_currency TEXT NOT NULL,
            to_currency TEXT NOT NULL,
            rate REAL NOT NULL,
            fetched_at INTEGER NOT NULL,
            UNIQUE(from_currency, to_currency)
        )",
        [],
    )?;
    tx.execute("INSERT INTO _schema_version (version) VALUES (6)", [])?;
    tx.commit()?;
    Ok(())
}

fn v7_session_security(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "ALTER TABLE sessions ADD COLUMN last_activity TEXT NOT NULL DEFAULT (datetime('now'))",
        [],
    )?;
    tx.execute("INSERT INTO _schema_version (version) VALUES (7)", [])?;
    tx.commit()?;
    Ok(())
}
