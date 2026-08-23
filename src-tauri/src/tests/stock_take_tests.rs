use super::common::*;

fn get_item(conn: &rusqlite::Connection, biz: &str, uid: &str, id: &str) -> serde_json::Value {
    let list = crate::crud::list(conn, biz, uid, "inventory", None, 50, 0).unwrap();
    list.into_iter().find(|r| r["id"] == serde_json::json!(id)).unwrap()
}

#[test]
fn test_initiate_snapshots_current_quantity_as_expected() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let rice_id = seed_inventory_item(&conn, &biz, "RICE-001", "Rice", 40, 500, 800);
    let beans_id = seed_inventory_item(&conn, &biz, "BEANS-001", "Beans", 15, 300, 500);

    let result = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    assert_eq!(result["status"].as_str().unwrap(), "in_progress");

    let items = result["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    let rice = items.iter().find(|i| i["inventory_record_id"] == serde_json::json!(rice_id)).unwrap();
    assert_eq!(rice["expected_qty"].as_i64().unwrap(), 40);
    assert!(rice["counted_qty"].is_null());
    let beans = items.iter().find(|i| i["inventory_record_id"] == serde_json::json!(beans_id)).unwrap();
    assert_eq!(beans["expected_qty"].as_i64().unwrap(), 15);
}

#[test]
fn test_cannot_initiate_a_second_stock_take_while_one_is_open() {
    // The real integrity guarantee, not just a nice-to-have: a second
    // concurrent count would make every variance ambiguous (which
    // count does a later sale's decrement get attributed to?), so
    // this is blocked outright.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    seed_inventory_item(&conn, &biz, "ITEM-001", "Item", 10, 100, 200);

    crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    let second = crate::stock_take::initiate(&mut conn, &biz, &uid);
    assert!(second.is_err());
    assert!(second.unwrap_err().to_string().contains("already in progress"));
}

#[test]
fn test_close_applies_counted_variance_to_inventory_quantity() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let rice_id = seed_inventory_item(&conn, &biz, "RICE-001", "Rice", 40, 500, 800);

    let initiated = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    let stock_take_id = initiated["id"].as_str().unwrap().to_string();
    let item_id = initiated["items"][0]["id"].as_str().unwrap().to_string();

    // The physical count found fewer units than the system expected —
    // shrinkage, the exact scenario this feature exists to catch.
    crate::stock_take::record_count(
        &conn, &biz, &uid,
        crate::stock_take::RecordCountRequest { stock_take_id: stock_take_id.clone(), item_id, counted_qty: 33 },
    ).unwrap();

    let summary = crate::stock_take::close(&mut conn, &biz, &uid, &stock_take_id).unwrap();
    assert_eq!(summary["items_counted"].as_i64().unwrap(), 1);
    assert_eq!(summary["items_skipped"].as_i64().unwrap(), 0);
    assert_eq!(summary["total_variance_units"].as_i64().unwrap(), -7);

    let rice = get_item(&conn, &biz, &uid, &rice_id);
    assert_eq!(rice["quantity"].as_i64().unwrap(), 33, "inventory quantity must now match the physical count");
}

#[test]
fn test_close_leaves_uncounted_items_untouched_and_reports_them_as_skipped() {
    // Partial counting is a legitimate, expected use of this feature —
    // a business that only had time to recount its top movers today
    // must not have every other item silently reset or zeroed.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    seed_inventory_item(&conn, &biz, "COUNTED-001", "Counted Item", 20, 100, 200);
    seed_inventory_item(&conn, &biz, "SKIPPED-001", "Skipped Item", 15, 100, 200);

    let initiated = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    let stock_take_id = initiated["id"].as_str().unwrap().to_string();
    let items = initiated["items"].as_array().unwrap();
    let counted_item = items.iter().find(|i| i["item_name"] == serde_json::json!("Counted Item")).unwrap();
    let skipped_inv_id = items.iter().find(|i| i["item_name"] == serde_json::json!("Skipped Item")).unwrap()["inventory_record_id"].as_str().unwrap().to_string();

    crate::stock_take::record_count(
        &conn, &biz, &uid,
        crate::stock_take::RecordCountRequest {
            stock_take_id: stock_take_id.clone(),
            item_id: counted_item["id"].as_str().unwrap().to_string(),
            counted_qty: 18,
        },
    ).unwrap();
    // Skipped Item never gets a record_count() call at all.

    let summary = crate::stock_take::close(&mut conn, &biz, &uid, &stock_take_id).unwrap();
    assert_eq!(summary["items_counted"].as_i64().unwrap(), 1);
    assert_eq!(summary["items_skipped"].as_i64().unwrap(), 1);

    let skipped = get_item(&conn, &biz, &uid, &skipped_inv_id);
    assert_eq!(skipped["quantity"].as_i64().unwrap(), 15, "an uncounted item's quantity must be completely untouched by close()");
}

