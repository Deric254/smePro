//! Schema migration system — idempotent, transactional, version-tracked.

use anyhow::Result;
use rusqlite::Connection;

const CURRENT_VERSION: i32 = 10;

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
    if current < 8 { v8_money_to_cents(conn)?; }
    if current < 9 { v9_ai_chat_history(conn)?; }
    if current < 10 { v10_customers_name_only(conn)?; }
    debug_assert_eq!(CURRENT_VERSION, 10, "bump this alongside the last `if current < N` check above");

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

/// Converts every business's money fields from legacy float dollars to
/// integer minor units (cents) — see money.rs for why this exists at
/// all. Runs exactly once, gated by the schema version like every
/// other migration here, entirely inside one transaction: either
/// everything converts together, or (on any error) none of it does
/// and the database is left exactly as it was.
///
/// Module tables are shared by module_id ACROSS every business in
/// this install (see ModuleDef::table_name — there's no per-business
/// table), so this runs once per physical table, never once per
/// business row that happens to reference it — looping per-business
/// here would silently double-convert data for any install with more
/// than one business sharing the same module.
///
/// A plain `UPDATE table SET col = CAST(ROUND(col * scale) AS
/// INTEGER)` is NOT enough and was the first, wrong version of this
/// migration: a column declared with REAL affinity converts an
/// inserted integer straight back into floating-point storage
/// (SQLite's own type-affinity rules), which is numerically exact but
/// breaks every typed `rusqlite` read (`row.get::<_, i64>`) that now
/// expects INTEGER storage throughout this codebase — caught by this
/// migration's own test suite, not assumed safe. SQLite has no ALTER
/// COLUMN TYPE, so the only correct fix is the standard rebuild: a new
/// table with the right column types, data copied across with the
/// conversion applied during the copy, old table dropped.
fn v8_money_to_cents(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;

    let module_ids: Vec<String> = {
        let mut stmt = tx.prepare("SELECT DISTINCT id FROM modules WHERE table_created = 1")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.filter_map(|r| r.ok()).collect()
    };

    for module_id in &module_ids {
        let on_disk_path = crate::modules_dir().join(format!("{module_id}.json"));
        let Ok(on_disk_raw) = std::fs::read_to_string(&on_disk_path) else { continue };
        let Ok(on_disk_def) = crate::module::ModuleDef::from_json_str(&on_disk_raw) else { continue };
        let table = on_disk_def.table_name();

        // Ground truth: what the table's OWN column type says right
        // now — not what any one business's schema snapshot claims,
        // since the physical table is the thing that actually
        // determines what rusqlite can read back.
        let existing_types: std::collections::HashMap<String, String> = {
            let mut stmt = tx.prepare(&format!("PRAGMA table_info({table})"))?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
            rows.filter_map(|r| r.ok()).collect()
        };
        if existing_types.is_empty() {
            continue; // registry claims a table exists but it doesn't — nothing to migrate
        }

        let money_fields: Vec<&crate::module::FieldDef> = on_disk_def
            .fields
            .iter()
            .filter(|f| {
                f.field_type == "money"
                    && existing_types
                        .get(&f.name)
                        .map(|t| t.eq_ignore_ascii_case("REAL"))
                        .unwrap_or(false)
            })
            .collect();

        if !money_fields.is_empty() {
            let old_table = format!("{table}__pre_money_v8");
            tx.execute(&format!("ALTER TABLE {table} RENAME TO {old_table}"), [])?;

            let mut cols = vec![
                "id TEXT PRIMARY KEY".to_string(),
                "business_id TEXT NOT NULL".to_string(),
            ];
            cols.extend(on_disk_def.field_column_defs()?);
            cols.push("created_at TEXT NOT NULL".to_string());
            cols.push("updated_at TEXT NOT NULL".to_string());
            cols.push("deleted_at TEXT".to_string());
            tx.execute(&format!("CREATE TABLE {table} ({})", cols.join(", ")), [])?;

            // Every currency actually present among businesses that
            // have rows in this table — usually just one, in this
            // local-first, normally-single-business system, but
            // handled correctly either way. money::decimal_places_for
            // is the single source of truth for how many places each
            // currency uses; it is never re-derived in SQL.
            let currencies: Vec<String> = {
                let mut stmt = tx.prepare(&format!(
                    "SELECT DISTINCT COALESCE(b.currency, 'USD') FROM {old_table} t
                     LEFT JOIN businesses b ON b.id = t.business_id"
                ))?;
                let rows = stmt.query_map([], |r| r.get(0))?;
                rows.filter_map(|r| r.ok()).collect()
            };

            for currency in &currencies {
                let scale = 10_i64.pow(crate::money::decimal_places_for(currency));

                let mut select_cols = vec!["t.id".to_string(), "t.business_id".to_string()];
                for f in &on_disk_def.fields {
                    if !existing_types.contains_key(&f.name) {
                        // A field the current on-disk schema has that
                        // this particular old table never had — added
                        // to modules/*.json at some point after this
                        // table was first created, unrelated to the
                        // money migration itself. Falls back to the
                        // field's own declared default (or NULL) rather
                        // than referencing a column that doesn't exist,
                        // which would otherwise fail the whole rebuild
                        // over an unrelated schema drift.
                        let default_sql = match &f.default {
                            Some(serde_json::Value::String(s)) => format!("'{}'", s.replace('\'', "''")),
                            Some(serde_json::Value::Number(n)) => n.to_string(),
                            Some(serde_json::Value::Bool(b)) => if *b { "1".into() } else { "0".into() },
                            _ => "NULL".into(),
                        };
                        select_cols.push(format!("{default_sql} AS {}", f.name));
                    } else if money_fields.iter().any(|mf| mf.name == f.name) {
                        select_cols.push(format!(
                            "CAST(ROUND(t.{col} * {scale}) AS INTEGER) AS {col}",
                            col = f.name
                        ));
                    } else {
                        select_cols.push(format!("t.{col}", col = f.name));
                    }
                }
                select_cols.push("t.created_at".to_string());
                select_cols.push("t.updated_at".to_string());
                select_cols.push("t.deleted_at".to_string());

                tx.execute(
                    &format!(
                        "INSERT INTO {table} SELECT {} FROM {old_table} t
                         LEFT JOIN businesses b ON b.id = t.business_id
                         WHERE COALESCE(b.currency, 'USD') = ?1",
                        select_cols.join(", ")
                    ),
                    rusqlite::params![currency],
                )?;
            }

            tx.execute(&format!("DROP TABLE {old_table}"), [])?;
            tx.execute(
                &format!("CREATE INDEX IF NOT EXISTS idx_{table}_business ON {table}(business_id, deleted_at)"),
                [],
            )?;
        }

        // Refresh EVERY business's frozen schema snapshot for this
        // module_id to the current on-disk definition. Per-business
        // deliberately — each business has its own `modules` row even
        // though the physical table above is shared and rebuilt only
        // once. This is what makes module.rs's validate() reject any
        // future float written to a money field, independent of the
        // SQL column's own affinity.
        tx.execute(
            "UPDATE modules SET schema_json = ?1 WHERE id = ?2",
            rusqlite::params![serde_json::to_string(&on_disk_def)?, module_id],
        )?;
    }

    tx.execute("INSERT INTO _schema_version (version) VALUES (8)", [])?;
    tx.commit()?;
    Ok(())
}

