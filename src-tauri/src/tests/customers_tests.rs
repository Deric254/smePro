use super::common::*;
use crate::customers::normalize_phone;

#[test]
fn test_normalize_phone_strips_formatting_but_keeps_digits() {
    assert_eq!(normalize_phone("0712 345 678"), "0712345678");
    assert_eq!(normalize_phone("0712-345-678"), "0712345678");
    assert_eq!(normalize_phone("(0712) 345.678"), "0712345678");
    assert_eq!(normalize_phone("+254 712 345 678"), "+254712345678");
    assert_eq!(normalize_phone("  0712345678  "), "0712345678");
}

#[test]
fn test_find_or_create_does_not_duplicate_when_formatting_differs() {
    // The exact regression this exists to prevent: the same person's
    // phone typed two different ways across two visits must resolve
    // to ONE customer record, not two — otherwise their real lifetime
    // value silently splits across two rows.
    let mut conn = test_db();
    let biz = test_business(&mut conn);

    let id1 = crate::customers::find_or_create(&conn, &biz, Some("Asha"), Some("0712 345 678")).unwrap();
    let id2 = crate::customers::find_or_create(&conn, &biz, Some("Asha"), Some("0712-345-678")).unwrap();
    let id3 = crate::customers::find_or_create(&conn, &biz, Some("Asha"), Some("0712345678")).unwrap();

    assert_eq!(id1, id2);
    assert_eq!(id2, id3);

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM customers WHERE business_id = ?1", rusqlite::params![biz], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn test_find_or_create_still_separates_genuinely_different_numbers() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);

    let id1 = crate::customers::find_or_create(&conn, &biz, Some("Asha"), Some("0712345678")).unwrap();
    let id2 = crate::customers::find_or_create(&conn, &biz, Some("Brian"), Some("0798765432")).unwrap();
    assert_ne!(id1, id2);
}

#[test]
fn test_pos_checkout_customer_phone_matches_customers_table_for_ltv() {
    // Proves the join customers::list()/detail() rely on
    // (sales.customer_phone = customers.phone) actually holds when the
    // phone is typed with formatting at checkout — both sides go
    // through the identical normalization now.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = seed_inventory_item(&conn, &biz, "ITEM-001", "Widget", 10, 100, 500);

    let req = crate::pos::CheckoutRequest {
        idempotency_key: None,
        discount_pct: None,
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id, quantity: 2 }],
        payment_method: Some("Cash".into()),
        customer: Some("Asha".into()),
        customer_phone: Some("0712-345-678".into()), // formatted with dashes
        allow_oversell: false,
        on_credit: false,
        due_date: None,
    };
    crate::pos::checkout(&mut conn, &biz, &uid, req).unwrap();

    let list = crate::customers::list(&conn, &biz).unwrap();
    let customers = list["customers"].as_array().unwrap();
    assert_eq!(customers.len(), 1);
    assert_eq!(customers[0]["lifetime_value"].as_i64().unwrap(), 1000); // 2 * 500
    assert_eq!(customers[0]["order_count"].as_i64().unwrap(), 1);
}

#[test]
fn test_find_or_create_name_only_matches_case_insensitively() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);

    let id1 = crate::customers::find_or_create(&conn, &biz, Some("Asha"), None).unwrap();
    let id2 = crate::customers::find_or_create(&conn, &biz, Some("asha"), None).unwrap();
    let id3 = crate::customers::find_or_create(&conn, &biz, Some("ASHA"), None).unwrap();
    assert_eq!(id1, id2);
    assert_eq!(id2, id3);
}

