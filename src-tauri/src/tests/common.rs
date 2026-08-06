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
