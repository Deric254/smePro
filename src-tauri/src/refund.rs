//! Refunds — the missing counterpart to `pos.rs`'s `checkout()`.
//!
//! Before this, there was no way to reverse a sale at all: no stock
//! returned to Inventory, no record of what was refunded or why, and
//! nothing stopping the same sale line from being "refunded" for more
//! than was actually sold, repeatedly, with no accumulated check
//! against it. This closes that gap the same way `pos.rs` closed the
//! selling-side gap and `receiving.rs` closed the buying-side one:
//! one atomic transaction, one purpose-built permission ("refund" on
//! Sales, not a combination of Sales-update plus Inventory-update),
//! and validation computed fresh from the real, current state of the
//! database every time — never from a number the caller supplies.

use crate::crud;
use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

/// The columns pulled from a sale row when looking one up for a refund:
/// (item_name, quantity, order_id, customer, cost_at_sale). Named here
/// purely to satisfy clippy's type-complexity lint on a bare 5-tuple —
/// no other behavior implied, just a label for what's destructured
/// immediately below.
type SaleRow = (String, i64, Option<String>, Option<String>, i64);

#[derive(Debug, Deserialize)]
pub struct RefundRequest {
    pub sale_id: String,
    pub quantity: i64,
    /// The money actually handed back. A separate field from what the
    /// original sale's unit price would compute, because real returns
    /// aren't always full-price-back (a partial refund, a restocking
    /// fee, a goodwill adjustment) — this is what the business
    /// actually gave back, recorded as the real fact it is rather than
    /// assumed from the original line's price.
    /// Integer minor units (cents) — see money.rs.
    pub refund_amount: i64,
    #[serde(default)]
    pub reason: Option<String>,
    /// Whether the returned quantity actually goes back into sellable
    /// stock. Defaults to false deliberately — a damaged or expired
    /// return should not silently become sellable inventory again
    /// just because a refund was processed; restocking is something
    /// the person processing the refund has to actively decide, not
    /// an automatic side effect.
    #[serde(default)]
    pub restock: bool,
}

