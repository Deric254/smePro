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
    receive(&mut conn, &biz, &uid, &po_b, Some(25), Some("2025-01-01"));
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
    receive(&mut conn, &biz, &uid, &po_id, Some(900), Some("2026-01-01"));

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
