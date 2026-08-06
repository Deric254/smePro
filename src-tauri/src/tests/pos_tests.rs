use super::common::*;

#[test]
fn test_checkout_deducts_stock() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let mut item = serde_json::Map::new();
    item.insert("sku".into(), serde_json::json!("RICE-001"));
    item.insert("name".into(), serde_json::json!("Rice"));
    item.insert("quantity".into(), serde_json::json!(100));
    item.insert("unit_cost".into(), serde_json::json!(50.0));
    item.insert("unit_price".into(), serde_json::json!(75.0));
    let inv_id = crate::crud::create(&mut conn, &biz, &uid, "inventory", &item).unwrap();

    let req = crate::pos::CheckoutRequest {
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id.clone(), quantity: 5 }],
        payment_method: Some("Cash".into()),
        customer: None,
        allow_oversell: false,
        on_credit: false,
        due_date: None,
    };
    let result = crate::pos::checkout(&mut conn, &biz, &uid, req).unwrap();
    assert_eq!(result.get("subtotal").unwrap().as_f64().unwrap(), 375.0);

    let list = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    assert_eq!(list[0].get("quantity").unwrap().as_i64().unwrap(), 95);
}

#[test]
fn test_checkout_oversell_blocked() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let mut item = serde_json::Map::new();
    item.insert("sku".into(), serde_json::json!("SUGAR-001"));
    item.insert("name".into(), serde_json::json!("Sugar"));
    item.insert("quantity".into(), serde_json::json!(2));
    item.insert("unit_cost".into(), serde_json::json!(30.0));
    item.insert("unit_price".into(), serde_json::json!(50.0));
    let inv_id = crate::crud::create(&mut conn, &biz, &uid, "inventory", &item).unwrap();

    let req = crate::pos::CheckoutRequest {
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id, quantity: 5 }],
        payment_method: None, customer: None,
        allow_oversell: false, on_credit: false, due_date: None,
    };
    assert!(crate::pos::checkout(&mut conn, &biz, &uid, req).is_err());
}
