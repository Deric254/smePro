use super::common::*;

fn make_inventory_item(conn: &rusqlite::Connection, biz: &str, sku: &str, name: &str, qty: i64, cost_cents: i64, price_cents: i64) -> String {
    // Delegates to seed_inventory_item() — crud::create() now forces
    // inventory to start at zero stock, so a test needing pre-existing
    // stock to refund against has to seed it like a real bulk catalog
    // migration would.
    seed_inventory_item(conn, biz, sku, name, qty, cost_cents, price_cents)
}

fn checkout_one(conn: &mut rusqlite::Connection, biz: &str, uid: &str, inv_id: &str, qty: i64) -> serde_json::Value {
    let req = crate::pos::CheckoutRequest {
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id.to_string(), quantity: qty }],
        payment_method: Some("Cash".into()),
        customer: None,
        customer_phone: None,
        allow_oversell: false,
        on_credit: false,
        due_date: None,
    };
    crate::pos::checkout(conn, biz, uid, req).unwrap()
}

#[test]
fn test_refund_restocks_and_records_correctly() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = make_inventory_item(&conn, &biz, "FLOUR-001", "Flour", 50, 2000, 3000);
    let sale = checkout_one(&mut conn, &biz, &uid, &inv_id, 10);
    let sale_id = sale["items"][0]["sale_id"].as_str().unwrap().to_string();

    let list_before = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    assert_eq!(list_before[0]["quantity"].as_i64().unwrap(), 40);

    let req = crate::refund::RefundRequest {
        sale_id: sale_id.clone(),
        quantity: 4,
        refund_amount: 12000,
        reason: Some("customer changed mind".into()),
        restock: true,
    };
    let result = crate::refund::process_refund(&mut conn, &biz, &uid, req).unwrap();
    assert_eq!(result["quantity_refunded"].as_i64().unwrap(), 4);
    assert_eq!(result["new_stock_level"].as_i64().unwrap(), 44);

    let list_after = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    assert_eq!(list_after[0]["quantity"].as_i64().unwrap(), 44);
}

#[test]
fn test_refund_without_restock_leaves_inventory_untouched() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = make_inventory_item(&conn, &biz, "EGGS-001", "Eggs", 30, 500, 800);
    let sale = checkout_one(&mut conn, &biz, &uid, &inv_id, 6);
    let sale_id = sale["items"][0]["sale_id"].as_str().unwrap().to_string();

    let req = crate::refund::RefundRequest {
        sale_id,
        quantity: 6,
        refund_amount: 4800,
        reason: Some("broken on arrival, not sellable".into()),
        restock: false,
    };
    crate::refund::process_refund(&mut conn, &biz, &uid, req).unwrap();

    // Stock stays at 24 (30 - 6 sold), never goes back up -- a
    // deliberately unsellable return must not silently become stock.
    let list = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    assert_eq!(list[0]["quantity"].as_i64().unwrap(), 24);
}

#[test]
fn test_refunding_more_than_was_sold_is_blocked() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = make_inventory_item(&conn, &biz, "OIL-001", "Cooking Oil", 20, 10000, 15000);
    let sale = checkout_one(&mut conn, &biz, &uid, &inv_id, 3);
    let sale_id = sale["items"][0]["sale_id"].as_str().unwrap().to_string();

    let req = crate::refund::RefundRequest {
        sale_id,
        quantity: 5, // more than the 3 actually sold
        refund_amount: 75000,
        reason: None,
        restock: true,
    };
    let result = crate::refund::process_refund(&mut conn, &biz, &uid, req);
    assert!(result.is_err());

    // And stock must be completely unaffected by the rejected attempt.
    let list = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    assert_eq!(list[0]["quantity"].as_i64().unwrap(), 17); // 20 - 3, refund never happened
}

#[test]
fn test_repeated_partial_refunds_correctly_accumulate() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = make_inventory_item(&conn, &biz, "SOAP-001", "Bar Soap", 100, 1000, 1500);
    let sale = checkout_one(&mut conn, &biz, &uid, &inv_id, 10);
    let sale_id = sale["items"][0]["sale_id"].as_str().unwrap().to_string();

    // First partial refund: 4 of the 10 -- must succeed.
    let first = crate::refund::RefundRequest {
        sale_id: sale_id.clone(),
        quantity: 4,
        refund_amount: 6000,
        reason: None,
        restock: true,
    };
    crate::refund::process_refund(&mut conn, &biz, &uid, first).unwrap();

    // Second refund against the SAME sale: only 6 remains refundable
    // (10 - 4 already refunded). Asking for 7 must be rejected --
    // proving the check is a real running total, not resettable.
    let second_too_much = crate::refund::RefundRequest {
        sale_id: sale_id.clone(),
        quantity: 7,
        refund_amount: 10500,
        reason: None,
        restock: true,
    };
    assert!(crate::refund::process_refund(&mut conn, &biz, &uid, second_too_much).is_err());

    // Exactly the remaining 6 must succeed.
    let second_exact = crate::refund::RefundRequest {
        sale_id: sale_id.clone(),
        quantity: 6,
        refund_amount: 9000,
        reason: None,
        restock: true,
    };
    crate::refund::process_refund(&mut conn, &biz, &uid, second_exact).unwrap();

    // Now fully refunded -- even quantity 1 more must be rejected.
    let third = crate::refund::RefundRequest {
        sale_id,
        quantity: 1,
        refund_amount: 1500,
        reason: None,
        restock: true,
    };
    assert!(crate::refund::process_refund(&mut conn, &biz, &uid, third).is_err());

    // Stock: 100 - 10 sold + 4 + 6 restocked = 100.
    let list = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    assert_eq!(list[0]["quantity"].as_i64().unwrap(), 100);
}

#[test]
fn test_refund_requires_a_real_sale() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let req = crate::refund::RefundRequest {
        sale_id: "not-a-real-sale-id".into(),
        quantity: 1,
        refund_amount: 1000,
        reason: None,
        restock: false,
    };
    assert!(crate::refund::process_refund(&mut conn, &biz, &uid, req).is_err());
}
