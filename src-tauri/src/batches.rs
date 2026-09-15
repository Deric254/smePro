//! Batch-costed inventory with FEFO (First-Expired, First-Out)
//! consumption.
//!
//! Before this, one Inventory row meant one blended `unit_cost` +
//! `unit_price` for the whole item — every receipt merged the new
//! cost into that single number via a quantity-weighted average (see
//! receiving.rs's own doc comment for that history), and there was no
//! way to give one delivery a different selling price than another,
//! or to know which specific delivery is closest to expiring.
//!
//! Going forward, every receipt becomes its own tracked lot — a row in
//! `inventory_batches` — with its own quantity, cost, price, and
//! (optional) expiry date. A sale, repack, or any other consumption
//! automatically draws from the batch closest to expiring first; no
//! manual batch selection exists anywhere in this app, by design (see
//! `fefo_consume_in_tx` below).
//!
//! LEGACY STOCK IS FROZEN, NOT MIGRATED. Whatever an item's `quantity`/
//! `unit_cost`/`unit_price` were the moment this shipped stays exactly
//! as it was — no batch, no expiry, on purpose. `receiving.rs` no
//! longer blends a new receipt into that number at all; every future
//! receipt creates a batch instead, which means legacy's own cost/
//! price columns on the Inventory row can never change again from this
//! point on (there is no code path left that writes to them). This
//! file treats "legacy" as nothing more than the portion of
//! `Inventory.quantity` NOT currently accounted for by any live batch
//! — computed on the fly (`Inventory.quantity - SUM(live batch
//! quantity_remaining)`), never stored anywhere as its own number, so
//! there is no separate counter that could ever drift out of sync
//! with the real total. `Inventory.quantity` itself remains the single
//! source of truth every other part of the app already reads (the
//! dashboard, low-stock, slow movers, stock runway) — nothing about
//! this feature changes what that column means or who updates it; it
//! just now has two possible sources feeding it (legacy directly, or a
//! batch's `quantity_remaining`) instead of one.
//!
//! DELIBERATE CHOICE: `Inventory.unit_cost`/`unit_price` are NEVER
//! written by anything in this file. Once a single batch exists for an
//! item, those two columns are frozen legacy history, nothing more —
//! not "the current price," not "the front-of-queue price," just
//! whatever they were the moment the last legacy-affecting write ever
//! happened. Overwriting them with a live front-of-queue batch's price
//! (the more literal reading of "display-only summary") was considered
//! and rejected: it would make the legacy value itself unrecoverable
//! the instant it happened, with no way to tell, later, whether legacy
//! stock is still the active FEFO tier or a batch is. A genuinely
//! live, FEFO-aware display price is instead computed fresh on every
//! read via `front_of_queue` below, returned by the batches-list
//! endpoint alongside the batch breakdown — it is a read-time
//! computation, never a write. Known, deliberate limitation: screens
//! that read `Inventory.unit_price` directly today (the POS product
//! grid, generic Inventory list/export, existing reports) keep showing
//! the frozen legacy price even once batches are the active tier —
//! correctness of the legacy figure was chosen over convenience for
//! this pass; updating those screens to call the batches-list endpoint
//! instead is a follow-up, not something this file does on its own.
//!
//! FEFO ORDER, PRECISELY: batches with a real `expiry_date`, soonest
//! first; then batches with no expiry date at all, oldest-received
//! first; legacy stock last of all, always — treated as the oldest
//! "no expiry" stock there is, since it predates this feature
//! entirely. This is a strict total order with no ties left
//! unresolved (received_at, then id, break any remaining tie), so
//! FEFO consumption is fully deterministic.

use anyhow::{anyhow, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

/// One row of `inventory_batches`, as returned to callers/API responses.
#[derive(Debug, Serialize)]
pub struct BatchRow {
    pub id: String,
    pub inventory_record_id: String,
    pub source_po_number: Option<String>,
    pub quantity_received: i64,
    pub quantity_remaining: i64,
    pub unit_cost: i64,
    pub unit_price: i64,
    pub expiry_date: Option<String>,
    pub received_at: String,
}

/// Where one portion of a FEFO consumption came from — a specific
/// batch, or the item's legacy (pre-batch) stock. Kept as an enum
/// rather than a bare `Option<String>` so every call site has to
/// consciously handle both cases rather than treating `None` as an
/// afterthought.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConsumedFrom {
    Batch { batch_id: String, expiry_date: Option<String> },
    Legacy,
}

