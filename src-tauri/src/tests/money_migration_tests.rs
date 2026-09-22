use super::common::*;

/// Simulates an existing pre-migration install: a business whose
/// "inventory" module was enabled back when unit_cost/unit_price were
/// "real" (float dollars), with real data sitting in the table under
/// that old schema. This is hand-built deliberately, bypassing
/// business_panel::enable_module (which would use the CURRENT,
/// already-money-aware on-disk JSON) — the whole point is to recreate
/// exactly what an old database looks like the moment before it's
/// opened by this new code.
fn seed_legacy_inventory_business(conn: &mut rusqlite::Connection) -> (String, String) {
    let business_id = crate::business_panel::create_business(conn, "Legacy Biz", "USD", "UTC")
        .expect("create business");

    let legacy_schema = serde_json::json!({
        "id": "inventory",
        "display_name": "Inventory",
        "fields": [
            { "name": "sku", "type": "text", "required": true, "unique": true },
            { "name": "name", "type": "text", "required": true },
            { "name": "category", "type": "text", "required": false },
            { "name": "quantity", "type": "integer", "required": true, "default": 0 },
            { "name": "unit", "type": "unit", "required": false },
            { "name": "unit_cost", "type": "real", "required": true, "default": 0.0 },
            { "name": "unit_price", "type": "real", "required": true, "default": 0.0 },
            { "name": "currency", "type": "currency", "required": false },
            { "name": "reorder_level", "type": "integer", "required": false, "default": 5 },
            { "name": "expiry_date", "type": "date", "required": false }
        ],
        "actions": ["create", "read", "update", "delete", "export", "sell", "receive", "repack"],
        "default_roles": { "Owner": ["create", "read", "update", "delete", "export"] }
    })
    .to_string();

    conn.execute(
        "INSERT INTO modules (id, business_id, display_name, schema_json, enabled, table_created, created_at)
         VALUES ('inventory', ?1, 'Inventory', ?2, 1, 1, datetime('now'))",
        rusqlite::params![business_id, legacy_schema],
    )
    .unwrap();

    conn.execute_batch(
        "CREATE TABLE module_inventory (
            id TEXT PRIMARY KEY, business_id TEXT NOT NULL,
            sku TEXT UNIQUE, name TEXT, category TEXT, quantity INTEGER, unit TEXT,
            unit_cost REAL, unit_price REAL, currency TEXT, reorder_level INTEGER, expiry_date TEXT,
            created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT
        )",
    )
    .unwrap();

    let item_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO module_inventory (id, business_id, sku, name, quantity, unit_cost, unit_price, created_at, updated_at)
         VALUES (?1, ?2, 'RICE-001', 'Rice', 100, 19.99, 29.99, datetime('now'), datetime('now'))",
        rusqlite::params![item_id, business_id],
    )
    .unwrap();

    (business_id, item_id)
}