/// Persisted AI chat history — before this, every question and answer
/// lived only in React state, gone the moment the panel closed or the
/// app restarted. Two tables: `ai_chat_sessions` (one row per
/// conversation, so a business can hold several distinct threads
/// rather than one endless scrollback) and `ai_chat_messages` (every
/// question/answer pair within a session, in order). Scoped by
/// `business_id` AND `user_id` — one user's chat history is their own,
/// not shared across every login on the account, same privacy model
/// as everything else per-user in this app.
fn v9_ai_chat_history(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "CREATE TABLE IF NOT EXISTS ai_chat_sessions (
            id TEXT PRIMARY KEY,
            business_id TEXT NOT NULL,
            user_id TEXT NOT NULL,
            title TEXT NOT NULL DEFAULT 'New chat',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
        [],
    )?;
    tx.execute(
        "CREATE INDEX IF NOT EXISTS idx_ai_chat_sessions_user
         ON ai_chat_sessions(business_id, user_id, updated_at)",
        [],
    )?;
    tx.execute(
        "CREATE TABLE IF NOT EXISTS ai_chat_messages (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL REFERENCES ai_chat_sessions(id) ON DELETE CASCADE,
            role TEXT NOT NULL CHECK (role IN ('user','ai')),
            content TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
        [],
    )?;
    tx.execute(
        "CREATE INDEX IF NOT EXISTS idx_ai_chat_messages_session
         ON ai_chat_messages(session_id, created_at)",
        [],
    )?;
    tx.execute("INSERT INTO _schema_version (version) VALUES (9)", [])?;
    tx.commit()?;
    Ok(())
}

