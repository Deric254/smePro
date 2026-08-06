use super::common::*;

#[test]
fn test_receipt_generation() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let mut item = serde_json::Map::new();
    item.insert("sku".into(), serde_json::json!("MILK-001"));
    item.insert("name".into(), serde_json::json!("Milk"));
    item.insert("quantity".into(), serde_json::json!(20));
    item.insert("unit_cost".into(), serde_json::json!(40.0));
    item.insert("unit_price".into(), serde_json::json!(55.0));
    let inv_id = crate::crud::create(&mut conn, &biz, &uid, "inventory", &item).unwrap();

    let req = crate::pos::CheckoutRequest {
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id, quantity: 2 }],
        payment_method: Some("M-Pesa".into()),
        customer: Some("John Doe".into()),
        allow_oversell: false, on_credit: false, due_date: None,
    };
    let order = crate::pos::checkout(&mut conn, &biz, &uid, req).unwrap();
    let order_id = order.get("order_id").unwrap().as_str().unwrap();

    let receipt = crate::receipt::generate(&conn, &biz, &uid, order_id).unwrap();
    assert_eq!(receipt.items.len(), 1);
    assert_eq!(receipt.items[0].quantity, 2);
    assert_eq!(receipt.items[0].line_total, 110.0);
    assert_eq!(receipt.customer, Some("John Doe".into()));
    assert_eq!(receipt.payment_method, Some("M-Pesa".into()));
    assert_eq!(receipt.subtotal, 110.0);
}

#[test]
fn test_receipt_order_not_found() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let result = crate::receipt::generate(&conn, &biz, &uid, "fake-order-id");
    assert!(result.is_err());
}
