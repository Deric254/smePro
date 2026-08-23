//! Proves receipt::generate() actually carries a business's configured
//! logo and slogan through, end-to-end (branding save -> checkout ->
//! receipt) — this exact path had no test coverage before, even though
//! both receipt.rs and ReceiptView.tsx/InvoiceView.tsx already
//! referenced the fields. Reading the code isn't the same as proving
//! it: this test actually sets branding, runs a real checkout, and
//! asserts on the generated receipt's fields.

use super::common::*;
use base64::Engine as _;

const MINIMAL_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG magic bytes
    0, 0, 0, 0, // padding — enough bytes to pass the 8-byte minimum check
];

#[test]
fn test_receipt_carries_configured_logo_and_slogan() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let data_dir = std::env::temp_dir().join(format!("smepro_receipt_branding_test_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&data_dir).unwrap();
    let b64 = base64::engine::general_purpose::STANDARD.encode(MINIMAL_PNG);
    crate::business_branding::update_branding(&mut conn, &biz, Some(&b64), Some("Fresh daily"), &data_dir).unwrap();

    let inv_id = seed_inventory_item(&conn, &biz, "BREAD-001", "Loaf of Bread", 10, 100, 200);
    let req = crate::pos::CheckoutRequest {
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id, quantity: 1 }],
        payment_method: Some("Cash".into()),
        customer: None,
        customer_phone: None,
        allow_oversell: false,
        on_credit: false,
        due_date: None,
    };
    let order = crate::pos::checkout(&mut conn, &biz, &uid, req).unwrap();
    let order_id = order.get("order_id").unwrap().as_str().unwrap();

    let receipt = crate::receipt::generate(&conn, &biz, &uid, order_id).unwrap();
    assert_eq!(receipt.business_slogan.as_deref(), Some("Fresh daily"));
    assert!(receipt.business_logo_path.is_some(), "receipt must carry the configured logo path");
    assert!(receipt.business_logo_path.unwrap().ends_with(".png"));
}

#[test]
fn test_receipt_renders_cleanly_with_no_branding_configured() {
    // The opposite case: a business that never set a logo or slogan
    // must still generate a valid receipt — None, not an error, and
    // not a placeholder value standing in for "not set".
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = seed_inventory_item(&conn, &biz, "PLAIN-001", "Plain Item", 5, 50, 100);
    let req = crate::pos::CheckoutRequest {
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id, quantity: 1 }],
        payment_method: Some("Cash".into()),
        customer: None,
        customer_phone: None,
        allow_oversell: false,
        on_credit: false,
        due_date: None,
    };
    let order = crate::pos::checkout(&mut conn, &biz, &uid, req).unwrap();
    let order_id = order.get("order_id").unwrap().as_str().unwrap();

    let receipt = crate::receipt::generate(&conn, &biz, &uid, order_id).unwrap();
    assert_eq!(receipt.business_slogan, None);
    assert_eq!(receipt.business_logo_path, None);
}
