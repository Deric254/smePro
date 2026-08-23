use super::common::*;
use serde_json::{json, Value};

#[test]
fn test_crud_create_and_list() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let mut record = serde_json::Map::new();
    record.insert("sku".into(), json!("TEST-001"));
    record.insert("name".into(), json!("Test Item"));
    record.insert("quantity".into(), json!(10));
    record.insert("unit_cost".into(), json!(500));
    record.insert("unit_price".into(), json!(1000));

    let _id = crate::crud::create(&conn, &biz, &uid, "inventory", &record)
        .expect("create record");

    let list = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0)
        .expect("list records");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].get("sku").unwrap().as_str().unwrap(), "TEST-001");
}

#[test]
fn test_crud_update_applies_partial_patch() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    // Seeded directly (not via crud::create()) so this test can start
    // with a genuinely nonzero quantity — the point here is proving an
    // untouched field survives a partial patch, which is a weaker
    // check if the "untouched" value were 0 to begin with.
    let id = seed_inventory_item(&conn, &biz, "EDIT-001", "Original Name", 10, 500, 1000);

    // Only correcting the price typo — quantity, name, sku untouched.
    let mut patch = serde_json::Map::new();
    patch.insert("unit_price".into(), json!(1200));
    crate::crud::update(&conn, &biz, &uid, "inventory", &id, &patch, false).unwrap();

    let list = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    let updated = list.iter().find(|r| r["id"] == json!(id)).unwrap();
    assert_eq!(updated["unit_price"].as_i64().unwrap(), 1200);
    assert_eq!(updated["name"].as_str().unwrap(), "Original Name"); // untouched
    assert_eq!(updated["quantity"].as_i64().unwrap(), 10); // untouched
}

#[test]
fn test_crud_update_rejects_float_into_money_field() {
    // Proves the gap is actually closed: update() must apply exactly
    // the same type enforcement as create() for "money" fields — a
    // float dollar value must not be able to sneak past validation
    // just by going through an edit instead of a create.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let mut record = serde_json::Map::new();
    record.insert("sku".into(), json!("EDIT-002"));
    record.insert("name".into(), json!("Item"));
    record.insert("quantity".into(), json!(5));
    record.insert("unit_cost".into(), json!(300));
    record.insert("unit_price".into(), json!(600));
    let id = crate::crud::create(&conn, &biz, &uid, "inventory", &record).unwrap();

    let mut bad_patch = serde_json::Map::new();
    bad_patch.insert("unit_price".into(), json!(19.99)); // float — must be rejected
    let result = crate::crud::update(&conn, &biz, &uid, "inventory", &id, &bad_patch, false);
    assert!(result.is_err());

    // And the original integer value must be untouched by the
    // rejected attempt — not partially applied, not corrupted.
    let list = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    let untouched = list.iter().find(|r| r["id"] == json!(id)).unwrap();
    assert_eq!(untouched["unit_price"].as_i64().unwrap(), 600);
}

#[test]
fn test_crud_update_does_not_require_absent_required_fields() {
    // A partial PATCH must not fail just because it omits a field
    // that's required on create — the record already has a stored
    // value for it, and this update isn't touching that field at all.
    // Uses "reorder_level" (not "quantity") as the touched field here —
    // quantity is deliberately blocked from generic edits, see
    // test_crud_update_rejects_direct_quantity_edit_on_inventory below.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let id = seed_inventory_item(&conn, &biz, "EDIT-003", "Item", 2, 100, 150);

    // "name" and "unit_cost" are both required on inventory, but this
    // patch only touches reorder_level — must succeed.
    let mut patch = serde_json::Map::new();
    patch.insert("reorder_level".into(), json!(3));
    let result = crate::crud::update(&conn, &biz, &uid, "inventory", &id, &patch, false);
    assert!(result.is_ok());
}

#[test]
fn test_crud_create_forces_inventory_quantity_to_zero() {
    // The clarified rule: EVERY inventory item starts at zero stock,
    // full stop — not just "editing quantity later is blocked" but
    // "you never get to set it at creation either." Stock only enters
    // through Purchasing receiving an order. Whatever quantity the
    // caller tries to supply at creation is silently discarded, not
    // rejected — this is normal, expected behavior for this form, not
    // an error condition.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let mut record = serde_json::Map::new();
    record.insert("sku".into(), json!("OPEN-001"));
    record.insert("name".into(), json!("Attempted Opening Stock Item"));
    record.insert("quantity".into(), json!(40)); // must be ignored, not honored
    record.insert("unit_cost".into(), json!(200));
    record.insert("unit_price".into(), json!(350));
    let id = crate::crud::create(&conn, &biz, &uid, "inventory", &record).unwrap();

    let list = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    let created = list.iter().find(|r| r["id"] == json!(id)).unwrap();
    assert_eq!(created["quantity"].as_i64().unwrap(), 0, "quantity must be forced to 0 regardless of what was supplied");
}

#[test]
fn test_crud_create_forces_zero_even_when_quantity_is_omitted() {
    // Same rule, from the other direction: a caller who doesn't mention
    // quantity at all (relying on the field's own JSON-declared
    // default of 0) must get exactly the same outcome as one who tried
    // to override it — zero either way.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let mut record = serde_json::Map::new();
    record.insert("sku".into(), json!("OPEN-002"));
    record.insert("name".into(), json!("No Quantity Supplied"));
    record.insert("unit_cost".into(), json!(200));
    record.insert("unit_price".into(), json!(350));
    let id = crate::crud::create(&conn, &biz, &uid, "inventory", &record).unwrap();

    let list = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    let created = list.iter().find(|r| r["id"] == json!(id)).unwrap();
    assert_eq!(created["quantity"].as_i64().unwrap(), 0);
}

