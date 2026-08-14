use super::common::*;
use crate::pos::{ServiceLine, ServiceSaleRequest};

#[test]
fn test_service_sale_creates_customer_record_with_correct_ltv() {
    // The exact gap this exists to close: before create_service_sale,
    // ServiceSale.tsx wrote sales rows with a customer_phone field but
    // never called customers::find_or_create — a service business's
    // repeat customers never appeared in the Customers list at all.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let req = ServiceSaleRequest {
        lines: vec![
            ServiceLine { description: "Consultation".into(), unit_price: 5000, quantity: 1 },
            ServiceLine { description: "Follow-up call".into(), unit_price: 2000, quantity: 2 },
        ],
        payment_method: Some("Cash".into()),
        customer: Some("Brian".into()),
        customer_phone: Some("0798 765 432".into()),
    };
    let summary = crate::pos::create_service_sale(&mut conn, &biz, &uid, req).unwrap();
    assert_eq!(summary["subtotal"].as_i64().unwrap(), 5000 + 2000 * 2);

    let list = crate::customers::list(&conn, &biz).unwrap();
    let customers = list["customers"].as_array().unwrap();
    assert_eq!(customers.len(), 1);
    assert_eq!(customers[0]["name"].as_str().unwrap(), "Brian");
    assert_eq!(customers[0]["lifetime_value"].as_i64().unwrap(), 9000);
}

#[test]
fn test_service_sale_without_phone_records_no_customer() {
    // Anonymous service sales must stay anonymous — same as goods
    // checkout, a phone is opt-in, not required.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let req = ServiceSaleRequest {
        lines: vec![ServiceLine { description: "Walk-in repair".into(), unit_price: 1000, quantity: 1 }],
        payment_method: Some("Cash".into()),
        customer: None,
        customer_phone: None,
    };
    crate::pos::create_service_sale(&mut conn, &biz, &uid, req).unwrap();

    let list = crate::customers::list(&conn, &biz).unwrap();
    assert!(list["customers"].as_array().unwrap().is_empty());
}

#[test]
fn test_service_sale_rejects_invalid_line_and_commits_nothing() {
    // All-or-nothing: a bad line anywhere in the request must leave
    // ZERO sales rows behind, not the valid lines that came before it
    // in the array.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let req = ServiceSaleRequest {
        lines: vec![
            ServiceLine { description: "Good line".into(), unit_price: 1000, quantity: 1 },
            ServiceLine { description: "".into(), unit_price: 500, quantity: 1 }, // empty description — invalid
        ],
        payment_method: Some("Cash".into()),
        customer: None,
        customer_phone: None,
    };
    let result = crate::pos::create_service_sale(&mut conn, &biz, &uid, req);
    assert!(result.is_err());

    let records = crate::crud::list(&conn, &biz, &uid, "sales", None, 50, 0).unwrap();
    assert_eq!(records.len(), 0, "a rejected service sale must leave no partial rows behind");
}

#[test]
fn test_service_sale_multiple_visits_accumulate_same_customer_ltv() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    for phone in ["0700111222", "0700-111-222", "0700 111 222"] {
        let req = ServiceSaleRequest {
            lines: vec![ServiceLine { description: "Session".into(), unit_price: 1000, quantity: 1 }],
            payment_method: Some("Cash".into()),
            customer: Some("Cynthia".into()),
            customer_phone: Some(phone.to_string()),
        };
        crate::pos::create_service_sale(&mut conn, &biz, &uid, req).unwrap();
    }

    let list = crate::customers::list(&conn, &biz).unwrap();
    let customers = list["customers"].as_array().unwrap();
    assert_eq!(customers.len(), 1, "differently-formatted visits must still be one customer");
    assert_eq!(customers[0]["lifetime_value"].as_i64().unwrap(), 3000);
    assert_eq!(customers[0]["order_count"].as_i64().unwrap(), 3);
}
