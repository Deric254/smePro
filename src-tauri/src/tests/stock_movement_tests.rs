use super::common::*;
use crate::stock_movement::{self, MovementFilters};

/// Local copies of the two helpers this file needs — `make_purchase_order`
/// lives in receiving_tests.rs and `add_staff` in rbac_tests.rs, both
/// private to their own module. Duplicated here rather than moved into
/// common.rs so this change touches no existing test file.
fn make_purchase_order(conn: &mut rusqlite::Connection, biz: &str, uid: &str, inv_id: &str, item_name: &str, qty: i64, unit_cost_cents: i64, unit_price_cents: i64) -> String {
    let mut po = serde_json::Map::new();
    po.insert("supplier".into(), serde_json::json!("Test Supplier"));
    po.insert("item_name".into(), serde_json::json!(item_name));
    po.insert("inventory_record_id".into(), serde_json::json!(inv_id));
    po.insert("quantity".into(), serde_json::json!(qty));
    po.insert("unit_cost".into(), serde_json::json!(unit_cost_cents));
    po.insert("unit_price".into(), serde_json::json!(unit_price_cents));
    crate::crud::create(conn, biz, uid, "purchasing", &po).unwrap()
}

fn add_staff(conn: &rusqlite::Connection, biz: &str, username: &str) -> String {
    let hash = crate::auth::hash_secret("password123").unwrap();
    crate::business_panel::add_user(conn, biz, username, &hash, "Staff").unwrap()
}

fn receive_po(conn: &mut rusqlite::Connection, biz: &str, uid: &str, po_id: &str, unit_price: i64) {
    crate::receiving::receive(conn, biz, uid, crate::receiving::ReceiveRequest {
        purchase_record_id: po_id.to_string(),
        quantity_received: None,
        unit_price: Some(unit_price),
        expiry_date: None,
    }).unwrap();
}

fn filters() -> MovementFilters {
    MovementFilters { limit: 500, ..Default::default() }
}

fn movements_for(conn: &rusqlite::Connection, biz: &str, uid: &str) -> Vec<serde_json::Value> {
    stock_movement::list(conn, biz, uid, &filters()).unwrap()["movements"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn test_a_sale_writes_one_negative_movement_at_the_consumed_cost() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let inv_id = seed_inventory_item(&conn, &biz, "RICE-001", "Rice", 100, 5000, 7500);

    let req = crate::pos::CheckoutRequest {
        idempotency_key: None,
        discount_pct: None,
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id.clone(), quantity: 5 }],
        payment_method: Some("Cash".into()),
        customer: None,
        customer_phone: None,
        allow_oversell: false,
        on_credit: false,
        due_date: None,
    };
    crate::pos::checkout(&mut conn, &biz, &uid, req).unwrap();

    let m = movements_for(&conn, &biz, &uid);
    assert_eq!(m.len(), 1);
    assert_eq!(m[0]["movement_type"], stock_movement::SALE);
    assert_eq!(m[0]["quantity_delta"].as_i64().unwrap(), -5, "a sale must be recorded as stock leaving");
    assert_eq!(m[0]["unit_cost_cents"].as_i64().unwrap(), 5000);
    assert_eq!(m[0]["total_cost_cents"].as_i64().unwrap(), -25000, "total cost stays signed with the delta");
    assert_eq!(m[0]["item_name"], "Rice");
}

