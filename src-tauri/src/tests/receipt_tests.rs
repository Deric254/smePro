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
