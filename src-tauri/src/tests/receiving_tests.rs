use super::common::*;
use serde_json::json;

fn test_food_business(conn: &mut rusqlite::Connection) -> String {
    let id = crate::business_panel::create_business(conn, "Test Food Biz", "USD", "UTC").expect("create business");
    crate::onboarding::apply_business_type(conn, &id, "food").expect("enable modules");
    id
}

// cost_cents/price_cents are integer minor units (e.g. 2000 = $20.00),
// matching the "money" field type's on-the-wire and on-disk
// representation everywhere in the app now — see money.rs.
//
// Delegates to common.rs's seed_inventory_item() rather than
// crud::create() directly: every inventory item created through
// create() is now forced to start at zero stock (see crud.rs), so a
// test needing pre-existing stock to receive more against has to seed
// it the same way a real bulk catalog migration would.
fn make_inventory_item(conn: &rusqlite::Connection, biz: &str, sku: &str, name: &str, qty: i64, cost_cents: i64, price_cents: i64) -> String {
    seed_inventory_item(conn, biz, sku, name, qty, cost_cents, price_cents)
}

fn make_purchase_order(conn: &mut rusqlite::Connection, biz: &str, uid: &str, inv_id: &str, item_name: &str, qty: i64, unit_cost_cents: i64, unit_price_cents: i64) -> String {
    let mut po = serde_json::Map::new();
    po.insert("supplier".into(), serde_json::json!("Test Supplier"));
    po.insert("item_name".into(), serde_json::json!(item_name));
    po.insert("inventory_record_id".into(), serde_json::json!(inv_id));
    po.insert("quantity".into(), serde_json::json!(qty));
    po.insert("unit_cost".into(), serde_json::json!(unit_cost_cents));
    // Required since v31_purchasing_unit_price — "I'm paying this, I
    // will sell this, period" decided together at purchase time now,
    // not asked for again at receive.
    po.insert("unit_price".into(), serde_json::json!(unit_price_cents));
    crate::crud::create(conn, biz, uid, "purchasing", &po).unwrap()
}

#[test]
fn test_receiving_creates_a_batch_at_the_po_cost_and_never_touches_legacy_cost() {
    // BATCH REWRITE: this test used to be named
    // test_receiving_computes_real_weighted_average_cost and asserted a
    // blended (50*2000 + 50*3000) / 100 = 2500 cost got written onto
    // the Inventory row. That behavior is gone on purpose — see
    // batches.rs's own module doc comment: every receipt now creates
    // its own independent batch at EXACTLY the PO's cost, and the
    // Inventory row's own unit_cost/unit_price are frozen legacy
    // history from the moment any batch exists for that item. This
    // test now proves both halves of that: the new batch carries the
    // PO's exact, unblended cost, and legacy stays untouched.
    let mut conn = test_db();
    let biz = test_food_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    // 50 on hand at $20.00 (2000 cents) each.
    let inv_id = make_inventory_item(&conn, &biz, "SUGAR-001", "Sugar", 50, 2000, 3000);
    // Receiving 50 more, this time at $30.00 (3000 cents) each (price went up).
    // Sold at exactly that same figure too — a batch's own price is
    // its own now, no fallback to compare it against.
    let po_id = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Sugar", 50, 3000, 3000);

    let req = crate::receiving::ReceiveRequest { purchase_record_id: po_id, quantity_received: None, unit_price: None, expiry_date: None };
    let result = crate::receiving::receive(&mut conn, &biz, &uid, req).unwrap();

    // Exactly this delivery's own cost — no averaging with the 50
    // already on hand at 2000.
    assert_eq!(result["batch_unit_cost"].as_i64().unwrap(), 3000);
    // Defaults to this purchase order's own declared price, since this
    // call didn't override it — never Inventory's legacy unit_price.
    assert_eq!(result["batch_unit_price"].as_i64().unwrap(), 3000);

    // Inventory.quantity is still the single total everything else
    // reads; unit_cost is frozen legacy history, exactly what it was
    // before this receipt.
    let list = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    assert_eq!(list[0]["quantity"].as_i64().unwrap(), 100);
    assert_eq!(list[0]["unit_cost"].as_i64().unwrap(), 2000, "legacy cost must never be touched by a receipt once batches exist");

    // The new batch itself: 50 units at 3000, legacy still shows the
    // other 50 at the original 2000.
    let summary = crate::batches::list_batches(&conn, &biz, &uid, &inv_id).unwrap();
    assert_eq!(summary["batches"].as_array().unwrap().len(), 1);
    assert_eq!(summary["batches"][0]["quantity_remaining"].as_i64().unwrap(), 50);
    assert_eq!(summary["batches"][0]["unit_cost"].as_i64().unwrap(), 3000);
    assert_eq!(summary["legacy_quantity"].as_i64().unwrap(), 50);
    assert_eq!(summary["legacy_unit_cost"].as_i64().unwrap(), 2000);
}