#[test]
fn test_recounting_the_same_item_before_close_overwrites_not_rejects() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    seed_inventory_item(&conn, &biz, "ITEM-001", "Item", 10, 100, 200);

    let initiated = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    let stock_take_id = initiated["id"].as_str().unwrap().to_string();
    let item_id = initiated["items"][0]["id"].as_str().unwrap().to_string();

    crate::stock_take::record_count(&conn, &biz, &uid, crate::stock_take::RecordCountRequest { stock_take_id: stock_take_id.clone(), item_id: item_id.clone(), counted_qty: 7 }).unwrap();
    crate::stock_take::record_count(&conn, &biz, &uid, crate::stock_take::RecordCountRequest { stock_take_id: stock_take_id.clone(), item_id: item_id.clone(), counted_qty: 9 }).unwrap();

    let fetched = crate::stock_take::get(&conn, &biz, &uid, &stock_take_id).unwrap();
    assert_eq!(fetched["items"][0]["counted_qty"].as_i64().unwrap(), 9, "a recount must overwrite, not stack or reject");
}

#[test]
fn test_cannot_record_a_negative_count() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    seed_inventory_item(&conn, &biz, "ITEM-001", "Item", 10, 100, 200);

    let initiated = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    let stock_take_id = initiated["id"].as_str().unwrap().to_string();
    let item_id = initiated["items"][0]["id"].as_str().unwrap().to_string();

    let result = crate::stock_take::record_count(&conn, &biz, &uid, crate::stock_take::RecordCountRequest { stock_take_id, item_id, counted_qty: -3 });
    assert!(result.is_err());
}

#[test]
fn test_cannot_record_a_count_against_an_already_closed_stock_take() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    seed_inventory_item(&conn, &biz, "ITEM-001", "Item", 10, 100, 200);

    let initiated = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    let stock_take_id = initiated["id"].as_str().unwrap().to_string();
    let item_id = initiated["items"][0]["id"].as_str().unwrap().to_string();
    crate::stock_take::close(&mut conn, &biz, &uid, &stock_take_id).unwrap();

    let result = crate::stock_take::record_count(&conn, &biz, &uid, crate::stock_take::RecordCountRequest { stock_take_id: stock_take_id.clone(), item_id, counted_qty: 5 });
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("already closed"));
}

#[test]
fn test_cannot_close_the_same_stock_take_twice() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    seed_inventory_item(&conn, &biz, "ITEM-001", "Item", 10, 100, 200);

    let initiated = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    let stock_take_id = initiated["id"].as_str().unwrap().to_string();
    crate::stock_take::close(&mut conn, &biz, &uid, &stock_take_id).unwrap();

    let second_close = crate::stock_take::close(&mut conn, &biz, &uid, &stock_take_id);
    assert!(second_close.is_err());
    assert!(second_close.unwrap_err().to_string().contains("already closed"));
}

#[test]
fn test_after_close_a_new_stock_take_can_be_started() {
    // Confirms the "only one open at a time" rule is about concurrency,
    // not a one-time-ever limit — closing genuinely frees the slot.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    seed_inventory_item(&conn, &biz, "ITEM-001", "Item", 10, 100, 200);

    let first = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    crate::stock_take::close(&mut conn, &biz, &uid, first["id"].as_str().unwrap()).unwrap();

    let second = crate::stock_take::initiate(&mut conn, &biz, &uid);
    assert!(second.is_ok());
}

