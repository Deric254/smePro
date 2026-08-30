use super::common::*;

#[test]
fn test_receipt_generation() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = seed_inventory_item(&conn, &biz, "MILK-001", "Milk", 20, 4000, 5500);

    let req = crate::pos::CheckoutRequest {
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id, quantity: 2 }],
        payment_method: Some("M-Pesa".into()),
        customer: Some("John Doe".into()),
        customer_phone: None,
        allow_oversell: false, on_credit: false, due_date: None,
    };
    let order = crate::pos::checkout(&mut conn, &biz, &uid, req).unwrap();
    let order_id = order.get("order_id").unwrap().as_str().unwrap();

    let receipt = crate::receipt::generate(&conn, &biz, &uid, order_id).unwrap();
    assert_eq!(receipt.items.len(), 1);
    assert_eq!(receipt.items[0].quantity, 2);
    assert_eq!(receipt.items[0].line_total, 11000);
    assert_eq!(receipt.customer, Some("John Doe".into()));
    assert_eq!(receipt.payment_method, Some("M-Pesa".into()));
    assert_eq!(receipt.subtotal, 11000);
}

#[test]
fn test_receipt_order_not_found() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let result = crate::receipt::generate(&conn, &biz, &uid, "fake-order-id");
    assert!(result.is_err());
}

#[test]
fn test_receipt_stays_consistent_and_visible_after_refund() {
    // THE BUG THIS GUARDS AGAINST (see receipt.rs's own module doc
    // comment): refund.rs deducts the refunded amount from the sale
    // row's own `revenue` column so Bookkeeping/dashboard totals stay
    // honest. The receipt must never read that mutated column back as
    // its line total — it has to keep showing the sale exactly as it
    // was originally rung up (unit price × quantity, unaffected by any
    // refund), and separately, consistently disclose whatever's since
    // been refunded against each line — never silently shrinking the
    // printed total with no explanation.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    // Two different line items on the same order — so this also
    // proves a refund against ONE line is attributed to that line
    // only, never bleeding into the other, untouched line.
    let milk_id = seed_inventory_item(&conn, &biz, "MILK-002", "Milk", 20, 4000, 5500);
    let bread_id = seed_inventory_item(&conn, &biz, "BREAD-001", "Bread", 20, 2000, 3000);

    let checkout_req = crate::pos::CheckoutRequest {
        items: vec![
            crate::pos::CartItem { inventory_record_id: milk_id, quantity: 2 },
            crate::pos::CartItem { inventory_record_id: bread_id, quantity: 1 },
        ],
        payment_method: Some("Cash".into()),
        customer: None,
        customer_phone: None,
        allow_oversell: false, on_credit: false, due_date: None,
    };
    let order = crate::pos::checkout(&mut conn, &biz, &uid, checkout_req).unwrap();
    let order_id = order.get("order_id").unwrap().as_str().unwrap().to_string();
    let milk_sale_id = order["items"][0]["sale_id"].as_str().unwrap().to_string();

    // Sanity check: receipt matches the original sale before any refund.
    let before = crate::receipt::generate(&conn, &biz, &uid, &order_id).unwrap();
    assert_eq!(before.subtotal, 14000); // 2*5500 + 1*3000
    assert_eq!(before.total, before.subtotal); // no tax configured in test_business
    assert!(!before.is_refunded);
    assert_eq!(before.refunded_amount, 0);
    assert_eq!(before.net_total, before.total);

    let refund_req = crate::refund::RefundRequest {
        sale_id: milk_sale_id,
        quantity: 1,
        refund_amount: 5500,
        reason: Some("customer changed mind".into()),
        restock: false,
    };
    crate::refund::process_refund(&mut conn, &biz, &uid, refund_req).unwrap();

    let after = crate::receipt::generate(&conn, &biz, &uid, &order_id).unwrap();
    let milk_line = after.items.iter().find(|i| i.item_name == "Milk").unwrap();
    let bread_line = after.items.iter().find(|i| i.item_name == "Bread").unwrap();

    // The original sale, unchanged on BOTH lines — this is what "as
    // it is" means, refunded or not.
    assert_eq!(milk_line.quantity, 2);
    assert_eq!(milk_line.unit_price, 5500);
    assert_eq!(milk_line.line_total, 11000);
    assert_eq!(bread_line.line_total, 3000);
    assert_eq!(after.subtotal, 14000);
    assert_eq!(after.total, 14000);

    // The refund, attributed to the correct line only.
    assert_eq!(milk_line.quantity_refunded, 1);
    assert_eq!(milk_line.refunded_amount, 5500);
    assert_eq!(bread_line.quantity_refunded, 0);
    assert_eq!(bread_line.refunded_amount, 0);

    // ...and reported consistently, order-wide.
    assert!(after.is_refunded);
    assert_eq!(after.refunded_amount, 5500);
    assert_eq!(after.net_total, 8500); // 14000 - 5500
}
