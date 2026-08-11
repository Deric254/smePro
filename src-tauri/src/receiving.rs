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
//! KNOWN LIMITATION, stated plainly rather than hidden: this closes the
//! gap for the intended path (calling this function). It does NOT
//! prevent someone with "update" permission on Purchasing from manually
//! flipping the `received` checkbox through the generic record-update
//! endpoint without ever calling this — the generic module engine has
//! no concept of "this specific field can only change through this one
//! code path." Closing that fully would need field-level write
//! restrictions the engine doesn't have yet. What this DOES guarantee:
//! every purchase received through the intended flow is atomically
//! correct — the risk that's left is a workaround, not a hole in the
//! main path.
//!
//! SECOND FIX, same file: `receive()` used to update Inventory's
//! `quantity` but never touch `unit_cost` at all — the recorded cost
//! silently went stale the moment a supplier's price changed on any
//! repeat order, with nothing to warn anyone it had. It now computes a
//! real weighted average across the stock already on hand and what
//! just arrived, the same correctness this crate already applies to
//! stock and money everywhere else.

use crate::crud;
use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

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

    let row: Option<(String, i64, bool, Option<String>, String, i64)> = tx
        .query_row(
            &format!(
                "SELECT item_name, quantity, received, inventory_record_id, supplier, unit_cost
                 FROM {purchasing_table} WHERE id = ?1 AND business_id = ?2 AND deleted_at IS NULL"
            ),
            params![req.purchase_record_id, business_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .optional()?;
    let Some((item_name, ordered_qty, already_received, inventory_record_id, supplier, po_unit_cost)) = row else {
        return Err(anyhow!("purchase order not found: {}", req.purchase_record_id));
    };

    if already_received {
        return Err(anyhow!("this purchase order was already marked received — receiving it again would double-count the stock"));
    }

    let Some(inventory_record_id) = inventory_record_id else {
        return Err(anyhow!(
            "this purchase order isn't linked to an Inventory item (no inventory_record_id) — link it first so receiving can update the right stock"
        ));
    };

    let quantity_received = req.quantity_received.unwrap_or(ordered_qty);
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
        params![req.purchase_record_id, business_id],
    )?;

    // Same "commit is the one moment this becomes real" discipline as
    // checkout() — nothing above is durable until this line.
    tx.commit()?;

    let summary = json!({
        "purchase_record_id": req.purchase_record_id,
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
    });

    let _ = crate::audit::log(conn, business_id, Some(user_id), "_receiving", "receive", Some(&req.purchase_record_id), Some(&summary));

    Ok(summary)
}
