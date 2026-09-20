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
fn test_cancel_discards_counts_and_leaves_inventory_untouched() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let rice_id = seed_inventory_item(&conn, &biz, "RICE-001", "Rice", 40, 500, 800);

    let initiated = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    let stock_take_id = initiated["id"].as_str().unwrap().to_string();
    let item_id = initiated["items"][0]["id"].as_str().unwrap().to_string();

    // A count was entered but the take is abandoned before close —
    // this must not leak into inventory.quantity in any form.
    crate::stock_take::record_count(
        &conn, &biz, &uid,
        crate::stock_take::RecordCountRequest { stock_take_id: stock_take_id.clone(), item_id, counted_qty: 12 },
    ).unwrap();

    let cancelled = crate::stock_take::cancel(&mut conn, &biz, &uid, &stock_take_id).unwrap();
    assert_eq!(cancelled["status"].as_str().unwrap(), "cancelled");

    let rice = get_item(&conn, &biz, &uid, &rice_id);
    assert_eq!(rice["quantity"].as_i64().unwrap(), 40, "cancel must never write the discarded count to inventory");
}

#[test]
fn test_cancel_frees_the_lock_for_a_new_stock_take_and_for_checkout() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let inv_id = seed_inventory_item(&conn, &biz, "RICE-001", "Rice", 40, 500, 800);

    let initiated = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    let stock_take_id = initiated["id"].as_str().unwrap().to_string();
    crate::stock_take::cancel(&mut conn, &biz, &uid, &stock_take_id).unwrap();

    // The whole point of cancel: checkout works again immediately...
    let req = crate::pos::CheckoutRequest {
        idempotency_key: None,
        discount_pct: None,
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id, quantity: 1 }],
        payment_method: Some("Cash".into()),
        customer: None,
        customer_phone: None,
        allow_oversell: false,
        on_credit: false,
        due_date: None,
    };
    assert!(crate::pos::checkout(&mut conn, &biz, &uid, req).is_ok());

    // ...and a fresh stock take can be started without "already in
    // progress" complaining about the cancelled one.
    assert!(crate::stock_take::initiate(&mut conn, &biz, &uid).is_ok());
}

#[test]
fn test_cannot_cancel_an_already_closed_stock_take() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    seed_inventory_item(&conn, &biz, "ITEM-001", "Item", 10, 100, 200);

    let initiated = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    let stock_take_id = initiated["id"].as_str().unwrap().to_string();
    crate::stock_take::close(&mut conn, &biz, &uid, &stock_take_id).unwrap();

    let result = crate::stock_take::cancel(&mut conn, &biz, &uid, &stock_take_id);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("already closed"));
}

