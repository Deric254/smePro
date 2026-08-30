//! Schema migration system — idempotent, transactional, version-tracked.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};

const CURRENT_VERSION: i32 = 13;

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
    if current < 11 { v11_stock_takes(conn)?; }
    if current < 12 { v12_debt_settlement_payment_method(conn)?; }
    if current < 13 { v13_scope_unique_fields_to_business(conn)?; }
    debug_assert_eq!(CURRENT_VERSION, 13, "bump this alongside the last `if current < N` check above");

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
        // Was `modules_dir().join(...)` + `std::fs::read_to_string` —
        // see crate::MODULE_DEFS for why that silently failed on
        // Android for every module. For THIS migration specifically,
        // that silent failure meant every affected money column simply
        // never got converted from REAL to INTEGER cents on any Android
        // install — a correctness bug, not just a missing-feature one.
        let Some(on_disk_raw) = crate::module_json(module_id) else { continue };
        let Ok(on_disk_def) = crate::module::ModuleDef::from_json_str(on_disk_raw) else { continue };
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
            // Same business-scoped (not global) unique constraints
            // create_table() emits for a fresh table — this rebuild
            // must not regenerate the bug v13 exists to fix for a
            // table that happens to go through this v8 path instead.
            cols.extend(on_disk_def.business_scoped_unique_constraints());
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

