use super::common::*;
use serde_json::json;

fn make_inventory_item(conn: &rusqlite::Connection, biz: &str, sku: &str, name: &str, qty: i64, cost_cents: i64, price_cents: i64) -> String {
    // Delegates to seed_inventory_item() — crud::create() now forces
    // inventory to start at zero stock, so a test needing pre-existing
    // stock to repack against has to seed it like a real bulk catalog
    // migration would.
    seed_inventory_item(conn, biz, sku, name, qty, cost_cents, price_cents)
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

    let dozen_id = make_inventory_item(&conn, &biz, "EGGS-DOZEN", "Eggs (dozen)", 5, 300, 400);
    let single_id = make_inventory_item(&conn, &biz, "EGGS-SINGLE", "Eggs (single)", 0, 0, 35);

    let req = crate::repack::RepackRequest {
        source_record_id: dozen_id.clone(),
        source_quantity: 1,
        target_record_id: Some(single_id.clone()),
        target_quantity_produced: 12,
        new_target_name: None,
        new_target_unit_price: None,
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
    assert_eq!(single["unit_price"].as_i64().unwrap(), 35, "price untouched by repack — it was already comfortably above the real cost");
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
    let sack_id = make_inventory_item(&conn, &biz, "RICE-SACK", "Rice (10kg sack)", 3, 1000, 1500);
    let loose_id = make_inventory_item(&conn, &biz, "RICE-LOOSE", "Rice (loose kg)", 5, 90, 120);

    let req = crate::repack::RepackRequest {
        source_record_id: sack_id,
        source_quantity: 1,
        target_record_id: Some(loose_id.clone()),
        target_quantity_produced: 10,
        new_target_name: None,
        new_target_unit_price: None,
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

    let dozen_id = make_inventory_item(&conn, &biz, "EGGS-D2", "Eggs (dozen)", 2, 300, 400);
    let single_id = make_inventory_item(&conn, &biz, "EGGS-S2", "Eggs (single)", 0, 0, 20);

    let req = crate::repack::RepackRequest {
        source_record_id: dozen_id.clone(),
        source_quantity: 5, // only 2 dozens on hand
        target_record_id: Some(single_id),
        target_quantity_produced: 60,
        new_target_name: None,
        new_target_unit_price: None,
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
fn test_repack_rejects_a_result_that_would_price_the_target_below_cost() {
    // THE ACTUAL FIX Deric asked for (restored — see repack.rs's own
    // doc comment on why this was briefly not enforced, and why it is
    // again now): a repack that would leave the target costing more
    // than it currently sells for must be rejected outright, not
    // silently applied.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    // 300 consumed / 12 produced = 25 per single egg — priced at only
    // 20, which would be a real loss.
    let dozen_id = make_inventory_item(&conn, &biz, "EGGS-D3", "Eggs (dozen)", 5, 300, 400);
    let single_id = make_inventory_item(&conn, &biz, "EGGS-S3", "Eggs (single)", 0, 0, 20);

    let req = crate::repack::RepackRequest {
        source_record_id: dozen_id.clone(),
        source_quantity: 1,
        target_record_id: Some(single_id.clone()),
        target_quantity_produced: 12,
        new_target_name: None,
        new_target_unit_price: None,
        notes: None,
    };
    let result = crate::repack::repack(&mut conn, &biz, &uid, req);
    assert!(result.is_err(), "a repack that would price the target below its own cost must be rejected");

    // Nothing should have moved — a rejected repack must not
    // partially apply, exactly like the stock-insufficiency case above.
    let dozen = get_item(&conn, &biz, &uid, &dozen_id);
    assert_eq!(dozen["quantity"].as_i64().unwrap(), 5, "source stock must be untouched by a rejected repack");
    let single = get_item(&conn, &biz, &uid, &single_id);
    assert_eq!(single["quantity"].as_i64().unwrap(), 0, "target stock must be untouched by a rejected repack");
    assert_eq!(single["unit_cost"].as_i64().unwrap(), 0, "target cost must be untouched by a rejected repack");
}

#[test]
fn test_repack_rejects_same_source_and_target() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let id = make_inventory_item(&conn, &biz, "SELF-001", "Self", 10, 100, 150);

    let req = crate::repack::RepackRequest {
        source_record_id: id.clone(),
        source_quantity: 1,
        target_record_id: Some(id),
        target_quantity_produced: 1,
        new_target_name: None,
        new_target_unit_price: None,
        notes: None,
    };
    assert!(crate::repack::repack(&mut conn, &biz, &uid, req).is_err());
}
