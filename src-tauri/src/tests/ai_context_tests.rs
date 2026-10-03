use super::common::*;
use serde_json::json;

#[test]
fn test_build_snapshot_includes_category_trend_and_churn_signals() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    // Category profit: an inventory item with a category, sold once.
    let inv_id = seed_inventory_item(&conn, &biz, "SKU-1", "Widget", 100, 200, 500);
    conn.execute(
        "UPDATE module_inventory SET category = 'Hardware' WHERE id = ?1",
        rusqlite::params![inv_id],
    )
    .unwrap();
    let req = crate::pos::CheckoutRequest {
        idempotency_key: None,
        discount_pct: None,
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id, quantity: 1 }],
        payment_method: Some("Cash".into()),
        customer: Some("Asha".into()),
        customer_phone: Some("0700000001".into()),
        allow_oversell: false,
        on_credit: false,
        due_date: None,
    };
    crate::pos::checkout(&mut conn, &biz, &uid, req).unwrap();

    // A repeat customer overdue relative to her own rhythm, for the
    // churn signal — same back-dating approach as customers_tests.rs.
    let recent_inv = seed_inventory_item(&conn, &biz, "SKU-2", "Gadget", 50, 100, 300);
    for date in ["2024-01-01", "2024-01-21", "2024-02-10"] {
        let req = crate::pos::CheckoutRequest {
            idempotency_key: None,
            discount_pct: None,
            items: vec![crate::pos::CartItem { inventory_record_id: recent_inv.clone(), quantity: 1 }],
            payment_method: Some("Cash".into()),
            customer: Some("Brian".into()),
            customer_phone: Some("0700000002".into()),
            allow_oversell: false,
            on_credit: false,
            due_date: None,
        };
        let result = crate::pos::checkout(&mut conn, &biz, &uid, req).unwrap();
        let order_id = result["order_id"].as_str().unwrap();
        conn.execute(
            "UPDATE module_sales SET created_at = ?1, sale_date = ?2 WHERE business_id = ?3 AND order_id = ?4",
            rusqlite::params![format!("{date} 12:00:00"), date, biz, order_id],
        )
        .unwrap();
    }

    let snapshot = crate::ai_context::build_snapshot(&conn, &biz, &uid).unwrap();

    let categories = snapshot["profit_by_category"].as_array().unwrap();
    assert!(categories.iter().any(|c| c["category"] == json!("Hardware")), "the categorized Widget sale must appear");

    let trend = snapshot["item_margin_trend_last_30_days"].as_array().unwrap();
    // Only real for sales in the CURRENT 30-day window — Brian's
    // back-dated 2024 purchases fall well outside "today," so only
    // today's Widget sale should surface here.
    assert!(trend.iter().any(|t| t["item_name"] == json!("Widget")));

    let quiet = snapshot["customers_going_quiet"].as_array().unwrap();
    assert!(quiet.iter().any(|c| c["name"] == json!("Brian")), "Brian is 3 purchases in and long overdue relative to his own 20-day rhythm");
}

#[test]
fn test_build_snapshot_degrades_to_empty_without_reports_access() {
    // Same permission boundary as stock_runway/day_of_week_pattern
    // just above these fields in ai_context.rs — a role without
    // Reports access must get an empty array here, not an error and
    // not the real numbers, same as those two already do.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (owner_id, _) = test_owner(&mut conn, &biz);

    let role_id = crate::roles::create_role(&conn, &biz, "Clerk").expect("create a custom role");
    crate::roles::set_reports_flag(&conn, &biz, &role_id, false).expect("explicitly deny Reports access");
    let hash = crate::auth::hash_secret("password123").unwrap();
    let clerk_id = crate::business_panel::add_user(&conn, &biz, "clerk", &hash, "Clerk").expect("create clerk user");

    // Owner sanity check first — the fields must be non-empty for the
    // role that DOES have access, or this test would pass for the
    // wrong reason (everything empty regardless of role).
    let owner_snapshot = crate::ai_context::build_snapshot(&conn, &biz, &owner_id).unwrap();
    assert!(owner_snapshot["profit_by_category"].is_array());
    assert!(owner_snapshot["monthly_revenue"].is_array());

    let clerk_snapshot = crate::ai_context::build_snapshot(&conn, &biz, &clerk_id).unwrap();
    assert!(clerk_snapshot["profit_by_category"].as_array().unwrap().is_empty());
    assert!(clerk_snapshot["item_margin_trend_last_30_days"].as_array().unwrap().is_empty());
    assert!(clerk_snapshot["customers_going_quiet"].as_array().unwrap().is_empty());
    assert!(clerk_snapshot["monthly_revenue"].as_array().unwrap().is_empty());
}

