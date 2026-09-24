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
//! SECOND FIX, same file (HISTORICAL — see BATCH REWRITE below for the
//! current behavior): `receive()` used to update Inventory's
//! `quantity` but never touch `unit_cost` at all — the recorded cost
//! silently went stale the moment a supplier's price changed on any
//! repeat order. It used to compute a weighted average across the
//! stock already on hand and what just arrived.
//!
//! THIRD FIX, same file (ALSO HISTORICAL): blending two costs into one
//! rounded-to-the-cent `new_unit_cost` needed its own rounding-
//! reconciliation Bookkeeping post, since a blended average doesn't
//! always land on the exact value actually on hand plus actually paid
//! for.
//!
//! FOURTH FIX, same file (STILL CURRENT): the core of this logic is
//! split out into `receive_in_tx`, which runs against a `Transaction`
//! the CALLER already owns, rather than only ever being reachable
//! through `receive()`'s own newly-opened one. This is what lets
//! `excel_import::import()` call it directly, once per newly-created
//! Purchasing row, inside the single big transaction the whole import
//! already runs in — so a bulk-imported purchase order and its stock
//! arriving are one atomic step, not "import creates it unreceived,
//! then someone has to click Receive on each of what might be 150
//! rows." `receive()` itself is a thin wrapper: open a transaction,
//! call `receive_in_tx`, commit, audit-log.
//!
//! BATCH REWRITE (supersedes the SECOND/THIRD fixes above): this file
//! no longer blends a receipt's cost into Inventory's own `unit_cost`/
//! `unit_price` at all. Every receipt now creates its own independent,
//! FEFO-tracked batch (see `batches.rs`) — its own quantity, its own
//! cost, its own selling price, its own optional expiry date. Because
//! a batch's `unit_cost` is stored exactly as this delivery's own PO
//! cost (no averaging), the rounding-reconciliation "Stock
//! Revaluation" Bookkeeping post the THIRD FIX above introduced no
//! longer has anything to catch on this path — there's no remainder
//! left to lose. `Inventory.unit_cost`/`unit_price` are left
//! completely untouched by this file from now on; they remain frozen
//! at whatever they were the moment this feature shipped for any item
//! that already existed — including never being read as a fallback
//! price for a new batch anymore (see `ReceiveRequest`'s own doc
//! comment on `unit_price`: a batch's price is REQUIRED, on purpose,
//! never silently inherited from anywhere) — and
//! `batches.rs`'s module doc comment has the full reasoning.

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
    /// This delivery's own selling price — batches.rs's own module doc
    /// comment explains why this creates an independent batch instead
    /// of blending into Inventory's single `unit_price`. Genuinely
    /// optional now: since v31_purchasing_unit_price, the purchase
    /// order itself already carries a selling price (decided at the
    /// same time as its cost), and `receive_in_tx` defaults to that
    /// when this is omitted — never to Inventory's frozen legacy
    /// price, only to this specific purchase's own. Supply a value
    /// here only to override what was planned at order time (a
    /// supplier price change between ordering and delivery, a manual
    /// correction) — the ordinary case needs nothing here at all.
    /// Integer minor units (cents) — see money.rs.
    #[serde(default)]
    pub unit_price: Option<i64>,
    /// This specific delivery's expiry date, if any — what makes FEFO
    /// consumption possible at all for this batch. `None` means this
    /// batch never expires (sells after every dated batch, ahead of
    /// nothing but legacy stock — see batches.rs's FEFO ordering).
    #[serde(default)]
    pub expiry_date: Option<String>,
}