#[test]
fn test_first_receipt_on_zero_stock_creates_a_batch_at_exactly_the_po_cost() {
    let mut conn = test_db();
    let biz = test_food_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    // Zero on hand -- a brand new item, or one that was fully sold out.
    // Priced comfortably above the incoming cost so this test proves
    // what it's named for (a clean, undistorted first-cost receipt),
    // not the separate "can't receive above the current price" guard.
    let inv_id = make_inventory_item(&conn, &biz, "RICE-002", "Basmati Rice", 0, 0, 6000);
    let po_id = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Basmati Rice", 40, 4550, 6000);

    let req = crate::receiving::ReceiveRequest { purchase_record_id: po_id, quantity_received: None, unit_price: None, expiry_date: None };
    let result = crate::receiving::receive(&mut conn, &biz, &uid, req).unwrap();

    // The new batch's cost is exactly what was paid — there is nothing
    // left to blend against on a zero-stock item, batch or not.
    assert_eq!(result["batch_unit_cost"].as_i64().unwrap(), 4550);
}

#[test]
fn test_partial_delivery_receives_less_than_ordered() {
    let mut conn = test_db();
    let biz = test_food_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = make_inventory_item(&conn, &biz, "OIL-002", "Sunflower Oil", 10, 10000, 15000);
    let po_id = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Sunflower Oil", 100, 11000, 15000);

    // Ordered 100, only 60 actually arrived.
    let req = crate::receiving::ReceiveRequest { purchase_record_id: po_id, quantity_received: Some(60), unit_price: None, expiry_date: None };
    let result = crate::receiving::receive(&mut conn, &biz, &uid, req).unwrap();

    assert!(result["partial_delivery"].as_bool().unwrap());
    assert_eq!(result["new_stock_level"].as_i64().unwrap(), 70); // 10 + 60
}

#[test]
fn test_cannot_receive_the_same_purchase_order_twice() {
    let mut conn = test_db();
    let biz = test_food_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = make_inventory_item(&conn, &biz, "SALT-001", "Salt", 20, 500, 800);
    let po_id = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Salt", 30, 600, 800);

    let first = crate::receiving::ReceiveRequest { purchase_record_id: po_id.clone(), quantity_received: None, unit_price: None, expiry_date: None };
    crate::receiving::receive(&mut conn, &biz, &uid, first).unwrap();

    // Same PO again -- must be rejected, not silently double-count the stock.
    let second = crate::receiving::ReceiveRequest { purchase_record_id: po_id, quantity_received: None, unit_price: None, expiry_date: None };
    assert!(crate::receiving::receive(&mut conn, &biz, &uid, second).is_err());

    // Stock must reflect exactly one receipt, not two.
    let list = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    assert_eq!(list[0]["quantity"].as_i64().unwrap(), 50); // 20 + 30, not 20 + 30 + 30
}