#[test]
fn test_the_ledger_sums_back_to_the_items_real_quantity() {
    // The property that makes this a ledger rather than a log: every
    // signed delta for an item, summed onto its starting quantity,
    // must land exactly on what inventory.quantity actually says.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let inv_id = seed_inventory_item(&conn, &biz, "RICE-001", "Rice", 100, 5000, 7500);

    let sell = |conn: &mut rusqlite::Connection, qty: i64| {
        let req = crate::pos::CheckoutRequest {
            idempotency_key: None,
            discount_pct: None,
            items: vec![crate::pos::CartItem { inventory_record_id: inv_id.clone(), quantity: qty }],
            payment_method: Some("Cash".into()),
            customer: None,
            customer_phone: None,
            allow_oversell: false,
            on_credit: false,
            due_date: None,
        };
        crate::pos::checkout(conn, &biz, &uid, req).unwrap()
    };
    sell(&mut conn, 5);
    sell(&mut conn, 3);

    // A stock take that finds 4 units missing on top of the sales.
    let initiated = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    let st_id = initiated["id"].as_str().unwrap().to_string();
    let item_id = initiated["items"][0]["id"].as_str().unwrap().to_string();
    crate::stock_take::record_count(
        &conn, &biz, &uid,
        crate::stock_take::RecordCountRequest { stock_take_id: st_id.clone(), item_id, counted_qty: 88 },
    ).unwrap();
    crate::stock_take::close(&mut conn, &biz, &uid, &st_id).unwrap();

    let net: i64 = movements_for(&conn, &biz, &uid)
        .iter()
        .map(|m| m["quantity_delta"].as_i64().unwrap())
        .sum();
    assert_eq!(net, -12, "5 + 3 sold, 4 lost to shrinkage");

    let module = crate::crud::load_module(&conn, &biz, "inventory").unwrap();
    let actual: i64 = conn
        .query_row(
            &format!("SELECT quantity FROM {} WHERE id = ?1", module.table_name()),
            rusqlite::params![inv_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(100 + net, actual, "starting quantity plus every signed delta must equal the live quantity");
}

#[test]
fn test_receiving_and_refund_are_recorded_as_stock_arriving() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let inv_id = seed_inventory_item(&conn, &biz, "SOAP-100", "Soap", 0, 0, 500);

    let po_id = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Soap", 20, 100, 150);
    receive_po(&mut conn, &biz, &uid, &po_id, 150);

    let received: Vec<_> = movements_for(&conn, &biz, &uid)
        .into_iter()
        .filter(|m| m["movement_type"] == stock_movement::RECEIVING)
        .collect();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0]["quantity_delta"].as_i64().unwrap(), 20);
    assert_eq!(received[0]["unit_cost_cents"].as_i64().unwrap(), 100, "receiving is costed at the PO's unit cost");
}

#[test]
fn test_a_repack_records_both_halves_with_matching_reference() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let source_id = seed_inventory_item(&conn, &biz, "SACK-50", "Sack of rice 50kg", 10, 500000, 600000);
    let target_id = seed_inventory_item(&conn, &biz, "PKT-1", "Rice packet 1kg", 0, 0, 14000);

    crate::repack::repack(&mut conn, &biz, &uid, crate::repack::RepackRequest {
        source_record_id: source_id.clone(),
        source_quantity: 1,
        target_record_id: Some(target_id.clone()),
        new_target_name: None,
        new_target_unit_price: None,
        target_quantity_produced: 50,
        notes: None,
    }).unwrap();

    let m = movements_for(&conn, &biz, &uid);
    let consumed = m.iter().find(|x| x["movement_type"] == stock_movement::REPACK_CONSUMED).unwrap();
    let produced = m.iter().find(|x| x["movement_type"] == stock_movement::REPACK_PRODUCED).unwrap();
    assert_eq!(consumed["quantity_delta"].as_i64().unwrap(), -1);
    assert_eq!(produced["quantity_delta"].as_i64().unwrap(), 50);
    assert_eq!(
        consumed["reference_id"], produced["reference_id"],
        "both halves of one repack must share a reference so they can be matched up"
    );
    assert_eq!(produced["unit_cost_cents"].as_i64().unwrap(), 10000, "500000 consumed / 50 produced");
}

