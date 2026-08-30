use anyhow::Result;
use core_engine::{auth, business_panel, db, http_api, module_json};

/// Standalone backend runner for development and testing — no Tauri,
/// no webview, just the HTTP API with a demo business seeded. This is
/// the binary every test in this project's history has run against
/// (`cargo run --bin demo_seed`). The real packaged app uses
/// `src/main.rs` instead, which skips all this seeding and runs the
/// exact same `http_api::serve` invisibly inside the Tauri window.
fn main() -> Result<()> {
    let mut conn = db::open("erp.db")?;

    let business_id = business_panel::create_business(&conn, "Mama Nia General Store", "KES", "Africa/Nairobi")?;
    // Was a literal relative path string ("modules/inventory.json") —
    // only ever worked because this binary happens to run with the
    // source tree as its working directory. `module_json` (the same
    // compile-time embedded lookup the real app now uses — see
    // lib.rs's `MODULE_DEFS` doc comment) works the same way regardless
    // of working directory, matching what every other caller of
    // `enable_module` was already switched to.
    for id in ["inventory", "sales", "hr", "accounting", "purchasing", "debt_credit", "refunds", "invoice"] {
        let json = module_json(id).unwrap_or_else(|| panic!("missing embedded module definition: {id}"));
        business_panel::enable_module(&mut conn, &business_id, json)?;
    }

    let owner_password_hash = auth::hash_secret("correct horse battery staple")?;
    let owner_id = business_panel::add_user(&conn, &business_id, "nia", &owner_password_hash, "Owner")?;
    auth::set_security_questions(&conn, &owner_id, "First pet's name?", "Rex", "Mother's maiden name?", "Wanjiru")?;

    let staff_password_hash = auth::hash_secret("clerkpass123")?;
    let staff_id = business_panel::add_user(&conn, &business_id, "clerk", &staff_password_hash, "Staff")?;

    let manager_password_hash = auth::hash_secret("managerpass123")?;
    let manager_id = business_panel::add_user(&conn, &business_id, "kioko", &manager_password_hash, "Manager")?;

    let admin_code = "AC-7F2Q-9KXM";
    business_panel::set_admin_recovery_code(&conn, &business_id, &auth::hash_secret(admin_code)?)?;

    // A handful of realistic inventory items with stock (seeded via
    // insert_validated_record directly, matching the bulk/migration
    // path — see crud.rs's own doc comment on why create() itself now
    // forces new items to start at zero).
    let inventory_module = core_engine::crud::load_module(&conn, &business_id, "inventory")?;
    for (sku, name, qty, cost, price) in [
        ("RICE-5KG", "Rice 5kg Bag", 42, 45000i64, 65000i64),
        ("SUGAR-2KG", "Sugar 2kg Bag", 8, 22000, 32000),
        ("COOKOIL-1L", "Cooking Oil 1L", 15, 28000, 39000),
        ("SOAP-BAR", "Bathing Soap Bar", 8, 5000, 8500),
        ("MAIZE-2KG", "Maize Flour 2kg", 60, 18000, 26000),
    ] {
        let mut record: std::collections::HashMap<String, serde_json::Value> = std::collections::HashMap::new();
        record.insert("sku".into(), serde_json::json!(sku));
        record.insert("name".into(), serde_json::json!(name));
        record.insert("quantity".into(), serde_json::json!(qty));
        record.insert("unit_cost".into(), serde_json::json!(cost));
        record.insert("unit_price".into(), serde_json::json!(price));
        record.insert("reorder_level".into(), serde_json::json!(10));
        core_engine::crud::insert_validated_record(&conn, &business_id, &inventory_module, &record)?;
    }

    // A couple of real sales through the actual POS checkout path, so
    // the dashboard/analytics screens have genuine revenue data to
    // chart instead of an empty state.
    let items = core_engine::crud::list(&conn, &business_id, &owner_id, "inventory", None, 50, 0)?;
    let rice_id = items.iter().find(|r| r["sku"] == serde_json::json!("RICE-5KG")).unwrap()["id"].as_str().unwrap().to_string();
    let oil_id = items.iter().find(|r| r["sku"] == serde_json::json!("COOKOIL-1L")).unwrap()["id"].as_str().unwrap().to_string();
    let soap_id = items.iter().find(|r| r["sku"] == serde_json::json!("SOAP-BAR")).unwrap()["id"].as_str().unwrap().to_string();

    core_engine::pos::checkout(&mut conn, &business_id, &owner_id, core_engine::pos::CheckoutRequest {
        items: vec![core_engine::pos::CartItem { inventory_record_id: rice_id.clone(), quantity: 3 }],
        payment_method: Some("Cash".into()), customer: Some("Amina".into()), customer_phone: None,
        allow_oversell: false, on_credit: false, due_date: None,
    })?;
    core_engine::pos::checkout(&mut conn, &business_id, &owner_id, core_engine::pos::CheckoutRequest {
        items: vec![core_engine::pos::CartItem { inventory_record_id: oil_id, quantity: 2 }],
        payment_method: Some("M-Pesa".into()), customer: Some("Brian".into()), customer_phone: None,
        allow_oversell: false, on_credit: false, due_date: None,
    })?;
    core_engine::pos::checkout(&mut conn, &business_id, &owner_id, core_engine::pos::CheckoutRequest {
        items: vec![core_engine::pos::CartItem { inventory_record_id: soap_id, quantity: 2 }],
        payment_method: None, customer: None, customer_phone: None,
        allow_oversell: false, on_credit: false, due_date: None,
    })?;
    core_engine::pos::checkout(&mut conn, &business_id, &owner_id, core_engine::pos::CheckoutRequest {
        items: vec![core_engine::pos::CartItem { inventory_record_id: rice_id, quantity: 1 }],
        payment_method: Some("Cash".into()), customer: None, customer_phone: None,
        allow_oversell: false, on_credit: false, due_date: None,
    })?;

    let seed = serde_json::json!({
        "business_id": business_id,
        "owner_id": owner_id,
        "staff_id": staff_id,
        "manager_id": manager_id,
        "owner_username": "nia",
        "owner_password": "correct horse battery staple",
        "staff_username": "clerk",
        "staff_password": "clerkpass123",
        "manager_username": "kioko",
        "manager_password": "managerpass123",
        "admin_recovery_code": admin_code
    });
    std::fs::write("seed_ids.json", serde_json::to_string_pretty(&seed)?)?;
    println!("[seed] {seed}");

    http_api::serve(conn, "127.0.0.1:8080").map_err(anyhow::Error::msg)?;
    Ok(())
}
