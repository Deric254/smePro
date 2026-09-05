use super::common::*;
use serde_json::json;

fn checkout_one(conn: &mut rusqlite::Connection, biz: &str, uid: &str, inv_id: &str, qty: i64) -> serde_json::Value {
    let req = crate::pos::CheckoutRequest {
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id.to_string(), quantity: qty }],
        payment_method: Some("Cash".into()),
        customer: None,
        customer_phone: None,
        allow_oversell: false,
        on_credit: false,
        due_date: None,
    };
    crate::pos::checkout(conn, biz, uid, req).unwrap()
}

#[test]
fn test_gross_profit_summary_matches_revenue_minus_cost_across_multiple_sales() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let rice_id = seed_inventory_item(&conn, &biz, "RICE-001", "Rice", 100, 5000, 7500);
    let tea_id = seed_inventory_item(&conn, &biz, "TEA-001", "Tea", 50, 2000, 3500);

    checkout_one(&mut conn, &biz, &uid, &rice_id, 5); // revenue 37500, cost 25000
    checkout_one(&mut conn, &biz, &uid, &tea_id, 3); // revenue 10500, cost 6000

    let summary = crate::profit::summary(&conn, &biz, &uid).unwrap();
    assert_eq!(summary.revenue_cents, 37500 + 10500);
    assert_eq!(summary.cost_cents, 25000 + 6000);
    assert_eq!(summary.profit_cents, (37500 + 10500) - (25000 + 6000));
    assert_eq!(summary.sales_count, 2);
    assert!(summary.has_cost_data);
    assert!(summary.margin_pct.is_some());
}

#[test]
fn test_gross_profit_summary_reflects_refund_reversal() {
    // The whole point of reversing cost_at_sale (and revenue) on
    // refund.rs's write path is that THIS query never needs to know
    // about refunds at all — it just sums what's on the sales table,
    // which is already correct by the time it gets here.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = seed_inventory_item(&conn, &biz, "FLOUR-003", "Flour", 50, 2000, 3000);
    let sale = checkout_one(&mut conn, &biz, &uid, &inv_id, 10);
    let sale_id = sale["items"][0]["sale_id"].as_str().unwrap().to_string();

    let before = crate::profit::summary(&conn, &biz, &uid).unwrap();
    assert_eq!(before.profit_cents, 3000 * 10 - 2000 * 10);

    let req = crate::refund::RefundRequest {
        sale_id,
        quantity: 4,
        refund_amount: 12000,
        reason: None,
        restock: true,
    };
    crate::refund::process_refund(&mut conn, &biz, &uid, req).unwrap();

    let after = crate::profit::summary(&conn, &biz, &uid).unwrap();
    let expected_revenue = 3000 * 10 - 12000;
    let expected_cost = 2000 * 10 - 2000 * 4;
    assert_eq!(after.revenue_cents, expected_revenue);
    assert_eq!(after.cost_cents, expected_cost);
    assert_eq!(after.profit_cents, expected_revenue - expected_cost);
}

#[test]
fn test_gross_profit_summary_with_no_sales_degrades_honestly() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let summary = crate::profit::summary(&conn, &biz, &uid).unwrap();
    assert_eq!(summary.revenue_cents, 0);
    assert_eq!(summary.cost_cents, 0);
    assert_eq!(summary.profit_cents, 0);
    assert_eq!(summary.sales_count, 0);
    assert!(!summary.has_cost_data);
    assert!(summary.margin_pct.is_none(), "a margin percentage against zero revenue is undefined, not 0%");
}

#[test]
fn test_gross_profit_summary_flags_missing_historical_cost_data() {
    // A sale hand-created directly (not through checkout()) has no
    // real cost data behind it — cost_at_sale is forced to 0 (see
    // crud.rs's own "if module_id == sales" block), and has_cost_data
    // must reflect that honestly rather than implying a suspiciously
    // perfect 100% margin is a real result.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let mut record = serde_json::Map::new();
    record.insert("item_name".into(), json!("Hand-entered sale"));
    record.insert("quantity".into(), json!(1));
    record.insert("revenue".into(), json!(5000));
    crate::crud::create(&conn, &biz, &uid, "sales", &record).unwrap();

    let summary = crate::profit::summary(&conn, &biz, &uid).unwrap();
    assert_eq!(summary.revenue_cents, 5000);
    assert_eq!(summary.cost_cents, 0);
    assert!(!summary.has_cost_data, "cost_at_sale forced to 0 on a hand-created sale must not be mistaken for a real zero-cost result");
}
