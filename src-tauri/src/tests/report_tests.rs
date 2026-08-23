use super::common::*;

#[test]
fn test_category_report_does_not_error_on_a_null_grouping_field() {
    // A sale with no payment_method recorded (a completely normal,
    // valid state — see pos.rs, where the field is Option<String> and
    // simply omitted from the insert when the caller didn't supply
    // one) leaves that column NULL in the database. Grouping by it for
    // a "revenue by payment method" chart must not blow up the whole
    // report just because one row in the group has no value —
    // grouping ungrouped data has always been fine everywhere else in
    // SQL and should read back that way here too.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = seed_inventory_item(&conn, &biz, "NOPAY-001", "No Payment Method Item", 10, 100, 200);

    // Checkout with NO payment_method at all.
    let req = crate::pos::CheckoutRequest {
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id, quantity: 2 }],
        payment_method: None,
        customer: None,
        customer_phone: None,
        allow_oversell: false,
        on_credit: false,
        due_date: None,
    };
    crate::pos::checkout(&mut conn, &biz, &uid, req).unwrap();

    let result = crate::report::run(
        &conn,
        &biz,
        &uid,
        "sales",
        crate::report::ReportQuery {
            measure_field: Some("revenue"),
            aggregation: "sum",
            dimension: crate::report::Dimension::Category { field: "payment_method" },
            range_start: None,
            range_end: None,
        },
    );

    assert!(result.is_ok(), "category report must not error on a NULL grouping value: {result:?}");
    let points = result.unwrap();
    assert_eq!(points.len(), 1);
    assert_eq!(points[0].value, 400.0); // 2 * unit_price(200)
}

#[test]
fn test_category_report_groups_null_and_set_values_separately() {
    // Once NULL is handled, it must still be its OWN distinct group —
    // not silently merged into whichever named group happens to sort
    // next to it — so a "by payment method" chart shows an accurate
    // count for genuinely-unspecified sales alongside the real ones.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = seed_inventory_item(&conn, &biz, "MIX-001", "Mixed Payment Item", 20, 100, 200);

    let with_cash = crate::pos::CheckoutRequest {
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id.clone(), quantity: 1 }],
        payment_method: Some("Cash".into()),
        customer: None,
        customer_phone: None,
        allow_oversell: false,
        on_credit: false,
        due_date: None,
    };
    crate::pos::checkout(&mut conn, &biz, &uid, with_cash).unwrap();

    let without_method = crate::pos::CheckoutRequest {
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id, quantity: 1 }],
        payment_method: None,
        customer: None,
        customer_phone: None,
        allow_oversell: false,
        on_credit: false,
        due_date: None,
    };
    crate::pos::checkout(&mut conn, &biz, &uid, without_method).unwrap();

    let points = crate::report::run(
        &conn,
        &biz,
        &uid,
        "sales",
        crate::report::ReportQuery {
            measure_field: Some("revenue"),
            aggregation: "sum",
            dimension: crate::report::Dimension::Category { field: "payment_method" },
            range_start: None,
            range_end: None,
        },
    )
    .unwrap();

    assert_eq!(points.len(), 2, "NULL and 'Cash' must be two distinct groups, got: {points:?}");
    let labels: Vec<&str> = points.iter().map(|p| p.label.as_str()).collect();
    assert!(labels.contains(&"Cash"));
    // Whatever the fix names the NULL bucket, it must be a real,
    // non-empty label distinct from "Cash" — the exact string is an
    // implementation detail this test intentionally doesn't pin down,
    // beyond "it must not be the literal Rust default of an empty
    // String, which would print as a blank, mysterious chart slice".
    assert!(labels.iter().any(|l| *l != "Cash" && !l.is_empty()));
}