#[test]
fn test_migration_backfills_stocktake_permission_for_a_business_that_predates_it() {
    // The gap this specifically guards against: a business that
    // enabled Inventory before "stocktake" existed as an action has
    // its module definition and role permissions captured as a
    // SNAPSHOT at that moment (see crud.rs's load_module and
    // rbac::seed_default_roles) — simply shipping the new action in
    // inventory.json's template does nothing for that business unless
    // a migration explicitly goes back and patches it in. This test
    // simulates exactly that "already onboarded, pre-v11" state by
    // rolling back what v11 is supposed to add, then re-running
    // migrations and confirming both halves of the backfill actually
    // land: the stored schema snapshot AND the Owner role's grant.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (owner_uid, _) = test_owner(&mut conn, &biz);

    // Roll back to a "pre-v11" state: strip "stocktake" from the
    // stored schema_json snapshot, and revoke the permission grant —
    // exactly what a real pre-v11 business would look like.
    let schema_json: String = conn
        .query_row("SELECT schema_json FROM modules WHERE business_id = ?1 AND id = 'inventory'", [&biz], |r| r.get(0))
        .unwrap();
    let mut parsed: serde_json::Value = serde_json::from_str(&schema_json).unwrap();
    parsed["actions"] = serde_json::Value::Array(
        parsed["actions"].as_array().unwrap().iter().filter(|a| a.as_str() != Some("stocktake")).cloned().collect(),
    );
    let rolled_back_json = serde_json::to_string(&parsed).unwrap();
    conn.execute(
        "UPDATE modules SET schema_json = ?1 WHERE business_id = ?2 AND id = 'inventory'",
        rusqlite::params![rolled_back_json, biz],
    ).unwrap();
    conn.execute(
        "DELETE FROM permissions WHERE module_id = 'inventory' AND action = 'stocktake'",
        [],
    ).unwrap();
    conn.execute("DELETE FROM _schema_version WHERE version = 11", []).unwrap();

    // Confirm the rollback actually took — Owner can no longer
    // initiate a stock take, same as any genuinely pre-v11 business.
    assert!(crate::stock_take::initiate(&mut conn, &biz, &owner_uid).is_err());

    // Re-running migrations from this simulated "current = 10" state
    // must re-apply v11's backfill for this already-existing business.
    crate::db_migrations::run(&mut conn).unwrap();

    let patched_json: String = conn
        .query_row("SELECT schema_json FROM modules WHERE business_id = ?1 AND id = 'inventory'", [&biz], |r| r.get(0))
        .unwrap();
    let patched: serde_json::Value = serde_json::from_str(&patched_json).unwrap();
    assert!(
        patched["actions"].as_array().unwrap().iter().any(|a| a.as_str() == Some("stocktake")),
        "schema_json snapshot must be patched to include 'stocktake' for a pre-existing business"
    );

    // And the functional proof: Owner can now actually use the
    // feature, not just that a JSON blob looks right.
    let result = crate::stock_take::initiate(&mut conn, &biz, &owner_uid);
    assert!(result.is_ok(), "Owner role must be backfilled with the 'stocktake' permission: {result:?}");
}

#[test]
fn test_get_open_returns_none_when_nothing_is_in_progress() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    seed_inventory_item(&conn, &biz, "ITEM-001", "Item", 10, 100, 200);

    assert!(crate::stock_take::get_open(&conn, &biz, &uid).unwrap().is_none());

    let initiated = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    let open = crate::stock_take::get_open(&conn, &biz, &uid).unwrap();
    assert!(open.is_some());
    assert_eq!(open.unwrap()["id"], initiated["id"]);
}

#[test]
fn test_history_lists_past_stock_takes_most_recent_first() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    seed_inventory_item(&conn, &biz, "ITEM-001", "Item", 10, 100, 200);

    let first = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    crate::stock_take::close(&mut conn, &biz, &uid, first["id"].as_str().unwrap()).unwrap();
    let second = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();

    let history = crate::stock_take::list(&conn, &biz, &uid).unwrap();
    let entries = history["stock_takes"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["id"], second["id"], "most recent (still in_progress) must come first");
    assert_eq!(entries[0]["status"].as_str().unwrap(), "in_progress");
    assert_eq!(entries[1]["status"].as_str().unwrap(), "closed");
}

#[test]
fn test_staff_role_cannot_initiate_a_stock_take() {
    // "stocktake" is granted to Owner/Manager by default, not Staff —
    // matching the same trust boundary as sell/receive/repack.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let hash = crate::auth::hash_secret("password123").unwrap();
    let staff_uid = crate::business_panel::add_user(&conn, &biz, "staffer", &hash, "Staff").unwrap();

    let result = crate::stock_take::initiate(&mut conn, &biz, &staff_uid);
    assert!(result.is_err());
}