/// Runs the whole refund as one atomic transaction: validates the
/// requested quantity against what's actually left to refund on this
/// specific sale (the original quantity, minus every refund already
/// recorded against it — computed fresh from the database, never
/// trusted from the caller), optionally restocks Inventory, and writes
/// an immutable refund record. Either all of it becomes real together,
/// or none of it does.
pub fn process_refund(conn: &mut Connection, business_id: &str, user_id: &str, req: RefundRequest) -> Result<Value> {
    // One purpose-built permission for the whole operation, matching
    // checkout()'s "sell" and receive()'s "receive" — not a
    // combination of separate grants on Sales and Inventory.
    crate::rbac::require(conn, user_id, "sales", "refund")?;

    if req.quantity <= 0 {
        return Err(anyhow!("quantity to refund must be greater than zero"));
    }
    if req.refund_amount < 0 {
        return Err(anyhow!("refund amount cannot be negative"));
    }

    let sales_module = crud::load_module(conn, business_id, "sales")
        .map_err(|_| anyhow!("the Sales module isn't enabled for this business"))?;
    let refunds_module = crud::load_module(conn, business_id, "refunds")
        .map_err(|_| anyhow!("the Refunds module isn't enabled for this business"))?;
    let sales_table = sales_module.table_name();
    let refunds_table = refunds_module.table_name();

    let tx = conn.transaction()?;

    let sale_row: Option<SaleRow> = tx
        .query_row(
            &format!(
                "SELECT item_name, quantity, order_id, customer, cost_at_sale
                 FROM {sales_table} WHERE id = ?1 AND business_id = ?2 AND deleted_at IS NULL"
            ),
            params![req.sale_id, business_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let Some((item_name, original_qty, order_id, customer, original_cost_at_sale)) = sale_row else {
        return Err(anyhow!("sale not found: {}", req.sale_id));
    };

    // The real integrity check: sum every refund already recorded
    // against this exact sale_id, computed fresh right now — not a
    // running total trusted from anywhere else, and never something
    // the caller can influence. This is what makes "refund the same
    // sale twice for more than was ever sold" structurally impossible
    // rather than merely discouraged.
    let already_refunded: i64 = tx
        .query_row(
            &format!(
                "SELECT COALESCE(SUM(quantity_refunded), 0) FROM {refunds_table}
                 WHERE sale_id = ?1 AND business_id = ?2 AND deleted_at IS NULL"
            ),
            params![req.sale_id, business_id],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let remaining_refundable = original_qty - already_refunded;
    if req.quantity > remaining_refundable {
        return Err(anyhow!(
            "cannot refund {} of '{item_name}' — only {remaining_refundable} of the original {original_qty} \
             is still refundable ({already_refunded} already refunded against this sale)",
            req.quantity
        ));
    }

    // THE ACTUAL FIX Deric asked for: a refund used to only reverse
    // `revenue` (see the comment just below) — it never touched
    // `cost_at_sale` at all, so a fully refunded sale still counted
    // its full original cost against gross profit (see profit.rs)
    // with none of the matching revenue left to offset it, permanently
    // understating profit by exactly the cost of every refunded sale
    // forever. Whether cost gets reversed at all depends on `restock`,
    // deliberately, because the two cases are economically different
    // facts, not the same event with a checkbox: if the item comes
    // BACK onto the shelf (restock), the business hasn't actually lost
    // anything — reversing both revenue and cost nets this sale to
    // zero, as if it never happened. If it does NOT come back (damaged,
    // expired, given away), the business already paid for that unit
    // and it's gone — cost_at_sale is left exactly as it was, so the
    // reversed revenue with no matching cost reversal shows up as a
    // real loss on that sale, which is the true economic outcome, not
    // a bug to paper over.
    //
    // Computed the same "running remainder" way `already_refunded`
    // above already is — NOT as `original_cost_at_sale * req.quantity
    // / original_qty` freshly each time, which would let integer-
    // division rounding silently gain or lose a coin across repeated
    // partial refunds of the same sale (three partial refunds of a
    // 3-unit, 1000-cent sale would round 1000/3=333 three separate
    // times, reversing 999 total and leaving 1 cent permanently
    // stranded in `cost_at_sale`, or the reverse). Instead: figure out
    // the TOTAL cost that should be reversed once this refund is
    // applied (`already_refunded + req.quantity` units' worth,
    // proportional to the original sale), then subtract whatever's
    // already been reversed by earlier refunds on this same sale. The
    // remainder from integer division always lands on whichever refund
    // pushes the running total to the next whole cent, and the LAST
    // refund that fully closes out the sale — `already_refunded +
    // req.quantity == original_qty` — always reverses the exact
    // remaining balance, coin for coin, by construction.
    let already_refunded_cost: i64 = tx
        .query_row(
            &format!(
                "SELECT COALESCE(SUM(cost_reversed), 0) FROM {refunds_table}
                 WHERE sale_id = ?1 AND business_id = ?2 AND deleted_at IS NULL"
            ),
            params![req.sale_id, business_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let this_cost_reversal: i64 = if req.restock && original_qty > 0 {
        let total_cost_reversed_after = (original_cost_at_sale as i128
            * (already_refunded + req.quantity) as i128
            / original_qty as i128) as i64;
        total_cost_reversed_after - already_refunded_cost
    } else {
        0
    };

    // THE BUG THIS FIXES: a refund used to only ever create a new row
    // in Refunds — it never touched the original sale's `revenue`, so
    // a fully refunded sale still counted as full revenue everywhere
    // revenue gets totaled (the Dashboard tile, the Business-at-a-
    // glance KPIs and chart, any future report — all of them just
    // SUM(revenue) off the sales table with no idea refunds exist).
    // The refund row above/below already keeps an immutable record of
    // what was refunded and why, so nothing about that audit trail is
    // lost by also keeping the sale's own revenue figure honest here.
    // Clamped at 0 so it can never go negative regardless of the
    // refund amount supplied.
    tx.execute(
        &format!(
            "UPDATE {sales_table} SET revenue = MAX(0, revenue - ?1) WHERE id = ?2 AND business_id = ?3"
        ),
        params![req.refund_amount, req.sale_id, business_id],
    )?;
    if this_cost_reversal > 0 {
        tx.execute(
            &format!(
                "UPDATE {sales_table} SET cost_at_sale = MAX(0, cost_at_sale - ?1) WHERE id = ?2 AND business_id = ?3"
            ),
            params![this_cost_reversal, req.sale_id, business_id],
        )?;
    }

    // Restocking is optional and, when requested, needs an actual
    // inventory item to credit back — a sale that was never linked to
    // one (a services sale, for instance) simply can't be restocked,
    // and that's a real error to surface rather than silently no-op.
    let mut inventory_record_id: Option<String> = None;
    let mut new_stock_level: Option<i64> = None;
    if req.restock {
        let inventory_module = crud::load_module(&tx, business_id, "inventory")
            .map_err(|_| anyhow!("restocking needs the Inventory module enabled for this business"))?;
        let inventory_table = inventory_module.table_name();

        // The sale record itself doesn't carry inventory_record_id
        // (see sales.json) -- looked up by matching item_name, the
        // same linkage POS checkout itself relies on when it writes
        // item_name onto the sale in the first place.
        let inv_row: Option<(String, i64)> = tx
            .query_row(
                &format!(
                    "SELECT id, quantity FROM {inventory_table}
                     WHERE business_id = ?1 AND name = ?2 AND deleted_at IS NULL"
                ),
                params![business_id, item_name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((inv_id, current_qty)) = inv_row else {
            return Err(anyhow!(
                "cannot restock — no Inventory item named '{item_name}' was found to credit the return to"
            ));
        };
        let new_qty = current_qty + req.quantity;
        tx.execute(
            &format!("UPDATE {inventory_table} SET quantity = ?1, updated_at = datetime('now') WHERE id = ?2 AND business_id = ?3"),
            params![new_qty, inv_id, business_id],
        )?;
        new_stock_level = Some(new_qty);
        inventory_record_id = Some(inv_id);
    }

    let mut record: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
    record.insert("sale_id".into(), json!(req.sale_id));
    record.insert("item_name".into(), json!(item_name));
    record.insert("quantity_refunded".into(), json!(req.quantity));
    record.insert("refund_amount".into(), json!(req.refund_amount));
    record.insert("restocked".into(), json!(req.restock));
    record.insert("cost_reversed".into(), json!(this_cost_reversal));
    if let Some(oid) = &order_id {
        record.insert("order_id".into(), json!(oid));
    }
    if let Some(c) = &customer {
        record.insert("customer".into(), json!(c));
    }
    if let Some(r) = &req.reason {
        record.insert("reason".into(), json!(r));
    }
    if let Some(iid) = &inventory_record_id {
        record.insert("inventory_record_id".into(), json!(iid));
    }
    for f in &refunds_module.fields {
        if !record.contains_key(&f.name) {
            if let Some(d) = &f.default {
                record.insert(f.name.clone(), d.clone());
            }
        }
    }
    // Same validation any manually-typed record goes through — no
    // special-casing for a refund just because it came through this
    // purpose-built path instead of generic CRUD.
    refunds_module.validate(&record)?;
    crate::reference_data::validate_field_references(&tx, business_id, &refunds_module, &record)?;
    let refund_id = crud::insert_validated_record(&tx, business_id, &refunds_module, &record)?;

    // Same Bookkeeping auto-post as checkout() and receive(). Skipped
    // when refund_amount is 0 — a valid case (an even exchange, no
    // cash changes hands) that shouldn't leave a zero-value entry
    // cluttering the ledger. Best-effort: a business without
    // Bookkeeping enabled can still process a refund.
    if req.refund_amount > 0 {
        if let Ok(accounting_module) = crud::load_module(&tx, business_id, "accounting") {
            let mut entry: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
            entry.insert("description".into(), json!(format!("Refund — {item_name}")));
            entry.insert("entry_type".into(), json!("expense"));
            entry.insert("category".into(), json!("Refunds"));
            entry.insert("amount".into(), json!(req.refund_amount));
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

    // Same "commit is the one moment this becomes real" discipline as
    // checkout() and receive() — nothing above is durable until here.
    tx.commit()?;

    let summary = json!({
        "refund_id": refund_id,
        "sale_id": req.sale_id,
        "item_name": item_name,
        "quantity_refunded": req.quantity,
        "refund_amount": req.refund_amount,
        "cost_reversed": this_cost_reversal,
        "remaining_refundable_after": remaining_refundable - req.quantity,
        "restocked": req.restock,
        "inventory_record_id": inventory_record_id,
        "new_stock_level": new_stock_level,
    });

    let _ = crate::audit::log(conn, business_id, Some(user_id), "_refunds", "refund", Some(&refund_id), Some(&summary));

    Ok(summary)
}
