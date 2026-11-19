use super::common::*;
use crate::intelligence::{report, Insight, IntelligenceReport};
use serde_json::{json, Value};
use std::collections::HashMap;

// today = 2024-06-30, days = 30:
//   current   2024-06-01 ..= 2024-06-30
//   previous  2024-05-02 ..= 2024-05-31
//   before    2024-04-02 ..= 2024-05-01
const TODAY: &str = "2024-06-30";

fn put(conn: &rusqlite::Connection, biz: &str, module: &str, fields: &[(&str, Value)]) {
    let m = crate::crud::load_module(conn, biz, module).unwrap();
    let rec: HashMap<String, Value> = fields.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
    crate::crud::insert_validated_record(conn, biz, &m, &rec).unwrap();
}

fn sale(conn: &rusqlite::Connection, biz: &str, item: &str, customer: Option<&str>, revenue: i64, cost: i64, date: &str) {
    let mut f = vec![
        ("item_name", json!(item)), ("quantity", json!(1)), ("revenue", json!(revenue)),
        ("cost_at_sale", json!(cost)), ("discount_amount", json!(0)), ("sale_date", json!(date)),
    ];
    if let Some(c) = customer { f.push(("customer", json!(c))); }
    put(conn, biz, "sales", &f);
}

fn entry_described(conn: &rusqlite::Connection, biz: &str, kind: &str, category: &str, description: &str, amount: i64, date: &str) {
    put(conn, biz, "accounting", &[
        ("description", json!(description)), ("entry_type", json!(kind)),
        ("category", json!(category)), ("amount", json!(amount)), ("transaction_date", json!(date)),
    ]);
}

/// An entry typed in by a person.
fn entry(conn: &rusqlite::Connection, biz: &str, kind: &str, category: &str, amount: i64, date: &str) {
    entry_described(conn, biz, kind, category, &format!("{category} entry"), amount, date);
}

fn setup() -> (rusqlite::Connection, String, String) {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    (conn, biz, uid)
}

fn find<'a>(r: &'a IntelligenceReport, area: &str, kind: &str) -> Option<&'a Insight> {
    r.areas.iter().find(|a| a.id == area).and_then(|a| a.insights.iter().find(|i| i.kind == kind))
}

fn kpi_value(r: &IntelligenceReport, area: &str, label: &str) -> Option<f64> {
    r.areas.iter().find(|a| a.id == area).and_then(|a| a.kpis.iter().find(|k| k.label == label)).and_then(|k| k.value)
}

#[test]
fn empty_business_reports_all_eight_areas_and_invents_nothing() {
    let (conn, biz, uid) = setup();
    let r = report(&conn, &biz, &uid, TODAY, 30).unwrap();
    let ids: Vec<&str> = r.areas.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["sales", "profitability", "inventory", "customers", "expenses", "cash_flow", "performance", "growth"]);
    for id in ["sales", "profitability", "customers", "performance", "growth"] {
        let a = r.areas.iter().find(|a| a.id == id).unwrap();
        assert!(a.insights.is_empty(), "{id} must not produce findings from no data");
    }
    assert_eq!(kpi_value(&r, "sales", "Revenue"), Some(0.0));
    assert_eq!(r.period.start, "2024-06-01");
    assert_eq!(r.period.end, "2024-06-30");
    assert_eq!(r.previous_period.start, "2024-05-02");
    assert_eq!(r.previous_period.end, "2024-05-31");
}

#[test]
fn sales_drop_is_flagged_with_exact_impact_and_driver() {
    let (conn, biz, uid) = setup();
    // previous period: Widget 12 x 1000 (the 05-02 sale proves history covers the whole period)
    sale(&conn, &biz, "Widget", None, 1000, 400, "2024-05-02");
    for d in 3..=13 { sale(&conn, &biz, "Widget", None, 1000, 400, &format!("2024-05-{d:02}")); }
    // current period: Widget 5 x 1000, Gadget 2 x 500
    for d in 1..=5 { sale(&conn, &biz, "Widget", None, 1000, 400, &format!("2024-06-{d:02}")); }
    sale(&conn, &biz, "Gadget", None, 500, 200, "2024-06-10");
    sale(&conn, &biz, "Gadget", None, 500, 200, "2024-06-11");

    let r = report(&conn, &biz, &uid, TODAY, 30).unwrap();
    let i = find(&r, "sales", "sales_change").expect("a 50% drop must be flagged");
    assert_eq!(i.severity, "problem");
    assert_eq!(i.impact.as_ref().unwrap().amount_cents, 6000 - 12000);
    let cmp = i.comparison.as_ref().unwrap();
    assert_eq!(cmp.current, 6000.0);
    assert_eq!(cmp.previous, 12000.0);
    assert_eq!(cmp.change_pct, Some(-50.0));
    assert!(i.evidence.iter().any(|e| e.label.contains("Widget")), "the product behind the drop must be named");
    assert!(!i.recommended_action.is_empty() && !i.why_it_matters.is_empty());
}

