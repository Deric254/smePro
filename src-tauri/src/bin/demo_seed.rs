use anyhow::Result;
use core_engine::{auth, business_panel, db, http_api};

/// Standalone backend runner for development and testing — no Tauri,
/// no webview, just the HTTP API with a demo business seeded. This is
/// the binary every test in this project's history has run against
/// (`cargo run --bin demo_seed`). The real packaged app uses
/// `src/main.rs` instead, which skips all this seeding and runs the
/// exact same `http_api::serve` invisibly inside the Tauri window.
fn main() -> Result<()> {
    let mut conn = db::open("erp.db")?;

    let business_id = business_panel::create_business(&conn, "Mama Nia General Store", "KES", "Africa/Nairobi")?;
    business_panel::enable_module(&mut conn, &business_id, "modules/inventory.json")?;
    business_panel::enable_module(&mut conn, &business_id, "modules/sales.json")?;
    business_panel::enable_module(&mut conn, &business_id, "modules/hr.json")?;
    business_panel::enable_module(&mut conn, &business_id, "modules/accounting.json")?;
    business_panel::enable_module(&mut conn, &business_id, "modules/purchasing.json")?;
    business_panel::enable_module(&mut conn, &business_id, "modules/debt_credit.json")?;

    let owner_password_hash = auth::hash_secret("correct horse battery staple")?;
    let owner_id = business_panel::add_user(&conn, &business_id, "nia", &owner_password_hash, "Owner")?;
    auth::set_security_questions(&conn, &owner_id, "First pet's name?", "Rex", "Mother's maiden name?", "Wanjiru")?;

    let staff_password_hash = auth::hash_secret("clerkpass123")?;
    let staff_id = business_panel::add_user(&conn, &business_id, "clerk", &staff_password_hash, "Staff")?;

    let manager_password_hash = auth::hash_secret("managerpass123")?;
    let manager_id = business_panel::add_user(&conn, &business_id, "kioko", &manager_password_hash, "Manager")?;

    let admin_code = "AC-7F2Q-9KXM";
    business_panel::set_admin_recovery_code(&conn, &business_id, &auth::hash_secret(admin_code)?)?;

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

    http_api::serve(conn, "127.0.0.1:8080");
    Ok(())
}