#[test]
fn test_receiving_no_longer_blends_or_rounds_an_uneven_delivery() {
    // BATCH REWRITE: this test used to prove the old weighted-average
    // formula rounded (10*1000 + 7*1333) / 17 to 1137 rather than
    // truncating to 1136. There is no blending step left on this path
    // at all to round — a batch's cost is stored exactly as the PO's
    // own unit_cost — so this now asserts the opposite: an uneven
    // delivery against uneven existing stock produces a batch at
    // exactly 1333, untouched by the 10 units already on hand at 1000.
    let mut conn = test_db();
    let biz = test_food_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = make_inventory_item(&conn, &biz, "FLOUR-001", "Flour", 10, 1000, 1500);
    let po_id = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Flour", 7, 1333, 1500);

    let req = crate::receiving::ReceiveRequest { purchase_record_id: po_id, quantity_received: None, unit_price: None, expiry_date: None };
    let result = crate::receiving::receive(&mut conn, &biz, &uid, req).unwrap();

    assert_eq!(result["batch_unit_cost"].as_i64().unwrap(), 1333);
    let list = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    assert_eq!(list[0]["unit_cost"].as_i64().unwrap(), 1000, "legacy cost must stay exactly what it was, no blending");
}

#[test]
fn test_purchase_order_without_inventory_link_is_rejected() {
    let mut conn = test_db();
    let biz = test_food_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    // `crud::create()` now resolves `item_name` against Inventory at
    // creation time and rejects the row outright if there's no match
    // (see crud.rs's "THE BUG THIS FIXES" comment on the purchasing
    // block) — so a truly unlinked PO can no longer be produced through
    // this path at all. Seed the item, create the PO normally so it
    // gets linked, then strip the link directly in the DB to simulate
    // the orphaned-record case (e.g. the Inventory item was deleted
    // after the PO was made) that `receive()` itself is meant to guard
    // against.
    let mut inv = serde_json::Map::new();
    inv.insert("sku".into(), serde_json::json!("UNLINK-001"));
    inv.insert("name".into(), serde_json::json!("Unlinked Item"));
    inv.insert("quantity".into(), serde_json::json!(0));
    inv.insert("unit_cost".into(), serde_json::json!(400));
    inv.insert("unit_price".into(), serde_json::json!(600));
    crate::crud::create(&conn, &biz, &uid, "inventory", &inv).unwrap();

    let mut po = serde_json::Map::new();
    po.insert("supplier".into(), serde_json::json!("Test Supplier"));
    po.insert("item_name".into(), serde_json::json!("Unlinked Item"));
    po.insert("quantity".into(), serde_json::json!(10));
    po.insert("unit_cost".into(), serde_json::json!(500));
    po.insert("unit_price".into(), serde_json::json!(700));
    let po_id = crate::crud::create(&conn, &biz, &uid, "purchasing", &po).unwrap();

    conn.execute(
        "UPDATE module_purchasing SET inventory_record_id = NULL WHERE id = ?1",
        rusqlite::params![po_id],
    )
    .unwrap();

    let req = crate::receiving::ReceiveRequest { purchase_record_id: po_id, quantity_received: None, unit_price: None, expiry_date: None };
    assert!(crate::receiving::receive(&mut conn, &biz, &uid, req).is_err());
}

#[test]
fn test_purchasing_quantity_cost_price_are_frozen_once_received() {
    // Companion to excel_import.rs's own guard on this exact same
    // three-field list (see its comment there): once a PO has been
    // received, the batch it created is the real record of what was
    // actually paid and charged. Hand-editing the PO's own
    // quantity/unit_cost/unit_price afterward through the plain
    // single-record update — not just through Excel reconciliation —
    // must be rejected too, or the PO silently drifts from the batch
    // that was actually created from its old values.
    let mut conn = test_db();
    let biz = test_food_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let inv_id = make_inventory_item(&conn, &biz, "FROZEN-001", "Frozen Item", 0, 0, 0);
    let po_id = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Frozen Item", 10, 500, 700);

    // Unreceived: still an ordinary editable field.
    let mut patch = serde_json::Map::new();
    patch.insert("unit_cost".into(), json!(550));
    assert!(crate::crud::update(&conn, &biz, &uid, "purchasing", &po_id, &patch, false).is_ok());

    let req = crate::receiving::ReceiveRequest { purchase_record_id: po_id.clone(), quantity_received: None, unit_price: None, expiry_date: None };
    crate::receiving::receive(&mut conn, &biz, &uid, req).unwrap();

    // Received: now frozen, on all three fields.
    for (field, value) in [("quantity", json!(999)), ("unit_cost", json!(1)), ("unit_price", json!(1))] {
        let mut bad_patch = serde_json::Map::new();
        bad_patch.insert(field.into(), value);
        let err = crate::crud::update(&conn, &biz, &uid, "purchasing", &po_id, &bad_patch, false).unwrap_err();
        assert!(err.to_string().contains("already been received"), "unexpected error for '{field}': {err}");
    }

    // Fields outside this list (e.g. supplier) remain freely editable
    // even after receipt — only the three that a batch was actually
    // derived from are frozen.
    let mut supplier_patch = serde_json::Map::new();
    supplier_patch.insert("supplier".into(), json!("New Supplier Name"));
    assert!(crate::crud::update(&conn, &biz, &uid, "purchasing", &po_id, &supplier_patch, false).is_ok());
}

