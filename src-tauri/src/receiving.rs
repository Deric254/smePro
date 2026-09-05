//! Receiving stock — the buying-side counterpart to `pos.rs`.
//!
//! Before this, recording a purchase order and actually having stock
//! show up in Inventory were two completely disconnected actions —
//! marking a purchase "received" didn't touch inventory at all,
//! exactly the same gap `pos.rs` closed on the selling side. This is
//! that fix for the buying side: `receive()` increases the linked
//! inventory item's quantity AND marks the purchase order received, in
//! one transaction. Either both happen or neither does.
//!
//! CORRECTION to an earlier version of this comment: it used to claim
//! the generic module engine has no way to stop someone with plain
//! "update" on Purchasing from flipping `received` through the generic
//! record-update endpoint instead of calling this function. That's no
//! longer true (and, per crud.rs's own doc comment on
//! `is_update_blocked_field`, was the actual bug that comment names and
//! fixes): the engine now DOES have a concept of "this field can only
//! change through its dedicated action" — `crud::update()` unconditionally
//! rejects any attempt to set `purchasing.received` directly, regardless
//! of the caller's role or permissions, for every single-record update.
//! The only way `received` becomes true is through this function.
//!
//! SECOND FIX, same file: `receive()` used to update Inventory's
//! `quantity` but never touch `unit_cost` at all — the recorded cost
//! silently went stale the moment a supplier's price changed on any
//! repeat order, with nothing to warn anyone it had. It now computes a
//! real weighted average across the stock already on hand and what
//! just arrived, the same correctness this crate already applies to
//! stock and money everywhere else.
//!
//! THIRD FIX, same file, same shape as repack.rs's own rounding fix:
//! blending two costs into one rounded-to-the-cent `new_unit_cost` and
//! multiplying it back out by `new_qty` doesn't always land on the
//! exact value that was actually on hand plus actually paid for — a
//! few cents can appear or vanish from the stock valuation on every
//! receipt, purely from rounding. The exact remainder is now computed
//! and, when nonzero, posted to Bookkeeping as its own "Stock
//! Revaluation" entry (a rounding gain as income, a loss as an
//! expense) — never silently absorbed. Note this is separate from the
//! Purchasing expense entry below, which was already exact (it's
//! quantity received × the PO's own unit cost, not the blended
//! average) — this fix is specifically for the inventory *valuation*
//! side, not the cash side.
//!
//! FOURTH FIX, same file: the core of this logic is now split out into
//! `receive_in_tx`, which runs against a `Transaction` the CALLER
//! already owns, rather than only ever being reachable through
//! `receive()`'s own newly-opened one. This is what lets
//! `excel_import::import()` call it directly, once per newly-created
//! Purchasing row, inside the single big transaction the whole import
//! already runs in — so a bulk-imported purchase order and its stock
//! arriving are one atomic step, not "import creates it unreceived,
//! then someone has to click Receive on each of what might be 150
//! rows." `receive()` itself is now a thin wrapper: open a
//! transaction, call `receive_in_tx`, commit, audit-log. Same math,
//! same Bookkeeping posting, same rounding reconciliation, whichever
//! caller reaches it — one implementation, so there's no way for the
//! two paths to quietly drift out of sync with each other.

use crate::crud;
use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;

/// Generates the next PO number for a business. Format: PO-1, PO-2,
/// etc. Scoped per business, exactly the same shape and the same
/// bug-avoidance as `invoice::generate_number` — see that function's
/// own doc comment for the full reasoning. Copied rather than shared
/// because the two operate on different tables/columns/prefixes, not
/// because the logic itself differs: take the MAX of every po_number
/// ever assigned to this business (deleted or not) and go one past it,
/// rather than a live-row COUNT, so a deleted purchase order's number
/// is retired, never silently reused by the next one created.
/// `CAST(... AS INTEGER)` on anything not shaped like `PO-<n>` safely
/// evaluates to 0 in SQLite rather than erroring.
pub fn generate_po_number(conn: &Connection, business_id: &str) -> Result<String> {
    let max_existing: i64 = conn.query_row(
        "SELECT COALESCE(MAX(CAST(SUBSTR(po_number, 4) AS INTEGER)), 0)
         FROM module_purchasing WHERE business_id = ?1 AND po_number LIKE 'PO-%'",
        params![business_id],
        |r| r.get(0),
    )?;
    Ok(format!("PO-{}", max_existing + 1))
}

#[derive(Debug, Deserialize)]
pub struct ReceiveRequest {
    pub purchase_record_id: String,
    /// Defaults to the purchase order's own recorded quantity — present
    /// as a separate field for the real-world case of a partial
    /// delivery (ordered 100, only 60 showed up), so what actually
    /// arrived isn't forced to match what was ordered.
    #[serde(default)]
    pub quantity_received: Option<i64>,
}

