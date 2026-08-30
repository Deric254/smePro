use super::common::*;

#[test]
fn test_checkout_deducts_stock() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = seed_inventory_item(&conn, &biz, "RICE-001", "Rice", 100, 5000, 7500);

    let req = crate::pos::CheckoutRequest {
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id.clone(), quantity: 5 }],
        payment_method: Some("Cash".into()),
        customer: None,
        customer_phone: None,
        allow_oversell: false,
        on_credit: false,
        due_date: None,
    };
    let result = crate::pos::checkout(&mut conn, &biz, &uid, req).unwrap();
    assert_eq!(result.get("subtotal").unwrap().as_i64().unwrap(), 37500);

    let list = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    assert_eq!(list[0].get("quantity").unwrap().as_i64().unwrap(), 95);
}

#[test]
fn test_checkout_auto_generates_invoice() {
    // THE BEHAVIOR THIS GUARDS: every completed order gets a real
    // invoice automatically (see invoice::create_invoice_for_order),
    // in the SAME transaction as the sale — this is the single most
    // important thing to verify about that feature, since it runs
    // unconditionally on every checkout for any business with the
    // Invoice module enabled. A bug in it would silently fail every
    // checkout for those businesses, not just invoice creation.
    let mut conn = test_db();
    let biz = test_business(&mut conn); // "retail" — includes "invoice"
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = seed_inventory_item(&conn, &biz, "TEA-001", "Tea", 50, 2000, 3500);
    let req = crate::pos::CheckoutRequest {
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id, quantity: 3 }],
        payment_method: Some("Cash".into()),
        customer: Some("Amina Yusuf".into()),
        customer_phone: None,
        allow_oversell: false, on_credit: false, due_date: None,
    };
    let order = crate::pos::checkout(&mut conn, &biz, &uid, req).unwrap();
    let order_id = order.get("order_id").unwrap().as_str().unwrap().to_string();

    let invoices = crate::crud::list(&conn, &biz, &uid, "invoice", None, 50, 0).unwrap();
    assert_eq!(invoices.len(), 1);
    let inv = &invoices[0];
    assert_eq!(inv["customer"].as_str().unwrap(), "Amina Yusuf");
    assert_eq!(inv["source_sale_id"].as_str().unwrap(), order_id);
    assert_eq!(inv["subtotal"].as_i64().unwrap(), 10500); // 3 * 3500
    assert_eq!(inv["total"].as_i64().unwrap(), 10500); // no tax configured
    // A cash sale is already paid — not left sitting in "draft"
    // waiting for someone to manually advance it.
    assert_eq!(inv["status"].as_str().unwrap(), "paid");
    assert!(inv["paid_at"].as_str().is_some());
    assert!(inv["invoice_number"].as_str().unwrap().starts_with("INV-"));
}

#[test]
fn test_checkout_on_credit_invoice_is_sent_not_paid() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = seed_inventory_item(&conn, &biz, "RICE-001", "Rice", 50, 2000, 3500);
    let req = crate::pos::CheckoutRequest {
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id, quantity: 2 }],
        payment_method: None,
        customer: Some("Kofi Mensah".into()),
        customer_phone: None,
        allow_oversell: false, on_credit: true, due_date: Some("2026-12-31".into()),
    };
    crate::pos::checkout(&mut conn, &biz, &uid, req).unwrap();

    let invoices = crate::crud::list(&conn, &biz, &uid, "invoice", None, 50, 0).unwrap();
    assert_eq!(invoices.len(), 1);
    // Genuinely owed, not yet paid — and due whenever the credit sale
    // itself said, not defaulted to "today" the way a cash sale is.
    assert_eq!(invoices[0]["status"].as_str().unwrap(), "sent");
    assert_eq!(invoices[0]["due_date"].as_str().unwrap(), "2026-12-31");
    assert!(invoices[0]["paid_at"].is_null());
}

#[test]
fn test_checkout_walk_in_customer_still_gets_invoice() {
    // No customer name at all (a plain walk-in cash sale) — `customer`
    // is a required field on the Invoice module, so this has to fall
    // back to a real placeholder rather than either crashing the
    // checkout or silently skipping the invoice.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = seed_inventory_item(&conn, &biz, "SOAP-001", "Soap", 50, 500, 1000);
    let req = crate::pos::CheckoutRequest {
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id, quantity: 1 }],
        payment_method: Some("Cash".into()),
        customer: None,
        customer_phone: None,
        allow_oversell: false, on_credit: false, due_date: None,
    };
    crate::pos::checkout(&mut conn, &biz, &uid, req).unwrap();

    let invoices = crate::crud::list(&conn, &biz, &uid, "invoice", None, 50, 0).unwrap();
    assert_eq!(invoices.len(), 1);
    assert_eq!(invoices[0]["customer"].as_str().unwrap(), "Walk-in customer");
}

#[test]
fn test_two_checkouts_get_distinct_invoice_numbers() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = seed_inventory_item(&conn, &biz, "EGGS-001", "Eggs", 50, 1000, 1500);
    for _ in 0..2 {
        let req = crate::pos::CheckoutRequest {
            items: vec![crate::pos::CartItem { inventory_record_id: inv_id.clone(), quantity: 1 }],
            payment_method: Some("Cash".into()), customer: None, customer_phone: None,
            allow_oversell: false, on_credit: false, due_date: None,
        };
        crate::pos::checkout(&mut conn, &biz, &uid, req).unwrap();
    }

    let invoices = crate::crud::list(&conn, &biz, &uid, "invoice", None, 50, 0).unwrap();
    assert_eq!(invoices.len(), 2);
    let numbers: std::collections::HashSet<&str> =
        invoices.iter().map(|i| i["invoice_number"].as_str().unwrap()).collect();
    assert_eq!(numbers.len(), 2, "each order must get its own distinct invoice number");
}

#[test]
fn test_checkout_oversell_blocked() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = seed_inventory_item(&conn, &biz, "SUGAR-001", "Sugar", 2, 3000, 5000);

    let req = crate::pos::CheckoutRequest {
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id, quantity: 5 }],
        payment_method: None, customer: None, customer_phone: None,
        allow_oversell: false, on_credit: false, due_date: None,
    };
    assert!(crate::pos::checkout(&mut conn, &biz, &uid, req).is_err());
}
