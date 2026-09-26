use super::common::*;
use serde_json::json;

fn checkout_one(conn: &mut rusqlite::Connection, biz: &str, uid: &str, inv_id: &str, qty: i64) -> serde_json::Value {
    let req = crate::pos::CheckoutRequest {
        idempotency_key: None,
        discount_pct: None,
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
    assert_eq!(summary.cost_bearing_sales_count, 2, "both sales went through checkout() and have real cost data");
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
    assert_eq!(summary.cost_bearing_sales_count, 0);
    assert!(summary.margin_pct.is_none(), "a margin percentage against zero revenue is undefined, not 0%");
}

#[test]
fn test_gross_profit_summary_counts_cost_bearing_sales_separately_from_total() {
    // THE ACTUAL FIX Deric asked for: a single business can easily
    // have a MIX of real-cost sales (through checkout()) and cost-
    // blind ones (hand-created, or predating this feature) — the old
    // boolean `has_cost_data` would say the exact same "true" whether
    // 1 of 50 sales had real cost data or all 50 did. This proves the
    // real count distinguishes them: 2 real sales plus 1 hand-created
    // one must report sales_count = 3, cost_bearing_sales_count = 2 —
    // not collapsed into a flag that hides how thin that 2-out-of-3
    // actually is.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = seed_inventory_item(&conn, &biz, "SALT-001", "Salt", 50, 500, 900);
    checkout_one(&mut conn, &biz, &uid, &inv_id, 2);
    checkout_one(&mut conn, &biz, &uid, &inv_id, 1);

    // Direct creation via crud::create is rejected for "sales" (every
    // sale now has to come from checkout/service-sale), so a
    // cost-blind row — the kind a restored pre-feature backup would
    // still contain — is seeded here via insert_validated_record, the
    // same system-insert path a restore would have used.
    let mut record = serde_json::Map::new();
    record.insert("item_name".into(), json!("Hand-entered sale"));
    record.insert("quantity".into(), json!(1));
    record.insert("revenue".into(), json!(2000));
    record.insert("cost_at_sale".into(), json!(0));
    record.insert("discount_amount".into(), json!(0));
    let module = crate::crud::load_module(&conn, &biz, "sales").unwrap();
    let record_map: std::collections::HashMap<String, serde_json::Value> = record.into_iter().collect();
    crate::crud::insert_validated_record(&conn, &biz, &module, &record_map).unwrap();

    let summary = crate::profit::summary(&conn, &biz, &uid).unwrap();
    assert_eq!(summary.sales_count, 3);
    assert_eq!(summary.cost_bearing_sales_count, 2, "exactly the 2 real checkouts, not the 1 cost-blind row");
}


#[test]
fn test_gross_profit_summary_includes_shrinkage_from_closed_stock_take() {
    // THE ACTUAL FIX for this session's gap #4: a stock take's
    // write-off must show up in the same profit report Sales' own
    // numbers do, as a distinct figure — not folded into cost_cents/
    // profit_cents (see profit.rs's own comment on why those two stay
    // Sales-only), and not silently absent either.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let rice_id = seed_inventory_item(&conn, &biz, "RICE-001", "Rice", 40, 500, 800);
    checkout_one(&mut conn, &biz, &uid, &rice_id, 5); // revenue 4000, cost 2500

    let before = crate::profit::summary(&conn, &biz, &uid).unwrap();
    assert_eq!(before.shrinkage_cents, 0);
    assert_eq!(before.profit_cents_after_shrinkage, before.profit_cents);

    let initiated = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    let stock_take_id = initiated["id"].as_str().unwrap().to_string();
    let item_id = initiated["items"][0]["id"].as_str().unwrap().to_string();
    // Expected 35 (40 - 5 sold), counted 30 -> shrinkage of 5 units at
    // the item's legacy cost of 500 cents = 2500 cents written off.
    crate::stock_take::record_count(
        &conn, &biz, &uid,
        crate::stock_take::RecordCountRequest { stock_take_id: stock_take_id.clone(), item_id, counted_qty: 30 },
    ).unwrap();
    crate::stock_take::close(&mut conn, &biz, &uid, &stock_take_id).unwrap();

    let after = crate::profit::summary(&conn, &biz, &uid).unwrap();
    assert_eq!(after.shrinkage_cents, 2500);
    assert_eq!(after.profit_cents, before.profit_cents, "Sales-only profit_cents must be untouched by shrinkage");
    assert_eq!(after.profit_cents_after_shrinkage, before.profit_cents - 2500);
    let expected_margin = (before.profit_cents - 2500) as f64 / after.revenue_cents as f64 * 100.0;
    assert!((after.margin_pct_after_shrinkage.unwrap() - expected_margin).abs() < 0.001);
}

#[test]
fn test_gross_profit_summary_excludes_shrinkage_from_a_cancelled_stock_take() {
    // Cancelling never wrote anything to inventory (see
    // stock_take.rs::cancel) — a count entered then abandoned must
    // never surface as a real cost here.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    seed_inventory_item(&conn, &biz, "RICE-001", "Rice", 40, 500, 800);

    let initiated = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    let stock_take_id = initiated["id"].as_str().unwrap().to_string();
    let item_id = initiated["items"][0]["id"].as_str().unwrap().to_string();
    crate::stock_take::record_count(
        &conn, &biz, &uid,
        crate::stock_take::RecordCountRequest { stock_take_id: stock_take_id.clone(), item_id, counted_qty: 10 },
    ).unwrap();
    crate::stock_take::cancel(&mut conn, &biz, &uid, &stock_take_id).unwrap();

    let summary = crate::profit::summary(&conn, &biz, &uid).unwrap();
    assert_eq!(summary.shrinkage_cents, 0, "a cancelled stock take must never contribute shrinkage");
}

#[test]
fn test_profit_by_item_includes_per_item_shrinkage() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let rice_id = seed_inventory_item(&conn, &biz, "RICE-001", "Rice", 40, 500, 800);
    let tea_id = seed_inventory_item(&conn, &biz, "TEA-001", "Tea", 50, 200, 350);
    checkout_one(&mut conn, &biz, &uid, &rice_id, 5);
    checkout_one(&mut conn, &biz, &uid, &tea_id, 3);

    let initiated = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    let stock_take_id = initiated["id"].as_str().unwrap().to_string();
    let rice_item_id = initiated["items"].as_array().unwrap().iter()
        .find(|i| i["inventory_record_id"].as_str().unwrap() == rice_id)
        .unwrap()["id"].as_str().unwrap().to_string();
    // Rice: expected 35, counted 30 -> 5 units shrinkage at 500 cents = 2500.
    // Tea is left uncounted (skipped) -> zero shrinkage for Tea.
    crate::stock_take::record_count(
        &conn, &biz, &uid,
        crate::stock_take::RecordCountRequest { stock_take_id: stock_take_id.clone(), item_id: rice_item_id, counted_qty: 30 },
    ).unwrap();
    crate::stock_take::close(&mut conn, &biz, &uid, &stock_take_id).unwrap();

    let by_item = crate::profit::by_item(&conn, &biz, &uid, 20).unwrap();
    let rice = by_item.iter().find(|i| i.item_name == "Rice").unwrap();
    let tea = by_item.iter().find(|i| i.item_name == "Tea").unwrap();
    assert_eq!(rice.shrinkage_cents, 2500);
    assert_eq!(rice.profit_cents_after_shrinkage, rice.profit_cents - 2500);
    assert_eq!(tea.shrinkage_cents, 0, "Tea was never counted in this stock take — no write-off to attribute to it");
    assert_eq!(tea.profit_cents_after_shrinkage, tea.profit_cents);
}

