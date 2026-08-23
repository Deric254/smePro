use super::common::*;

#[test]
fn test_disable_module_flips_enabled_flag_but_keeps_data() {
    // disable_module existed correctly before this — soft disable,
    // never drops the table — but had no HTTP route calling it at all
    // (see http_api.rs's new POST /modules/{id}/disable). This proves
    // the underlying function itself does what its own doc comment
    // claims: the flag flips, but a record created while the module
    // was enabled is still there afterward, readable the moment it's
    // re-enabled.
    let mut conn = test_db();
    let biz = test_business(&mut conn); // enables "inventory" via retail preset
    let (uid, _) = test_owner(&mut conn, &biz);

    let mut item = serde_json::Map::new();
    item.insert("sku".into(), serde_json::json!("ITEM-001"));
    item.insert("name".into(), serde_json::json!("Test Item"));
    item.insert("quantity".into(), serde_json::json!(10));
    item.insert("unit_cost".into(), serde_json::json!(100));
    item.insert("unit_price".into(), serde_json::json!(200));
    let record_id = crate::crud::create(&conn, &biz, &uid, "inventory", &item).unwrap();

    crate::business_panel::disable_module(&conn, &biz, "inventory").unwrap();

    let enabled: i64 = conn
        .query_row(
            "SELECT enabled FROM modules WHERE business_id = ?1 AND id = 'inventory'",
            rusqlite::params![biz],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(enabled, 0);

    // Disabled means the RBAC/enabled gate blocks the normal read path
    // now (see crud::load_module) — this proves the disable actually
    // does something, not just that the flag flipped in isolation.
    assert!(crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).is_err());

    // Re-enabling brings the SAME record right back — proves the
    // table and its row were never touched, only the flag.
    let path = crate::modules_dir().join("inventory.json");
    crate::business_panel::enable_module(&mut conn, &biz, &path.to_string_lossy()).unwrap();
    let records = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].get("id").unwrap().as_str().unwrap(), record_id);
}

#[test]
fn test_disable_unknown_module_fails_cleanly() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let _ = test_owner(&mut conn, &biz);

    let result = crate::business_panel::disable_module(&conn, &biz, "not_a_real_module");
    assert!(result.is_err());
}