#[test]
fn test_v8_migration_converts_legacy_float_dollars_to_integer_cents() {
    let mut conn = test_db(); // already fully migrated to CURRENT_VERSION on a clean schema
    let (business_id, item_id) = seed_legacy_inventory_business(&mut conn);

    // Roll back the version marker to simulate "this is what the file
    // looked like the moment before the v8 migration ran" — the
    // legacy data above was seeded AFTER test_db() already applied v8
    // to nothing, so this forces run() to apply v8 for real, now that
    // real legacy data actually exists to convert.
    //
    // Deletes version 8 AND EVERYTHING AFTER IT, not just row 8 alone
    // — `current` (see db_migrations::run) is computed as MAX(version)
    // in this table, not "is row 8 specifically present." Once later
    // migrations (v9, v10, ...) exist, leaving their rows behind while
    // only deleting row 8 leaves MAX(version) unchanged at whatever
    // the newest migration is — current stays >= 8, `if current < 8`
    // never fires, and v8 silently never re-runs at all. This exact
    // gap broke this test for real the moment migrations were added
    // past v8 in the same session that first wrote it — confirmed via
    // a real GitHub Actions run, not just found by reading the code.
    conn.execute("DELETE FROM _schema_version WHERE version >= 8", []).unwrap();

    crate::db_migrations::run(&mut conn).expect("v8 migration");

    let (unit_cost, unit_price): (i64, i64) = conn
        .query_row(
            "SELECT unit_cost, unit_price FROM module_inventory WHERE id = ?1",
            [&item_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();

    // $19.99 -> 1999 cents, $29.99 -> 2999 cents. Exact, not
    // approximately-equal — this is precisely the guarantee the whole
    // migration exists to provide.
    assert_eq!(unit_cost, 1999);
    assert_eq!(unit_price, 2999);

    // The business's frozen schema snapshot must now declare these
    // fields as "money", not "real" — this is what makes
    // module.validate() reject any future float written here,
    // independent of the SQL column's own affinity.
    let schema_json: String = conn
        .query_row(
            "SELECT schema_json FROM modules WHERE business_id = ?1 AND id = 'inventory'",
            [&business_id],
            |r| r.get(0),
        )
        .unwrap();
    let refreshed = crate::module::ModuleDef::from_json_str(&schema_json).unwrap();
    let unit_price_field = refreshed.fields.iter().find(|f| f.name == "unit_price").unwrap();
    assert_eq!(unit_price_field.field_type, "money");

    // And going forward, a float written to this now-"money" field
    // through the normal validated path must be rejected outright —
    // not coerced, not silently truncated.
    let mut bad_record = std::collections::HashMap::new();
    bad_record.insert("sku".to_string(), serde_json::json!("BAD-001"));
    bad_record.insert("name".to_string(), serde_json::json!("Bad Item"));
    bad_record.insert("quantity".to_string(), serde_json::json!(1));
    bad_record.insert("unit_cost".to_string(), serde_json::json!(1000));
    bad_record.insert("unit_price".to_string(), serde_json::json!(19.99)); // float — must be rejected
    assert!(refreshed.validate(&bad_record).is_err());
}

#[test]
fn test_v8_migration_is_idempotent() {
    let mut conn = test_db();
    let (_, item_id) = seed_legacy_inventory_business(&mut conn);
    // See the matching comment on the test above — must delete every
    // version >= 8, not just row 8 alone, or `current` (MAX(version))
    // never actually drops below 8 once later migrations exist.
    conn.execute("DELETE FROM _schema_version WHERE version >= 8", []).unwrap();

    crate::db_migrations::run(&mut conn).expect("first run");
    let after_first: i64 = conn
        .query_row("SELECT unit_cost FROM module_inventory WHERE id = ?1", [&item_id], |r| r.get(0))
        .unwrap();

    // Running again must be a pure no-op — the version gate means v8
    // never re-executes, so a value that was already correctly
    // converted must not get multiplied by 100 a second time.
    crate::db_migrations::run(&mut conn).expect("second run, must no-op");
    let after_second: i64 = conn
        .query_row("SELECT unit_cost FROM module_inventory WHERE id = ?1", [&item_id], |r| r.get(0))
        .unwrap();

    assert_eq!(after_first, 1999);
    assert_eq!(after_first, after_second);
}

#[test]
fn test_v2_v4_v7_migrations_are_idempotent_when_rerun() {
    // Regression test for a real bug this session found and fixed: v2,
    // v4 and v7 used a bare `ALTER TABLE ADD COLUMN` with no existence
    // check — exactly the class of bug that made v34 fail this same
    // kind of rollback-and-rerun test with "duplicate column name"
    // before it was fixed. These three are lower-traffic tables
    // (businesses, users, sessions) so nothing had exercised the
    // rerun path for them until now.
    //
    // v4's own totp_secret/totp_enabled columns are no longer a good
    // fixture for this test: v38 (2FA removal) drops them again later
    // in this same full rerun, on purpose — asserting their presence
    // here would fail for a reason that has nothing to do with what
    // this test actually checks (ADD COLUMN idempotency). `slogan` and
    // `last_activity` still cover v2 and v7 the same way; v4 is
    // covered instead by v38's own rerun-safety, asserted separately
    // in `test_v38_is_idempotent_when_rerun` below.
    let mut conn = test_db();
    conn.execute("DELETE FROM _schema_version WHERE version >= 2", []).unwrap();
    crate::db_migrations::run(&mut conn).expect("first rerun from v2 must succeed");
    crate::db_migrations::run(&mut conn).expect("second rerun must no-op, not fail with 'duplicate column name'");

    for (table, column) in [("businesses", "slogan"), ("sessions", "last_activity")] {
        let count: i64 = conn.query_row(
            &format!("SELECT count(*) FROM pragma_table_info('{table}') WHERE name='{column}'"),
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(count, 1, "{table}.{column} must exist exactly once, not zero and not duplicated");
    }
}

/// v38 removes tax_rates, the two totp_* columns, and the HR module
/// registry row — but the HR module's own DATA TABLE must survive if
/// it holds any real rows (see v38's own doc comment: disabling/
/// removing a module never destroys a business's existing data). This
/// hand-builds a pre-v38 database with a real `module_hr` row present,
/// the same way `seed_legacy_inventory_business` above hand-builds a
/// pre-money-migration inventory table — `module_json("hr")` no longer
/// exists to go through the normal `enable_module` path, since hr.json
/// itself is gone.
#[test]
fn test_v38_keeps_hr_data_but_removes_hr_from_the_registry() {
    let mut conn = test_db();
    let biz = crate::business_panel::create_business(&mut conn, "HR Test Biz", "USD", "UTC")
        .expect("create business");

    conn.execute_batch(&format!(
        "CREATE TABLE module_hr (
            id TEXT PRIMARY KEY, business_id TEXT NOT NULL, full_name TEXT NOT NULL,
            salary INTEGER, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
            created_by TEXT, deleted_at TEXT
         );
         INSERT INTO module_hr (id, business_id, full_name, salary, created_at, updated_at)
         VALUES ('row1', '{biz}', 'A Real Employee', 250000, datetime('now'), datetime('now'));
         INSERT INTO modules (id, business_id, display_name, schema_json, enabled, table_created, created_at)
         VALUES ('hr', '{biz}', 'HR / Staff', '{{}}', 1, 1, datetime('now'));"
    )).expect("hand-seed pre-v38 hr module + real row");

    // Roll `_schema_version` back below 38 so `run()` (which gates
    // purely on that table's MAX(version), not on what the seeded
    // rows above look like) actually re-executes v38 against this
    // hand-built "pre-v38" state — the same fix the v8 test above
    // needed for the identical reason, spelled out in its comment.
    conn.execute("DELETE FROM _schema_version WHERE version >= 38", []).unwrap();

    crate::db_migrations::run(&mut conn).expect("v38 must succeed with real hr data present");

    // Data is untouched.
    let salary: i64 = conn.query_row("SELECT salary FROM module_hr WHERE id = 'row1'", [], |r| r.get(0)).unwrap();
    assert_eq!(salary, 250000, "a business's real HR data must never be silently deleted");

    // But it's gone from the registry, which is what actually hides it
    // from the UI (see api.ts's /modules).
    let registry_count: i64 = conn.query_row(
        "SELECT count(*) FROM modules WHERE business_id = ?1 AND id = 'hr'",
        rusqlite::params![biz], |r| r.get(0),
    ).unwrap();
    assert_eq!(registry_count, 0);
}

/// The empty-table counterpart to the test above: a business that
/// enabled HR but never actually entered a record should have the
/// dead table cleaned up entirely, not left behind as orphaned schema
/// — the same "don't leave dead code or mess" the notifications-table
/// drop (v32) already established for a fully-removed feature with no
/// real data to protect.
#[test]
fn test_v38_drops_module_hr_table_when_empty() {
    let mut conn = test_db();
    let biz = crate::business_panel::create_business(&mut conn, "Empty HR Biz", "USD", "UTC")
        .expect("create business");

    conn.execute_batch(&format!(
        "CREATE TABLE module_hr (
            id TEXT PRIMARY KEY, business_id TEXT NOT NULL, full_name TEXT NOT NULL,
            salary INTEGER, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
            created_by TEXT, deleted_at TEXT
         );
         INSERT INTO modules (id, business_id, display_name, schema_json, enabled, table_created, created_at)
         VALUES ('hr', '{biz}', 'HR / Staff', '{{}}', 1, 1, datetime('now'));"
    )).expect("hand-seed pre-v38 hr module, no rows");

    // Same schema-version rollback the test above needs, for the
    // same reason — see that test's comment.
    conn.execute("DELETE FROM _schema_version WHERE version >= 38", []).unwrap();

    crate::db_migrations::run(&mut conn).expect("v38 must succeed with an empty hr table present");

    let table_exists: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='module_hr'", [], |r| r.get(0),
    ).unwrap();
    assert_eq!(table_exists, 0, "an empty module_hr table should be dropped, not left as dead schema");
}

/// v38 is a rerun-safety test in the same spirit as the v2/v4/v7 test
/// above: running the DROP COLUMN / DROP TABLE statements a second
/// time against a database that already has them gone must no-op, not
/// error — the same guard-before-mutate shape this whole file uses.
#[test]
fn test_v38_is_idempotent_when_rerun() {
    let mut conn = test_db(); // already fully migrated, including v38
    crate::db_migrations::run(&mut conn).expect("rerunning v38 against an already-migrated db must no-op");
}
