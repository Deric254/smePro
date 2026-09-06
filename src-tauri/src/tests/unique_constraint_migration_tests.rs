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
    let (_biz_a, biz_b) = seed_two_businesses_with_globally_unique_sku(&mut conn);
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

#[test]
fn test_inventory_unique_name_is_enforced_case_and_whitespace_insensitively() {
    // THE ACTUAL FIX Deric asked for: two DIFFERENT skus must not be
    // able to share the same item name — case and whitespace
    // differences don't count as "different" (see
    // module.rs::create_table's own doc comment on why). This exercises
    // the constraint as a brand-new business would actually get it —
    // straight from create_table() at module-enable time, since
    // test_business() enables Inventory fresh — which is the same
    // end state v18 backfills onto an already-existing table; see
    // test_v18_migration_backfills_the_index_for_a_pre_existing_table
    // below for the migration path specifically.
    let mut conn = test_db();
    let biz = test_business(&mut conn);

    conn.execute(
        "INSERT INTO module_inventory (id, business_id, sku, name, quantity, unit_cost, unit_price, created_at, updated_at)
         VALUES (?1, ?2, 'RICE-001', 'Rice', 10, 500, 900, datetime('now'), datetime('now'))",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), biz],
    )
    .unwrap();

    let collision = conn.execute(
        "INSERT INTO module_inventory (id, business_id, sku, name, quantity, unit_cost, unit_price, created_at, updated_at)
         VALUES (?1, ?2, 'RICE-002', '  rice  ', 5, 400, 800, datetime('now'), datetime('now'))",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), biz],
    );
    assert!(collision.is_err(), "a different SKU with the same name (different case/spacing) must still be rejected");

    // A genuinely different name, for the same business, must still
    // work — the fix must not have blocked inventory creation outright.
    conn.execute(
        "INSERT INTO module_inventory (id, business_id, sku, name, quantity, unit_cost, unit_price, created_at, updated_at)
         VALUES (?1, ?2, 'BEANS-001', 'Beans', 5, 400, 800, datetime('now'), datetime('now'))",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), biz],
    )
    .expect("a genuinely different name must still be allowed");

    // A DIFFERENT business must still be able to use the exact same
    // name — this is business-scoped, not global, same as sku.
    let other_biz = test_business(&mut conn);
    conn.execute(
        "INSERT INTO module_inventory (id, business_id, sku, name, quantity, unit_cost, unit_price, created_at, updated_at)
         VALUES (?1, ?2, 'RICE-001', 'Rice', 10, 500, 900, datetime('now'), datetime('now'))",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), other_biz],
    )
    .expect("an unrelated business must be able to use the same item name");
}

#[test]
fn test_v18_migration_backfills_the_index_for_a_pre_existing_table() {
    // Unlike the test above (a brand-new business, which gets the
    // index straight from create_table()'s own already-current logic),
    // this reproduces an install whose Inventory table was created
    // BEFORE this feature existed — the only case v18 itself actually
    // needs to do anything for.
    let mut conn = test_db();
    let biz = test_business(&mut conn);

    // Simulate the pre-fix table shape: the index this migration adds
    // simply isn't there yet, same as any table created before this
    // code changed.
    conn.execute("DROP INDEX IF EXISTS idx_module_inventory_unique_name", []).unwrap();

    conn.execute(
        "INSERT INTO module_inventory (id, business_id, sku, name, quantity, unit_cost, unit_price, created_at, updated_at)
         VALUES (?1, ?2, 'RICE-001', 'Rice', 10, 500, 900, datetime('now'), datetime('now'))",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), biz],
    )
    .unwrap();
    // Confirm the index is really gone before the migration runs —
    // otherwise this test would trivially pass for the wrong reason.
    conn.execute(
        "INSERT INTO module_inventory (id, business_id, sku, name, quantity, unit_cost, unit_price, created_at, updated_at)
         VALUES (?1, ?2, 'RICE-002', 'rice', 5, 400, 800, datetime('now'), datetime('now'))",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), biz],
    )
    .expect("with the index dropped, a colliding name must be insertable — proves the drop above really worked");
    // ...and remove that duplicate again — the migration below detects
    // EXISTING collisions and skips rather than failing, so this test
    // needs a genuinely clean table to prove the ADD path specifically,
    // not the skip path (that's the test right after this one).
    conn.execute("DELETE FROM module_inventory WHERE sku = 'RICE-002'", []).unwrap();

    conn.execute("DELETE FROM _schema_version WHERE version >= 18", []).unwrap();
    crate::db_migrations::run(&mut conn).expect("v18 migration");

    let collision = conn.execute(
        "INSERT INTO module_inventory (id, business_id, sku, name, quantity, unit_cost, unit_price, created_at, updated_at)
         VALUES (?1, ?2, 'RICE-003', 'RICE', 5, 400, 800, datetime('now'), datetime('now'))",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), biz],
    );
    assert!(collision.is_err(), "v18 must have backfilled real enforcement onto the pre-existing table");
}

