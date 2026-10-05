use super::common::*;
use serde_json::json;

// None of what this file tests had any coverage before this feature
// shipped — see the batch-costing spec's own "Acceptance criteria"
// section, which explicitly calls out FEFO ordering, multi-batch sale
// splitting, repack consuming across batches, and refund re-crediting
// as required test coverage.

fn make_purchase_order(conn: &mut rusqlite::Connection, biz: &str, uid: &str, inv_id: &str, item_name: &str, qty: i64, unit_cost_cents: i64, unit_price_cents: i64) -> String {
    let mut po = serde_json::Map::new();
    po.insert("supplier".into(), json!("Test Supplier"));
    po.insert("item_name".into(), json!(item_name));
    po.insert("inventory_record_id".into(), json!(inv_id));
    po.insert("quantity".into(), json!(qty));
    po.insert("unit_cost".into(), json!(unit_cost_cents));
    // Required since v31_purchasing_unit_price.
    po.insert("unit_price".into(), json!(unit_price_cents));
    crate::crud::create(conn, biz, uid, "purchasing", &po).unwrap()
}

fn receive(
    conn: &mut rusqlite::Connection,
    biz: &str,
    uid: &str,
    po_id: &str,
    unit_price: Option<i64>,
    expiry_date: Option<&str>,
) -> serde_json::Value {
    let req = crate::receiving::ReceiveRequest {
        purchase_record_id: po_id.to_string(),
        quantity_received: None,
        unit_price,
        expiry_date: expiry_date.map(|s| s.to_string()),
        on_credit: false,
        due_date: None,
    };
    crate::receiving::receive(conn, biz, uid, req).unwrap()
}

fn checkout_one(conn: &mut rusqlite::Connection, biz: &str, uid: &str, inv_id: &str, qty: i64) -> serde_json::Value {
    let req = crate::pos::CheckoutRequest {
        idempotency_key: None,
        discount_pct: None,
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id.to_string(), quantity: qty }],
        payment_method: Some("Cash".into()),
        customer: None,
        customer_phone: None,
        allow_oversell: false,
        on_credit: false,
        due_date: None,
    };
    crate::pos::checkout(conn, biz, uid, req).unwrap()
}

/// Confirms `Inventory.quantity` still equals legacy + every live
/// batch's own remaining quantity — the one invariant the whole
/// legacy/batch coexistence design depends on (see batches.rs's own
/// module doc comment). Checked after every step in the tests below
/// that move stock, not just once at the end.
fn assert_quantity_invariant(conn: &rusqlite::Connection, biz: &str, uid: &str, inv_id: &str) {
    let list = crate::crud::list(conn, biz, uid, "inventory", None, 50, 0).unwrap();
    let item = list.iter().find(|r| r["id"] == json!(inv_id)).unwrap();
    let total_quantity = item["quantity"].as_i64().unwrap();
    let summary = crate::batches::list_batches(conn, biz, uid, inv_id).unwrap();
    let legacy = summary["legacy_quantity"].as_i64().unwrap();
    let batches_total: i64 = summary["batches"].as_array().unwrap().iter()
        .map(|b| b["quantity_remaining"].as_i64().unwrap())
        .sum();
    assert_eq!(total_quantity, legacy + batches_total, "Inventory.quantity must always equal legacy + live batch quantity");
}

#[test]
fn test_fefo_consumes_soonest_expiry_first_then_undated_then_legacy_last() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    // 5 legacy units at cost 10, price 20 — must sell dead last.
    let inv_id = seed_inventory_item(&conn, &biz, "YOG-001", "Yogurt", 5, 10, 20);

    // Batch A: expires far in the future.
    let po_a = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Yogurt", 5, 12, 22);
    receive(&mut conn, &biz, &uid, &po_a, Some(22), Some("2030-01-01"));
    // Batch B: expires soonest — must sell FIRST, ahead of A even
    // though A was received first.
    let po_b = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Yogurt", 5, 14, 25);
    receive(&mut conn, &biz, &uid, &po_b, Some(25), Some("2029-01-01"));
    // Batch C: no expiry at all — sells after every dated batch, but
    // still ahead of legacy.
    let po_c = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Yogurt", 5, 16, 28);
    receive(&mut conn, &biz, &uid, &po_c, Some(28), None);

    assert_quantity_invariant(&conn, &biz, &uid, &inv_id);
    let list = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    assert_eq!(list[0]["quantity"].as_i64().unwrap(), 20, "5 legacy + 5+5+5 across three batches");

    // Sell 12: must draw 5 from B (soonest expiry), then 5 from A,
    // then 2 from C — never touching legacy, never touching C's
    // remaining 3.
    let result = checkout_one(&mut conn, &biz, &uid, &inv_id, 12);
    assert_eq!(result["subtotal"].as_i64().unwrap(), 5 * 25 + 5 * 22 + 2 * 28, "revenue must reflect each portion's own price, in FEFO order");

    let sales = crate::crud::list(&conn, &biz, &uid, "sales", None, 50, 0).unwrap();
    assert_eq!(sales.len(), 3, "one sales row per portion consumed");
    let mut by_cost: Vec<i64> = sales.iter().map(|s| s["unit_price"].as_i64().unwrap()).collect();
    by_cost.sort();
    assert_eq!(by_cost, vec![22, 25, 28]);
    let total_cost_at_sale: i64 = sales.iter().map(|s| s["cost_at_sale"].as_i64().unwrap()).sum();
    assert_eq!(total_cost_at_sale, 5 * 14 + 5 * 12 + 2 * 16, "cost must reflect each portion's own batch cost, not a blended figure");

    let summary = crate::batches::list_batches(&conn, &biz, &uid, &inv_id).unwrap();
    assert_eq!(summary["batches"].as_array().unwrap().len(), 1, "A and B are now fully consumed and drop out of the live list");
    assert_eq!(summary["batches"][0]["quantity_remaining"].as_i64().unwrap(), 3, "C has 3 left (5 - 2)");
    assert_eq!(summary["legacy_quantity"].as_i64().unwrap(), 5, "legacy must be completely untouched — FEFO never reached it");
    assert_quantity_invariant(&conn, &biz, &uid, &inv_id);
}

