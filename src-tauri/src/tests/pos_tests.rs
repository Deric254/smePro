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