/// Raw module_sales insert with an explicit created_at — the only way
/// to spread sales across months in a test that must run instantly.
/// Same shape as business_pulse_tests.rs's seed_sale (see its doc
/// comment for why cost_at_sale/discount_amount are explicit zeros).
fn seed_sale_at(conn: &rusqlite::Connection, biz: &str, revenue_cents: i64, created_at: &str) {
    conn.execute(
        "INSERT INTO module_sales (id, business_id, item_name, quantity, revenue, unit_price, cost_at_sale, discount_amount, created_at, updated_at)
         VALUES (lower(hex(randomblob(16))), ?1, 'Test Item', 1, ?2, ?2, 0, 0, ?3, ?3)",
        rusqlite::params![biz, revenue_cents, created_at],
    )
    .unwrap();
}

#[test]
fn test_build_snapshot_gives_the_assistant_month_by_month_revenue() {
    // The regression this guards: the snapshot used to carry only an
    // all-time revenue total, so "how is revenue vs last month" was
    // unanswerable and the assistant wrongly claimed no prior-month
    // data existed. Prior months must be present, in decimal currency
    // (USD: 2 places), oldest first, with only the current month
    // flagged as in progress.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    seed_sale_at(&conn, &biz, 10000, "2025-01-15 10:00:00");
    seed_sale_at(&conn, &biz, 5000, "2025-01-20 10:00:00");
    seed_sale_at(&conn, &biz, 15000, "2025-02-10 10:00:00");
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    seed_sale_at(&conn, &biz, 2500, &now);

    let snapshot = crate::ai_context::build_snapshot(&conn, &biz, &uid).unwrap();
    let months = snapshot["monthly_revenue"].as_array().unwrap();

    assert_eq!(months.len(), 3);
    assert_eq!(months[0]["month"], json!("2025-01"));
    assert_eq!(months[0]["revenue"].as_f64().unwrap(), 150.0);
    assert_eq!(months[0]["month_in_progress"], json!(false));
    assert_eq!(months[1]["month"], json!("2025-02"));
    assert_eq!(months[1]["revenue"].as_f64().unwrap(), 150.0);
    assert_eq!(months[1]["month_in_progress"], json!(false));
    assert_eq!(months[2]["month"], json!(&now[..7]));
    assert_eq!(months[2]["revenue"].as_f64().unwrap(), 25.0);
    assert_eq!(months[2]["month_in_progress"], json!(true));
}

#[test]
fn test_build_snapshot_monthly_revenue_is_capped_to_latest_12_months() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    // 14 consecutive past months: 2024-01 .. 2025-02.
    for i in 0..14 {
        let (year, month) = (2024 + i / 12, i % 12 + 1);
        seed_sale_at(&conn, &biz, 1000, &format!("{year}-{month:02}-10 10:00:00"));
    }

    let snapshot = crate::ai_context::build_snapshot(&conn, &biz, &uid).unwrap();
    let months = snapshot["monthly_revenue"].as_array().unwrap();

    assert_eq!(months.len(), 12);
    assert_eq!(months[0]["month"], json!("2024-03"), "the 2 oldest months must be the ones dropped");
    assert_eq!(months[11]["month"], json!("2025-02"));
}