#[test]
fn test_update_batch_price_enforces_price_floor_and_owner_only_cost_edits() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let hash = crate::auth::hash_secret("password123").unwrap();
    let manager_id = crate::business_panel::add_user(&conn, &biz, "manager", &hash, "Manager").unwrap();

    let inv_id = seed_inventory_item(&conn, &biz, "SOAP-100", "Soap", 0, 0, 500);
    let po_id = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Soap", 20, 100, 150);
    let receive_result = receive(&mut conn, &biz, &uid, &po_id, Some(150), None);
    let batch_id = receive_result["batch_id"].as_str().unwrap().to_string();

    // Manager can edit price alone.
    let price_only = crate::batches::UpdateBatchPriceRequest { batch_id: batch_id.clone(), unit_price: 130, unit_cost: None };
    crate::batches::update_batch_price(&mut conn, &biz, &manager_id, price_only).unwrap();
    let summary = crate::batches::list_batches(&conn, &biz, &uid, &inv_id).unwrap();
    assert_eq!(summary["batches"][0]["unit_price"].as_i64().unwrap(), 130);
    assert_eq!(summary["batches"][0]["unit_cost"].as_i64().unwrap(), 100, "cost must be untouched by a price-only edit");

    // Manager attempting a cost edit is rejected — cost edits are
    // Owner-only per the spec, even though "update_batch_price" itself
    // is granted to Manager for price.
    let manager_cost_edit = crate::batches::UpdateBatchPriceRequest { batch_id: batch_id.clone(), unit_price: 130, unit_cost: Some(90) };
    assert!(crate::batches::update_batch_price(&mut conn, &biz, &manager_id, manager_cost_edit).is_err(), "a Manager must not be able to edit a batch's cost");

    // Owner can edit both.
    let owner_cost_edit = crate::batches::UpdateBatchPriceRequest { batch_id: batch_id.clone(), unit_price: 140, unit_cost: Some(90) };
    crate::batches::update_batch_price(&mut conn, &biz, &uid, owner_cost_edit).unwrap();
    let summary = crate::batches::list_batches(&conn, &biz, &uid, &inv_id).unwrap();
    assert_eq!(summary["batches"][0]["unit_cost"].as_i64().unwrap(), 90);
    assert_eq!(summary["batches"][0]["unit_price"].as_i64().unwrap(), 140);

    // Price below the batch's own (just-edited) cost must be rejected,
    // same rule create_batch_in_tx already enforces at receipt time.
    let below_cost = crate::batches::UpdateBatchPriceRequest { batch_id, unit_price: 50, unit_cost: None };
    assert!(crate::batches::update_batch_price(&mut conn, &biz, &uid, below_cost).is_err(), "a batch can never be priced below its own cost");
}

#[test]
fn test_update_batch_price_blocked_while_a_stock_take_is_open() {
    // close()'s FEFO consumption reads a batch's live unit_cost/
    // unit_price at close time to price a write-off — see
    // stock_take.rs::close() and this guard's own comment in
    // batches.rs. A price/cost edit landing mid-count must not be
    // allowed to change that out from under an in-progress reconciliation.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = seed_inventory_item(&conn, &biz, "SOAP-100", "Soap", 0, 0, 500);
    let po_id = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Soap", 20, 100, 150);
    let receive_result = receive(&mut conn, &biz, &uid, &po_id, Some(150), None);
    let batch_id = receive_result["batch_id"].as_str().unwrap().to_string();

    crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();

    let edit = crate::batches::UpdateBatchPriceRequest { batch_id: batch_id.clone(), unit_price: 200, unit_cost: None };
    let result = crate::batches::update_batch_price(&mut conn, &biz, &uid, edit);
    assert!(result.is_err(), "batch price/cost must be frozen while a stock take is open");
    assert!(result.unwrap_err().to_string().contains("stock take"));

    let summary = crate::batches::list_batches(&conn, &biz, &uid, &inv_id).unwrap();
    assert_eq!(summary["batches"][0]["unit_price"].as_i64().unwrap(), 150, "the blocked edit must not have partially applied");
}

#[test]
fn test_refund_credits_back_the_exact_batch_it_was_sold_from() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = seed_inventory_item(&conn, &biz, "JUICE-001", "Juice", 0, 0, 800);
    let po_id = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Juice", 10, 500, 800);
    receive(&mut conn, &biz, &uid, &po_id, Some(800), None);
    assert_quantity_invariant(&conn, &biz, &uid, &inv_id);

    let sale = checkout_one(&mut conn, &biz, &uid, &inv_id, 4);
    let sale_id = sale["items"][0]["sale_id"].as_str().unwrap().to_string();

    let summary_after_sale = crate::batches::list_batches(&conn, &biz, &uid, &inv_id).unwrap();
    assert_eq!(summary_after_sale["batches"][0]["quantity_remaining"].as_i64().unwrap(), 6, "10 - 4 sold");
    assert_quantity_invariant(&conn, &biz, &uid, &inv_id);

    let refund_req = crate::refund::RefundRequest {
        sale_id,
        quantity: 3,
        refund_amount: 3 * 800,
        reason: Some("customer changed mind".into()),
        restock: true,
    };
    crate::refund::process_refund(&mut conn, &biz, &uid, refund_req).unwrap();

    // The 3 returned units must land back on the SAME batch, not as
    // a generic bump to Inventory.quantity or a new legacy unit.
    let summary_after_refund = crate::batches::list_batches(&conn, &biz, &uid, &inv_id).unwrap();
    assert_eq!(summary_after_refund["batches"].as_array().unwrap().len(), 1, "still the same one batch, not a second one created");
    assert_eq!(summary_after_refund["batches"][0]["quantity_remaining"].as_i64().unwrap(), 9, "6 + 3 refunded");
    assert_eq!(summary_after_refund["legacy_quantity"].as_i64().unwrap(), 0, "a batch-sourced refund must never inflate legacy");
    assert_quantity_invariant(&conn, &biz, &uid, &inv_id);
}

#[test]
fn test_repack_consumes_source_batches_via_fefo_and_produces_an_independent_target_batch() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    // Source: 2 legacy units at cost 200, plus a batch of 3 at cost 300
    // (soonest-expiring, so it must be consumed before legacy).
    let source_id = seed_inventory_item(&conn, &biz, "CHEESE-WHEEL", "Cheese (wheel)", 2, 200, 900);
    let po_id = make_purchase_order(&mut conn, &biz, &uid, &source_id, "Cheese (wheel)", 3, 300, 900);
    receive(&mut conn, &biz, &uid, &po_id, Some(900), Some("2029-06-01"));

    let target_id = seed_inventory_item(&conn, &biz, "CHEESE-SLICE", "Cheese (sliced)", 0, 0, 50);

    // Consume 3 — exactly the batch's own quantity, so this repack
    // should draw entirely from the batch and never touch the 2
    // legacy units.
    let req = crate::repack::RepackRequest {
        source_record_id: source_id.clone(),
        source_quantity: 3,
        target_record_id: Some(target_id.clone()),
        target_quantity_produced: 30,
        new_target_name: None,
        new_target_unit_price: None,
        notes: None,
        ..Default::default()
    };
    let result = crate::repack::repack(&mut conn, &biz, &uid, req).unwrap();
    // 3 * 300 = 900 consumed / 30 produced = 30 exactly.
    assert_eq!(result["new_batch_unit_cost"].as_i64().unwrap(), 30);

    let source_summary = crate::batches::list_batches(&conn, &biz, &uid, &source_id).unwrap();
    assert_eq!(source_summary["batches"].as_array().unwrap().len(), 0, "the source's one batch is now fully consumed");
    assert_eq!(source_summary["legacy_quantity"].as_i64().unwrap(), 2, "the 2 legacy units must be completely untouched — FEFO consumed the batch first");

    let target_summary = crate::batches::list_batches(&conn, &biz, &uid, &target_id).unwrap();
    assert_eq!(target_summary["batches"][0]["quantity_remaining"].as_i64().unwrap(), 30);
    assert_eq!(target_summary["batches"][0]["unit_cost"].as_i64().unwrap(), 30);

    assert_quantity_invariant(&conn, &biz, &uid, &source_id);
    assert_quantity_invariant(&conn, &biz, &uid, &target_id);
}