#[test]
fn test_find_or_create_name_only_never_collides_with_phone_tracked_same_name() {
    // A phone-tracked "John" and a name-only "John" (different real
    // people, one of whom never gave a phone) must stay two separate
    // records — the partial unique index scopes name-dedup to
    // `WHERE phone IS NULL` specifically so this can't merge them.
    let mut conn = test_db();
    let biz = test_business(&mut conn);

    let phone_tracked = crate::customers::find_or_create(&conn, &biz, Some("John"), Some("0700111222")).unwrap();
    let name_only = crate::customers::find_or_create(&conn, &biz, Some("John"), None).unwrap();
    assert_ne!(phone_tracked, name_only);

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM customers WHERE business_id = ?1", rusqlite::params![biz], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn test_find_or_create_rejects_completely_anonymous() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let result = crate::customers::find_or_create(&conn, &biz, None, None);
    assert!(result.is_err());
}

#[test]
fn test_pos_checkout_tracks_customer_by_name_only() {
    // End-to-end through the real checkout path, not find_or_create
    // directly — proves a cashier who only gets a name (no phone
    // offered) still gets that customer tracked with a correct LTV,
    // which was the exact gap: before this, checkout only called
    // find_or_create when a phone was present at all.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = seed_inventory_item(&conn, &biz, "ITEM-002", "Gadget", 10, 100, 300);

    let req = crate::pos::CheckoutRequest {
        idempotency_key: None,
        discount_pct: None,
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id, quantity: 1 }],
        payment_method: Some("Cash".into()),
        customer: Some("Walk-in Dennis".into()),
        customer_phone: None, // no phone offered — name only
        allow_oversell: false,
        on_credit: false,
        due_date: None,
    };
    crate::pos::checkout(&mut conn, &biz, &uid, req).unwrap();

    let list = crate::customers::list(&conn, &biz).unwrap();
    let customers = list["customers"].as_array().unwrap();
    assert_eq!(customers.len(), 1);
    assert_eq!(customers[0]["name"].as_str().unwrap(), "Walk-in Dennis");
    assert!(customers[0]["phone"].is_null());
    assert_eq!(customers[0]["lifetime_value"].as_i64().unwrap(), 300);
}

#[test]
fn test_search_matches_by_partial_name_or_phone() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);

    crate::customers::find_or_create(&conn, &biz, Some("Asha Njoroge"), Some("0712345678")).unwrap();
    crate::customers::find_or_create(&conn, &biz, Some("Brian Otieno"), Some("0798765432")).unwrap();

    let by_name = crate::customers::search(&conn, &biz, "Asha").unwrap();
    assert_eq!(by_name.len(), 1);
    assert_eq!(by_name[0]["name"].as_str().unwrap(), "Asha Njoroge");

    let by_phone = crate::customers::search(&conn, &biz, "071234").unwrap();
    assert_eq!(by_phone.len(), 1);
    assert_eq!(by_phone[0]["name"].as_str().unwrap(), "Asha Njoroge");

    let by_partial_phone_formatted = crate::customers::search(&conn, &biz, "0712-345").unwrap();
    assert_eq!(by_partial_phone_formatted.len(), 1, "formatted partial phone search must still normalize before matching");
}

#[test]
fn test_search_by_name_only_does_not_leak_every_phoned_customer() {
    // Regression test for the exact bug caught during review:
    // normalize_phone("Asha") strips every character to "", and
    // `phone LIKE '%%'` would match EVERY customer who has any phone
    // at all if the phone clause weren't conditionally excluded.
    let mut conn = test_db();
    let biz = test_business(&mut conn);

    crate::customers::find_or_create(&conn, &biz, Some("Zainab"), Some("0700000001")).unwrap();
    crate::customers::find_or_create(&conn, &biz, Some("Kwame"), Some("0700000002")).unwrap();
    crate::customers::find_or_create(&conn, &biz, Some("Asha"), Some("0700000003")).unwrap();

    let results = crate::customers::search(&conn, &biz, "Asha").unwrap();
    assert_eq!(results.len(), 1, "a name search must not return every customer who merely has a phone number");
    assert_eq!(results[0]["name"].as_str().unwrap(), "Asha");
}

#[test]
fn test_search_empty_query_returns_nothing() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    crate::customers::find_or_create(&conn, &biz, Some("Asha"), Some("0712345678")).unwrap();

    let results = crate::customers::search(&conn, &biz, "").unwrap();
    assert!(results.is_empty());
    let results = crate::customers::search(&conn, &biz, "   ").unwrap();
    assert!(results.is_empty());
}