/// Runs the whole receive-stock operation as one atomic transaction.
pub fn receive(conn: &mut Connection, business_id: &str, user_id: &str, req: ReceiveRequest) -> Result<Value> {
    // Same pattern as checkout: one purpose-built permission on the
    // module actually being financially affected (Inventory gaining
    // stock), not a combination of two separate modules' grants.
    crate::rbac::require(conn, user_id, "inventory", "receive")?;

    let purchasing_module = crud::load_module(conn, business_id, "purchasing")
        .map_err(|_| anyhow!("the Purchasing module isn't enabled for this business"))?;
    let inventory_module = crud::load_module(conn, business_id, "inventory")
        .map_err(|_| anyhow!("the Inventory module isn't enabled for this business — receiving needs it"))?;
    let purchasing_table = purchasing_module.table_name();
    let inventory_table = inventory_module.table_name();

    let tx = conn.transaction()?;
    let summary = receive_in_tx(
        &tx,
        business_id,
        &purchasing_table,
        &inventory_table,
        &req.purchase_record_id,
        req.quantity_received,
    )?;
    // Same discipline as checkout() and repack(): nothing above is
    // durable until this line.
    tx.commit()?;

    let _ = crate::audit::log(conn, business_id, Some(user_id), "_receiving", "receive", Some(&req.purchase_record_id), Some(&summary));

    Ok(summary)
}