#[test]
fn test_receiving_rejects_a_delivery_that_would_price_the_item_below_cost() {
    // Under the old weighted-average design this scenario arose from a
    // supplier price increase alone (a PO's cost could exceed the
    // item's separately-set price without anyone directly comparing
    // the two). That's no longer possible to construct this way at
    // all: since v31_purchasing_unit_price, a purchase order's own
    // unit_price/unit_cost are validated against each other (the same
    // `min_field` floor Inventory's legacy fields have always had) the
    // moment the PO is created — a PO priced below its own cost simply
    // can't exist. So the only remaining way to reach this guard is
    // the one legitimate case receive_in_tx's own belt-and-suspenders
    // check exists for: someone typing an explicit override at receive
    // time that undercuts the PO's own already-valid price. This test
    // now proves that path specifically.
    let mut conn = test_db();
    let biz = test_food_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    // A perfectly valid PO on its own: costs 5000, meant to sell at 6000.
    let inv_id = make_inventory_item(&conn, &biz, "SUGAR-002", "Sugar", 10, 2000, 3000);
    let po_id = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Sugar", 10, 5000, 6000);

    // ...but the person receiving it overrides the price down to 4000
    // — below this delivery's own 5000 cost.
    let req = crate::receiving::ReceiveRequest { purchase_record_id: po_id.clone(), quantity_received: None, unit_price: Some(4000), expiry_date: None };
    let result = crate::receiving::receive(&mut conn, &biz, &uid, req);
    assert!(result.is_err(), "must reject an override that would price this delivery below its own cost");

    // Nothing should have moved — a rejected receive must not
    // partially apply.
    let list = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    assert_eq!(list[0]["quantity"].as_i64().unwrap(), 10, "stock must be untouched by a rejected receive");
    assert_eq!(list[0]["unit_cost"].as_i64().unwrap(), 2000, "cost must be untouched by a rejected receive");
    let po_list = crate::crud::list(&conn, &biz, &uid, "purchasing", None, 50, 0).unwrap();
    let po = po_list.iter().find(|r| r["id"] == json!(po_id)).unwrap();
    assert_eq!(po["received"], json!(false), "the purchase order must still be unreceived, not partially processed");
}

#[test]
fn test_purchasing_cannot_be_created_priced_below_its_own_cost() {
    // Companion to the override test above, from the creation-time
    // angle: this is now the FIRST line of defense, not receive_in_tx's
    // redundant check — a PO priced below its own cost should never
    // exist to begin with.
    let mut conn = test_db();
    let biz = test_food_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let inv_id = make_inventory_item(&conn, &biz, "SUGAR-003", "Sugar", 0, 0, 0);

    let mut po = serde_json::Map::new();
    po.insert("supplier".into(), serde_json::json!("Test Supplier"));
    po.insert("item_name".into(), serde_json::json!("Sugar"));
    po.insert("inventory_record_id".into(), serde_json::json!(inv_id));
    po.insert("quantity".into(), serde_json::json!(10));
    po.insert("unit_cost".into(), serde_json::json!(5000));
    po.insert("unit_price".into(), serde_json::json!(4000)); // below cost
    let result = crate::crud::create(&conn, &biz, &uid, "purchasing", &po);
    assert!(result.is_err(), "creating a purchase order priced below its own cost must be rejected");
}