/// One priced/costed slice of a FEFO consumption. A single sale or
/// repack can produce several of these (front batch didn't have
/// enough on its own) — see `fefo_consume_in_tx`.
#[derive(Debug, Clone, Serialize)]
pub struct ConsumedPortion {
    pub from: ConsumedFrom,
    pub quantity: i64,
    pub unit_cost: i64,
    pub unit_price: i64,
}

impl ConsumedPortion {
    pub fn source_batch_id(&self) -> Option<&str> {
        match &self.from {
            ConsumedFrom::Batch { batch_id, .. } => Some(batch_id.as_str()),
            ConsumedFrom::Legacy => None,
        }
    }
}

/// Total live (not soft-deleted) batch stock remaining for one
/// Inventory item — the one number this file ever treats as "how much
/// of this item's stock is currently sitting in batches" rather than
/// in legacy. Every other function below either calls this or does
/// the exact same query itself for the same reason: `Inventory.
/// quantity` minus this is legacy, always computed, never stored.
pub(crate) fn live_batch_quantity_in_tx(
    tx: &rusqlite::Transaction<'_>,
    business_id: &str,
    inventory_record_id: &str,
) -> Result<i64> {
    let total: i64 = tx.query_row(
        "SELECT COALESCE(SUM(quantity_remaining), 0) FROM inventory_batches
         WHERE business_id = ?1 AND inventory_record_id = ?2 AND deleted_at IS NULL",
        params![business_id, inventory_record_id],
        |r| r.get(0),
    )?;
    Ok(total)
}

/// Inserts one new batch row. Enforces the exact same "price can never
/// be below cost" rule every other cost/price-setting write in this
/// app already holds itself to (crud::create, crud::update, repack,
/// receiving) — held here, per batch, per decision #4 of the spec,
/// rather than per item.
#[allow(clippy::too_many_arguments)]
pub(crate) fn create_batch_in_tx(
    tx: &rusqlite::Transaction<'_>,
    business_id: &str,
    inventory_record_id: &str,
    source_po_number: Option<&str>,
    quantity: i64,
    unit_cost: i64,
    unit_price: i64,
    expiry_date: Option<&str>,
    received_at: &str,
    created_by: Option<&str>,
) -> Result<String> {
    if quantity <= 0 {
        return Err(anyhow!("batch quantity must be greater than zero"));
    }
    if unit_cost < 0 || unit_price < 0 {
        return Err(anyhow!("batch cost/price cannot be negative"));
    }
    if unit_price < unit_cost {
        let business_currency: String = tx
            .query_row("SELECT currency FROM businesses WHERE id = ?1", params![business_id], |r| r.get(0))
            .unwrap_or_else(|_| "USD".to_string());
        let cost_display = crate::money::format_money(unit_cost, &business_currency);
        let price_display = crate::money::format_money(unit_price, &business_currency);
        return Err(anyhow!(
            "this batch would be priced at {price_display} while costing {cost_display} per unit — \
             a batch can never be priced below its own cost"
        ));
    }

    let id = Uuid::new_v4().to_string();
    tx.execute(
        "INSERT INTO inventory_batches
            (id, business_id, inventory_record_id, source_po_number, quantity_received,
             quantity_remaining, unit_cost, unit_price, expiry_date, received_at, created_by,
             created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7, ?8, ?9, ?10, datetime('now'), datetime('now'))",
        params![
            id,
            business_id,
            inventory_record_id,
            source_po_number,
            quantity,
            unit_cost,
            unit_price,
            expiry_date,
            received_at,
            created_by,
        ],
    )?;
    Ok(id)
}

/// Canonical FEFO ordering, shared by every place that reads
/// `inventory_batches` in consumption or front-of-queue order: a real
/// `expiry_date` first (soonest first), then no-expiry batches
/// oldest-received first, and — since v29_legacy_batch_backfill —
/// any synthetic legacy-migration row sorts dead last regardless of
/// its own `received_at`, preserving the "legacy sold last, always"
/// rule this file's module doc comment has described from the start.
/// Previously this ordering expression was hand-copied into every
/// consumer (this file's own two query sites, plus pos.rs, ai_context.rs,
/// stock_health.rs) — one shared constant now, so the next place that
/// reads batches in order can't quietly drift from what checkout()
/// actually charges by retyping it slightly differently.
pub(crate) const FEFO_ORDER_BY: &str =
    "is_legacy_migration ASC, (expiry_date IS NULL) ASC, expiry_date ASC, received_at ASC, id ASC";

