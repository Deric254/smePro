use super::common::*;

#[test]
fn test_slow_movers_ranks_by_capital_tied_up_not_raw_staleness() {
    // The actionable question is "which unsold item is costing me the
    // most", not "which one has been sitting the longest" — a $2 item
    // untouched for a year matters far less than a $50,000 item
    // untouched for five weeks. Both are stale enough to qualify;
    // only their order should differ.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    seed_inventory_item(&conn, &biz, "SKU-CHEAP", "Cheap Trinket", 50, 4, 10);
    seed_inventory_item(&conn, &biz, "SKU-EXPENSIVE", "Industrial Motor", 10, 500_000, 700_000);

    let today = "2024-06-01";
    let results = crate::stock_health::slow_movers(&conn, &biz, &uid, today, 30, 20).unwrap();

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].item_name, "Industrial Motor", "far more capital tied up must rank first regardless of staleness");
    assert_eq!(results[0].value_at_risk_cents, 10 * 500_000);
    assert_eq!(results[1].item_name, "Cheap Trinket");
    assert_eq!(results[1].value_at_risk_cents, 50 * 4);
}

#[test]
fn test_slow_movers_excludes_items_sold_within_the_window() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let recently_sold = seed_inventory_item(&conn, &biz, "SKU-FRESH", "Fresh Mover", 20, 100, 200);
    seed_inventory_item(&conn, &biz, "SKU-STALE", "Stale Item", 20, 100, 200);

    let req = crate::pos::CheckoutRequest {
        idempotency_key: None,
        discount_pct: None,
        items: vec![crate::pos::CartItem { inventory_record_id: recently_sold, quantity: 1 }],
        payment_method: Some("Cash".into()),
        customer: None,
        customer_phone: None,
        allow_oversell: false,
        on_credit: false,
        due_date: None,
    };
    crate::pos::checkout(&mut conn, &biz, &uid, req).unwrap();

    let today = "2024-06-01";
    let results = crate::stock_health::slow_movers(&conn, &biz, &uid, today, 30, 20).unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].item_name, "Stale Item");
    assert_eq!(results[0].last_sale_at, None);
    assert_eq!(results[0].days_since_last_sale, None);
}

#[test]
fn test_slow_movers_stale_after_days_is_clamped() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    seed_inventory_item(&conn, &biz, "SKU-1", "Item", 5, 100, 200);

    // Out-of-range inputs must not panic or silently return nothing —
    // same clamp-not-reject rule receiving.rs and pos.rs already use
    // for caller-supplied numbers.
    let results = crate::stock_health::slow_movers(&conn, &biz, &uid, "2024-06-01", 0, 500).unwrap();
    assert_eq!(results.len(), 1);
}
