use super::common::*;

/// Simulates an existing, already-fully-migrated (post-v8) install
/// that still carries the bug v13 exists to fix: inventory's `sku`
/// field is declared `unique: true`, and the physical table was built
/// back when that meant a bare, GLOBAL column-level `UNIQUE(sku)` —
/// wrong, because module_inventory is one single table shared by every
/// business in the install. Hand-built deliberately, bypassing
/// module::ModuleDef::create_table (which would use the current, already
/// -fixed logic) — the whole point is to reproduce exactly what a real
/// pre-fix database looks like the moment before this code opens it.
fn seed_two_businesses_with_globally_unique_sku(conn: &mut rusqlite::Connection) -> (String, String) {
    let biz_a = crate::business_panel::create_business(conn, "Biz A", "USD", "UTC").expect("create business A");
    let biz_b = crate::business_panel::create_business(conn, "Biz B", "USD", "UTC").expect("create business B");

    let schema = serde_json::json!({
        "id": "inventory",
        "display_name": "Inventory",
        "fields": [
            { "name": "sku", "type": "text", "required": true, "unique": true },
            { "name": "name", "type": "text", "required": true },
            { "name": "category", "type": "text", "required": false },
            { "name": "quantity", "type": "integer", "required": true, "default": 0 },
            { "name": "unit", "type": "unit", "required": false },
            { "name": "unit_cost", "type": "money", "required": true, "default": 0 },
            { "name": "unit_price", "type": "money", "required": true, "default": 0 },
            { "name": "currency", "type": "currency", "required": false },
            { "name": "reorder_level", "type": "integer", "required": false, "default": 5 },
            { "name": "expiry_date", "type": "date", "required": false }
        ],
        "actions": ["create", "read", "update", "delete", "export", "sell", "receive", "repack"],
        "default_roles": { "Owner": ["create", "read", "update", "delete", "export"] }
    })
    .to_string();

    for biz in [&biz_a, &biz_b] {
        conn.execute(
            "INSERT INTO modules (id, business_id, display_name, schema_json, enabled, table_created, created_at)
             VALUES ('inventory', ?1, 'Inventory', ?2, 1, 1, datetime('now'))",
            rusqlite::params![biz, schema],
        )
        .unwrap();
    }

    // The old, buggy shape: a bare column-level UNIQUE on `sku` alone,
    // with no business_id in the constraint at all — exactly what
    // module::ModuleDef::field_column_defs() used to emit.
    conn.execute_batch(
        "CREATE TABLE module_inventory (
            id TEXT PRIMARY KEY, business_id TEXT NOT NULL,
            sku TEXT NOT NULL UNIQUE, name TEXT NOT NULL, category TEXT,
            quantity INTEGER NOT NULL, unit TEXT,
            unit_cost INTEGER NOT NULL, unit_price INTEGER NOT NULL,
            currency TEXT, reorder_level INTEGER, expiry_date TEXT,
            created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT
        )",
    )
    .unwrap();

    // Business A already owns "SHARED-001" under the old (buggy)
    // constraint — this is only possible at all because the table
    // isn't yet in the state this test will assert against.
    conn.execute(
        "INSERT INTO module_inventory (id, business_id, sku, name, quantity, unit_cost, unit_price, created_at, updated_at)
         VALUES (?1, ?2, 'SHARED-001', 'Business A Item', 10, 500, 900, datetime('now'), datetime('now'))",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), biz_a],
    )
    .unwrap();
    // Business B has its own, differently-named item — this row's
    // presence is what proves the rebuild doesn't lose or corrupt
    // pre-existing data for a business that never touched the
    // colliding SKU at all.
    conn.execute(
        "INSERT INTO module_inventory (id, business_id, sku, name, quantity, unit_cost, unit_price, created_at, updated_at)
         VALUES (?1, ?2, 'B-ONLY-001', 'Business B Item', 20, 300, 600, datetime('now'), datetime('now'))",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), biz_b],
    )
    .unwrap();

    (biz_a, biz_b)
}

