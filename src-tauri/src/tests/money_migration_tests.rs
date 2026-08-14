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