#[test]
fn test_v18_migration_lets_a_soft_deleted_name_be_reused() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);

    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO module_inventory (id, business_id, sku, name, quantity, unit_cost, unit_price, created_at, updated_at)
         VALUES (?1, ?2, 'OLD-001', 'Discontinued Item', 0, 100, 200, datetime('now'), datetime('now'))",
        rusqlite::params![id, biz],
    )
    .unwrap();
    conn.execute(
        "UPDATE module_inventory SET deleted_at = datetime('now') WHERE id = ?1",
        rusqlite::params![id],
    )
    .unwrap();

    // A soft-deleted item's name must not permanently squat on that
    // name — every other query against this table already treats a
    // soft-deleted row as gone, and this index holds itself to the
    // same standard (see its own `WHERE deleted_at IS NULL`).
    conn.execute(
        "INSERT INTO module_inventory (id, business_id, sku, name, quantity, unit_cost, unit_price, created_at, updated_at)
         VALUES (?1, ?2, 'NEW-001', 'Discontinued Item', 10, 100, 200, datetime('now'), datetime('now'))",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), biz],
    )
    .expect("a soft-deleted item's name must be reusable by a new item");
}

#[test]
fn test_v18_migration_skips_gracefully_when_a_collision_already_exists() {
    // THE REAL CONSTRAINT this migration has to respect: every business
    // shares the same physical module_inventory table, so the unique
    // index either applies to everyone or (safely) to no one — it
    // can never single out just the business with the pre-existing
    // duplicate. Reproduces exactly that: a business that already has
    // two different SKUs sharing a name BEFORE v18 gets a chance to
    // run, proving the migration detects it and skips instead of
    // failing the whole migration run (which would otherwise refuse to
    // start the app for every business on the install, not just this
    // one).
    let mut conn = test_db();
    let biz = test_business(&mut conn);

    conn.execute(
        "INSERT INTO module_inventory (id, business_id, sku, name, quantity, unit_cost, unit_price, created_at, updated_at)
         VALUES (?1, ?2, 'DUP-001', 'Duplicate Name', 10, 500, 900, datetime('now'), datetime('now'))",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), biz],
    )
    .unwrap();
    // Force this in directly, bypassing whatever index already exists
    // from the normal migration run inside test_db() — reproducing
    // the pre-v18 state where this was still possible.
    conn.execute("DROP INDEX IF EXISTS idx_module_inventory_unique_name", []).unwrap();
    conn.execute(
        "INSERT INTO module_inventory (id, business_id, sku, name, quantity, unit_cost, unit_price, created_at, updated_at)
         VALUES (?1, ?2, 'DUP-002', 'duplicate name', 5, 400, 800, datetime('now'), datetime('now'))",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), biz],
    )
    .expect("the collision must be reproducible with the index gone, or this test proves nothing");

    conn.execute("DELETE FROM _schema_version WHERE version >= 18", []).unwrap();
    crate::db_migrations::run(&mut conn).expect("v18 must not fail the whole migration run just because one business has a collision");

    // The index must genuinely not have been created — a third,
    // unrelated duplicate must still be able to sneak in, proving the
    // skip was real and not just this test's assertion being wrong.
    conn.execute(
        "INSERT INTO module_inventory (id, business_id, sku, name, quantity, unit_cost, unit_price, created_at, updated_at)
         VALUES (?1, ?2, 'DUP-003', 'DUPLICATE NAME', 3, 300, 700, datetime('now'), datetime('now'))",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), biz],
    )
    .expect("the unique index must genuinely be absent while a collision exists, not silently still enforcing");
}

fn has_index(conn: &rusqlite::Connection, index_name: &str) -> bool {
    conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='index' AND name=?1",
        rusqlite::params![index_name],
        |r| r.get::<_, i64>(0),
    )
    .unwrap()
        > 0
}

#[test]
fn test_refunds_table_gets_a_sale_id_index_on_a_fresh_business() {
    // THE ACTUAL FIX Deric asked for: performance. A fresh business
    // enabling Refunds goes through create_table() directly, which
    // should already build this index — see module.rs::create_table's
    // own comment on why refund.rs needs it (two SUM(...) WHERE
    // sale_id = ?1 lookups per refund, previously with nothing to
    // index on).
    let mut conn = test_db();
    let _biz = test_business(&mut conn);
    assert!(has_index(&conn, "idx_module_refunds_sale_id"), "a fresh Refunds table must already have the sale_id index from create_table()");
}

#[test]
fn test_v19_migration_backfills_the_refunds_sale_id_index() {
    // Unlike the test above (a brand-new business, which gets the
    // index straight from create_table()'s own already-current logic),
    // this reproduces an install whose Refunds table was created
    // BEFORE this fix existed — the only case v19 itself actually
    // needs to do anything for. Unlike v18's inventory-name index,
    // there's no "existing collision" case to worry about here (this
    // is a plain, not UNIQUE, index) — it should just always get added.
    let mut conn = test_db();
    let _biz = test_business(&mut conn);

    conn.execute("DROP INDEX IF EXISTS idx_module_refunds_sale_id", []).unwrap();
    assert!(!has_index(&conn, "idx_module_refunds_sale_id"), "sanity check — the index must really be gone before the migration runs");

    conn.execute("DELETE FROM _schema_version WHERE version >= 19", []).unwrap();
    crate::db_migrations::run(&mut conn).expect("v19 migration");

    assert!(has_index(&conn, "idx_module_refunds_sale_id"), "v19 must have backfilled the index onto the pre-existing table");
}