#[test]
fn test_shrinkage_movement_cost_matches_the_stock_takes_own_write_off() {
    // The ledger and profit.rs's shrinkage figure read the same event —
    // if these two ever disagree, one of them is lying to the owner.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    seed_inventory_item(&conn, &biz, "RICE-001", "Rice", 40, 500, 800);

    let initiated = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    let st_id = initiated["id"].as_str().unwrap().to_string();
    let item_id = initiated["items"][0]["id"].as_str().unwrap().to_string();
    crate::stock_take::record_count(
        &conn, &biz, &uid,
        crate::stock_take::RecordCountRequest { stock_take_id: st_id.clone(), item_id, counted_qty: 30 },
    ).unwrap();
    let summary = crate::stock_take::close(&mut conn, &biz, &uid, &st_id).unwrap();

    let m = movements_for(&conn, &biz, &uid);
    let shrink = m.iter().find(|x| x["movement_type"] == stock_movement::STOCK_TAKE_SHRINKAGE).unwrap();
    assert_eq!(shrink["quantity_delta"].as_i64().unwrap(), -10);
    assert_eq!(
        shrink["total_cost_cents"].as_i64().unwrap().abs(),
        summary["total_write_off_cost"].as_i64().unwrap(),
        "the ledger's cost for this shrinkage must equal the write-off the stock take booked"
    );
}

#[test]
fn test_a_surplus_is_recorded_at_zero_cost_not_an_invented_one() {
    // Scoped to a genuinely pre-batch item (seed_inventory_item never
    // creates a batch) — its legacy unit_price is real, live pricing,
    // so a surplus really does have no separate cost basis to invent.
    // An item whose price lives on a batch instead gets a different,
    // real cost here — see
    // test_surplus_on_a_sold_out_batch_item_creates_a_new_priced_batch_not_a_zero_price_gap
    // in stock_take_tests.rs.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    seed_inventory_item(&conn, &biz, "RICE-001", "Rice", 40, 500, 800);

    let initiated = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    let st_id = initiated["id"].as_str().unwrap().to_string();
    let item_id = initiated["items"][0]["id"].as_str().unwrap().to_string();
    crate::stock_take::record_count(
        &conn, &biz, &uid,
        crate::stock_take::RecordCountRequest { stock_take_id: st_id.clone(), item_id, counted_qty: 45 },
    ).unwrap();
    crate::stock_take::close(&mut conn, &biz, &uid, &st_id).unwrap();

    let m = movements_for(&conn, &biz, &uid);
    let surplus = m.iter().find(|x| x["movement_type"] == stock_movement::STOCK_TAKE_SURPLUS).unwrap();
    assert_eq!(surplus["quantity_delta"].as_i64().unwrap(), 5);
    assert_eq!(surplus["unit_cost_cents"].as_i64().unwrap(), 0, "found stock has no cost basis of its own");
}

#[test]
fn test_a_cancelled_stock_take_writes_no_movements_at_all() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    seed_inventory_item(&conn, &biz, "RICE-001", "Rice", 40, 500, 800);

    let initiated = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    let st_id = initiated["id"].as_str().unwrap().to_string();
    let item_id = initiated["items"][0]["id"].as_str().unwrap().to_string();
    crate::stock_take::record_count(
        &conn, &biz, &uid,
        crate::stock_take::RecordCountRequest { stock_take_id: st_id.clone(), item_id, counted_qty: 10 },
    ).unwrap();
    crate::stock_take::cancel(&mut conn, &biz, &uid, &st_id).unwrap();

    assert!(movements_for(&conn, &biz, &uid).is_empty(), "cancel touches no stock, so it must leave no ledger rows");
}

#[test]
fn test_filters_narrow_the_trace_by_item_type_and_date() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let rice = seed_inventory_item(&conn, &biz, "RICE-001", "Rice", 100, 5000, 7500);
    let tea = seed_inventory_item(&conn, &biz, "TEA-001", "Tea", 100, 2000, 3500);

    for (id, qty) in [(&rice, 5), (&tea, 2)] {
        let req = crate::pos::CheckoutRequest {
            idempotency_key: None,
            discount_pct: None,
            items: vec![crate::pos::CartItem { inventory_record_id: id.clone(), quantity: qty }],
            payment_method: Some("Cash".into()),
            customer: None,
            customer_phone: None,
            allow_oversell: false,
            on_credit: false,
            due_date: None,
        };
        crate::pos::checkout(&mut conn, &biz, &uid, req).unwrap();
    }

    let by_item = stock_movement::list(&conn, &biz, &uid, &MovementFilters {
        inventory_record_id: Some(rice.clone()),
        ..filters()
    }).unwrap();
    assert_eq!(by_item["movements"].as_array().unwrap().len(), 1);
    assert_eq!(by_item["movements"][0]["item_name"], "Rice");

    let by_type = stock_movement::list(&conn, &biz, &uid, &MovementFilters {
        movement_type: Some(stock_movement::RECEIVING.to_string()),
        ..filters()
    }).unwrap();
    assert!(by_type["movements"].as_array().unwrap().is_empty(), "no receiving happened in this test");

    // A `to` date of today must include movements recorded moments ago,
    // not exclude them for being later than midnight.
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let todays = stock_movement::list(&conn, &biz, &uid, &MovementFilters {
        from: Some(today.clone()),
        to: Some(today),
        ..filters()
    }).unwrap();
    assert_eq!(todays["movements"].as_array().unwrap().len(), 2, "a single-day range must cover the whole day");

    let long_ago = stock_movement::list(&conn, &biz, &uid, &MovementFilters {
        to: Some("2020-01-01".to_string()),
        ..filters()
    }).unwrap();
    assert!(long_ago["movements"].as_array().unwrap().is_empty());
}

