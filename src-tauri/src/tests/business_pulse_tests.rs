use super::common::*;
use crate::business_pulse;

/// Inserts a sale directly into module_sales with an EXPLICIT
/// created_at, bypassing crud::create (which always stamps
/// datetime('now') — see crud.rs's own insert logic) — this is the
/// only way to get sales history spread across two different months
/// in a test that has to run instantly, not actually wait a month.
fn seed_sale(conn: &rusqlite::Connection, business_id: &str, revenue_cents: i64, created_at: &str) {
    conn.execute(
        "INSERT INTO module_sales (id, business_id, item_name, quantity, revenue, unit_price, created_at, updated_at)
         VALUES (lower(hex(randomblob(16))), ?1, 'Test Item', 1, ?2, ?2, ?3, ?3)",
        rusqlite::params![business_id, revenue_cents, created_at],
    )
    .unwrap();
}

#[test]
fn test_no_data_returns_has_data_false_not_an_error() {
    let mut conn = test_db();
    let business_id = test_business(&mut conn);
    let (user_id, _) = test_owner(&mut conn, &business_id);

    let pulse = business_pulse::compute(&conn, &business_id, &user_id);
    assert!(!pulse.has_data);
    assert_eq!(pulse.pct_change, None);
    assert!(!pulse.recommendations.is_empty(), "must still say SOMETHING, not an empty list");
}

#[test]
fn test_single_month_of_history_is_not_enough_for_a_trend() {
    // One data point can't show a trend — reporting "flat" or "up" off
    // a single month would be a guess wearing a real number's clothes.
    let mut conn = test_db();
    let business_id = test_business(&mut conn);
    let (user_id, _) = test_owner(&mut conn, &business_id);

    seed_sale(&conn, &business_id, 10000, "2026-01-15 10:00:00");

    let pulse = business_pulse::compute(&conn, &business_id, &user_id);
    assert!(!pulse.has_data);
}

#[test]
fn test_computes_real_percentage_change_across_two_months() {
    let mut conn = test_db();
    let business_id = test_business(&mut conn);
    let (user_id, _) = test_owner(&mut conn, &business_id);

    // January: $100.00. February: $150.00 — a real 50% increase.
    seed_sale(&conn, &business_id, 10000, "2026-01-15 10:00:00");
    seed_sale(&conn, &business_id, 15000, "2026-02-10 10:00:00");

    let pulse = business_pulse::compute(&conn, &business_id, &user_id);
    assert!(pulse.has_data);
    assert_eq!(pulse.revenue_last_period_cents, 10000);
    assert_eq!(pulse.revenue_this_period_cents, 15000);
    let pct = pulse.pct_change.expect("should compute a real percentage");
    assert!((pct - 50.0).abs() < 0.01, "expected ~50% increase, got {pct}");
    assert!(pulse.recommendations.iter().any(|r| r.contains("up")), "a 50% jump should surface a real up-trend note");
}

#[test]
fn test_zero_last_period_gives_none_not_a_fabricated_percentage() {
    // Going from $0 to any positive number isn't a real "percentage
    // increase" in any meaningful sense — reporting one would be a
    // fabricated number, not a computed one.
    let mut conn = test_db();
    let business_id = test_business(&mut conn);
    let (user_id, _) = test_owner(&mut conn, &business_id);

    seed_sale(&conn, &business_id, 0, "2026-01-15 10:00:00");
    seed_sale(&conn, &business_id, 20000, "2026-02-10 10:00:00");

    let pulse = business_pulse::compute(&conn, &business_id, &user_id);
    assert!(pulse.has_data);
    assert_eq!(pulse.pct_change, None);
}

#[test]
fn test_low_stock_count_reflected_in_recommendations() {
    let mut conn = test_db();
    let business_id = test_business(&mut conn);
    let (user_id, _) = test_owner(&mut conn, &business_id);

    seed_sale(&conn, &business_id, 10000, "2026-01-15 10:00:00");
    seed_sale(&conn, &business_id, 10000, "2026-02-10 10:00:00");

    let inv_id = seed_inventory_item(&conn, &business_id, "LOW-001", "Almost Out", 1, 100, 200);
    let mut reorder_patch = serde_json::Map::new();
    reorder_patch.insert("reorder_level".into(), serde_json::json!(5));
    crate::crud::update(&conn, &business_id, &user_id, "inventory", &inv_id, &reorder_patch, false).unwrap();

    let pulse = business_pulse::compute(&conn, &business_id, &user_id);
    assert_eq!(pulse.low_stock_count, 1);
    assert!(pulse.recommendations.iter().any(|r| r.contains("low on stock")));
}

#[test]
fn test_never_panics_on_a_business_with_no_sales_module_enabled() {
    // A business type that never enabled "sales" at all (e.g. one
    // still mid-onboarding) must degrade gracefully, not panic the
    // whole chat response over an optional performance summary.
    let mut conn = test_db();
    let business_id = crate::business_panel::create_business(&mut conn, "No Sales Yet", "USD", "UTC").unwrap();
    let hash = crate::auth::hash_secret("password123").unwrap();
    let user_id = crate::business_panel::add_user(&conn, &business_id, "owner", &hash, "Owner").unwrap();

    let pulse = business_pulse::compute(&conn, &business_id, &user_id);
    assert!(!pulse.has_data);
}