/// Consumes `qty_needed` units of one Inventory item in strict FEFO
/// order — batches with a real expiry date first (soonest first),
/// then no-expiry batches (oldest received first), then legacy stock
/// last of all. Returns the exact portions consumed, each carrying its
/// OWN cost/price, so the caller (pos.rs::checkout, repack.rs) can
/// price/cost each portion independently rather than assuming one
/// blended per-unit number for the whole line.
///
/// Does NOT touch `Inventory.quantity` itself — every existing caller
/// already reads that value before calling this and writes the new
/// total back itself in its own single UPDATE, exactly as it did
/// before batches existed; this function only ever adjusts
/// `inventory_batches.quantity_remaining` for the batches it actually
/// draws from. `current_inventory_qty` must be the value read in the
/// SAME transaction, before any write this operation makes — used
/// purely to compute how much of `qty_needed` legacy stock can supply
/// once every live batch is exhausted.
///
/// Callers are expected to have already checked `qty_needed <=
/// current_inventory_qty` (oversell protection is each caller's own
/// business rule — e.g. pos.rs's `allow_oversell`); this function
/// trusts that check rather than repeating it, but never silently
/// under-consumes if that invariant is somehow violated — it returns a
/// hard error instead of overselling.
pub(crate) fn fefo_consume_in_tx(
    tx: &rusqlite::Transaction<'_>,
    business_id: &str,
    inventory_record_id: &str,
    qty_needed: i64,
    current_inventory_qty: i64,
    legacy_unit_cost: i64,
    legacy_unit_price: i64,
) -> Result<Vec<ConsumedPortion>> {
    if qty_needed <= 0 {
        return Err(anyhow!("internal error: FEFO consumption called with a non-positive quantity"));
    }

    let mut remaining_needed = qty_needed;
    let mut portions: Vec<ConsumedPortion> = Vec::new();

    // Live batches, in strict FEFO order — see this file's own module
    // doc comment for the exact ordering rule. `(expiry_date IS NULL)`
    // evaluates to 0 for a real date and 1 for NULL, so ordering by it
    // ascending puts every dated batch before every undated one.
    let mut stmt = tx.prepare(&format!(
        "SELECT id, quantity_remaining, unit_cost, unit_price, expiry_date
         FROM inventory_batches
         WHERE business_id = ?1 AND inventory_record_id = ?2 AND deleted_at IS NULL AND quantity_remaining > 0
         ORDER BY {FEFO_ORDER_BY}"
    ))?;
    let batch_rows: Vec<(String, i64, i64, i64, Option<String>)> = stmt
        .query_map(params![business_id, inventory_record_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);

    for (batch_id, batch_remaining, batch_unit_cost, batch_unit_price, batch_expiry) in batch_rows {
        if remaining_needed <= 0 {
            break;
        }
        let take = remaining_needed.min(batch_remaining);
        if take <= 0 {
            continue;
        }
        // Defensive `AND quantity_remaining >= ?` guard: this is the
        // one write in this loop, so it double-checks the row hasn't
        // moved since the SELECT above rather than trusting it blindly
        // — belt-and-suspenders under SQLite's single-writer model,
        // not a fix for a race this app's own Mutex-serialized
        // connection can actually hit today.
        let updated = tx.execute(
            "UPDATE inventory_batches SET quantity_remaining = quantity_remaining - ?1, updated_at = datetime('now')
             WHERE id = ?2 AND business_id = ?3 AND quantity_remaining >= ?1",
            params![take, batch_id, business_id],
        )?;
        if updated == 0 {
            return Err(anyhow!(
                "internal inventory inconsistency: batch {batch_id} no longer has the stock this operation \
                 expected — aborting rather than risk overselling"
            ));
        }
        portions.push(ConsumedPortion {
            from: ConsumedFrom::Batch { batch_id, expiry_date: batch_expiry },
            quantity: take,
            unit_cost: batch_unit_cost,
            unit_price: batch_unit_price,
        });
        remaining_needed -= take;
    }

    if remaining_needed > 0 {
        let live_batches_total = live_batch_quantity_in_tx(tx, business_id, inventory_record_id)?;
        // live_batches_total here is POST-consumption (should be 0 for
        // every batch this loop touched), so legacy_available is
        // whatever of current_inventory_qty isn't already accounted
        // for by batches that still have something left (batches this
        // loop didn't touch, if any — shouldn't happen since it walks
        // every live batch, but computed honestly rather than assumed).
        let legacy_available = (current_inventory_qty - live_batches_total).max(0);
        let take = remaining_needed.min(legacy_available);
        if take > 0 {
            portions.push(ConsumedPortion {
                from: ConsumedFrom::Legacy,
                quantity: take,
                unit_cost: legacy_unit_cost,
                unit_price: legacy_unit_price,
            });
            remaining_needed -= take;
        }
    }

    if remaining_needed > 0 {
        return Err(anyhow!(
            "internal inventory inconsistency: {remaining_needed} unit(s) of item {inventory_record_id} \
             could not be sourced from any batch or legacy stock — aborting rather than overselling"
        ));
    }

    Ok(portions)
}

/// Refund-time restock into a specific batch — see refund.rs. Credits
/// `quantity` back onto the originating batch if it's still live; if
/// that batch was deleted since the sale, creates a brand-new batch
/// dated today instead, at the batch's original cost/price (passed in
/// by the caller from the sale's own frozen `cost_at_sale`/
/// `unit_price`, since a deleted batch's own numbers are gone).
/// Callers with no `source_batch_id` at all (legacy-stock sales, or
/// sales made before this feature existed) never call this — they
/// keep crediting `Inventory.quantity` directly, exactly as before,
/// which correctly grows the legacy pool.
pub(crate) fn restore_to_batch_in_tx(
    tx: &rusqlite::Transaction<'_>,
    business_id: &str,
    inventory_record_id: &str,
    batch_id: &str,
    quantity: i64,
    original_unit_cost: i64,
    original_unit_price: i64,
    today: &str,
) -> Result<()> {
    let live: Option<i64> = tx
        .query_row(
            "SELECT quantity_remaining FROM inventory_batches
             WHERE id = ?1 AND business_id = ?2 AND deleted_at IS NULL",
            params![batch_id, business_id],
            |r| r.get(0),
        )
        .optional()?;
    if live.is_some() {
        tx.execute(
            "UPDATE inventory_batches SET quantity_remaining = quantity_remaining + ?1, updated_at = datetime('now')
             WHERE id = ?2 AND business_id = ?3",
            params![quantity, batch_id, business_id],
        )?;
    } else {
        create_batch_in_tx(
            tx,
            business_id,
            inventory_record_id,
            None,
            quantity,
            original_unit_cost,
            original_unit_price,
            None,
            today,
            None,
        )?;
    }
    Ok(())
}

/// Edits a specific batch's price (Owner/Manager) and, optionally, its
/// cost (Owner only) — see http_api.rs's `POST /inventory/:id/batches/
/// :batchId/price`. The existing "price can't be below cost" rule
/// applies per batch, exactly like `create_batch_in_tx` enforces at
/// creation.
#[derive(Debug, Deserialize)]
pub struct UpdateBatchPriceRequest {
    pub batch_id: String,
    pub unit_price: i64,
    #[serde(default)]
    pub unit_cost: Option<i64>,
}

pub fn update_batch_price(
    conn: &mut rusqlite::Connection,
    business_id: &str,
    user_id: &str,
    req: UpdateBatchPriceRequest,
) -> Result<Value> {
    crate::rbac::require(conn, user_id, "inventory", "update_batch_price")?;
    if req.unit_cost.is_some() {
        // Cost edits are Owner-only, per the spec — a wider grant on
        // "update_batch_price" only ever covers price.
        crate::rbac::require_owner(conn, user_id)?;
    }
    if req.unit_price < 0 {
        return Err(anyhow!("price cannot be negative"));
    }
    if let Some(c) = req.unit_cost {
        if c < 0 {
            return Err(anyhow!("cost cannot be negative"));
        }
    }

    let tx = conn.transaction()?;
    let existing: Option<(String, i64, i64)> = tx
        .query_row(
            "SELECT inventory_record_id, unit_cost, unit_price FROM inventory_batches
             WHERE id = ?1 AND business_id = ?2 AND deleted_at IS NULL",
            params![req.batch_id, business_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((inventory_record_id, current_cost, _current_price)) = existing else {
        return Err(anyhow!("batch not found: {}", req.batch_id));
    };
    let new_cost = req.unit_cost.unwrap_or(current_cost);
    if req.unit_price < new_cost {
        let business_currency: String = tx
            .query_row("SELECT currency FROM businesses WHERE id = ?1", params![business_id], |r| r.get(0))
            .unwrap_or_else(|_| "USD".to_string());
        let cost_display = crate::money::format_money(new_cost, &business_currency);
        let price_display = crate::money::format_money(req.unit_price, &business_currency);
        return Err(anyhow!(
            "this would price the batch at {price_display} while it costs {cost_display} per unit — \
             a batch can never be priced below its own cost"
        ));
    }

    tx.execute(
        "UPDATE inventory_batches SET unit_cost = ?1, unit_price = ?2, updated_at = datetime('now')
         WHERE id = ?3 AND business_id = ?4",
        params![new_cost, req.unit_price, req.batch_id, business_id],
    )?;
    tx.commit()?;

    let summary = json!({
        "batch_id": req.batch_id,
        "inventory_record_id": inventory_record_id,
        "unit_cost": new_cost,
        "unit_price": req.unit_price,
    });
    let _ = crate::audit::log(conn, business_id, Some(user_id), "_inventory_batches", "update_batch_price", Some(&req.batch_id), Some(&summary));
    Ok(summary)
}

/// Lists every live batch for one Inventory item, in FEFO order, plus
/// the item's legacy quantity (computed, never stored — see this
/// file's own module doc comment) and a read-time "front of queue"
/// display cost/price: the batch that would be consumed next, or
/// legacy's own frozen cost/price if no batch has anything left.
pub fn list_batches(
    conn: &rusqlite::Connection,
    business_id: &str,
    user_id: &str,
    inventory_record_id: &str,
) -> Result<Value> {
    crate::rbac::require(conn, user_id, "inventory", "read")?;
    let inventory_module = crate::crud::load_module(conn, business_id, "inventory")
        .map_err(|_| anyhow!("the Inventory module isn't enabled for this business"))?;
    let inventory_table = inventory_module.table_name();

    let inv_row: Option<(String, i64, i64, i64)> = conn
        .query_row(
            &format!("SELECT name, quantity, unit_cost, unit_price FROM {inventory_table} WHERE id = ?1 AND business_id = ?2 AND deleted_at IS NULL"),
            params![inventory_record_id, business_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((name, total_quantity, legacy_unit_cost, legacy_unit_price)) = inv_row else {
        return Err(anyhow!("inventory item not found: {inventory_record_id}"));
    };

    let mut stmt = conn.prepare(&format!(
        "SELECT id, inventory_record_id, source_po_number, quantity_received, quantity_remaining,
                unit_cost, unit_price, expiry_date, received_at
         FROM inventory_batches
         WHERE business_id = ?1 AND inventory_record_id = ?2 AND deleted_at IS NULL AND quantity_remaining > 0
         ORDER BY {FEFO_ORDER_BY}"
    ))?;
    let batches: Vec<BatchRow> = stmt
        .query_map(params![business_id, inventory_record_id], |r| {
            Ok(BatchRow {
                id: r.get(0)?,
                inventory_record_id: r.get(1)?,
                source_po_number: r.get(2)?,
                quantity_received: r.get(3)?,
                quantity_remaining: r.get(4)?,
                unit_cost: r.get(5)?,
                unit_price: r.get(6)?,
                expiry_date: r.get(7)?,
                received_at: r.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let live_batches_total: i64 = batches.iter().map(|b| b.quantity_remaining).sum();
    let legacy_quantity = (total_quantity - live_batches_total).max(0);

    let (front_unit_cost, front_unit_price, front_source) = match batches.first() {
        Some(b) => (b.unit_cost, b.unit_price, "batch"),
        None => (legacy_unit_cost, legacy_unit_price, "legacy"),
    };

    Ok(json!({
        "inventory_record_id": inventory_record_id,
        "item_name": name,
        "total_quantity": total_quantity,
        "legacy_quantity": legacy_quantity,
        "legacy_unit_cost": legacy_unit_cost,
        "legacy_unit_price": legacy_unit_price,
        "batches": batches,
        "front_of_queue_unit_cost": front_unit_cost,
        "front_of_queue_unit_price": front_unit_price,
        "front_of_queue_source": front_source,
    }))
}