#[test]
fn short_history_gives_no_comparison_and_says_why() {
    let (conn, biz, uid) = setup();
    for d in 10..=20 { sale(&conn, &biz, "Widget", None, 1000, 400, &format!("2024-06-{d:02}")); }
    let r = report(&conn, &biz, &uid, TODAY, 30).unwrap();
    assert!(find(&r, "sales", "sales_change").is_none());
    let a = r.areas.iter().find(|a| a.id == "sales").unwrap();
    assert_eq!(a.kpis[0].previous, None, "no previous figure may be invented");
    assert!(a.notes.iter().any(|n| n.contains("day(s) of sales history")));
    assert!(r.areas.iter().find(|a| a.id == "performance").unwrap().insights.is_empty());
}

#[test]
fn profitability_flags_missing_costs_and_items_sold_below_cost() {
    let (conn, biz, uid) = setup();
    for d in 1..=3 { sale(&conn, &biz, "Widget", None, 1000, 1500, &format!("2024-06-{d:02}")); } // loses 500 each
    for d in 4..=6 { sale(&conn, &biz, "Gadget", None, 1000, 0, &format!("2024-06-{d:02}")); } // no cost recorded
    let r = report(&conn, &biz, &uid, TODAY, 30).unwrap();

    let gap = find(&r, "profitability", "cost_data_gap").unwrap();
    assert_eq!(gap.impact.as_ref().unwrap().amount_cents, 3000, "revenue with no recorded cost");
    assert_eq!(kpi_value(&r, "profitability", "Cost data coverage"), Some(50.0));

    let loss = find(&r, "profitability", "loss_making_items").unwrap();
    assert_eq!(loss.severity, "problem");
    assert_eq!(loss.impact.as_ref().unwrap().amount_cents, -1500);
    assert!(loss.evidence.iter().any(|e| e.label == "Widget"));
    assert!(!loss.evidence.iter().any(|e| e.label == "Gadget"), "an item with no cost data can't be called a loss");
}

#[test]
fn expenses_separate_operating_costs_from_system_postings_and_cash_flow_uses_all() {
    let (conn, biz, uid) = setup();
    entry(&conn, &biz, "income", "Sales", 20000, "2024-06-05");
    entry(&conn, &biz, "expense", "Rent", 1000, "2024-06-06");
    entry_described(&conn, &biz, "expense", "Purchasing", "Purchase received — PO-7", 9000, "2024-06-07");   // posted by smePro itself
    entry_described(&conn, &biz, "expense", "Refunds", "Refund — SALE-3", 500, "2024-06-08");                // posted by smePro itself
    entry(&conn, &biz, "expense", "Purchasing", 300, "2024-06-09");                                           // typed by a person: a real operating expense
    let r = report(&conn, &biz, &uid, TODAY, 30).unwrap();
    assert_eq!(kpi_value(&r, "expenses", "Operating expenses"), Some(1300.0), "rent 1000 + the typed 'Purchasing' entry 300");
    assert_eq!(kpi_value(&r, "expenses", "Refunds paid out"), Some(500.0));
    assert_eq!(kpi_value(&r, "expenses", "Supplier purchases paid"), Some(9000.0));
    assert_eq!(kpi_value(&r, "cash_flow", "Cash in"), Some(20000.0));
    assert_eq!(kpi_value(&r, "cash_flow", "Cash out"), Some(10800.0));
    assert_eq!(kpi_value(&r, "cash_flow", "Net cash flow"), Some(9200.0));
    let cash = r.areas.iter().find(|a| a.id == "cash_flow").unwrap();
    assert_eq!(cash.status, "partial");
    assert!(cash.missing.iter().any(|m| m.contains("Opening cash")), "the missing balance must be stated, not guessed");
}