#[test]
fn test_pos_lookup_products_reflects_live_front_of_queue_price_not_stale_legacy_price() {
    // The bug this guards against: the POS product grid (pos::lookup_products)
    // used to read Inventory's own `unit_price` column directly — correct
    // only until an item's first batch exists, after which it could show a
    // cashier a different price than checkout() would actually charge.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    // 5 legacy units at price 20 — this is what the OLD lookup_products
    // would have kept showing forever, even once a differently-priced
    // batch became the active FEFO tier.
    let inv_id = seed_inventory_item(&conn, &biz, "SODA-001", "Soda", 5, 10, 20);

    // Before any batch exists, lookup_products must show the legacy price.
    let before = crate::pos::lookup_products(&conn, &biz, &uid, Some("Soda"), 10).unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0]["unit_price"].as_i64().unwrap(), 20, "no batch yet — legacy price");

    // A new delivery arrives, priced differently from legacy and with an
    // expiry date, so it's now the front-of-queue batch.
    let po = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Soda", 5, 12, 35);
    receive(&mut conn, &biz, &uid, &po, Some(35), Some("2030-01-01"));

    let after = crate::pos::lookup_products(&conn, &biz, &uid, Some("Soda"), 10).unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(
        after[0]["unit_price"].as_i64().unwrap(),
        35,
        "a live batch now exists and must be reflected immediately, not the stale legacy price"
    );

    // And it must be the SAME number an actual sale would charge for the
    // very next unit — the whole point of this fix.
    let sale = checkout_one(&mut conn, &biz, &uid, &inv_id, 1);
    assert_eq!(sale["subtotal"].as_i64().unwrap(), 35, "checkout must charge exactly what the grid displayed");

    // Sell out the entire front batch — the grid must fall back to
    // legacy's price the instant the batch is exhausted, again matching
    // exactly what the next sale would actually charge.
    let _ = checkout_one(&mut conn, &biz, &uid, &inv_id, 4);
    let after_exhausted = crate::pos::lookup_products(&conn, &biz, &uid, Some("Soda"), 10).unwrap();
    assert_eq!(
        after_exhausted[0]["unit_price"].as_i64().unwrap(),
        20,
        "batch fully consumed — must fall back to legacy price, not keep showing the exhausted batch's price"
    );

    assert_quantity_invariant(&conn, &biz, &uid, &inv_id);
}


// ---------------------------------------------------------------------
// Expired stock can never be sold or repacked; repacked units inherit
// the expiry of the stock they were made from.
// ---------------------------------------------------------------------

fn try_checkout(conn: &mut rusqlite::Connection, biz: &str, uid: &str, inv_id: &str, qty: i64, allow_oversell: bool) -> anyhow::Result<serde_json::Value> {
    let req = crate::pos::CheckoutRequest {
        idempotency_key: None,
        discount_pct: None,
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id.to_string(), quantity: qty }],
        payment_method: Some("Cash".into()),
        customer: None,
        customer_phone: None,
        allow_oversell,
        on_credit: false,
        due_date: None,
    };
    crate::pos::checkout(conn, biz, uid, req)
}

fn local_date_offset(days: i64) -> String {
    (chrono::Local::now().date_naive() + chrono::Duration::days(days)).to_string()
}

#[test]
fn test_checkout_refuses_expired_batch_and_sells_the_fresh_one() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let inv_id = seed_inventory_item(&conn, &biz, "MILK-001", "Milk", 0, 0, 100);

    // Expired batch of 5 (would be FIRST in FEFO order if it were sellable).
    let po_old = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Milk", 5, 50, 80);
    receive(&mut conn, &biz, &uid, &po_old, Some(80), Some("2000-01-01"));
    // Fresh batch of 5.
    let po_new = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Milk", 5, 60, 90);
    receive(&mut conn, &biz, &uid, &po_new, Some(90), Some("2999-12-31"));

    // 5 sellable: selling 5 must come entirely from the fresh batch, at ITS price.
    let sale = checkout_one(&mut conn, &biz, &uid, &inv_id, 5);
    assert_eq!(sale["subtotal"].as_i64().unwrap(), 5 * 90, "must be charged at the fresh batch's price, never the expired one's");

    // 6th unit: only expired stock is left — must be refused, with a message that says why.
    let err = try_checkout(&mut conn, &biz, &uid, &inv_id, 1, false).unwrap_err().to_string();
    assert!(err.contains("expired"), "error must explain expired stock is why: {err}");

    // Nothing moved: the expired batch is untouched and still on the books.
    let summary = crate::batches::list_batches(&conn, &biz, &uid, &inv_id).unwrap();
    assert_eq!(summary["batches"].as_array().unwrap().len(), 1);
    assert_eq!(summary["batches"][0]["quantity_remaining"].as_i64().unwrap(), 5);
    assert_quantity_invariant(&conn, &biz, &uid, &inv_id);
}

#[test]
fn test_allow_oversell_never_dips_into_expired_stock() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let inv_id = seed_inventory_item(&conn, &biz, "CHEESE-9", "Cheese", 0, 0, 100);
    let po = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Cheese", 4, 50, 80);
    receive(&mut conn, &biz, &uid, &po, Some(80), Some("2000-01-01"));

    // Only expired stock exists. Even with oversell on, the 4 expired units must NOT be drawn.
    let _ = try_checkout(&mut conn, &biz, &uid, &inv_id, 2, true);
    let summary = crate::batches::list_batches(&conn, &biz, &uid, &inv_id).unwrap();
    assert_eq!(summary["batches"][0]["quantity_remaining"].as_i64().unwrap(), 4, "expired batch must be untouched by an oversold sale");
}

#[test]
fn test_a_batch_is_still_sellable_on_its_expiry_date_and_expired_the_day_after() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let today_item = seed_inventory_item(&conn, &biz, "BREAD-T", "Bread today", 0, 0, 100);
    let po = make_purchase_order(&mut conn, &biz, &uid, &today_item, "Bread today", 3, 50, 80);
    receive(&mut conn, &biz, &uid, &po, Some(80), Some(&local_date_offset(0)));
    checkout_one(&mut conn, &biz, &uid, &today_item, 1); // expires TODAY: still sellable

    let yday_item = seed_inventory_item(&conn, &biz, "BREAD-Y", "Bread yesterday", 0, 0, 100);
    let po = make_purchase_order(&mut conn, &biz, &uid, &yday_item, "Bread yesterday", 3, 50, 80);
    receive(&mut conn, &biz, &uid, &po, Some(80), Some(&local_date_offset(-1)));
    assert!(try_checkout(&mut conn, &biz, &uid, &yday_item, 1, false).is_err(), "expired yesterday: must be refused");
}