#[test]
fn test_gross_profit_summary_flags_missing_historical_cost_data() {
    // A sale with no real cost data behind it — the kind a restored
    // pre-feature backup would still contain, seeded here via
    // insert_validated_record since direct creation through
    // crud::create is now rejected for "sales" entirely — must still
    // have cost_bearing_sales_count reflect that honestly (0, not 1)
    // rather than implying a suspiciously perfect 100% margin is a
    // real result.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let mut record = serde_json::Map::new();
    record.insert("item_name".into(), json!("Hand-entered sale"));
    record.insert("quantity".into(), json!(1));
    record.insert("revenue".into(), json!(5000));
    record.insert("cost_at_sale".into(), json!(0));
    record.insert("discount_amount".into(), json!(0));
    let module = crate::crud::load_module(&conn, &biz, "sales").unwrap();
    let record_map: std::collections::HashMap<String, serde_json::Value> = record.into_iter().collect();
    crate::crud::insert_validated_record(&conn, &biz, &module, &record_map).unwrap();

    let summary = crate::profit::summary(&conn, &biz, &uid).unwrap();
    assert_eq!(summary.revenue_cents, 5000);
    assert_eq!(summary.cost_cents, 0);
    assert_eq!(summary.cost_bearing_sales_count, 0, "cost_at_sale forced to 0 on a hand-created sale must not be counted as real cost data");
}

