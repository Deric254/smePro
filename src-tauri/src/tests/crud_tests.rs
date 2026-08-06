use super::common::*;
use serde_json::json;

#[test]
fn test_crud_create_and_list() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let mut record = serde_json::Map::new();
    record.insert("sku".into(), json!("TEST-001"));
    record.insert("name".into(), json!("Test Item"));
    record.insert("quantity".into(), json!(10));
    record.insert("unit_cost".into(), json!(5.0));
    record.insert("unit_price".into(), json!(10.0));

    let _id = crate::crud::create(&mut conn, &biz, &uid, "inventory", &record)
        .expect("create record");

    let list = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0)
        .expect("list records");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].get("sku").unwrap().as_str().unwrap(), "TEST-001");
}

#[test]
fn test_crud_soft_delete() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let mut record = serde_json::Map::new();
    record.insert("sku".into(), json!("DEL-001"));
    record.insert("name".into(), json!("Delete Me"));
    record.insert("quantity".into(), json!(1));
    record.insert("unit_cost".into(), json!(1.0));
    record.insert("unit_price".into(), json!(2.0));

    let id = crate::crud::create(&mut conn, &biz, &uid, "inventory", &record).unwrap();
    crate::crud::delete(&mut conn, &biz, &uid, "inventory", &id).unwrap();

    let list = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    assert_eq!(list.len(), 0);

    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM module_inventory WHERE id = ?1",
        [id], |r| r.get(0)
    ).unwrap();
    assert_eq!(count, 1);
}
