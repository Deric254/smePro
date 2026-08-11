use super::common::*;
use serde_json::json;

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

    let _id = crate::crud::create(&mut conn, &biz, &uid, "inventory", &record)
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

    let mut record = serde_json::Map::new();
    record.insert("sku".into(), json!("EDIT-001"));
    record.insert("name".into(), json!("Original Name"));
    record.insert("quantity".into(), json!(10));
    record.insert("unit_cost".into(), json!(500));
    record.insert("unit_price".into(), json!(1000));
    let id = crate::crud::create(&mut conn, &biz, &uid, "inventory", &record).unwrap();

    // Only correcting the price typo — quantity, name, sku untouched.
    let mut patch = serde_json::Map::new();
    patch.insert("unit_price".into(), json!(1200));
    crate::crud::update(&conn, &biz, &uid, "inventory", &id, &patch).unwrap();

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
    let id = crate::crud::create(&mut conn, &biz, &uid, "inventory", &record).unwrap();

    let mut bad_patch = serde_json::Map::new();
    bad_patch.insert("unit_price".into(), json!(19.99)); // float — must be rejected
    let result = crate::crud::update(&conn, &biz, &uid, "inventory", &id, &bad_patch);
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
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let mut record = serde_json::Map::new();
    record.insert("sku".into(), json!("EDIT-003"));
    record.insert("name".into(), json!("Item"));
    record.insert("quantity".into(), json!(2));
    record.insert("unit_cost".into(), json!(100));
    record.insert("unit_price".into(), json!(150));
    let id = crate::crud::create(&mut conn, &biz, &uid, "inventory", &record).unwrap();

    // "name" and "unit_cost" are both required on inventory, but this
    // patch only touches quantity — must succeed.
    let mut patch = serde_json::Map::new();
    patch.insert("quantity".into(), json!(7));
    let result = crate::crud::update(&conn, &biz, &uid, "inventory", &id, &patch);
    assert!(result.is_ok());
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

    let id = crate::crud::create(&mut conn, &biz, &uid, "inventory", &record).unwrap();
    crate::crud::delete(&mut conn, &biz, &uid, "inventory", &id).unwrap();

    let list = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    assert_eq!(list.len(), 0);

    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM module_inventory WHERE id = ?1",
        [id], |r| r.get(0)
    ).unwrap();
    assert_eq!(count, 1);
}