#[test]
fn negative_net_cash_is_flagged() {
    let (conn, biz, uid) = setup();
    entry(&conn, &biz, "income", "Sales", 1000, "2024-06-05");
    entry(&conn, &biz, "expense", "Rent", 4000, "2024-06-06");
    entry(&conn, &biz, "expense", "Wages", 2000, "2024-06-07");
    let r = report(&conn, &biz, &uid, TODAY, 30).unwrap();
    let i = find(&r, "cash_flow", "cash_flow_trend").unwrap();
    assert_eq!(i.impact.as_ref().unwrap().amount_cents, 1000 - 6000);
    assert_eq!(i.severity, "watch", "no earlier period to show a sustained outflow");
}

#[test]
fn inventory_flags_dead_stock_and_sellers_that_ran_out() {
    let (conn, biz, uid) = setup();
    seed_inventory_item(&conn, &biz, "OLD-1", "Old Stock", 10, 500, 800); // never sold
    seed_inventory_item(&conn, &biz, "GONE-1", "Sold Out Item", 0, 300, 600);
    sale(&conn, &biz, "Other", None, 100, 50, "2024-04-01"); // 90 days of history
    sale(&conn, &biz, "Sold Out Item", None, 600, 300, "2024-06-20");
    let r = report(&conn, &biz, &uid, TODAY, 30).unwrap();

    let dead = find(&r, "inventory", "dead_stock").unwrap();
    assert_eq!(dead.impact.as_ref().unwrap().amount_cents, 10 * 500);
    assert!(dead.evidence.iter().any(|e| e.label == "Old Stock"));

    let out = find(&r, "inventory", "sold_out_sellers").unwrap();
    assert_eq!(out.severity, "problem");
    assert_eq!(out.impact.as_ref().unwrap().amount_cents, 600);
    assert_eq!(kpi_value(&r, "inventory", "Stock value at cost"), Some(5000.0));
}

#[test]
fn lapsed_customers_are_found_with_the_revenue_they_represented() {
    let (conn, biz, uid) = setup();
    sale(&conn, &biz, "Widget", Some("Anna"), 1000, 400, "2024-05-02");
    sale(&conn, &biz, "Widget", Some("Ben"), 1000, 400, "2024-05-10");
    sale(&conn, &biz, "Widget", Some("Cara"), 1000, 400, "2024-05-15");
    sale(&conn, &biz, "Widget", Some("Dan"), 1000, 400, "2024-05-20");
    sale(&conn, &biz, "Widget", Some("Dan"), 1000, 400, "2024-06-10");
    let r = report(&conn, &biz, &uid, TODAY, 30).unwrap();
    let i = find(&r, "customers", "lapsed_customers").unwrap();
    assert_eq!(i.impact.as_ref().unwrap().amount_cents, -3000, "Anna, Ben and Cara, 1000 each");
    assert!(i.evidence.iter().any(|e| e.label == "Anna"));
    assert!(!i.evidence.iter().any(|e| e.label == "Dan"), "Dan bought again");
    assert!(i.confidence.limitations.iter().any(|l| l.contains("phone number")));
    assert_eq!(kpi_value(&r, "customers", "Identified customers"), Some(1.0));
}

#[test]
fn growth_needs_three_covered_periods_and_reports_the_trend() {
    let (conn, biz, uid) = setup();
    // before 1000, previous 2000, current 4000 (10 sales each so every period has data)
    for k in 0..10 { sale(&conn, &biz, "Widget", None, 100, 40, &format!("2024-04-{:02}", 2 + k)); }
    for k in 0..10 { sale(&conn, &biz, "Widget", None, 200, 80, &format!("2024-05-{:02}", 2 + k)); }
    for k in 0..10 { sale(&conn, &biz, "Widget", None, 400, 160, &format!("2024-06-{:02}", 1 + k)); }
    let r = report(&conn, &biz, &uid, TODAY, 30).unwrap();
    let g = find(&r, "growth", "revenue_growth").unwrap();
    assert_eq!(g.severity, "good");
    assert_eq!(g.impact.as_ref().unwrap().amount_cents, 4000 - 2000);
    let o = find(&r, "growth", "revenue_outlook").unwrap();
    assert!(o.what_happened.contains("USD 23.33"), "average of 1000, 2000, 4000 cents is 2333 cents = 23.33");
    assert!(o.confidence.limitations.iter().any(|l| l.contains("seasonality")));

    // the same data seen from a date with too little history must not claim a trend
    let early = report(&conn, &biz, &uid, "2024-05-12", 30).unwrap();
    assert!(find(&early, "growth", "revenue_growth").is_none());
}