#[test]
fn test_close_persists_write_off_cost_onto_the_stock_take_item_row() {
    // db_migrations.rs's v34 column exists specifically so this value
    // is a real, permanent, queryable fact — not just something that
    // passes through the close() response and the audit log and is
    // then gone. Checked directly against the row, independent of
    // profit.rs's own tests reading it back through summary()/by_item().
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let rice_id = seed_inventory_item(&conn, &biz, "RICE-001", "Rice", 40, 500, 800);
    let tea_id = seed_inventory_item(&conn, &biz, "TEA-001", "Tea", 20, 200, 350);

    let initiated = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    let stock_take_id = initiated["id"].as_str().unwrap().to_string();
    let items = initiated["items"].as_array().unwrap();
    let rice_item_id = items.iter().find(|i| i["inventory_record_id"].as_str().unwrap() == rice_id).unwrap()["id"].as_str().unwrap().to_string();
    let tea_item_id = items.iter().find(|i| i["inventory_record_id"].as_str().unwrap() == tea_id).unwrap()["id"].as_str().unwrap().to_string();

    // Rice: shrinkage of 10 units at 500 cents = 5000.
    crate::stock_take::record_count(&conn, &biz, &uid, crate::stock_take::RecordCountRequest { stock_take_id: stock_take_id.clone(), item_id: rice_item_id.clone(), counted_qty: 30 }).unwrap();
    // Tea: exact match, no write-off.
    crate::stock_take::record_count(&conn, &biz, &uid, crate::stock_take::RecordCountRequest { stock_take_id: stock_take_id.clone(), item_id: tea_item_id.clone(), counted_qty: 20 }).unwrap();
    crate::stock_take::close(&mut conn, &biz, &uid, &stock_take_id).unwrap();

    let rice_write_off: i64 = conn.query_row("SELECT write_off_cost_cents FROM stock_take_items WHERE id = ?1", rusqlite::params![rice_item_id], |r| r.get(0)).unwrap();
    let tea_write_off: i64 = conn.query_row("SELECT write_off_cost_cents FROM stock_take_items WHERE id = ?1", rusqlite::params![tea_item_id], |r| r.get(0)).unwrap();
    assert_eq!(rice_write_off, 5000);
    assert_eq!(tea_write_off, 0, "an exact-match count must persist an explicit 0, not leave the column unset");
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
    // DELETE ...version >= 11 here, not just ...version = 11: current
    // is computed as MAX(version) below, so simulating "pre-v11"
    // requires clearing every version at or above it, not just the one
    // row that happened to be the highest at the time this test was
    // first written. Deleting only 11 stopped correctly simulating
    // "current = 10" the moment a later migration (v12) shipped and
    // left its own row sitting above it — MAX(version) would read
    // back as 12, `current < 11` would be false, and v11 would never
    // re-run, which is exactly the bug this fix closes rather than
    // papering over: the technique itself needed to not assume 11 is
    // the newest migration forever.
    conn.execute("DELETE FROM _schema_version WHERE version >= 11", []).unwrap();

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
fn test_close_applies_a_positive_variance_directly_at_legacy_cost() {
    // "Found" stock (counted > expected) is applied directly now —
    // legacy quantity is derived (quantity minus what batches account
    // for), so raising it alone can't desync anything, and a stock
    // take can trust its own count without routing the surplus through
    // Purchasing first.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let rice_id = seed_inventory_item(&conn, &biz, "RICE-001", "Rice", 40, 500, 800);

    let initiated = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    let stock_take_id = initiated["id"].as_str().unwrap().to_string();
    let item_id = initiated["items"][0]["id"].as_str().unwrap().to_string();

    // Physically found MORE than the system expected.
    crate::stock_take::record_count(
        &conn, &biz, &uid,
        crate::stock_take::RecordCountRequest { stock_take_id: stock_take_id.clone(), item_id, counted_qty: 47 },
    ).unwrap();

    let summary = crate::stock_take::close(&mut conn, &biz, &uid, &stock_take_id).unwrap();
    assert_eq!(summary["items_counted"].as_i64().unwrap(), 1);
    assert_eq!(summary["total_variance_units"].as_i64().unwrap(), 7);
    let adj = &summary["adjustments"][0];
    assert_eq!(adj["variance"].as_i64().unwrap(), 7);
    assert!((adj["variance_pct"].as_f64().unwrap() - 17.5).abs() < 0.001, "7/40 * 100 = 17.5%");
    assert_eq!(adj["write_off_cost"].as_i64().unwrap(), 0, "a surplus has no write-off cost — only shrinkage does");

    let rice = get_item(&conn, &biz, &uid, &rice_id);
    assert_eq!(rice["quantity"].as_i64().unwrap(), 47, "a positive variance is now applied directly to quantity");
}

#[test]
fn test_surplus_on_a_sold_out_batch_item_creates_a_new_priced_batch_not_a_zero_price_gap() {
    // THE BUG THIS FIXES: an item created after batches existed always
    // has unit_price = 0 on its own legacy column (see crud.rs) — real
    // pricing lives entirely on its batch(es). If that item's only
    // batch sells all the way down to quantity_remaining = 0, and a
    // stock take then finds MORE physical stock than expected, the old
    // behavior (bump legacy quantity directly) left that surplus
    // priced at nothing: pos.rs's lookup_products has no batch left to
    // read a price from, so it fell through to the permanently-zero
    // legacy column — real, sellable-looking stock showing up as
    // $0.00 at the POS screen.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    // A batch-only item: zero legacy price, exactly like crud::create()
    // would leave it, with all five of its units living on one real,
    // priced batch.
    let item_id = seed_inventory_item(&conn, &biz, "GADGET-001", "Gadget", 5, 0, 0);
    {
        let tx = conn.transaction().unwrap();
        crate::batches::create_batch_in_tx(
            &tx, &biz, &item_id, None, 5, 300, 500, None, "2026-01-01T00:00:00Z", Some(&uid),
        ).unwrap();
        tx.commit().unwrap();
    }

    // Sold all the way down: quantity_remaining hits 0 on that batch,
    // legacy quantity follows it down to 0 too (mirroring what a real
    // series of checkouts would leave behind).
    conn.execute("UPDATE inventory_batches SET quantity_remaining = 0 WHERE inventory_record_id = ?1", rusqlite::params![item_id]).unwrap();
    conn.execute("UPDATE inventory SET quantity = 0 WHERE id = ?1", rusqlite::params![item_id]).unwrap();

    // A physical count finds 20 on the shelf anyway.
    let initiated = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    let stock_take_id = initiated["id"].as_str().unwrap().to_string();
    let stt_item_id = initiated["items"][0]["id"].as_str().unwrap().to_string();
    crate::stock_take::record_count(
        &conn, &biz, &uid,
        crate::stock_take::RecordCountRequest { stock_take_id: stock_take_id.clone(), item_id: stt_item_id, counted_qty: 20 },
    ).unwrap();
    crate::stock_take::close(&mut conn, &biz, &uid, &stock_take_id).unwrap();

    // The surplus must now be real, priced, sellable stock — a new
    // batch at the item's last known price — not an unpriced legacy
    // bump.
    let (new_batch_qty, new_batch_price): (i64, i64) = conn.query_row(
        "SELECT quantity_remaining, unit_price FROM inventory_batches
         WHERE inventory_record_id = ?1 AND quantity_remaining > 0",
        rusqlite::params![item_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert_eq!(new_batch_qty, 20, "the full surplus should land on the new batch");
    assert_eq!(new_batch_price, 500, "priced at the item's last known batch price, not zero");

    // And the exact price the POS screen would show is no longer zero.
    let products = crate::pos::lookup_products(&conn, &biz, &uid, None, 50).unwrap();
    let gadget = products.iter().find(|p| p["id"] == serde_json::json!(item_id)).unwrap();
    assert_eq!(gadget["unit_price"].as_i64().unwrap(), 500, "POS must never show a sold-out-batch item's surplus at $0");
}

#[test]
fn test_close_reports_variance_pct_and_write_off_cost_for_shrinkage() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    // 40 units @ 500 cents cost each.
    seed_inventory_item(&conn, &biz, "RICE-001", "Rice", 40, 500, 800);

    let initiated = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    let stock_take_id = initiated["id"].as_str().unwrap().to_string();
    let item_id = initiated["items"][0]["id"].as_str().unwrap().to_string();

    // 7 units physically missing.
    crate::stock_take::record_count(
        &conn, &biz, &uid,
        crate::stock_take::RecordCountRequest { stock_take_id: stock_take_id.clone(), item_id, counted_qty: 33 },
    ).unwrap();

    let summary = crate::stock_take::close(&mut conn, &biz, &uid, &stock_take_id).unwrap();
    let adj = &summary["adjustments"][0];
    assert_eq!(adj["variance"].as_i64().unwrap(), -7);
    assert!((adj["variance_pct"].as_f64().unwrap() - (-17.5)).abs() < 0.001, "-7/40 * 100 = -17.5%");
    assert_eq!(adj["write_off_cost"].as_i64().unwrap(), 7 * 500, "7 units written off at 500 cents cost each");
    assert_eq!(summary["total_write_off_cost"].as_i64().unwrap(), 7 * 500);
}

#[test]
fn test_close_reports_n_a_variance_pct_when_expected_qty_is_zero() {
    // A zero baseline makes any percentage undefined (divide by zero),
    // not a misleading 0% or an infinite number — must come back as
    // the literal string "n/a".
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    seed_inventory_item(&conn, &biz, "ITEM-001", "Item", 0, 100, 200);

    let initiated = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    let stock_take_id = initiated["id"].as_str().unwrap().to_string();
    let item_id = initiated["items"][0]["id"].as_str().unwrap().to_string();

    crate::stock_take::record_count(
        &conn, &biz, &uid,
        crate::stock_take::RecordCountRequest { stock_take_id: stock_take_id.clone(), item_id, counted_qty: 5 },
    ).unwrap();

    let summary = crate::stock_take::close(&mut conn, &biz, &uid, &stock_take_id).unwrap();
    assert_eq!(summary["adjustments"][0]["variance_pct"].as_str().unwrap(), "n/a");
}

#[test]
fn test_history_rollup_reports_max_and_average_variance_pct() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    seed_inventory_item(&conn, &biz, "RICE-001", "Rice", 40, 500, 800); // will vary -17.5%
    seed_inventory_item(&conn, &biz, "BEANS-001", "Beans", 20, 300, 500); // will vary +25%

    let initiated = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    let stock_take_id = initiated["id"].as_str().unwrap().to_string();
    let items = initiated["items"].as_array().unwrap();
    let rice_item_id = items.iter().find(|i| i["item_name"] == serde_json::json!("Rice")).unwrap()["id"].as_str().unwrap().to_string();
    let beans_item_id = items.iter().find(|i| i["item_name"] == serde_json::json!("Beans")).unwrap()["id"].as_str().unwrap().to_string();

    crate::stock_take::record_count(&conn, &biz, &uid, crate::stock_take::RecordCountRequest { stock_take_id: stock_take_id.clone(), item_id: rice_item_id, counted_qty: 33 }).unwrap();
    crate::stock_take::record_count(&conn, &biz, &uid, crate::stock_take::RecordCountRequest { stock_take_id: stock_take_id.clone(), item_id: beans_item_id, counted_qty: 25 }).unwrap();

    crate::stock_take::close(&mut conn, &biz, &uid, &stock_take_id).unwrap();

    let history = crate::stock_take::list(&conn, &biz, &uid).unwrap();
    let entry = &history["stock_takes"][0];
    assert!((entry["max_variance_pct"].as_f64().unwrap() - 25.0).abs() < 0.001, "worst swing is Beans at +25%, not Rice's -17.5%");
    assert!((entry["avg_variance_pct"].as_f64().unwrap() - 21.25).abs() < 0.001, "(17.5 + 25) / 2 = 21.25");
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

#[test]
fn test_open_stock_take_blocks_checkout() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let inv_id = seed_inventory_item(&conn, &biz, "RICE-001", "Rice", 40, 500, 800);

    crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();

    let req = crate::pos::CheckoutRequest {
        idempotency_key: None,
        discount_pct: None,
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id, quantity: 1 }],
        payment_method: Some("Cash".into()),
        customer: None,
        customer_phone: None,
        allow_oversell: false,
        on_credit: false,
        due_date: None,
    };
    let result = crate::pos::checkout(&mut conn, &biz, &uid, req);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("stock take is in progress"));
}

