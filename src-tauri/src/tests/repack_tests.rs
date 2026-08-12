use super::common::*;
use serde_json::json;

fn make_inventory_item(conn: &mut rusqlite::Connection, biz: &str, uid: &str, sku: &str, name: &str, qty: i64, cost_cents: i64, price_cents: i64) -> String {
    let mut item = serde_json::Map::new();
    item.insert("sku".into(), json!(sku));
    item.insert("name".into(), json!(name));
    item.insert("quantity".into(), json!(qty));
    item.insert("unit_cost".into(), json!(cost_cents));
    item.insert("unit_price".into(), json!(price_cents));
    crate::crud::create(conn, biz, uid, "inventory", &item).unwrap()
}

fn get_item(conn: &rusqlite::Connection, biz: &str, uid: &str, id: &str) -> serde_json::Value {
    let list = crate::crud::list(conn, biz, uid, "inventory", None, 50, 0).unwrap();
    list.into_iter().find(|r| r["id"] == json!(id)).unwrap()
}

#[test]
fn test_repack_a_dozen_eggs_into_singles_produces_the_exact_correct_cost() {
    // The exact real-world scenario this fix exists for: a dozen eggs
    // bought for 300 (cents — so $3.00, but the currency doesn't
    // matter here, only the arithmetic does), broken into 12 single
    // eggs. Each single egg must correctly cost 25, not whatever the
    // single-egg record happened to have before (here: nothing, it's
    // brand new stock).
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let dozen_id = make_inventory_item(&mut conn, &biz, &uid, "EGGS-DOZEN", "Eggs (dozen)", 5, 300, 400);
    let single_id = make_inventory_item(&mut conn, &biz, &uid, "EGGS-SINGLE", "Eggs (single)", 0, 0, 20);

    let req = crate::repack::RepackRequest {
        source_record_id: dozen_id.clone(),
        source_quantity: 1,
        target_record_id: single_id.clone(),
        target_quantity_produced: 12,
        notes: None,
    };
    let result = crate::repack::repack(&mut conn, &biz, &uid, req).unwrap();

    assert_eq!(result["target_unit_cost_after"].as_i64().unwrap(), 25);

    let dozen = get_item(&conn, &biz, &uid, &dozen_id);
    assert_eq!(dozen["quantity"].as_i64().unwrap(), 4, "4 dozens left after breaking 1");
    assert_eq!(dozen["unit_cost"].as_i64().unwrap(), 300, "the dozen record's own cost never changes from a repack");

    let single = get_item(&conn, &biz, &uid, &single_id);
    assert_eq!(single["quantity"].as_i64().unwrap(), 12);
    assert_eq!(single["unit_cost"].as_i64().unwrap(), 25);
    // Selling each at 20 against a cost of 25 would be a real loss —
    // exactly the kind of thing correct cost tracking is supposed to
    // let a shopkeeper actually notice, rather than hide.
    assert!(single["unit_price"].as_i64().unwrap() < single["unit_cost"].as_i64().unwrap());
}

#[test]
fn test_repack_blends_with_existing_target_stock_at_a_different_cost() {
    // The target item already has stock on hand at its own cost —
    // the new cost must be a genuine weighted average of the two,
    // not just overwritten by the incoming repack's cost.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    // 10kg sack costing 1000 cents, broken into loose kg already
    // holding 5kg at 90 cents/kg.
    let sack_id = make_inventory_item(&mut conn, &biz, &uid, "RICE-SACK", "Rice (10kg sack)", 3, 1000, 1500);
    let loose_id = make_inventory_item(&mut conn, &biz, &uid, "RICE-LOOSE", "Rice (loose kg)", 5, 90, 120);

    let req = crate::repack::RepackRequest {
        source_record_id: sack_id,
        source_quantity: 1,
        target_record_id: loose_id.clone(),
        target_quantity_produced: 10,
        notes: None,
    };
    let result = crate::repack::repack(&mut conn, &biz, &uid, req).unwrap();

    // (5*90 + 1*1000) / 15 = (450 + 1000) / 15 = 1450/15 = 96.67 -> 97 rounded
    assert_eq!(result["target_unit_cost_after"].as_i64().unwrap(), 97);
    let loose = get_item(&conn, &biz, &uid, &loose_id);
    assert_eq!(loose["quantity"].as_i64().unwrap(), 15);
    assert_eq!(loose["unit_cost"].as_i64().unwrap(), 97);
}

#[test]
fn test_repack_cannot_consume_more_than_available_stock() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let dozen_id = make_inventory_item(&mut conn, &biz, &uid, "EGGS-D2", "Eggs (dozen)", 2, 300, 400);
    let single_id = make_inventory_item(&mut conn, &biz, &uid, "EGGS-S2", "Eggs (single)", 0, 0, 20);

    let req = crate::repack::RepackRequest {
        source_record_id: dozen_id.clone(),
        source_quantity: 5, // only 2 dozens on hand
        target_record_id: single_id,
        target_quantity_produced: 60,
        notes: None,
    };
    let result = crate::repack::repack(&mut conn, &biz, &uid, req);
    assert!(result.is_err());

    // Nothing should have moved — a rejected repack must not
    // partially apply.
    let dozen = get_item(&conn, &biz, &uid, &dozen_id);
    assert_eq!(dozen["quantity"].as_i64().unwrap(), 2);
}

#[test]
fn test_repack_rejects_same_source_and_target() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let id = make_inventory_item(&mut conn, &biz, &uid, "SELF-001", "Self", 10, 100, 150);

    let req = crate::repack::RepackRequest {
        source_record_id: id.clone(),
        source_quantity: 1,
        target_record_id: id,
        target_quantity_produced: 1,
        notes: None,
    };
    assert!(crate::repack::repack(&mut conn, &biz, &uid, req).is_err());
}