/// Checks out one item for the given customer and back-dates that
/// sale's created_at to `date` (YYYY-MM-DD) — checkout() itself always
/// stamps `datetime('now')`, so controlling the gap between purchases
/// for a deterministic test means moving it after the fact, the same
/// way a test would fake "3 batches received on 3 different days"
/// elsewhere in this suite.
fn checkout_for_customer_on(
    conn: &mut rusqlite::Connection,
    biz: &str,
    uid: &str,
    inv_id: &str,
    customer: &str,
    phone: &str,
    date: &str,
) {
    let req = crate::pos::CheckoutRequest {
        idempotency_key: None,
        discount_pct: None,
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id.to_string(), quantity: 1 }],
        payment_method: Some("Cash".into()),
        customer: Some(customer.to_string()),
        customer_phone: Some(phone.to_string()),
        allow_oversell: false,
        on_credit: false,
        due_date: None,
    };
    let result = crate::pos::checkout(conn, biz, uid, req).unwrap();
    let order_id = result["order_id"].as_str().unwrap();
    conn.execute(
        "UPDATE module_sales SET created_at = ?1 WHERE business_id = ?2 AND order_id = ?3",
        rusqlite::params![format!("{date} 12:00:00"), biz, order_id],
    )
    .unwrap();
}

#[test]
fn test_repeat_purchase_risk_flags_customer_overdue_relative_to_own_rhythm() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let inv_id = seed_inventory_item(&conn, &biz, "ITEM-001", "Widget", 100, 100, 500);

    // Asha buys like clockwork every 20 days, then goes quiet — by
    // "today" (2024-03-15) it has been 33 days since her last
    // purchase, well past her own 20-day rhythm × the 1.5 default.
    checkout_for_customer_on(&mut conn, &biz, &uid, &inv_id, "Asha", "0700000001", "2024-01-01");
    checkout_for_customer_on(&mut conn, &biz, &uid, &inv_id, "Asha", "0700000001", "2024-01-21");
    checkout_for_customer_on(&mut conn, &biz, &uid, &inv_id, "Asha", "0700000001", "2024-02-10");

    // Brian buys on the same 20-day rhythm and is still well within
    // it as of "today" — must NOT be flagged just for having a gap at
    // all.
    checkout_for_customer_on(&mut conn, &biz, &uid, &inv_id, "Brian", "0700000002", "2024-02-01");
    checkout_for_customer_on(&mut conn, &biz, &uid, &inv_id, "Brian", "0700000002", "2024-02-21");
    checkout_for_customer_on(&mut conn, &biz, &uid, &inv_id, "Brian", "0700000002", "2024-03-10");

    // Cynthia has only ever bought once — no rhythm exists to compare
    // against, so she must not appear in this report at all.
    checkout_for_customer_on(&mut conn, &biz, &uid, &inv_id, "Cynthia", "0700000003", "2024-03-01");

    let results = crate::customers::repeat_purchase_risk(&conn, &biz, &uid, "2024-03-15", 1.5).unwrap();

    assert_eq!(results.len(), 2, "only the two repeat customers, never the one-time buyer");
    let asha = results.iter().find(|r| r.name.as_deref() == Some("Asha")).unwrap();
    assert_eq!(asha.order_count, 3);
    assert!((asha.avg_days_between_purchases - 20.0).abs() < 0.01);
    assert_eq!(asha.days_since_last_purchase, 33);
    assert!(asha.at_risk, "33 days since last purchase is well past 20 × 1.5 = 30");

    let brian = results.iter().find(|r| r.name.as_deref() == Some("Brian")).unwrap();
    assert!(!brian.at_risk, "5 days since last purchase is nowhere near 20 × 1.5 = 30");

    assert!(results.iter().all(|r| r.name.as_deref() != Some("Cynthia")));
}