/// Creates a new Purchasing order and receives it immediately, in one
/// transaction — the on-screen "+ New" counterpart to what
/// `excel_import::import()` already does for a bulk-imported purchasing
/// row. The moment a purchasing record is created here, it's received
/// in the same transaction via the same `receive_in_tx` mechanics
/// `receive()` below and the Excel import path both share.
pub fn create_and_receive(
    conn: &mut Connection,
    business_id: &str,
    user_id: &str,
    body: &serde_json::Map<String, Value>,
) -> Result<Value> {
    // THE BUG THIS FIXES: `expiry_date` isn't a declared field on the
    // `purchasing` module (it belongs to the BATCH this receipt
    // creates — see batches.rs — not to the purchase order row
    // itself), so `crud::create` below silently drops it: `module.
    // validate()` only checks declared fields, and `insert_validated_
    // record` only ever writes columns for fields the module
    // declares, so an extra key in `body` is simply never persisted
    // anywhere. That's fine for the purchasing ROW, but this function
    // used to also throw the value away for the BATCH, by hardcoding
    // `None` on the `receive_in_tx` call below regardless of what the
    // caller sent — there was no way, through the on-screen "+ New"
    // form (the only reachable manual receive path — see this
    // function's own doc comment above), to ever record an expiry
    // date for perishable stock. Pulled out here, before `body` is
    // handed to `crud::create`, and threaded through to `receive_in_
    // tx` explicitly instead.
    let expiry_date = body.get("expiry_date").and_then(|v| v.as_str()).map(|s| s.to_string());

    // Same permission the manual-receive and bulk-import paths both
    // require, checked up front for the same reason: creating a
    // purchasing order this way can increase Inventory stock, the
    // exact same effect `receive()` has.
    crate::rbac::require(conn, user_id, "inventory", "receive")?;

    let tx = conn.transaction()?;
    let id = crud::create(&tx, business_id, user_id, "purchasing", body)?;
    let purchasing_table = crud::load_module(&tx, business_id, "purchasing")?.table_name();
    let summary = receive_in_tx(&tx, business_id, &purchasing_table, "module_inventory", &id, None, None, expiry_date.as_deref(), Some(user_id))?;
    // Same discipline as receive() and repack(): nothing above is
    // durable until this line.
    tx.commit()?;

    let _ = crate::audit::log(conn, business_id, Some(user_id), "_receiving", "receive", Some(&id), Some(&summary));

    Ok(json!({"id": id, "receiving": summary}))
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
        req.unit_price,
        req.expiry_date.as_deref(),
        Some(user_id),
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
    unit_price_override: Option<i64>,
    expiry_date: Option<&str>,
    created_by: Option<&str>,
) -> Result<Value> {
    // Receiving adds stock — blocked for the duration of an open
    // stock take for the same reason checkout is (see stock_take.rs).
    // Guarded here rather than in each of `receive()` and
    // `create_and_receive()` separately since both funnel through this
    // one shared mechanics function — as does `excel_import::import()`,
    // which gets the same protection for free rather than silently
    // being left as a hole.
    crate::stock_take::require_no_open_stock_take(tx, business_id)?;

    let row: Option<(String, i64, bool, Option<String>, String, i64, i64, Option<String>)> = tx
        .query_row(
            &format!(
                "SELECT item_name, quantity, received, inventory_record_id, supplier, unit_cost, unit_price, po_number
                 FROM {purchasing_table} WHERE id = ?1 AND business_id = ?2 AND deleted_at IS NULL"
            ),
            params![purchase_record_id, business_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)),
        )
        .optional()?;
    let Some((item_name, ordered_qty, already_received, inventory_record_id, supplier, po_unit_cost, po_unit_price, po_number)) = row else {
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

    let inv_row: Option<(String, i64)> = tx
        .query_row(
            &format!("SELECT name, quantity FROM {inventory_table} WHERE id = ?1 AND business_id = ?2 AND deleted_at IS NULL"),
            params![inventory_record_id, business_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((inventory_name, current_qty)) = inv_row else {
        return Err(anyhow!("linked inventory item not found: {inventory_record_id}"));
    };

    let new_qty = current_qty + quantity_received;

    // A batch's selling price defaults to the purchase order's own
    // declared price (never Inventory's frozen legacy unit_price).
    // unit_price_override exists for when today's actual delivery needs
    // a different price than planned (a supplier price change).
    let batch_unit_price = unit_price_override.unwrap_or(po_unit_price);
    if batch_unit_price < 0 {
        return Err(anyhow!("price cannot be negative"));
    }
    if batch_unit_price < po_unit_cost {
        let business_currency: String = tx
            .query_row("SELECT currency FROM businesses WHERE id = ?1", params![business_id], |r| r.get(0))
            .unwrap_or_else(|_| "USD".to_string());
        let cost_display = crate::money::format_money(po_unit_cost, &business_currency);
        let price_display = crate::money::format_money(batch_unit_price, &business_currency);
        return Err(anyhow!(
            "receiving this would create a batch of '{inventory_name}' costing {cost_display} per unit while \
             priced at {price_display} — set a higher price for this delivery before receiving it"
        ));
    }

    // Inventory.quantity is still the one number every other part of
    // this app reads — it just now gets there via a batch's own
    // quantity_received rather than a blended weighted-average.
    // unit_cost/unit_price are deliberately NOT touched here anymore —
    // see batches.rs's own doc comment for why those two columns stay
    // frozen legacy history from this point on.
    tx.execute(
        &format!("UPDATE {inventory_table} SET quantity = ?1, updated_at = datetime('now') WHERE id = ?2 AND business_id = ?3"),
        params![new_qty, inventory_record_id, business_id],
    )?;

    tx.execute(
        &format!("UPDATE {purchasing_table} SET received = 1, updated_at = datetime('now') WHERE id = ?1 AND business_id = ?2"),
        params![purchase_record_id, business_id],
    )?;


    let received_at = chrono::Utc::now().to_rfc3339();
    let batch_id = crate::batches::create_batch_in_tx(
        tx,
        business_id,
        &inventory_record_id,
        po_number.as_deref(),
        quantity_received,
        po_unit_cost,
        batch_unit_price,
        expiry_date,
        &received_at,
        created_by,
    )?;

    // Ledger entry for the same quantity change, in the same
    // transaction — see stock_movement.rs. Referenced to the batch this
    // delivery created, so a movement can be traced back to the exact
    // batch (and therefore the exact PO and cost) it came from.
    crate::stock_movement::record_in_tx(
        tx,
        business_id,
        created_by,
        &inventory_record_id,
        &inventory_name,
        crate::stock_movement::RECEIVING,
        quantity_received,
        po_unit_cost,
        Some(&batch_id),
    )?;

    // Same Bookkeeping auto-post as before this feature, same
    // reasoning: one expense entry for what was actually paid to the
    // supplier for this delivery (quantity received × the PO's own
    // unit cost — the real cash outlay, unaffected by batching, since
    // a batch's own unit_cost IS exactly this PO's unit_cost, no
    // averaging involved to ever drift from it). Best-effort: a
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

    // NOTE: the weighted-average rounding-reconciliation Bookkeeping
    // post ("Stock Revaluation") that used to live here is gone,
    // deliberately, not merely removed by oversight — it existed ONLY
    // to catch the cents lost/gained when a blended weighted-average
    // cost got rounded to the nearest cent. A batch's own unit_cost IS
    // exactly this PO's own unit_cost, stored exactly, with no
    // averaging and therefore no rounding step of any kind — there is
    // no remainder left for this mechanism to ever need to catch on
    // this code path anymore.

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
        "batch_id": batch_id,
        "batch_unit_cost": po_unit_cost,
        "batch_unit_price": batch_unit_price,
        "batch_expiry_date": expiry_date,
    });

    // Committing and audit-logging are each caller's own responsibility
    // — see this function's own doc comment for why (a manual receive
    // commits/logs once per call; a bulk import commits once for the
    // whole batch and logs once per row, inside its own loop).
    Ok(summary)
}