#[test]
fn test_pos_lookup_ignores_expired_batches_for_price_and_quantity() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let inv_id = seed_inventory_item(&conn, &biz, "JUICE-1", "Juice", 0, 0, 100);
    let po_old = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Juice", 5, 50, 70);
    receive(&mut conn, &biz, &uid, &po_old, Some(70), Some("2000-01-01"));
    let po_new = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Juice", 3, 60, 95);
    receive(&mut conn, &biz, &uid, &po_new, Some(95), Some("2999-12-31"));

    let rows = crate::pos::lookup_products(&conn, &biz, &uid, Some("Juice"), 10).unwrap();
    assert_eq!(rows[0]["unit_price"].as_i64().unwrap(), 95, "grid must show the price checkout will actually charge, not the expired batch's");
    assert_eq!(rows[0]["quantity"].as_f64().unwrap() as i64, 3, "grid must show sellable quantity only");
}

#[test]
fn test_stock_take_can_still_write_off_expired_stock() {
    // The block must not trap expired stock on the books forever:
    // plain FEFO consumption (what stock take uses) still reaches it.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let inv_id = seed_inventory_item(&conn, &biz, "YOG-X", "Yogurt", 0, 0, 100);
    let po = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Yogurt", 4, 50, 80);
    receive(&mut conn, &biz, &uid, &po, Some(80), Some("2000-01-01"));

    let tx = conn.transaction().unwrap();
    let portions = crate::batches::fefo_consume_in_tx(&tx, &biz, &inv_id, 4, 4, 50, 80).unwrap();
    assert_eq!(portions.iter().map(|p| p.quantity).sum::<i64>(), 4);
    tx.rollback().unwrap();
}

#[test]
fn test_invalid_expiry_date_format_is_rejected_at_receiving() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let inv_id = seed_inventory_item(&conn, &biz, "TEA-1", "Tea", 0, 0, 100);
    let po = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Tea", 4, 50, 80);
    let req = crate::receiving::ReceiveRequest {
        purchase_record_id: po,
        quantity_received: None,
        unit_price: Some(80),
        expiry_date: Some("31/12/2030".into()),
        on_credit: false,
        due_date: None,
    };
    assert!(crate::receiving::receive(&mut conn, &biz, &uid, req).is_err(), "a non-ISO date would compare wrong and let expired stock through");
}

fn repack_one(conn: &mut rusqlite::Connection, biz: &str, uid: &str, source: &str, qty: i64, target: &str, produced: i64) -> anyhow::Result<serde_json::Value> {
    crate::repack::repack(conn, biz, uid, crate::repack::RepackRequest {
        source_record_id: source.to_string(),
        source_quantity: qty,
        target_record_id: Some(target.to_string()),
        target_quantity_produced: produced,
        new_target_name: None,
        new_target_unit_price: None,
        notes: None,
        ..Default::default()
    })
}

#[test]
fn test_repacked_units_inherit_the_source_batch_expiry() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let source = seed_inventory_item(&conn, &biz, "RICE-SACK", "Rice sack", 0, 0, 900);
    let po = make_purchase_order(&mut conn, &biz, &uid, &source, "Rice sack", 2, 300, 900);
    receive(&mut conn, &biz, &uid, &po, Some(900), Some("2031-03-15"));
    let target = seed_inventory_item(&conn, &biz, "RICE-BAG", "Rice bag", 0, 0, 40);

    repack_one(&mut conn, &biz, &uid, &source, 1, &target, 10).unwrap();

    let summary = crate::batches::list_batches(&conn, &biz, &uid, &target).unwrap();
    assert_eq!(summary["batches"][0]["expiry_date"].as_str().unwrap(), "2031-03-15", "repacked units must carry the source batch's expiry");
}

#[test]
fn test_new_item_created_by_repack_inherits_expiry_category_currency_and_reorder_level() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let source = seed_inventory_item(&conn, &biz, "BEANS-SACK", "Beans sack", 0, 0, 900);
    conn.execute(
        "UPDATE module_inventory SET category = 'Grains', currency = 'KES', reorder_level = 12 WHERE id = ?1",
        rusqlite::params![source],
    )
    .unwrap();
    let po = make_purchase_order(&mut conn, &biz, &uid, &source, "Beans sack", 2, 300, 900);
    receive(&mut conn, &biz, &uid, &po, Some(900), Some("2031-03-15"));

    let result = crate::repack::repack(
        &mut conn,
        &biz,
        &uid,
        crate::repack::RepackRequest {
            source_record_id: source,
            source_quantity: 1,
            target_record_id: None,
            target_quantity_produced: 10,
            new_target_name: Some("Beans 1kg".into()),
            new_target_unit_price: Some(60),
            notes: None,
            ..Default::default()
        },
    )
    .unwrap();
    let target = result["target_record_id"].as_str().unwrap().to_string();

    let item = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0)
        .unwrap()
        .into_iter()
        .find(|r| r["id"] == json!(target))
        .unwrap();
    assert_eq!(item["category"].as_str().unwrap(), "Grains");
    assert_eq!(item["currency"].as_str().unwrap(), "KES");
    assert_eq!(item["reorder_level"].as_i64().unwrap(), 12, "re-order level comes from the mother item, not the module default of 5");
    // 1 sack costing 300 broken into 10 bags: 30 each, at the 60 entered.
    assert_eq!(item["unit_cost"].as_i64().unwrap(), 30);
    assert_eq!(item["unit_price"].as_i64().unwrap(), 60);
    assert_eq!(item["quantity"].as_i64().unwrap(), 10);

    let summary = crate::batches::list_batches(&conn, &biz, &uid, &target).unwrap();
    assert_eq!(summary["batches"][0]["expiry_date"].as_str().unwrap(), "2031-03-15");
}

#[test]
fn test_repack_inherits_an_item_level_expiry_date_when_the_schema_has_that_field() {
    // Some businesses' saved Inventory schema still carries an
    // item-level `expiry_date` field (the stored schema outlives the
    // shipped inventory.json), with the expiry typed on the item
    // itself and no batch behind it.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    conn.execute("ALTER TABLE module_inventory ADD COLUMN expiry_date TEXT", []).unwrap();
    conn.execute(
        "UPDATE modules SET schema_json = json_insert(schema_json, '$.fields[#]', json('{\"name\":\"expiry_date\",\"type\":\"date\",\"required\":false}')) WHERE id = 'inventory'",
        [],
    )
    .unwrap();
    let source = seed_inventory_item(&conn, &biz, "SUGAR-2KG", "Sugar 2kg", 5, 200, 500);
    conn.execute("UPDATE module_inventory SET expiry_date = '2031-01-15' WHERE id = ?1", rusqlite::params![source]).unwrap();

    let result = crate::repack::repack(
        &mut conn,
        &biz,
        &uid,
        crate::repack::RepackRequest {
            source_record_id: source,
            source_quantity: 1,
            target_record_id: None,
            target_quantity_produced: 2,
            new_target_name: Some("Sugar 1kg".into()),
            new_target_unit_price: Some(150),
            notes: None,
            ..Default::default()
        },
    )
    .unwrap();
    let target = result["target_record_id"].as_str().unwrap().to_string();

    let item = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0)
        .unwrap()
        .into_iter()
        .find(|r| r["id"] == json!(target))
        .unwrap();
    assert_eq!(item["expiry_date"].as_str().unwrap(), "2031-01-15");
    let summary = crate::batches::list_batches(&conn, &biz, &uid, &target).unwrap();
    assert_eq!(summary["batches"][0]["expiry_date"].as_str().unwrap(), "2031-01-15");
}

