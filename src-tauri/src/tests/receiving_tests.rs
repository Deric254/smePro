use super::common::*;

fn test_food_business(conn: &mut rusqlite::Connection) -> String {
    let id = crate::business_panel::create_business(conn, "Test Food Biz", "USD", "UTC").expect("create business");
    crate::onboarding::apply_business_type(conn, &id, "food").expect("enable modules");
    id
}

// cost_cents/price_cents are integer minor units (e.g. 2000 = $20.00),
// matching the "money" field type's on-the-wire and on-disk
// representation everywhere in the app now — see money.rs.
fn make_inventory_item(conn: &mut rusqlite::Connection, biz: &str, uid: &str, sku: &str, name: &str, qty: i64, cost_cents: i64, price_cents: i64) -> String {
    let mut item = serde_json::Map::new();
    item.insert("sku".into(), serde_json::json!(sku));
    item.insert("name".into(), serde_json::json!(name));
    item.insert("quantity".into(), serde_json::json!(qty));
    item.insert("unit_cost".into(), serde_json::json!(cost_cents));
    item.insert("unit_price".into(), serde_json::json!(price_cents));
    crate::crud::create(conn, biz, uid, "inventory", &item).unwrap()
}

fn make_purchase_order(conn: &mut rusqlite::Connection, biz: &str, uid: &str, inv_id: &str, item_name: &str, qty: i64, unit_cost_cents: i64) -> String {
    let mut po = serde_json::Map::new();
    po.insert("supplier".into(), serde_json::json!("Test Supplier"));
    po.insert("item_name".into(), serde_json::json!(item_name));
    po.insert("inventory_record_id".into(), serde_json::json!(inv_id));
    po.insert("quantity".into(), serde_json::json!(qty));
    po.insert("unit_cost".into(), serde_json::json!(unit_cost_cents));
    crate::crud::create(conn, biz, uid, "purchasing", &po).unwrap()
}

#[test]
fn test_receiving_computes_real_weighted_average_cost() {
    let mut conn = test_db();
    let biz = test_food_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    // 50 on hand at $20.00 (2000 cents) each.
    let inv_id = make_inventory_item(&mut conn, &biz, &uid, "SUGAR-001", "Sugar", 50, 2000, 3000);
    // Receiving 50 more, this time at $30.00 (3000 cents) each (price went up).
    let po_id = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Sugar", 50, 3000);

    let req = crate::receiving::ReceiveRequest { purchase_record_id: po_id, quantity_received: None };
    let result = crate::receiving::receive(&mut conn, &biz, &uid, req).unwrap();

    // (50*2000 + 50*3000) / 100 = 2500 cents ($25.00) exactly.
    assert_eq!(result["new_weighted_average_cost"].as_i64().unwrap(), 2500);
    let list = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    assert_eq!(list[0]["unit_cost"].as_i64().unwrap(), 2500);
    assert_eq!(list[0]["quantity"].as_i64().unwrap(), 100);
}

#[test]
fn test_first_receipt_on_zero_stock_takes_the_new_cost_exactly() {
    let mut conn = test_db();
    let biz = test_food_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    // Zero on hand -- a brand new item, or one that was fully sold out.
    let inv_id = make_inventory_item(&mut conn, &biz, &uid, "RICE-002", "Basmati Rice", 0, 0, 0);
    let po_id = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Basmati Rice", 40, 4550);

    let req = crate::receiving::ReceiveRequest { purchase_record_id: po_id, quantity_received: None };
    let result = crate::receiving::receive(&mut conn, &biz, &uid, req).unwrap();

    // No distortion from the zero -- the new cost is exactly what was paid.
    assert_eq!(result["new_weighted_average_cost"].as_i64().unwrap(), 4550);
}

#[test]
fn test_partial_delivery_receives_less_than_ordered() {
    let mut conn = test_db();
    let biz = test_food_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = make_inventory_item(&mut conn, &biz, &uid, "OIL-002", "Sunflower Oil", 10, 10000, 15000);
    let po_id = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Sunflower Oil", 100, 11000);

    // Ordered 100, only 60 actually arrived.
    let req = crate::receiving::ReceiveRequest { purchase_record_id: po_id, quantity_received: Some(60) };
    let result = crate::receiving::receive(&mut conn, &biz, &uid, req).unwrap();

    assert_eq!(result["partial_delivery"].as_bool().unwrap(), true);
    assert_eq!(result["new_stock_level"].as_i64().unwrap(), 70); // 10 + 60
}

#[test]
fn test_cannot_receive_the_same_purchase_order_twice() {
    let mut conn = test_db();
    let biz = test_food_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = make_inventory_item(&mut conn, &biz, &uid, "SALT-001", "Salt", 20, 500, 800);
    let po_id = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Salt", 30, 600);

    let first = crate::receiving::ReceiveRequest { purchase_record_id: po_id.clone(), quantity_received: None };
    crate::receiving::receive(&mut conn, &biz, &uid, first).unwrap();

    // Same PO again -- must be rejected, not silently double-count the stock.
    let second = crate::receiving::ReceiveRequest { purchase_record_id: po_id, quantity_received: None };
    assert!(crate::receiving::receive(&mut conn, &biz, &uid, second).is_err());

    // Stock must reflect exactly one receipt, not two.
    let list = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    assert_eq!(list[0]["quantity"].as_i64().unwrap(), 50); // 20 + 30, not 20 + 30 + 30
}

#[test]
fn test_weighted_average_rounds_to_nearest_cent_on_uneven_division() {
    let mut conn = test_db();
    let biz = test_food_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    // 10 on hand at 1000 cents, receiving 7 more at 1333 cents.
    // (10*1000 + 7*1333) / 17 = (10000 + 9331) / 17 = 19331 / 17 =
    // 1137.117... which must round to 1137, not truncate to 1136 the
    // way plain integer division would.
    let inv_id = make_inventory_item(&mut conn, &biz, &uid, "FLOUR-001", "Flour", 10, 1000, 1500);
    let po_id = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Flour", 7, 1333);

    let req = crate::receiving::ReceiveRequest { purchase_record_id: po_id, quantity_received: None };
    let result = crate::receiving::receive(&mut conn, &biz, &uid, req).unwrap();

    assert_eq!(result["new_weighted_average_cost"].as_i64().unwrap(), 1137);
}

#[test]
fn test_purchase_order_without_inventory_link_is_rejected() {
    let mut conn = test_db();
    let biz = test_food_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let mut po = serde_json::Map::new();
    po.insert("supplier".into(), serde_json::json!("Test Supplier"));
    po.insert("item_name".into(), serde_json::json!("Unlinked Item"));
    po.insert("quantity".into(), serde_json::json!(10));
    po.insert("unit_cost".into(), serde_json::json!(500));
    // Deliberately no inventory_record_id.
    let po_id = crate::crud::create(&mut conn, &biz, &uid, "purchasing", &po).unwrap();

    let req = crate::receiving::ReceiveRequest { purchase_record_id: po_id, quantity_received: None };
    assert!(crate::receiving::receive(&mut conn, &biz, &uid, req).is_err());
}