/// Allows a customer to be tracked by NAME ALONE, not just phone —
/// see the customers table's own doc comment in schema.sql for the
/// honest trade-off (name-only matching is weaker than phone; this is
/// a deliberate accommodation for cashiers/businesses that don't ask
/// for a phone number, not a claim that it's just as reliable).
///
/// Checks the table's OWN current column state via PRAGMA table_info
/// — same approach as v8's money migration — rather than assuming
/// every install is on the same prior schema: a fresh install already
/// has `phone` nullable straight from schema.sql (nothing to rebuild),
/// while an existing install still has the original `phone TEXT NOT
/// NULL` and needs a real table rebuild (SQLite has no `ALTER COLUMN`
/// to just relax a NOT NULL constraint in place). The partial unique
/// index for name-only dedup is created unconditionally either way,
/// since `CREATE UNIQUE INDEX IF NOT EXISTS` is safe to run whether or
/// not it already exists.
fn v10_customers_name_only(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;

    let phone_is_not_null: bool = {
        let mut stmt = tx.prepare("PRAGMA table_info(customers)")?;
        let mut rows = stmt.query([])?;
        let mut found = false;
        while let Some(row) = rows.next()? {
            let name: String = row.get(1)?;
            if name == "phone" {
                let notnull: i64 = row.get(3)?;
                found = notnull == 1;
                break;
            }
        }
        found
    };

    if phone_is_not_null {
        // Standard SQLite rebuild: no in-place way to drop a NOT NULL
        // constraint, so create the correctly-shaped table, copy every
        // row across unchanged (no data transformation needed here,
        // unlike v8's money conversion — existing phone values are
        // already valid, non-null strings), drop the old table, rename.
        tx.execute(
            "CREATE TABLE customers_new (
                id            TEXT PRIMARY KEY,
                business_id   TEXT NOT NULL REFERENCES businesses(id) ON DELETE CASCADE,
                name          TEXT,
                phone         TEXT,
                created_at    TEXT NOT NULL,
                updated_at    TEXT NOT NULL,
                UNIQUE(business_id, phone)
            )",
            [],
        )?;
        tx.execute(
            "INSERT INTO customers_new (id, business_id, name, phone, created_at, updated_at)
             SELECT id, business_id, name, phone, created_at, updated_at FROM customers",
            [],
        )?;
        tx.execute("DROP TABLE customers", [])?;
        tx.execute("ALTER TABLE customers_new RENAME TO customers", [])?;
    }

    tx.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_customers_name_only
         ON customers(business_id, name) WHERE phone IS NULL",
        [],
    )?;

    tx.execute("INSERT INTO _schema_version (version) VALUES (10)", [])?;
    tx.commit()?;
    Ok(())
}