#[test]
fn test_repack_spanning_batches_inherits_the_earliest_expiry() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let source = seed_inventory_item(&conn, &biz, "FLOUR-S", "Flour sack", 0, 0, 900);
    let po_a = make_purchase_order(&mut conn, &biz, &uid, &source, "Flour sack", 1, 300, 900);
    receive(&mut conn, &biz, &uid, &po_a, Some(900), Some("2032-01-01"));
    let po_b = make_purchase_order(&mut conn, &biz, &uid, &source, "Flour sack", 1, 300, 900);
    receive(&mut conn, &biz, &uid, &po_b, Some(900), Some("2030-06-01"));
    let target = seed_inventory_item(&conn, &biz, "FLOUR-B", "Flour bag", 0, 0, 40);

    // Repacking 2 sacks draws from BOTH batches: the new units get the earlier date.
    repack_one(&mut conn, &biz, &uid, &source, 2, &target, 20).unwrap();
    let summary = crate::batches::list_batches(&conn, &biz, &uid, &target).unwrap();
    assert_eq!(summary["batches"][0]["expiry_date"].as_str().unwrap(), "2030-06-01");
}

#[test]
fn test_repack_of_undated_stock_stays_undated_and_expired_source_is_refused() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let source = seed_inventory_item(&conn, &biz, "SUGAR-S", "Sugar sack", 3, 200, 500); // legacy, no expiry
    let target = seed_inventory_item(&conn, &biz, "SUGAR-B", "Sugar bag", 0, 0, 20);
    repack_one(&mut conn, &biz, &uid, &source, 1, &target, 10).unwrap();
    let summary = crate::batches::list_batches(&conn, &biz, &uid, &target).unwrap();
    assert!(summary["batches"][0]["expiry_date"].is_null(), "no source expiry means no invented one");

    let expired_src = seed_inventory_item(&conn, &biz, "OIL-S", "Oil drum", 0, 0, 900);
    let po = make_purchase_order(&mut conn, &biz, &uid, &expired_src, "Oil drum", 2, 300, 900);
    receive(&mut conn, &biz, &uid, &po, Some(900), Some("2000-01-01"));
    let oil_target = seed_inventory_item(&conn, &biz, "OIL-B", "Oil bottle", 0, 0, 40);
    let err = repack_one(&mut conn, &biz, &uid, &expired_src, 1, &oil_target, 10).unwrap_err().to_string();
    assert!(err.contains("expired"), "expired stock must not be repackable: {err}");
}

// ---------------------------------------------------------------------
// Stock take reasons, the expiry report's totals, and the one-touch
// "write off all expired".
// ---------------------------------------------------------------------

fn item_quantity(conn: &rusqlite::Connection, inv_id: &str) -> i64 {
    conn.query_row("SELECT quantity FROM module_inventory WHERE id = ?1", rusqlite::params![inv_id], |r| r.get(0)).unwrap()
}

#[test]
fn test_stock_take_stores_the_chosen_reason_and_rejects_an_unknown_one() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let inv_id = seed_inventory_item(&conn, &biz, "SOAP-1", "Soap", 10, 50, 80);

    let st = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    let st_id = st["id"].as_str().unwrap().to_string();
    assert_eq!(st["kind"].as_str().unwrap(), "count");
    assert!(st["reasons"].as_array().unwrap().iter().any(|r| r["code"] == "expired"), "the dropdown's list must come from the server and include 'expired'");
    let item_id = st["items"][0]["id"].as_str().unwrap().to_string();

    let bad = crate::stock_take::record_count(&conn, &biz, &uid, crate::stock_take::RecordCountRequest {
        stock_take_id: st_id.clone(), item_id: item_id.clone(), counted_qty: 7, reason: Some("because".into()),
    });
    assert!(bad.is_err(), "a reason that isn't in the list must be rejected");

    crate::stock_take::record_count(&conn, &biz, &uid, crate::stock_take::RecordCountRequest {
        stock_take_id: st_id.clone(), item_id, counted_qty: 7, reason: Some("damaged".into()),
    }).unwrap();
    let closed = crate::stock_take::close(&mut conn, &biz, &uid, &st_id).unwrap();
    assert_eq!(closed["adjustments"][0]["reason"].as_str().unwrap(), "damaged");
    assert_eq!(closed["adjustments"][0]["reason_label"].as_str().unwrap(), "Damaged");
    assert_eq!(item_quantity(&conn, &inv_id), 7);
}

