use super::common::*;
use serde_json::json;

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

#[test]
fn test_restocked_refund_reverses_cost_proportionally() {
    // THE ACTUAL FIX Deric asked for: a restocked refund must reverse
    // BOTH revenue and cost, proportional to how much of the original
    // sale is being returned — the item is back on the shelf, so this
    // portion of the sale should net to zero profit impact, exactly
    // as if it never happened.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = make_inventory_item(&conn, &biz, "FLOUR-002", "Flour", 50, 2000, 3000);
    let sale = checkout_one(&mut conn, &biz, &uid, &inv_id, 10);
    let sale_id = sale["items"][0]["sale_id"].as_str().unwrap().to_string();

    let sales_before = crate::crud::list(&conn, &biz, &uid, "sales", None, 50, 0).unwrap();
    let before = sales_before.iter().find(|r| r["id"] == json!(sale_id)).unwrap();
    assert_eq!(before["cost_at_sale"], json!(2000 * 10));

    let req = crate::refund::RefundRequest {
        sale_id: sale_id.clone(),
        quantity: 4, // 4 of the 10
        refund_amount: 12000,
        reason: Some("customer changed mind".into()),
        restock: true,
    };
    let result = crate::refund::process_refund(&mut conn, &biz, &uid, req).unwrap();
    assert_eq!(result["cost_reversed"], json!(2000 * 4));

    let sales_after = crate::crud::list(&conn, &biz, &uid, "sales", None, 50, 0).unwrap();
    let after = sales_after.iter().find(|r| r["id"] == json!(sale_id)).unwrap();
    assert_eq!(after["revenue"], json!(3000 * 10 - 12000));
    assert_eq!(after["cost_at_sale"], json!(2000 * 10 - 2000 * 4), "cost must be reversed proportionally, same as revenue");

    let refunds = crate::crud::list(&conn, &biz, &uid, "refunds", None, 50, 0).unwrap();
    assert_eq!(refunds[0]["cost_reversed"], json!(2000 * 4));
}

#[test]
fn test_non_restocked_refund_leaves_cost_intact_showing_a_real_loss() {
    // THE ACTUAL FIX Deric asked for, the other half: a refund where
    // the item does NOT come back (damaged, expired, given away) must
    // NOT reverse cost — the business already paid for that unit and
    // it's gone. Reversing revenue but not cost is what makes that
    // sale correctly show as a real loss in gross profit, rather than
    // a neutral non-event it wasn't.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = make_inventory_item(&conn, &biz, "EGGS-002", "Eggs", 30, 500, 800);
    let sale = checkout_one(&mut conn, &biz, &uid, &inv_id, 6);
    let sale_id = sale["items"][0]["sale_id"].as_str().unwrap().to_string();

    let req = crate::refund::RefundRequest {
        sale_id: sale_id.clone(),
        quantity: 6,
        refund_amount: 4800,
        reason: Some("broken on arrival, not sellable".into()),
        restock: false,
    };
    let result = crate::refund::process_refund(&mut conn, &biz, &uid, req).unwrap();
    assert_eq!(result["cost_reversed"], json!(0), "no restock means no cost reversal — the cost was really incurred");

    let sales_after = crate::crud::list(&conn, &biz, &uid, "sales", None, 50, 0).unwrap();
    let after = sales_after.iter().find(|r| r["id"] == json!(sale_id)).unwrap();
    assert_eq!(after["revenue"], json!(0), "the money was handed back");
    assert_eq!(after["cost_at_sale"], json!(500 * 6), "the cost stays — this sale is now a real loss, not neutralized");
}

#[test]
fn test_repeated_partial_refunds_reverse_cost_with_no_coin_lost_or_gained() {
    // "Must not lose any coin" — refunding the same sale across
    // multiple partial refunds must reverse cost_at_sale down to
    // EXACTLY 0 by the time the sale is fully refunded, and the sum
    // of every refund's own `cost_reversed` must exactly equal the
    // original `cost_at_sale` — never a cent short, never a cent over.
    // See refund.rs's own doc comment on why this is computed as a
    // running remainder rather than fresh proportional math each time.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = make_inventory_item(&conn, &biz, "SOAP-002", "Bar Soap", 100, 1000, 1500);
    let sale = checkout_one(&mut conn, &biz, &uid, &inv_id, 3);
    let sale_id = sale["items"][0]["sale_id"].as_str().unwrap().to_string();
    let original_cost = 1000 * 3;

    let mut total_reversed = 0i64;
    for _ in 0..3 {
        let req = crate::refund::RefundRequest {
            sale_id: sale_id.clone(),
            quantity: 1,
            refund_amount: 1500,
            reason: None,
            restock: true,
        };
        let result = crate::refund::process_refund(&mut conn, &biz, &uid, req).unwrap();
        total_reversed += result["cost_reversed"].as_i64().unwrap();
    }

    assert_eq!(total_reversed, original_cost, "the three refunds together must reverse exactly the original cost, no more, no less");

    let sales_after = crate::crud::list(&conn, &biz, &uid, "sales", None, 50, 0).unwrap();
    let after = sales_after.iter().find(|r| r["id"] == json!(sale_id)).unwrap();
    assert_eq!(after["cost_at_sale"], json!(0), "fully refunded sale must land at exactly 0 cost remaining, not a stray coin either way");
}