/// The actual receive logic, runnable against a `Transaction` the
/// caller already owns — see this file's own "FOURTH FIX" doc comment
/// for why this is split out from `receive()` above: it's what lets
/// `excel_import::import()` call this directly, once per newly-created
/// Purchasing row, inside the ONE transaction the whole import already
/// runs in, so a bulk-imported order and its stock arriving happen
/// atomically together rather than needing a separate Receive click
/// per row afterward. Takes table names rather than re-deriving them
/// from `ModuleDef`s, since `excel_import::import()` already has both
/// on hand (the module it's importing, plus a lookup of the other) and
/// there's no reason to load either module definition twice per row of
/// a large import. Does NOT check rbac or commit/audit-log — those are
/// each caller's own responsibility (a single manual receive checks
/// "receive" once and audit-logs once per call; a bulk import checks
/// "receive" once for the whole batch up front and audit-logs once per
/// row actually received, inside the loop) — this function is purely
/// the atomic stock-and-cost mechanics both share.
pub(crate) fn receive_in_tx(
    tx: &rusqlite::Transaction<'_>,
    business_id: &str,
    purchasing_table: &str,
    inventory_table: &str,
    purchase_record_id: &str,
    quantity_received_override: Option<i64>,
) -> Result<Value> {
    let row: Option<(String, i64, bool, Option<String>, String, i64)> = tx
        .query_row(
            &format!(
                "SELECT item_name, quantity, received, inventory_record_id, supplier, unit_cost
                 FROM {purchasing_table} WHERE id = ?1 AND business_id = ?2 AND deleted_at IS NULL"
            ),
            params![purchase_record_id, business_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .optional()?;
    let Some((item_name, ordered_qty, already_received, inventory_record_id, supplier, po_unit_cost)) = row else {
        return Err(anyhow!("purchase order not found: {purchase_record_id}"));
    };

    if already_received {
        return Err(anyhow!("this purchase order was already marked received — receiving it again would double-count the stock"));
    }

    let Some(inventory_record_id) = inventory_record_id else {
        return Err(anyhow!(
            "this purchase order isn't linked to an Inventory item (no inventory_record_id) — link it first so receiving can update the right stock"
        ));
    };

    let quantity_received = quantity_received_override.unwrap_or(ordered_qty);
    if quantity_received <= 0 {
        return Err(anyhow!("quantity received must be greater than zero"));
    }

    let inv_row: Option<(String, i64, i64)> = tx
        .query_row(
            &format!("SELECT name, quantity, unit_cost FROM {inventory_table} WHERE id = ?1 AND business_id = ?2 AND deleted_at IS NULL"),
            params![inventory_record_id, business_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((inventory_name, current_qty, current_unit_cost)) = inv_row else {
        return Err(anyhow!("linked inventory item not found: {inventory_record_id}"));
    };

    let new_qty = current_qty + quantity_received;
    // A real weighted average, not a silent overwrite -- if this
    // exact item was already in stock at one cost and this delivery
    // came in at a different one (supplier price changes, a different
    // batch, anything), the recorded cost after this needs to reflect
    // both quantities fairly, not just whichever one was written most
    // recently. When current_qty is 0 (first-ever receipt, or fully
    // sold out before now), this naturally reduces to exactly the new
    // delivery's cost -- no special case needed, the zero contributes
    // nothing to the weighted sum.
    // Integer cents throughout (see money.rs) — the weighted-average
    // numerator is an exact i64 product-sum, no float ever involved.
    // Plain integer division truncates toward zero, which would
    // silently shave fractions of a cent off the recorded cost every
    // single time this runs; adding half the divisor before dividing
    // rounds to the nearest cent instead, the one deliberate rounding
    // point in this calculation.
    let numerator = current_qty * current_unit_cost + quantity_received * po_unit_cost;
    let new_unit_cost = (numerator + new_qty / 2) / new_qty;
    tx.execute(
        &format!("UPDATE {inventory_table} SET quantity = ?1, unit_cost = ?2, updated_at = datetime('now') WHERE id = ?3 AND business_id = ?4"),
        params![new_qty, new_unit_cost, inventory_record_id, business_id],
    )?;

    tx.execute(
        &format!("UPDATE {purchasing_table} SET received = 1, updated_at = datetime('now') WHERE id = ?1 AND business_id = ?2"),
        params![purchase_record_id, business_id],
    )?;

    // Same Bookkeeping auto-post as checkout() and process_refund(),
    // same reasoning: one expense entry for what was actually paid to
    // the supplier for this delivery (quantity received × the PO's
    // unit cost, not the new weighted-average — this is the real cash
    // outlay, not the recalculated stock valuation). Best-effort: a
    // business without Bookkeeping enabled can still receive stock.
    if let Ok(accounting_module) = crud::load_module(&tx, business_id, "accounting") {
        let mut entry: HashMap<String, Value> = HashMap::new();
        entry.insert("description".into(), json!(format!("Purchase received — {item_name} from {supplier}")));
        entry.insert("entry_type".into(), json!("expense"));
        entry.insert("category".into(), json!("Purchasing"));
        entry.insert("amount".into(), json!(quantity_received * po_unit_cost));
        for f in &accounting_module.fields {
            if !entry.contains_key(&f.name) {
                if let Some(d) = &f.default {
                    entry.insert(f.name.clone(), d.clone());
                }
            }
        }
        accounting_module.validate(&entry)?;
        crate::reference_data::validate_field_references(&tx, business_id, &accounting_module, &entry)?;
        crud::insert_validated_record(&tx, business_id, &accounting_module, &entry)?;
    }

    // Same reconciliation discipline as repack.rs: `new_qty *
    // new_unit_cost` (what actually gets stored) doesn't always equal
    // `numerator` (what was actually on hand plus actually paid for) —
    // rounding the blended cost to the nearest cent can land a few
    // cents above or below the exact value. Positive means the stored
    // valuation came in LOWER than the true value (an unrecorded
    // loss); negative means it came in HIGHER (value from nowhere).
    // Posted as its own labeled Bookkeeping entry whenever nonzero, so
    // the stock ledger and the books always reconcile to the cent.
    let stored_inventory_value = new_qty * new_unit_cost;
    let rounding_adjustment_cents = numerator - stored_inventory_value;
    if rounding_adjustment_cents != 0 {
        if let Ok(accounting_module) = crud::load_module(&tx, business_id, "accounting") {
            let (entry_type, amount) = if rounding_adjustment_cents > 0 {
                ("expense", rounding_adjustment_cents)
            } else {
                ("income", -rounding_adjustment_cents)
            };
            let mut entry: HashMap<String, Value> = HashMap::new();
            entry.insert(
                "description".into(),
                json!(format!(
                    "Receiving rounding {} — {inventory_name}",
                    if rounding_adjustment_cents > 0 { "loss" } else { "gain" }
                )),
            );
            entry.insert("entry_type".into(), json!(entry_type));
            entry.insert("category".into(), json!("Stock Revaluation"));
            entry.insert("amount".into(), json!(amount));
            for f in &accounting_module.fields {
                if !entry.contains_key(&f.name) {
                    if let Some(d) = &f.default {
                        entry.insert(f.name.clone(), d.clone());
                    }
                }
            }
            accounting_module.validate(&entry)?;
            crate::reference_data::validate_field_references(&tx, business_id, &accounting_module, &entry)?;
            crud::insert_validated_record(&tx, business_id, &accounting_module, &entry)?;
        }
    }

    let summary = json!({
        "purchase_record_id": purchase_record_id,
        "item_name": item_name,
        "supplier": supplier,
        "inventory_record_id": inventory_record_id,
        "inventory_name": inventory_name,
        "quantity_ordered": ordered_qty,
        "quantity_received": quantity_received,
        "new_stock_level": new_qty,
        "partial_delivery": quantity_received != ordered_qty,
        "received_at_unit_cost": po_unit_cost,
        "new_weighted_average_cost": new_unit_cost,
        "exact_value_on_hand": numerator,
        "stored_inventory_value": stored_inventory_value,
        "rounding_adjustment_cents": rounding_adjustment_cents,
        "rounding_adjustment_posted_to_bookkeeping": rounding_adjustment_cents != 0,
    });

    // Committing and audit-logging are each caller's own responsibility
    // — see this function's own doc comment for why (a manual receive
    // commits/logs once per call; a bulk import commits once for the
    // whole batch and logs once per row, inside its own loop).
    Ok(summary)
}