#[test]
fn test_write_off_expired_clears_only_expired_batches_at_their_own_cost() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let a = seed_inventory_item(&conn, &biz, "A-1", "Alpha", 0, 0, 100);
    let po1 = make_purchase_order(&mut conn, &biz, &uid, &a, "Alpha", 5, 50, 80);
    receive(&mut conn, &biz, &uid, &po1, Some(80), Some("2000-01-01")); // expired, cost 50
    let po2 = make_purchase_order(&mut conn, &biz, &uid, &a, "Alpha", 3, 70, 90);
    receive(&mut conn, &biz, &uid, &po2, Some(90), Some("2001-01-01")); // expired, cost 70
    let po3 = make_purchase_order(&mut conn, &biz, &uid, &a, "Alpha", 4, 60, 95);
    receive(&mut conn, &biz, &uid, &po3, Some(95), Some("2999-12-31")); // fresh
    let b = seed_inventory_item(&conn, &biz, "B-1", "Bravo", 6, 40, 70); // no expiry at all

    // Preview changes NOTHING but reports exactly what the real run will do.
    let preview = crate::stock_take::write_off_expired(&mut conn, &biz, &uid, true).unwrap();
    assert_eq!(preview["units_written_off"].as_i64().unwrap(), 8);
    assert_eq!(preview["total_write_off_cost"].as_i64().unwrap(), 5 * 50 + 3 * 70);
    assert!(preview["stock_take_id"].is_null());
    assert_eq!(item_quantity(&conn, &a), 12, "preview must not touch stock");
    let takes: i64 = conn.query_row("SELECT COUNT(*) FROM stock_takes", [], |r| r.get(0)).unwrap();
    assert_eq!(takes, 0, "preview must not create a stock take");

    let done = crate::stock_take::write_off_expired(&mut conn, &biz, &uid, false).unwrap();
    assert_eq!(done["units_written_off"].as_i64().unwrap(), 8);
    assert_eq!(done["total_write_off_cost"].as_i64().unwrap(), 460);
    assert_eq!(done["items_written_off"].as_i64().unwrap(), 1);

    // Only the 8 expired units left; the fresh batch and the undated item are untouched.
    assert_eq!(item_quantity(&conn, &a), 4);
    assert_eq!(item_quantity(&conn, &b), 6);
    assert_quantity_invariant(&conn, &biz, &uid, &a);
    let remaining = crate::batches::list_batches(&conn, &biz, &uid, &a).unwrap();
    assert_eq!(remaining["batches"].as_array().unwrap().len(), 1);
    assert_eq!(remaining["batches"][0]["quantity_remaining"].as_i64().unwrap(), 4);

    // Recorded like any other write-off: closed stock take, reason, cost, ledger, profit.
    let (status, kind): (String, String) = conn.query_row("SELECT status, kind FROM stock_takes", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((status.as_str(), kind.as_str()), ("closed", "expired_write_off"));
    let (reason, cost): (String, i64) = conn.query_row("SELECT reason, write_off_cost_cents FROM stock_take_items", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((reason.as_str(), cost), ("expired", 460));
    let ledger: i64 = conn.query_row(
        "SELECT SUM(quantity_delta) FROM stock_movements WHERE movement_type = 'expired_write_off' AND inventory_record_id = ?1",
        rusqlite::params![a], |r| r.get(0)).unwrap();
    assert_eq!(ledger, -8);
    assert_eq!(crate::profit::summary(&conn, &biz, &uid).unwrap().shrinkage_cents, 460, "the loss must reach the profit report");

    // Pressing it again is harmless: nothing left, nothing written, no empty stock take.
    let again = crate::stock_take::write_off_expired(&mut conn, &biz, &uid, false).unwrap();
    assert_eq!(again["items_written_off"].as_i64().unwrap(), 0);
    let takes: i64 = conn.query_row("SELECT COUNT(*) FROM stock_takes", [], |r| r.get(0)).unwrap();
    assert_eq!(takes, 1);
}

#[test]
fn test_write_off_expired_is_blocked_during_an_open_stock_take_and_for_staff() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let inv_id = seed_inventory_item(&conn, &biz, "X-1", "Xylo", 0, 0, 100);
    let po = make_purchase_order(&mut conn, &biz, &uid, &inv_id, "Xylo", 2, 50, 80);
    receive(&mut conn, &biz, &uid, &po, Some(80), Some("2000-01-01"));

    let hash = crate::auth::hash_secret("password123").unwrap();
    let staff = crate::business_panel::add_user(&conn, &biz, "staffer", &hash, "Staff").unwrap();
    assert!(crate::stock_take::write_off_expired(&mut conn, &biz, &staff, false).is_err(), "staff may not write stock off");

    crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    assert!(crate::stock_take::write_off_expired(&mut conn, &biz, &uid, false).is_err(), "no stock movement while a count is open");
    assert_eq!(item_quantity(&conn, &inv_id), 2);
}

#[test]
fn test_expiry_report_totals_cover_everything_not_just_the_listed_page() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let today = crate::batches::local_today();

    let a = seed_inventory_item(&conn, &biz, "R-A", "Ra", 0, 0, 100);
    let b = seed_inventory_item(&conn, &biz, "R-B", "Rb", 0, 0, 100);
    for (inv, name, qty, cost, exp) in [
        (&a, "Ra", 5, 50, "2000-01-01".to_string()),            // expired
        (&a, "Ra", 2, 60, local_date_offset(-1)),               // expired yesterday
        (&b, "Rb", 4, 70, "2001-06-01".to_string()),            // expired
        (&b, "Rb", 3, 80, local_date_offset(0)),                // expires today: NOT expired
        (&b, "Rb", 6, 90, local_date_offset(10)),               // expiring soon
        (&b, "Rb", 9, 90, "2999-12-31".to_string()),            // far future: in neither
    ] {
        let po = make_purchase_order(&mut conn, &biz, &uid, inv, name, qty, cost, cost + 20);
        receive(&mut conn, &biz, &uid, &po, Some(cost + 20), Some(&exp));
    }

    // limit = 1 lists a single row, but the totals still describe everything.
    let report = crate::stock_health::expiring_batches(&conn, &biz, &uid, &today, 30, 1).unwrap();
    assert_eq!(report.items.len(), 1);
    assert_eq!(report.items[0].expiry_date, "2000-01-01", "expired batches sort first");
    assert!(report.items[0].days_to_expiry < 0);
    let s = report.summary;
    assert_eq!(s.expired_batches, 3);
    assert_eq!(s.expired_items, 2);
    assert_eq!(s.expired_units, 5 + 2 + 4);
    assert_eq!(s.expired_cost_value, 5 * 50 + 2 * 60 + 4 * 70);
    assert_eq!(s.expiring_batches, 2, "today + 10 days away are 'expiring'; the far-future batch is not");
    assert_eq!(s.expiring_units, 3 + 6);
}


// ---------------------------------------------------------------------
// One-touch "write off expired", stock-take reasons, and the expiring
// report's complete totals.
// ---------------------------------------------------------------------

fn seed_expired_and_fresh(conn: &mut rusqlite::Connection, biz: &str, uid: &str) -> (String, String) {
    // Item A: an expired batch of 5 (cost 50) plus a fresh batch of 3 (cost 60).
    let a = seed_inventory_item(conn, biz, "MILK-W", "Milk", 0, 0, 100);
    let po = make_purchase_order(conn, biz, uid, &a, "Milk", 5, 50, 80);
    receive(conn, biz, uid, &po, Some(80), Some("2000-01-01"));
    let po = make_purchase_order(conn, biz, uid, &a, "Milk", 3, 60, 90);
    receive(conn, biz, uid, &po, Some(90), Some("2999-12-31"));
    // Item B: only fresh stock — must never be touched.
    let b = seed_inventory_item(conn, biz, "TEA-W", "Tea", 0, 0, 100);
    let po = make_purchase_order(conn, biz, uid, &b, "Tea", 4, 40, 70);
    receive(conn, biz, uid, &po, Some(70), Some("2999-12-31"));
    (a, b)
}

fn item_qty(conn: &rusqlite::Connection, biz: &str, uid: &str, id: &str) -> i64 {
    let list = crate::crud::list(conn, biz, uid, "inventory", None, 50, 0).unwrap();
    list.into_iter().find(|r| r["id"] == serde_json::json!(id)).unwrap()["quantity"].as_i64().unwrap()
}