#[test]
fn test_open_stock_take_blocks_receiving() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let inv_id = seed_inventory_item(&conn, &biz, "RICE-001", "Rice", 40, 500, 800);

    let mut po = serde_json::Map::new();
    po.insert("supplier".into(), serde_json::json!("Test Supplier"));
    po.insert("item_name".into(), serde_json::json!("Rice"));
    po.insert("inventory_record_id".into(), serde_json::json!(inv_id));
    po.insert("quantity".into(), serde_json::json!(10));
    po.insert("unit_cost".into(), serde_json::json!(500));
    po.insert("unit_price".into(), serde_json::json!(800));
    let po_id = crate::crud::create(&conn, &biz, &uid, "purchasing", &po).unwrap();

    crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();

    let req = crate::receiving::ReceiveRequest { purchase_record_id: po_id, quantity_received: None, unit_price: None, expiry_date: None };
    let result = crate::receiving::receive(&mut conn, &biz, &uid, req);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("stock take is in progress"));
}

#[test]
fn test_open_stock_take_blocks_refund() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let inv_id = seed_inventory_item(&conn, &biz, "FLOUR-001", "Flour", 50, 2000, 3000);

    let checkout_req = crate::pos::CheckoutRequest {
        idempotency_key: None,
        discount_pct: None,
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id, quantity: 10 }],
        payment_method: Some("Cash".into()),
        customer: None,
        customer_phone: None,
        allow_oversell: false,
        on_credit: false,
        due_date: None,
    };
    let sale = crate::pos::checkout(&mut conn, &biz, &uid, checkout_req).unwrap();
    let sale_id = sale["items"][0]["sale_id"].as_str().unwrap().to_string();

    crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();

    let req = crate::refund::RefundRequest { sale_id, quantity: 4, refund_amount: 12000, reason: None, restock: true };
    let result = crate::refund::process_refund(&mut conn, &biz, &uid, req);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("stock take is in progress"));
}

#[test]
fn test_open_stock_take_blocks_repack() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let dozen_id = seed_inventory_item(&conn, &biz, "EGG-DOZEN", "Eggs — Dozen", 5, 3000, 4000);
    let single_id = seed_inventory_item(&conn, &biz, "EGG-SINGLE", "Eggs — Single", 0, 0, 400);

    crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();

    let req = crate::repack::RepackRequest {
        source_record_id: dozen_id,
        source_quantity: 1,
        target_record_id: Some(single_id),
        target_quantity_produced: 12,
        new_target_name: None,
        new_target_unit_price: None,
        notes: None,
    };
    let result = crate::repack::repack(&mut conn, &biz, &uid, req);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("stock take is in progress"));
}