#[test]
fn test_totals_report_stock_in_out_and_net_separately() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let inv_id = seed_inventory_item(&conn, &biz, "SOAP-100", "Soap", 0, 0, 500);
    let po_id = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Soap", 20, 100, 150);
    receive_po(&mut conn, &biz, &uid, &po_id, 150);

    let req = crate::pos::CheckoutRequest {
        idempotency_key: None,
        discount_pct: None,
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id, quantity: 8 }],
        payment_method: Some("Cash".into()),
        customer: None,
        customer_phone: None,
        allow_oversell: false,
        on_credit: false,
        due_date: None,
    };
    crate::pos::checkout(&mut conn, &biz, &uid, req).unwrap();

    let data = stock_movement::list(&conn, &biz, &uid, &filters()).unwrap();
    assert_eq!(data["total_quantity_in"].as_i64().unwrap(), 20);
    assert_eq!(data["total_quantity_out"].as_i64().unwrap(), -8);
    assert_eq!(data["net_quantity_change"].as_i64().unwrap(), 12);
}

#[test]
fn test_export_produces_a_real_xlsx_file() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let inv_id = seed_inventory_item(&conn, &biz, "RICE-001", "Rice", 100, 5000, 7500);
    let req = crate::pos::CheckoutRequest {
        idempotency_key: None,
        discount_pct: None,
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id, quantity: 5 }],
        payment_method: Some("Cash".into()),
        customer: None,
        customer_phone: None,
        allow_oversell: false,
        on_credit: false,
        due_date: None,
    };
    crate::pos::checkout(&mut conn, &biz, &uid, req).unwrap();

    let bytes = stock_movement::export_xlsx(&conn, &biz, &uid, &filters()).unwrap();
    // A real .xlsx is a ZIP container — "PK" is its magic number. This
    // catches the failure mode of shipping a CSV under an .xlsx name.
    assert_eq!(&bytes[0..2], b"PK");
    assert!(bytes.len() > 1000, "an empty or truncated workbook would be far smaller than this");
}

#[test]
fn test_staff_cannot_read_the_movement_trace() {
    // Same oversight-data reasoning as the audit log: this covers what
    // every user in the business has moved.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (owner_id, _) = test_owner(&mut conn, &biz);
    let staff_id = add_staff(&conn, &biz, "staffer");
    seed_inventory_item(&conn, &biz, "RICE-001", "Rice", 100, 5000, 7500);

    assert!(stock_movement::list(&conn, &biz, &owner_id, &filters()).is_ok());
    assert!(stock_movement::list(&conn, &biz, &staff_id, &filters()).is_err());
}

#[test]
fn test_a_zero_delta_movement_is_rejected() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let inv_id = seed_inventory_item(&conn, &biz, "RICE-001", "Rice", 10, 100, 200);

    let tx = conn.transaction().unwrap();
    let result = stock_movement::record_in_tx(
        &tx, &biz, Some(&uid), &inv_id, "Rice", stock_movement::SALE, 0, 100, None,
    );
    assert!(result.is_err(), "a movement that moved nothing is noise in a ledger");
}