#[test]
fn test_write_off_expired_removes_only_expired_units_and_records_everything() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let (a, b) = seed_expired_and_fresh(&mut conn, &biz, &uid);
    assert_eq!(item_qty(&conn, &biz, &uid, &a), 8);

    // Preview: full numbers, zero effect.
    let preview = crate::stock_take::write_off_expired(&mut conn, &biz, &uid, true).unwrap();
    assert_eq!(preview["units_written_off"].as_i64().unwrap(), 5);
    assert_eq!(preview["total_write_off_cost"].as_i64().unwrap(), 5 * 50);
    assert!(preview["stock_take_id"].is_null());
    assert_eq!(item_qty(&conn, &biz, &uid, &a), 8, "a preview must change nothing");
    let takes: i64 = conn.query_row("SELECT COUNT(*) FROM stock_takes WHERE business_id = ?1", [&biz], |r| r.get(0)).unwrap();
    assert_eq!(takes, 0, "a preview must not leave a stock take behind");

    // The real thing matches the preview exactly.
    let done = crate::stock_take::write_off_expired(&mut conn, &biz, &uid, false).unwrap();
    assert_eq!(done["units_written_off"], preview["units_written_off"]);
    assert_eq!(done["total_write_off_cost"], preview["total_write_off_cost"]);
    assert_eq!(done["items_written_off"].as_i64().unwrap(), 1);

    // Only the 5 expired units left; the fresh batch and the other item are untouched.
    assert_eq!(item_qty(&conn, &biz, &uid, &a), 3);
    assert_eq!(item_qty(&conn, &biz, &uid, &b), 4);
    let batches = crate::batches::list_batches(&conn, &biz, &uid, &a).unwrap();
    assert_eq!(batches["batches"].as_array().unwrap().len(), 1, "expired batch is gone from the live list");
    assert_eq!(batches["batches"][0]["quantity_remaining"].as_i64().unwrap(), 3);
    assert_quantity_invariant(&conn, &biz, &uid, &a);

    // Recorded as a closed stock take of its own kind, line reason "expired", with the cost.
    let (status, kind): (String, String) = conn.query_row(
        "SELECT status, kind FROM stock_takes WHERE business_id = ?1", [&biz], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((status.as_str(), kind.as_str()), ("closed", "expired_write_off"));
    let (reason, cost): (String, i64) = conn.query_row(
        "SELECT reason, write_off_cost_cents FROM stock_take_items", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((reason.as_str(), cost), ("expired", 250));

    // Stock movement ledger and profit report both see it.
    let (mtype, delta): (String, i64) = conn.query_row(
        "SELECT movement_type, quantity_delta FROM stock_movements WHERE business_id = ?1 AND movement_type = 'expired_write_off'", [&biz], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((mtype.as_str(), delta), ("expired_write_off", -5));
    let profit = crate::profit::summary(&conn, &biz, &uid).unwrap();
    assert_eq!(profit.shrinkage_cents, 250, "the loss must reach the profit report");

    // Pressing it again is harmless: nothing left, no new stock take.
    let again = crate::stock_take::write_off_expired(&mut conn, &biz, &uid, false).unwrap();
    assert_eq!(again["items_written_off"].as_i64().unwrap(), 0);
    let takes: i64 = conn.query_row("SELECT COUNT(*) FROM stock_takes WHERE business_id = ?1", [&biz], |r| r.get(0)).unwrap();
    assert_eq!(takes, 1);
}

#[test]
fn test_write_off_expired_is_blocked_while_a_stock_take_is_open() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let (a, _b) = seed_expired_and_fresh(&mut conn, &biz, &uid);
    crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    let err = crate::stock_take::write_off_expired(&mut conn, &biz, &uid, false).unwrap_err().to_string();
    assert!(err.contains("stock take is in progress"), "{err}");
    assert_eq!(item_qty(&conn, &biz, &uid, &a), 8, "nothing may change");
}

#[test]
fn test_stock_take_reasons_are_validated_stored_and_reported() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    seed_inventory_item(&conn, &biz, "RICE-R", "Rice", 10, 100, 200);
    let st = crate::stock_take::initiate(&mut conn, &biz, &uid).unwrap();
    let codes: Vec<&str> = st["reasons"].as_array().unwrap().iter().map(|r| r["code"].as_str().unwrap()).collect();
    assert!(codes.contains(&"expired") && codes.contains(&"miscount"), "the server must supply the dropdown list");
    let st_id = st["id"].as_str().unwrap().to_string();
    let item_id = st["items"][0]["id"].as_str().unwrap().to_string();

    let bad = crate::stock_take::record_count(&conn, &biz, &uid, crate::stock_take::RecordCountRequest {
        stock_take_id: st_id.clone(), item_id: item_id.clone(), counted_qty: 8, reason: Some("because".into()) });
    assert!(bad.is_err(), "an unknown reason code must be rejected");

    crate::stock_take::record_count(&conn, &biz, &uid, crate::stock_take::RecordCountRequest {
        stock_take_id: st_id.clone(), item_id, counted_qty: 8, reason: Some("miscount".into()) }).unwrap();
    let closed = crate::stock_take::close(&mut conn, &biz, &uid, &st_id).unwrap();
    assert_eq!(closed["adjustments"][0]["reason"].as_str().unwrap(), "miscount");
    assert_eq!(closed["adjustments"][0]["reason_label"].as_str().unwrap(), "Miscount / recount correction");
}

#[test]
fn test_expiry_report_totals_are_complete_even_when_the_list_is_cut_short() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let (_a, _b) = seed_expired_and_fresh(&mut conn, &biz, &uid); // 1 expired batch: 5 units @ 50
    let c = seed_inventory_item(&conn, &biz, "EGG-E", "Eggs", 0, 0, 100);
    let po = make_purchase_order(&mut conn, &biz, &uid, &c, "Eggs", 2, 30, 60);
    receive(&mut conn, &biz, &uid, &po, Some(60), Some("1999-06-01")); // expired: 2 @ 30
    let po = make_purchase_order(&mut conn, &biz, &uid, &c, "Eggs", 6, 30, 60);
    receive(&mut conn, &biz, &uid, &po, Some(60), Some(&local_date_offset(0))); // expires today: NOT expired
    let po = make_purchase_order(&mut conn, &biz, &uid, &c, "Eggs", 7, 30, 60);
    receive(&mut conn, &biz, &uid, &po, Some(60), Some(&local_date_offset(10))); // expiring soon

    let report = crate::stock_health::expiring_batches(&conn, &biz, &uid, &crate::batches::local_today(), 30, 1).unwrap();
    assert_eq!(report.items.len(), 1, "the list honours its limit");
    assert_eq!(report.items[0].days_to_expiry < 0, true, "expired batches sort first");
    assert_eq!(report.summary.expired_batches, 2);
    assert_eq!(report.summary.expired_items, 2);
    assert_eq!(report.summary.expired_units, 7);
    assert_eq!(report.summary.expired_cost_value, 5 * 50 + 2 * 30);
    assert_eq!(report.summary.expiring_batches, 2, "today's batch and the +10d batch (fresh 2999 batches are outside 30 days)");
    assert_eq!(report.summary.expiring_units, 13);
}

fn repack_from(conn: &mut rusqlite::Connection, biz: &str, uid: &str, source: &str, batch: &str, qty: i64, target: &str, produced: i64) -> anyhow::Result<serde_json::Value> {
    crate::repack::repack(conn, biz, uid, crate::repack::RepackRequest {
        source_record_id: source.to_string(),
        source_batch_id: Some(batch.to_string()),
        source_quantity: qty,
        target_record_id: Some(target.to_string()),
        target_quantity_produced: produced,
        ..Default::default()
    })
}