#[test]
fn every_insight_carries_the_full_structure() {
    let (conn, biz, uid) = setup();
    sale(&conn, &biz, "Widget", None, 1000, 400, "2024-05-02");
    for d in 3..=13 { sale(&conn, &biz, "Widget", None, 1000, 400, &format!("2024-05-{d:02}")); }
    for d in 1..=3 { sale(&conn, &biz, "Widget", None, 1000, 1500, &format!("2024-06-{d:02}")); }
    let r = report(&conn, &biz, &uid, TODAY, 30).unwrap();
    let mut seen = 0;
    for a in &r.areas {
        for i in &a.insights {
            seen += 1;
            assert!(!i.what_happened.is_empty(), "{}", i.kind);
            assert!(!i.evidence.is_empty(), "{} has no evidence", i.kind);
            assert!(!i.why_it_matters.is_empty(), "{}", i.kind);
            assert!(!i.recommended_action.is_empty(), "{}", i.kind);
            assert!(["high", "medium", "low"].contains(&i.confidence.level.as_str()), "{}", i.kind);
            assert!(!i.period.start.is_empty() && !i.period.end.is_empty(), "{}", i.kind);
        }
    }
    assert!(seen >= 2, "the seeded data should produce several findings, got {seen}");
}

#[test]
fn amounts_in_sentences_use_the_same_grouping_as_the_screens() {
    let (conn, biz, uid) = setup();
    sale(&conn, &biz, "Widget", None, 1_000_000, 400_000, "2024-05-02");
    for d in 3..=13 { sale(&conn, &biz, "Widget", None, 1_000_000, 400_000, &format!("2024-05-{d:02}")); }
    for d in 1..=5 { sale(&conn, &biz, "Widget", None, 1_000_000, 400_000, &format!("2024-06-{d:02}")); }
    let r = report(&conn, &biz, &uid, TODAY, 30).unwrap();
    let i = find(&r, "sales", "sales_change").unwrap();
    assert!(i.what_happened.contains("USD 50,000.00"), "{}", i.what_happened);
    assert!(i.what_happened.contains("USD 120,000.00"), "{}", i.what_happened);
}

fn debt(conn: &rusqlite::Connection, biz: &str, no: &str, party: &str, direction: &str, amount: i64, due: &str, settled: bool) {
    put(conn, biz, "debt_credit", &[
        ("entry_number", json!(no)), ("party_name", json!(party)), ("direction", json!(direction)),
        ("amount", json!(amount)), ("due_date", json!(due)), ("settled", json!(settled)),
    ]);
}

#[test]
fn overdue_customer_debts_and_supplier_payments_due_are_found_and_settled_ones_are_not() {
    let (conn, biz, uid) = setup();
    debt(&conn, &biz, "D1", "Customer A", "owed_to_business", 5000, "2024-05-01", false); // 60 days overdue
    debt(&conn, &biz, "D2", "Customer B", "owed_to_business", 3000, "2024-05-10", true);  // settled: not owed
    debt(&conn, &biz, "D3", "Supplier S", "i_owe", 7000, "2024-07-05", false);             // due in 5 days
    debt(&conn, &biz, "D4", "Supplier T", "i_owe", 900, "2024-09-30", false);              // not due soon
    let r = report(&conn, &biz, &uid, TODAY, 30).unwrap();
    assert_eq!(kpi_value(&r, "cash_flow", "Owed to you (unpaid)"), Some(5000.0));
    assert_eq!(kpi_value(&r, "cash_flow", "You owe (unpaid)"), Some(7900.0));
    let late = find(&r, "cash_flow", "receivables_overdue").expect("an overdue customer debt must be flagged");
    assert_eq!(late.impact.as_ref().unwrap().amount_cents, 5000);
    let due = find(&r, "cash_flow", "payables_due").expect("a supplier payment due within 14 days must be flagged");
    assert_eq!(due.impact.as_ref().unwrap().amount_cents, 7000, "only the payment due within 14 days, not the later one");
}
