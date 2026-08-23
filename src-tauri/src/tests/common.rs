use rusqlite::Connection;

pub fn test_db() -> Connection {
    let mut conn = Connection::open_in_memory().expect("in-memory db");
    conn.execute_batch(include_str!("../../schema.sql"))
        .expect("schema application");
    crate::db_migrations::run(&mut conn).expect("migrations");
    conn
}

pub fn test_business(conn: &mut Connection) -> String {
    let id = crate::business_panel::create_business(conn, "Test Biz", "USD", "UTC")
        .expect("create business");
    crate::onboarding::apply_business_type(conn, &id, "retail")
        .expect("enable modules");
    id
}

pub fn test_owner(conn: &mut Connection, business_id: &str) -> (String, String) {
    let hash = crate::auth::hash_secret("password123").unwrap();
    let uid = crate::business_panel::add_user(conn, business_id, "owner", &hash, "Owner")
        .expect("create owner");
    let token = crate::auth::login(conn, business_id, "owner", "password123")
        .expect("login");
    (uid, token)
}

/// Seeds an inventory item that already has stock, for tests whose
/// point is exercising sell/refund/repack/receiving against an
/// existing stock level, not exercising item creation itself.
///
/// Deliberately does NOT go through `crud::create()` — since this
/// project's rule is that every inventory item created through that
/// single-record path starts at zero stock (see crud.rs's own doc
/// comment on `create()`), a test that needs pre-existing stock has to
/// represent it the same way the real system would: as data that
/// already existed before this moment, seeded in bulk — exactly what
/// `insert_validated_record()` is for, and exactly what
/// excel_import.rs's own bulk-upload path calls directly for the same
/// reason (a spreadsheet-driven initial catalog load, or reconciling a
/// stock take). Going through the full purchasing receive() workflow
/// instead, for every test that merely needs "an item with 40 units,"
/// would make each of those tests actually about receiving, which
/// isn't what most of them are testing.
pub fn seed_inventory_item(
    conn: &Connection,
    business_id: &str,
    sku: &str,
    name: &str,
    quantity: i64,
    unit_cost: i64,
    unit_price: i64,
) -> String {
    let module = crate::crud::load_module(conn, business_id, "inventory").expect("load inventory module");
    let mut record: std::collections::HashMap<String, serde_json::Value> = std::collections::HashMap::new();
    record.insert("sku".into(), serde_json::json!(sku));
    record.insert("name".into(), serde_json::json!(name));
    record.insert("quantity".into(), serde_json::json!(quantity));
    record.insert("unit_cost".into(), serde_json::json!(unit_cost));
    record.insert("unit_price".into(), serde_json::json!(unit_price));
    crate::crud::insert_validated_record(conn, business_id, &module, &record).expect("seed inventory item")
}