fn batch_remaining(conn: &rusqlite::Connection, biz: &str, uid: &str, inv: &str, batch: &str) -> i64 {
    let summary = crate::batches::list_batches(conn, biz, uid, inv).unwrap();
    summary["batches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["id"] == json!(batch))
        .map(|b| b["quantity_remaining"].as_i64().unwrap())
        .unwrap_or(0)
}

#[test]
fn test_repack_draws_from_the_chosen_batch_not_the_front_of_the_queue() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let source = seed_inventory_item(&conn, &biz, "SACK", "Sack", 0, 0, 900);
    let target = seed_inventory_item(&conn, &biz, "BAG", "Bag", 0, 0, 120);
    // Front of the FEFO queue (soonest expiry), then a later batch the
    // person actually picks.
    let po_front = make_purchase_order(&mut conn, &biz, &uid, &source, "Sack", 3, 400, 900);
    receive(&mut conn, &biz, &uid, &po_front, Some(900), Some("2031-01-01"));
    let po_later = make_purchase_order(&mut conn, &biz, &uid, &source, "Sack", 5, 500, 900);
    receive(&mut conn, &biz, &uid, &po_later, Some(900), Some("2032-01-01"));

    let summary = crate::batches::list_batches(&conn, &biz, &uid, &source).unwrap();
    let front_id = summary["batches"][0]["id"].as_str().unwrap().to_string();
    let later_id = summary["batches"][1]["id"].as_str().unwrap().to_string();

    let result = repack_from(&mut conn, &biz, &uid, &source, &later_id, 2, &target, 20).unwrap();

    assert_eq!(batch_remaining(&conn, &biz, &uid, &source, &later_id), 3, "the chosen batch is the one consumed");
    assert_eq!(batch_remaining(&conn, &biz, &uid, &source, &front_id), 3, "the front batch is untouched");
    // Costed on the chosen batch's own cost: 2 × 500 over 20 bags.
    assert_eq!(result["new_batch_unit_cost"].as_i64().unwrap(), 50);
    assert_eq!(result["new_batch_expiry_date"].as_str().unwrap(), "2032-01-01");
}

#[test]
fn test_repack_rejects_a_chosen_batch_that_cannot_cover_the_quantity() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let source = seed_inventory_item(&conn, &biz, "SACK", "Sack", 0, 0, 900);
    let target = seed_inventory_item(&conn, &biz, "BAG", "Bag", 0, 0, 120);
    let po_a = make_purchase_order(&mut conn, &biz, &uid, &source, "Sack", 1, 400, 900);
    receive(&mut conn, &biz, &uid, &po_a, Some(900), Some("2031-01-01"));
    let po_b = make_purchase_order(&mut conn, &biz, &uid, &source, "Sack", 5, 500, 900);
    receive(&mut conn, &biz, &uid, &po_b, Some(900), Some("2032-01-01"));

    let summary = crate::batches::list_batches(&conn, &biz, &uid, &source).unwrap();
    let small_id = summary["batches"][0]["id"].as_str().unwrap().to_string();

    // The item has 6 in total, but the chosen batch only has 1: it must
    // fail rather than quietly spill into another batch.
    assert!(repack_from(&mut conn, &biz, &uid, &source, &small_id, 2, &target, 20).is_err());
    assert_eq!(batch_remaining(&conn, &biz, &uid, &source, &small_id), 1);
    let item = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0)
        .unwrap()
        .into_iter()
        .find(|r| r["id"] == json!(source))
        .unwrap();
    assert_eq!(item["quantity"].as_i64().unwrap(), 6, "a rejected repack changes nothing");
}

#[test]
fn test_repack_rejects_a_batch_belonging_to_another_item() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let source = seed_inventory_item(&conn, &biz, "SACK", "Sack", 0, 0, 900);
    let other = seed_inventory_item(&conn, &biz, "OTHER", "Other", 0, 0, 900);
    let target = seed_inventory_item(&conn, &biz, "BAG", "Bag", 0, 0, 120);
    let po_s = make_purchase_order(&mut conn, &biz, &uid, &source, "Sack", 4, 400, 900);
    receive(&mut conn, &biz, &uid, &po_s, Some(900), None);
    let po_o = make_purchase_order(&mut conn, &biz, &uid, &other, "Other", 4, 400, 900);
    receive(&mut conn, &biz, &uid, &po_o, Some(900), None);

    let other_batch = crate::batches::list_batches(&conn, &biz, &uid, &other).unwrap()["batches"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(repack_from(&mut conn, &biz, &uid, &source, &other_batch, 1, &target, 10).is_err());
    assert_eq!(batch_remaining(&conn, &biz, &uid, &other, &other_batch), 4);
}

#[test]
fn test_repack_can_draw_from_the_legacy_pool_explicitly() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    // 4 legacy units, then a dated batch of 3 that FEFO would sell first.
    let source = seed_inventory_item(&conn, &biz, "SACK", "Sack", 4, 300, 900);
    let target = seed_inventory_item(&conn, &biz, "BAG", "Bag", 0, 0, 120);
    let po = make_purchase_order(&mut conn, &biz, &uid, &source, "Sack", 3, 500, 900);
    receive(&mut conn, &biz, &uid, &po, Some(900), Some("2031-01-01"));

    let result = repack_from(&mut conn, &biz, &uid, &source, crate::batches::LEGACY_SOURCE, 2, &target, 20).unwrap();
    // Costed at the legacy cost (2 × 300 / 20), and the dated batch is untouched.
    assert_eq!(result["new_batch_unit_cost"].as_i64().unwrap(), 30);
    let summary = crate::batches::list_batches(&conn, &biz, &uid, &source).unwrap();
    assert_eq!(summary["legacy_quantity"].as_i64().unwrap(), 2);
    assert_eq!(summary["batches"][0]["quantity_remaining"].as_i64().unwrap(), 3);
}

#[test]
fn test_repack_into_a_new_item_stores_the_chosen_unit() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let source = seed_inventory_item(&conn, &biz, "SACK", "Sack", 5, 300, 900);
    let unit = crate::reference_data::list_units(&conn, &biz).unwrap()[0]["name"].as_str().unwrap().to_string();

    let result = crate::repack::repack(&mut conn, &biz, &uid, crate::repack::RepackRequest {
        source_record_id: source.clone(),
        source_quantity: 1,
        target_quantity_produced: 10,
        new_target_name: Some("Bag 1kg".into()),
        new_target_unit_price: Some(60),
        new_target_unit: Some(unit.clone()),
        ..Default::default()
    })
    .unwrap();
    let target = result["target_record_id"].as_str().unwrap().to_string();
    let item = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0)
        .unwrap()
        .into_iter()
        .find(|r| r["id"] == json!(target))
        .unwrap();
    assert_eq!(item["unit"].as_str().unwrap(), unit);

    // An unknown unit is refused, same as everywhere else units are used.
    let bad = crate::repack::repack(&mut conn, &biz, &uid, crate::repack::RepackRequest {
        source_record_id: source,
        source_quantity: 1,
        target_quantity_produced: 10,
        new_target_name: Some("Bag 2kg".into()),
        new_target_unit_price: Some(120),
        new_target_unit: Some("not-a-unit".into()),
        ..Default::default()
    });
    assert!(bad.is_err());
}