#[test]
fn test_crud_bulk_import_path_still_allows_real_opening_quantity() {
    // insert_validated_record() — what excel_import.rs's bulk-create
    // branch calls directly, bypassing create() entirely — is
    // deliberately NOT forced to zero. A spreadsheet-driven initial
    // catalog load needs to seed each item's real, already-existing
    // stock count; that's a sanctioned bulk migration/stock-take path,
    // not the same single ad-hoc creation this fix targets.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let module = crate::crud::load_module(&conn, &biz, "inventory").unwrap();

    let mut record: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
    record.insert("sku".into(), json!("MIGRATE-001"));
    record.insert("name".into(), json!("Migrated Item"));
    record.insert("quantity".into(), json!(75));
    record.insert("unit_cost".into(), json!(100));
    record.insert("unit_price".into(), json!(180));
    let id = crate::crud::insert_validated_record(&conn, &biz, &module, &record).unwrap();

    let list = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    let created = list.iter().find(|r| r["id"] == json!(id)).unwrap();
    assert_eq!(created["quantity"].as_i64().unwrap(), 75);
}

#[test]
fn test_crud_update_rejects_direct_quantity_edit_on_inventory() {
    // The actual gap being closed: once an inventory item exists, its
    // stock LEVEL must not be changeable through the single-record
    // PATCH endpoint (bulk_import=false) — only through
    // sell/receive/refund/repack, each of which enforces oversell
    // protection and a floor at zero that this generic endpoint does
    // not.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let id = seed_inventory_item(&conn, &biz, "LOCK-001", "Locked Quantity Item", 25, 100, 180);

    // A direct attempt to zero it out — exactly the scenario this
    // fix exists to prevent — must be rejected outright.
    let mut zero_patch = serde_json::Map::new();
    zero_patch.insert("quantity".into(), json!(0));
    let result = crate::crud::update(&conn, &biz, &uid, "inventory", &id, &zero_patch, false);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("cannot be edited directly"));

    // Also rejected even mixed in with an otherwise-legitimate field
    // edit — the whole patch fails, nothing is partially applied.
    let mut mixed_patch = serde_json::Map::new();
    mixed_patch.insert("name".into(), json!("Renamed"));
    mixed_patch.insert("quantity".into(), json!(999));
    let result = crate::crud::update(&conn, &biz, &uid, "inventory", &id, &mixed_patch, false);
    assert!(result.is_err());

    // The stock level must be completely untouched by both rejected
    // attempts — still exactly what create() set it to.
    let list = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    let untouched = list.iter().find(|r| r["id"] == json!(id)).unwrap();
    assert_eq!(untouched["quantity"].as_i64().unwrap(), 25);
    assert_eq!(untouched["name"].as_str().unwrap(), "Locked Quantity Item"); // untouched too, patch was fully rejected
}

#[test]
fn test_crud_update_allows_quantity_edit_when_bulk_import_flag_is_set() {
    // The exemption excel_import.rs relies on for its documented stock
    // take feature: with bulk_import=true, the same quantity edit that
    // test_crud_update_rejects_direct_quantity_edit_on_inventory just
    // proved gets rejected must succeed instead. This is what keeps a
    // legitimate spreadsheet reconciliation working while the single
    // ad-hoc form edit stays blocked.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let id = seed_inventory_item(&conn, &biz, "BULK-001", "Bulk Reconciled Item", 25, 100, 180);

    let mut patch = serde_json::Map::new();
    patch.insert("quantity".into(), json!(18)); // e.g. a stock take found fewer units than expected
    let result = crate::crud::update(&conn, &biz, &uid, "inventory", &id, &patch, true);
    assert!(result.is_ok());

    let list = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    let updated = list.iter().find(|r| r["id"] == json!(id)).unwrap();
    assert_eq!(updated["quantity"].as_i64().unwrap(), 18);
}

#[test]
fn test_crud_update_still_allows_other_inventory_fields() {
    // Confirms the block is scoped to "quantity" alone — every other
    // inventory field (price, cost, reorder level, category, etc.)
    // must remain freely editable through the generic form, same as
    // before this fix.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let id = seed_inventory_item(&conn, &biz, "FREE-001", "Freely Editable Item", 10, 100, 150);

    let mut patch = serde_json::Map::new();
    patch.insert("name".into(), json!("Renamed Item"));
    patch.insert("unit_price".into(), json!(175));
    patch.insert("reorder_level".into(), json!(8));
    let result = crate::crud::update(&conn, &biz, &uid, "inventory", &id, &patch, false);
    assert!(result.is_ok());

    let list = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    let updated = list.iter().find(|r| r["id"] == json!(id)).unwrap();
    assert_eq!(updated["name"].as_str().unwrap(), "Renamed Item");
    assert_eq!(updated["unit_price"].as_i64().unwrap(), 175);
    assert_eq!(updated["reorder_level"].as_i64().unwrap(), 8);
    assert_eq!(updated["quantity"].as_i64().unwrap(), 10); // untouched, wasn't in the patch
}

#[test]
fn test_crud_soft_delete() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let mut record = serde_json::Map::new();
    record.insert("sku".into(), json!("DEL-001"));
    record.insert("name".into(), json!("Delete Me"));
    record.insert("quantity".into(), json!(1));
    record.insert("unit_cost".into(), json!(100));
    record.insert("unit_price".into(), json!(200));

    let id = crate::crud::create(&conn, &biz, &uid, "inventory", &record).unwrap();
    crate::crud::delete(&conn, &biz, &uid, "inventory", &id).unwrap();

    let list = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    assert_eq!(list.len(), 0);

    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM module_inventory WHERE id = ?1",
        [id], |r| r.get(0)
    ).unwrap();
    assert_eq!(count, 1);
}