#[test]
fn test_v13_migration_scopes_unique_sku_per_business() {
    let mut conn = test_db(); // already fully migrated to CURRENT_VERSION on a clean schema
    let (biz_a, biz_b) = seed_two_businesses_with_globally_unique_sku(&mut conn);

    // Confirm the bug is really there before the fix runs — otherwise
    // this test would trivially pass for the wrong reason.
    let pre_fix_result = conn.execute(
        "INSERT INTO module_inventory (id, business_id, sku, name, quantity, unit_cost, unit_price, created_at, updated_at)
         VALUES (?1, ?2, 'SHARED-001', 'Business B Attempt', 1, 100, 200, datetime('now'), datetime('now'))",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), biz_b],
    );
    assert!(pre_fix_result.is_err(), "the legacy fixture must reproduce the bug (global UNIQUE) before the migration runs");

    // Same "delete every version >= N" reasoning as the v8 migration's
    // own tests: `current` is MAX(version), so only removing row 13
    // (and everything after it) actually forces v13 to run for real.
    conn.execute("DELETE FROM _schema_version WHERE version >= 13", []).unwrap();
    crate::db_migrations::run(&mut conn).expect("v13 migration");

    // THE FIX: business B can now use the exact same SKU business A
    // already has — they're unrelated businesses, this was always
    // supposed to be allowed.
    conn.execute(
        "INSERT INTO module_inventory (id, business_id, sku, name, quantity, unit_cost, unit_price, created_at, updated_at)
         VALUES (?1, ?2, 'SHARED-001', 'Business B Item, Same SKU', 5, 100, 200, datetime('now'), datetime('now'))",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), biz_b],
    )
    .expect("two unrelated businesses must be able to share a SKU after the fix");

    // STILL ENFORCED: business A cannot register a second item under
    // the SKU it already owns — the fix must not have simply removed
    // uniqueness altogether.
    let still_enforced = conn.execute(
        "INSERT INTO module_inventory (id, business_id, sku, name, quantity, unit_cost, unit_price, created_at, updated_at)
         VALUES (?1, ?2, 'SHARED-001', 'Business A Duplicate', 1, 100, 200, datetime('now'), datetime('now'))",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), biz_a],
    );
    assert!(still_enforced.is_err(), "a business must still be blocked from reusing its own SKU");

    // Pre-existing rows for both businesses must have survived the
    // rebuild untouched, and neither of the two rejected inserts above
    // (the pre-fix duplicate attempt on B, the still-enforced duplicate
    // attempt on A) should have added anything.
    let a_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM module_inventory WHERE business_id = ?1", [&biz_a], |r| r.get(0))
        .unwrap();
    let b_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM module_inventory WHERE business_id = ?1", [&biz_b], |r| r.get(0))
        .unwrap();
    let b_original_name: String = conn
        .query_row(
            "SELECT name FROM module_inventory WHERE business_id = ?1 AND sku = 'B-ONLY-001'",
            [&biz_b],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(a_count, 1); // just the original SHARED-001 row — the later duplicate attempt on A was correctly rejected
    assert_eq!(b_count, 2); // original B-ONLY-001 + the new, now-allowed SHARED-001 row
    assert_eq!(b_original_name, "Business B Item");
}

#[test]
fn test_v13_migration_is_idempotent_and_leaves_already_fixed_tables_alone() {
    let mut conn = test_db();
    let (biz_a, biz_b) = seed_two_businesses_with_globally_unique_sku(&mut conn);
    conn.execute("DELETE FROM _schema_version WHERE version >= 13", []).unwrap();

    crate::db_migrations::run(&mut conn).expect("first run applies v13");
    // Running again must be a pure no-op: the version gate means v13
    // never re-executes, and even if it somehow did, the detection
    // check must correctly see the table is already fixed and skip it
    // rather than attempting a second, unnecessary rebuild.
    crate::db_migrations::run(&mut conn).expect("second run must no-op cleanly");

    conn.execute(
        "INSERT INTO module_inventory (id, business_id, sku, name, quantity, unit_cost, unit_price, created_at, updated_at)
         VALUES (?1, ?2, 'SHARED-001', 'Still works after second run', 1, 100, 200, datetime('now'), datetime('now'))",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), biz_b],
    )
    .expect("fix must still hold after running migrations twice");
}