/// Seeds one sales row directly (bypassing checkout entirely, same as
/// the legacy-data tests above) with an explicit `sale_date` — this
/// is testing by_item_trend's own windowing arithmetic on the Sales
/// table's shape, not checkout's, so controlling sale_date precisely
/// matters far more here than reproducing a real checkout.
fn seed_sale(conn: &rusqlite::Connection, biz: &str, item_name: &str, revenue: i64, cost_at_sale: i64, sale_date: &str) {
    let mut record = serde_json::Map::new();
    record.insert("item_name".into(), json!(item_name));
    record.insert("quantity".into(), json!(1));
    record.insert("revenue".into(), json!(revenue));
    record.insert("cost_at_sale".into(), json!(cost_at_sale));
    record.insert("discount_amount".into(), json!(0));
    record.insert("sale_date".into(), json!(sale_date));
    let module = crate::crud::load_module(conn, biz, "sales").unwrap();
    let record_map: std::collections::HashMap<String, serde_json::Value> = record.into_iter().collect();
    crate::crud::insert_validated_record(conn, biz, &module, &record_map).unwrap();
}

#[test]
fn test_by_item_trend_flags_a_currently_losing_item_and_its_decline() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let today = "2024-06-30"; // current window: 2024-06-01..2024-06-30; previous: 2024-05-02..2024-05-31

    // Widget used to be healthy (80% margin last period) and is now
    // losing money outright this period — exactly the item this
    // report exists to surface.
    seed_sale(&conn, &biz, "Widget", 1000, 1200, "2024-06-05");
    seed_sale(&conn, &biz, "Widget", 1000, 1300, "2024-06-15");
    seed_sale(&conn, &biz, "Widget", 1000, 200, "2024-05-10");

    // Gadget is healthy this period with no prior-period sales at
    // all — must show a real current margin but no fabricated trend.
    seed_sale(&conn, &biz, "Gadget", 1000, 200, "2024-06-20");

    // Old Thing only sold well outside both windows — must not appear
    // in this report at all, it has nothing current to trend.
    seed_sale(&conn, &biz, "Old Thing", 1000, 100, "2024-01-01");

    let results = crate::profit::by_item_trend(&conn, &biz, &uid, today, 30, 20).unwrap();

    assert!(results.iter().all(|r| r.item_name != "Old Thing"));

    let widget = results.iter().find(|r| r.item_name == "Widget").unwrap();
    assert_eq!(widget.current_revenue_cents, 2000);
    assert_eq!(widget.current_cost_cents, 2500);
    assert_eq!(widget.current_profit_cents, -500);
    assert_eq!(widget.current_sales_count, 2);
    assert!(widget.is_losing_money, "cost exceeded revenue this period on real cost data");
    assert_eq!(widget.previous_revenue_cents, 1000);
    assert_eq!(widget.previous_profit_cents, 800);
    let change = widget.margin_pct_change_pts.expect("both periods have revenue, a real change must be computed");
    assert!(change < 0.0, "margin fell from 80% to -25%, the change must be negative");

    let gadget = results.iter().find(|r| r.item_name == "Gadget").unwrap();
    assert_eq!(gadget.current_profit_cents, 800);
    assert!(!gadget.is_losing_money);
    assert_eq!(gadget.previous_revenue_cents, 0);
    assert_eq!(gadget.previous_margin_pct, None, "no prior-period sales means no real previous margin to report");
    assert_eq!(gadget.margin_pct_change_pts, None, "can't compute a real change with no prior-period side");

    // Biggest current loss ranks first.
    assert_eq!(results[0].item_name, "Widget");
}

#[test]
fn test_by_item_trend_period_days_is_clamped() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    // Same day as `today` so it falls inside even the smallest
    // possible clamped window (1 day).
    seed_sale(&conn, &biz, "Widget", 1000, 200, "2024-06-30");

    // Out-of-range inputs must not panic or silently error — same
    // clamp-not-reject rule as stock_health::slow_movers. 0 clamps up
    // to 1, 100000 clamps down to 180; neither should blow up the
    // date arithmetic or the query.
    let results = crate::profit::by_item_trend(&conn, &biz, &uid, "2024-06-30", 0, 500).unwrap();
    assert_eq!(results.len(), 1);
    let results = crate::profit::by_item_trend(&conn, &biz, &uid, "2024-06-30", 100_000, 500).unwrap();
    assert_eq!(results.len(), 1);
}