/// Adds the Stock Take feature: a dedicated initiate → count → close
/// workflow for reconciling physical stock counts against what the
/// system thinks is on the shelf, distinct from (and complementary to)
/// the Excel bulk-import reconciliation path — this one is a guided,
/// point-in-time, per-item counting session with its own audit trail
/// and variance report, for a business that wants to do a real
/// walk-the-floor count rather than re-upload a spreadsheet.
///
/// Only one stock take can be open (`status = 'in_progress'`) per
/// business at a time — enforced by the partial unique index below,
/// not just application logic, so a second `initiate()` call racing
/// against the first fails at the database level regardless of what
/// bug might exist in the Rust code calling it. A stock take counting
/// against a moving, simultaneously-open second count would make
/// "what does this variance even mean" ambiguous — there's no clean
/// way to attribute a quantity change to one count or the other, so
/// this is prevented outright rather than allowed and reasoned about
/// after the fact.
///
/// THE REAL GAP THIS ALSO CLOSES, beyond the new tables: this app
/// stores each business's module definition (including which actions
/// are enabled for the Roles screen) as a SNAPSHOT in
/// `modules.schema_json`, captured once when that module was first
/// turned on — not read live from this crate's bundled
/// `modules/inventory.json` on every request (see crud.rs's
/// `load_module`). That means simply adding `"stocktake"` to
/// `inventory.json`'s `actions` and `default_roles` only affects a
/// business enabling Inventory for the very first time AFTER this
/// version ships — every business that already has Inventory enabled
/// today would silently never see the new action at all: not in the
/// Roles screen's checkbox list, and not granted to their existing
/// Owner/Manager roles, since `rbac::seed_default_roles` also only
/// ever runs once, at enable time. Both parts of that gap are patched
/// here, for every business that already has inventory enabled:
///  1. `modules.schema_json` is rewritten to add "stocktake" to its
///     stored `actions` list, if it isn't already there (so the Roles
///     screen can even show the checkbox).
///  2. Existing `Owner` and `Manager` roles (matched by name, the same
///     heuristic `seed_default_roles` itself already uses — a purely
///     cosmetic default, not an authorization rule, since an Owner can
///     freely rename or reconfigure roles afterward either way) are
///     directly granted the `stocktake` permission on `inventory`,
///     since there's no "first enable" moment left to seed it at.
fn v11_stock_takes(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;

    tx.execute(
        "CREATE TABLE IF NOT EXISTS stock_takes (
            id                  TEXT PRIMARY KEY,
            business_id         TEXT NOT NULL REFERENCES businesses(id) ON DELETE CASCADE,
            status              TEXT NOT NULL DEFAULT 'in_progress' CHECK (status IN ('in_progress','closed')),
            created_by_user_id  TEXT NOT NULL,
            created_at          TEXT NOT NULL DEFAULT (datetime('now')),
            closed_at           TEXT,
            closed_by_user_id   TEXT
        )",
        [],
    )?;
    // Enforced at the database level, not just in application code —
    // see the module doc comment above for why a second concurrent
    // stock take is prevented outright rather than reasoned about
    // after the fact.
    tx.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_stock_takes_one_open_per_business
         ON stock_takes(business_id) WHERE status = 'in_progress'",
        [],
    )?;
    tx.execute(
        "CREATE INDEX IF NOT EXISTS idx_stock_takes_business
         ON stock_takes(business_id, created_at)",
        [],
    )?;

    tx.execute(
        "CREATE TABLE IF NOT EXISTS stock_take_items (
            id                   TEXT PRIMARY KEY,
            stock_take_id        TEXT NOT NULL REFERENCES stock_takes(id) ON DELETE CASCADE,
            inventory_record_id  TEXT NOT NULL,
            item_name            TEXT NOT NULL,
            expected_qty         INTEGER NOT NULL,
            counted_qty          INTEGER,
            counted_at           TEXT
        )",
        [],
    )?;
    tx.execute(
        "CREATE INDEX IF NOT EXISTS idx_stock_take_items_stock_take
         ON stock_take_items(stock_take_id)",
        [],
    )?;

    // --- Backfill for businesses that already have inventory enabled ---
    let existing_inventory: Vec<(String, String)> = {
        let mut stmt = tx.prepare(
            "SELECT business_id, schema_json FROM modules WHERE id = 'inventory' AND enabled = 1",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    for (business_id, schema_json) in existing_inventory {
        // Part 1: patch the stored schema snapshot so the Roles screen
        // can show "stocktake" as a real, checkable action for this
        // business, not just for businesses onboarding from today
        // onward.
        let mut parsed: serde_json::Value = match serde_json::from_str(&schema_json) {
            Ok(v) => v,
            Err(_) => continue, // corrupt snapshot pre-dating this migration is out of scope to repair here; leave it untouched rather than risk making it worse
        };
        let mut changed = false;
        if let Some(actions) = parsed.get_mut("actions").and_then(|a| a.as_array_mut()) {
            if !actions.iter().any(|a| a.as_str() == Some("stocktake")) {
                actions.push(serde_json::Value::String("stocktake".to_string()));
                changed = true;
            }
        }
        if changed {
            let new_json = serde_json::to_string(&parsed).unwrap_or(schema_json);
            tx.execute(
                "UPDATE modules SET schema_json = ?1 WHERE business_id = ?2 AND id = 'inventory'",
                rusqlite::params![new_json, business_id],
            )?;
        }

        // Part 2: grant the permission directly to this business's
        // existing Owner/Manager roles — the one-time seeding function
        // that would normally do this has already run, for this
        // business, in the past; there's no "enable" moment left to
        // hook into.
        for role_name in ["Owner", "Manager"] {
            let role_id: Option<String> = tx
                .query_row(
                    "SELECT id FROM roles WHERE business_id = ?1 AND name = ?2",
                    rusqlite::params![business_id, role_name],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(role_id) = role_id {
                tx.execute(
                    "INSERT INTO permissions (id, role_id, module_id, action)
                     VALUES (lower(hex(randomblob(16))), ?1, 'inventory', 'stocktake')
                     ON CONFLICT(role_id, module_id, action) DO NOTHING",
                    rusqlite::params![role_id],
                )?;
            }
        }
    }

    tx.execute("INSERT INTO _schema_version (version) VALUES (11)", [])?;
    tx.commit()?;
    Ok(())
}

/// Adds the columns debt_settlement.rs's `settle` (and pos.rs's
/// credit-sale path) now need on the shared `module_debt_credit` /
/// `module_accounting` tables — `payment_method` and
/// `source_order_id` on the former, `payment_method` on the latter.
///
/// THE SAME TWO-PART GAP v11's own doc comment already names, for the
/// same reason: this app stores each business's module definition as
/// a SNAPSHOT in `modules.schema_json`, captured once at enable time —
/// not read live from this crate's bundled `modules/*.json` on every
/// request (see crud.rs's `load_module`). Editing debt_credit.json /
/// accounting.json only affects a business enabling those modules for
/// the very first time after this ships. Every business that already
/// had them enabled needs BOTH parts patched here:
///  1. The physical columns added to the actual table (this is NOT
///     covered by `ModuleDef::create_table`'s own
///     `CREATE TABLE IF NOT EXISTS` — that only builds from the
///     current schema the first time a module is enabled at all;
///     module tables are shared across every business on an install,
///     see `ModuleDef::table_name`, so it never runs again after
///     that).
///  2. The stored `modules.schema_json` snapshot itself, so
///     `ModuleDef::validate()` and the generic create/update form
///     both actually recognize these as real fields for this
///     business — without this part, the columns would exist but
///     every write to them would still be rejected as an unknown
///     field.
/// Skipping either part half-fixes it: columns without the schema
/// update means "unknown field" validation errors; schema update
/// without columns means "no such column" SQL errors. Both, together,
/// one transaction.
fn v12_debt_settlement_payment_method(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;

    // --- Part 1: physical columns ---
    for (table, column, col_type) in [
        ("module_debt_credit", "payment_method", "TEXT"),
        ("module_debt_credit", "source_order_id", "TEXT"),
        ("module_accounting", "payment_method", "TEXT"),
    ] {
        let table_exists: i64 = tx.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
            rusqlite::params![table],
            |r| r.get(0),
        )?;
        if table_exists == 0 {
            continue; // module never enabled on this install — create_table will build it right, whenever it first is
        }
        // SQLite has no `ADD COLUMN IF NOT EXISTS`; PRAGMA table_info
        // is the standard way to check first, same reasoning as the
        // table-existence check above.
        let already_has_column: i64 = tx.query_row(
            &format!("SELECT count(*) FROM pragma_table_info('{table}') WHERE name=?1"),
            rusqlite::params![column],
            |r| r.get(0),
        )?;
        if already_has_column == 0 {
            tx.execute(&format!("ALTER TABLE {table} ADD COLUMN {column} {col_type}"), [])?;
        }
    }

    // --- Part 2: schema_json snapshot backfill, same technique as v11 ---
    let new_fields: &[(&str, &str)] = &[
        ("payment_method", "text"),
        ("source_order_id", "text"),
    ];
    for (module_id, fields_to_add) in [
        ("debt_credit", new_fields),
        ("accounting", &new_fields[..1]), // accounting only gets payment_method, not source_order_id
    ] {
        let rows: Vec<(String, String)> = {
            let mut stmt = tx.prepare(
                "SELECT business_id, schema_json FROM modules WHERE id = ?1 AND enabled = 1",
            )?;
            let mapped = stmt.query_map(rusqlite::params![module_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            mapped.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (business_id, schema_json) in rows {
            let mut parsed: serde_json::Value = match serde_json::from_str(&schema_json) {
                Ok(v) => v,
                Err(_) => continue, // corrupt snapshot pre-dating this migration is out of scope to repair here; leave it untouched rather than risk making it worse
            };
            let mut changed = false;
            if let Some(fields) = parsed.get_mut("fields").and_then(|f| f.as_array_mut()) {
                for (name, field_type) in fields_to_add {
                    let already_present = fields.iter().any(|f| f.get("name").and_then(|n| n.as_str()) == Some(*name));
                    if !already_present {
                        fields.push(serde_json::json!({
                            "name": name,
                            "type": field_type,
                            "required": false,
                            "unique": false,
                            "default": null,
                        }));
                        changed = true;
                    }
                }
            }
            if changed {
                let new_json = serde_json::to_string(&parsed).unwrap_or(schema_json);
                tx.execute(
                    "UPDATE modules SET schema_json = ?1 WHERE business_id = ?2 AND id = ?3",
                    rusqlite::params![new_json, business_id, module_id],
                )?;
            }
        }
    }

    tx.execute("INSERT INTO _schema_version (version) VALUES (12)", [])?;
    tx.commit()?;
    Ok(())
}

/// Fixes a real correctness bug in the generic module engine — see
/// module.rs's `business_scoped_unique_constraints` doc comment for
/// the full explanation. A field marked `unique: true` in a module's
/// JSON definition (currently only inventory's `sku`) used to get a
/// bare, GLOBAL column-level `UNIQUE` constraint on a table that is
/// actually shared across every business in this install — so two
/// completely unrelated businesses could never both use the same SKU.
/// This rebuilds any table still carrying that old constraint shape
/// with it correctly scoped to `UNIQUE(business_id, field)` instead —
/// exactly what `create_table()` emits for a table built fresh after
/// this fix, and reproduced here for one already sitting on disk.
///
/// Detection, not blind rebuild: only a table that actually still
/// carries the old single-column UNIQUE autoindex gets touched — a
/// table already created on or after this fix (correctly
/// business-scoped from the start) is left alone. Safe by
/// construction either way: the replacement constraint is strictly
/// weaker than what it replaces (it can only permit combinations the
/// old one wrongly forbade, never the reverse), so copying existing
/// rows into it can never itself produce a new violation.
fn v13_scope_unique_fields_to_business(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;

    let module_ids: Vec<String> = {
        let mut stmt = tx.prepare("SELECT DISTINCT id FROM modules WHERE table_created = 1")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.filter_map(|r| r.ok()).collect()
    };

    for module_id in &module_ids {
        // Same reasoning as v8: the on-disk definition (what create_table
        // would build today), not any one business's possibly-stale
        // schema_json snapshot, is what actually says which fields are
        // declared unique.
        let Some(on_disk_raw) = crate::module_json(module_id) else { continue };
        let Ok(on_disk_def) = crate::module::ModuleDef::from_json_str(on_disk_raw) else { continue };
        let unique_fields: Vec<&str> = on_disk_def.fields.iter().filter(|f| f.unique).map(|f| f.name.as_str()).collect();
        if unique_fields.is_empty() {
            continue; // nothing on this module was ever declared unique — nothing to fix
        }
        let table = on_disk_def.table_name();

        let table_exists: i64 = tx.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
            rusqlite::params![table],
            |r| r.get(0),
        )?;
        if table_exists == 0 {
            continue; // registry claims a table exists but it doesn't — nothing to rebuild
        }

        // Find every UNIQUE index SQLite auto-created for a bare
        // column-level UNIQUE constraint (origin = 'u') and check
        // whether it covers exactly one of this module's unique
        // fields, alone — that shape is only ever produced by the old,
        // global-per-column constraint. A table already built correctly
        // by the fixed code instead carries a compound (business_id,
        // field) unique index, which this check deliberately does not
        // match, so it's correctly left untouched.
        let needs_rebuild = {
            let mut idx_stmt = tx.prepare(&format!("PRAGMA index_list({table})"))?;
            let indexes: Vec<(String, i64, String)> = idx_stmt
                .query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?)))?
                .filter_map(|r| r.ok())
                .collect();
            let mut found = false;
            for (index_name, is_unique, origin) in &indexes {
                if *is_unique != 1 || origin != "u" {
                    continue;
                }
                let mut info_stmt = tx.prepare(&format!("PRAGMA index_info({index_name})"))?;
                let cols: Vec<String> = info_stmt
                    .query_map([], |r| r.get::<_, String>(2))?
                    .filter_map(|r| r.ok())
                    .collect();
                if cols.len() == 1 && unique_fields.contains(&cols[0].as_str()) {
                    found = true;
                    break;
                }
            }
            found
        };
        if !needs_rebuild {
            continue;
        }

        // Ground truth for which columns the OLD table actually has —
        // same defensive approach as v8, since a field can have been
        // added to modules/*.json at some point after this specific
        // table was first created, unrelated to this fix.
        let existing_cols: std::collections::HashSet<String> = {
            let mut stmt = tx.prepare(&format!("PRAGMA table_info({table})"))?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
            rows.filter_map(|r| r.ok()).collect()
        };

        let old_table = format!("{table}__pre_unique_scope_v13");
        tx.execute(&format!("ALTER TABLE {table} RENAME TO {old_table}"), [])?;

        let mut cols = vec![
            "id TEXT PRIMARY KEY".to_string(),
            "business_id TEXT NOT NULL".to_string(),
        ];
        cols.extend(on_disk_def.field_column_defs()?);
        cols.push("created_at TEXT NOT NULL".to_string());
        cols.push("updated_at TEXT NOT NULL".to_string());
        cols.push("deleted_at TEXT".to_string());
        cols.extend(on_disk_def.business_scoped_unique_constraints());
        tx.execute(&format!("CREATE TABLE {table} ({})", cols.join(", ")), [])?;

        // Column-for-column copy — no data transformation needed here,
        // unlike v8's money conversion, since only the constraint shape
        // is changing. A declared field the old table doesn't actually
        // have yet falls back to its own default (or NULL), same
        // reasoning and same fallback v8 already uses, rather than
        // referencing a column that doesn't exist and failing the whole
        // rebuild over unrelated schema drift.
        let mut select_cols = vec!["t.id".to_string(), "t.business_id".to_string()];
        for f in &on_disk_def.fields {
            if existing_cols.contains(&f.name) {
                select_cols.push(format!("t.{}", f.name));
            } else {
                let default_sql = match &f.default {
                    Some(serde_json::Value::String(s)) => format!("'{}'", s.replace('\'', "''")),
                    Some(serde_json::Value::Number(n)) => n.to_string(),
                    Some(serde_json::Value::Bool(b)) => if *b { "1".into() } else { "0".into() },
                    _ => "NULL".into(),
                };
                select_cols.push(default_sql);
            }
        }
        select_cols.push("t.created_at".to_string());
        select_cols.push("t.updated_at".to_string());
        select_cols.push("t.deleted_at".to_string());

        tx.execute(
            &format!(
                "INSERT INTO {table} SELECT {} FROM {old_table} t",
                select_cols.join(", ")
            ),
            [],
        )?;

        tx.execute(&format!("DROP TABLE {old_table}"), [])?;
        tx.execute(
            &format!("CREATE INDEX IF NOT EXISTS idx_{table}_business ON {table}(business_id, deleted_at)"),
            [],
        )?;
    }

    tx.execute("INSERT INTO _schema_version (version) VALUES (13)", [])?;
    tx.commit()?;
    Ok(())
}
