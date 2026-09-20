//! Stock Take: a guided, point-in-time physical count that reconciles
//! actual shelf quantities against what the system thinks is on hand.
//!
//! THE GAP THIS CLOSES: before this, the only way to correct a real
//! discrepancy (shrinkage, miscount, damage, theft) between what
//! inventory.quantity says and what's physically on the shelf was
//! either (a) a bulk Excel re-upload — real, but spreadsheet-driven,
//! all-or-nothing, and awkward for "just recount these 12 items
//! today" — or (b) nothing at all, since a single ad-hoc field edit on
//! inventory.quantity is deliberately blocked (see crud.rs's
//! `is_update_blocked_field`). This is the missing middle:
//! a dedicated, walk-the-floor counting session with its own
//! before/after variance report and audit trail.
//!
//! THREE STEPS, each its own function below:
//!   1. `initiate()` — freezes a snapshot: one `stock_take_items` row
//!      per current inventory item, capturing `expected_qty` as it
//!      stands at this exact moment. Counting against a live,
//!      simultaneously-changing "expected" value would make every
//!      variance meaningless the instant a sale happened mid-count.
//!   2. `record_count()` — enters a physical count against one
//!      snapshotted item. Counting is deliberately allowed to be
//!      partial: a business that only has time to recount its top 20
//!      fast movers today is a completely normal, valid use of this
//!      feature, not an error condition. Anything never counted is
//!      simply left alone at close time — its expected value stands.
//!   3. `close()` — for every item that WAS counted, applies its
//!      variance to `inventory.quantity` in one atomic transaction,
//!      the same way receiving/refund/repack do, and returns a
//!      variance report. NEGATIVE variance (stock physically missing)
//!      is routed through `batches::fefo_consume_in_tx` — the exact
//!      same FEFO write-down checkout uses — so `inventory_batches`
//!      stays in sync instead of going stale; the report carries each
//!      write-off's real cost, not just a unit count. POSITIVE
//!      variance (physically finding MORE than expected) creates a new
//!      batch priced at the item's own most recent batch cost/price,
//!      if it has ever had one — surplus stock still needs an honest
//!      sellable price, and the last real price this item actually
//!      sold at is a far better source for that than leaving it priced
//!      at nothing (see close()'s own doc comment on why a bare
//!      legacy-quantity bump used to silently zero out the price of
//!      any item whose only batch had since sold out). Only a
//!      genuinely pre-batch item — one that has never had a batch at
//!      all — keeps the simpler direct-to-legacy-quantity treatment,
//!      since its legacy price is real, live pricing already.
//!      Uncounted items are untouched and separately reported as
//!      "skipped," not silently folded into "no change."
//!
//! ONLY ONE STOCK TAKE OPEN AT A TIME, per business — enforced by a
//! partial unique index in the schema (see db_migrations.rs's v11),
//! not just an application-level check, so a race between two
//! `initiate()` calls fails at the database level rather than
//! producing two simultaneously "current" counts with no way to tell
//! which one a given count belongs to.
//!
//! CHECKOUT, RECEIVING, REFUNDS, AND REPACK ARE ALL BLOCKED for the
//! duration of an open stock take (`require_no_open_stock_take`,
//! called at the top of each one's own transaction). That's a real
//! tradeoff — zero sales/stock movement while a count is open, so
//! counts should stay short — but it's what makes `close()`'s
//! `quantity = counted` an absolute, safe overwrite: with nothing else
//! able to touch stock mid-count, `expected_qty` at `initiate()` and
//! live `quantity` at `close()` can only differ by the count itself.

use crate::crud;
use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

/// Guard used by every stock-affecting write path — checkout,
/// receiving, refunds, repack — to enforce that they're blocked for
/// the duration of an open stock take. This is what makes `close()`'s
/// absolute overwrite (`quantity = counted`) safe: if nothing else
/// can touch stock while a count is open, `expected_qty` at
/// `initiate()` and live `quantity` at `close()` are guaranteed to
/// match apart from the count itself, so there's no delta-vs-overwrite
/// ambiguity to resolve.
///
/// Called as the first statement inside each operation's own
/// transaction (after `conn.transaction()?`), not before it — SQLite
/// is single-writer, so checking inside the same transaction the write
/// itself lands in closes the race where a stock take opens in the gap
/// between an earlier check and the write that follows it.
pub(crate) fn require_no_open_stock_take(conn: &Connection, business_id: &str) -> Result<()> {
    let open_id: Option<String> = conn
        .query_row(
            "SELECT id FROM stock_takes WHERE business_id = ?1 AND status = 'in_progress'",
            params![business_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = open_id {
        return Err(anyhow!(
            "a stock take is in progress (id: {id}) — finalize it before checkout, receiving, refunds, or repacking can continue"
        ));
    }
    Ok(())
}

/// Starts a new stock take: snapshots every current, non-deleted
/// inventory item's quantity as `expected_qty`. Fails outright if this
/// business already has one in progress — see the module doc comment
/// for why letting two run concurrently was never a good idea to
/// begin with, not just an edge case to tolerate.
pub fn initiate(conn: &mut Connection, business_id: &str, user_id: &str) -> Result<Value> {
    crate::rbac::require(conn, user_id, "inventory", "stocktake")?;

    let inventory_module = crud::load_module(conn, business_id, "inventory")
        .map_err(|_| anyhow!("the Inventory module isn't enabled for this business"))?;
    let table = inventory_module.table_name();

    let tx = conn.transaction()?;

    let already_open: Option<String> = tx
        .query_row(
            "SELECT id FROM stock_takes WHERE business_id = ?1 AND status = 'in_progress'",
            params![business_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(existing_id) = already_open {
        return Err(anyhow!(
            "a stock take is already in progress (id: {existing_id}) — close it before starting a new one"
        ));
    }

    let stock_take_id = Uuid::new_v4().to_string();
    tx.execute(
        "INSERT INTO stock_takes (id, business_id, status, created_by_user_id) VALUES (?1, ?2, 'in_progress', ?3)",
        params![stock_take_id, business_id, user_id],
    )?;

    let items: Vec<(String, String, i64)> = {
        let mut stmt = tx.prepare(&format!(
            "SELECT id, name, quantity FROM {table} WHERE business_id = ?1 AND deleted_at IS NULL ORDER BY name"
        ))?;
        let rows = stmt.query_map(params![business_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    for (inv_id, name, qty) in &items {
        tx.execute(
            "INSERT INTO stock_take_items (id, stock_take_id, inventory_record_id, item_name, expected_qty)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![Uuid::new_v4().to_string(), stock_take_id, inv_id, name, qty],
        )?;
    }

    tx.commit()?;

    let _ = crate::audit::log(
        conn,
        business_id,
        Some(user_id),
        "_stock_take",
        "initiate",
        Some(&stock_take_id),
        Some(&json!({ "item_count": items.len() })),
    );

    get(conn, business_id, user_id, &stock_take_id)
}

/// Cancels an open stock take with zero effect on inventory — the
/// missing escape hatch for a forgotten/abandoned count. Before this,
/// the only way to release the business-wide lock on checkout,
/// receiving, refunds, and repack was to `close()` it, which is a
/// safe no-op when nothing was counted but looks identical in the UI
/// and audit log to a real, deliberate reconciliation — nothing marks
/// it as "walked away from," so a shift-change or forgotten count can
/// silently block sales for hours with no obvious way out for a
/// non-technical staffer.
///
/// Recorded counts are discarded, not applied: `initiate()` never
/// touched `inventory.quantity` (only `expected_qty` snapshots), and
/// `record_count()` only ever wrote to `stock_take_items`, so there is
/// nothing to roll back here — this simply marks the stock take
/// `cancelled` (distinct from `closed`) and frees the lock, the same
/// way `close()` does, without running any variance/write-off logic.
///
/// REQUIRES A SCHEMA CHANGE: this assumes the `stock_takes.status`
/// column accepts `'cancelled'` alongside `'in_progress'`/`'closed'`.
/// If that column has a CHECK constraint restricting it to the two
/// existing values (see db_migrations.rs), it must be widened first —
/// this file has no visibility into that migration.
pub fn cancel(conn: &mut Connection, business_id: &str, user_id: &str, stock_take_id: &str) -> Result<Value> {
    crate::rbac::require(conn, user_id, "inventory", "stocktake")?;

    let tx = conn.transaction()?;

    let status: Option<String> = tx
        .query_row(
            "SELECT status FROM stock_takes WHERE id = ?1 AND business_id = ?2",
            params![stock_take_id, business_id],
            |r| r.get(0),
        )
        .optional()?;
    match status.as_deref() {
        None => return Err(anyhow!("stock take not found: {stock_take_id}")),
        Some("closed") => return Err(anyhow!("this stock take is already closed")),
        Some("cancelled") => return Err(anyhow!("this stock take is already cancelled")),
        _ => {}
    }

    // Reported in the audit entry so a cancel that discarded real
    // in-progress counts is distinguishable from one that discarded
    // nothing — useful context for whoever reviews the trail later,
    // even though neither case wrote anything to inventory.
    let counted_items: i64 = tx.query_row(
        "SELECT COUNT(*) FROM stock_take_items WHERE stock_take_id = ?1 AND counted_qty IS NOT NULL",
        params![stock_take_id],
        |r| r.get(0),
    )?;

    tx.execute(
        "UPDATE stock_takes SET status = 'cancelled', closed_at = datetime('now'), closed_by_user_id = ?1 WHERE id = ?2",
        params![user_id, stock_take_id],
    )?;

    tx.commit()?;

    let _ = crate::audit::log(
        conn,
        business_id,
        Some(user_id),
        "_stock_take",
        "cancel",
        Some(stock_take_id),
        Some(&json!({ "counted_items_discarded": counted_items })),
    );

    get(conn, business_id, user_id, stock_take_id)
}

#[derive(Debug, Deserialize)]
pub struct RecordCountRequest {
    pub stock_take_id: String,
    pub item_id: String,
    pub counted_qty: i64,
}

/// Records a physical count against one item in an open stock take.
/// Can be called repeatedly for the same item — a recount before
/// close is a correction, not an error, so this simply overwrites the
/// previous count rather than rejecting a second entry.
pub fn record_count(conn: &Connection, business_id: &str, user_id: &str, req: RecordCountRequest) -> Result<()> {
    crate::rbac::require(conn, user_id, "inventory", "stocktake")?;

    if req.counted_qty < 0 {
        return Err(anyhow!("counted quantity cannot be negative"));
    }

    let status: Option<String> = conn
        .query_row(
            "SELECT status FROM stock_takes WHERE id = ?1 AND business_id = ?2",
            params![req.stock_take_id, business_id],
            |r| r.get(0),
        )
        .optional()?;
    match status.as_deref() {
        None => return Err(anyhow!("stock take not found: {}", req.stock_take_id)),
        Some("closed") => return Err(anyhow!("this stock take is already closed — counts can no longer be recorded against it")),
        _ => {}
    }

    let changed = conn.execute(
        "UPDATE stock_take_items SET counted_qty = ?1, counted_at = datetime('now')
         WHERE id = ?2 AND stock_take_id = ?3",
        params![req.counted_qty, req.item_id, req.stock_take_id],
    )?;
    if changed == 0 {
        return Err(anyhow!("stock take item not found: {}", req.item_id));
    }
    Ok(())
}

/// Closes a stock take: applies every counted item's variance to
/// `inventory.quantity` in one atomic transaction (all adjustments
/// land together, or — if the process dies mid-close — none do), then
/// marks the stock take closed so no further counts can be recorded
/// against it. Returns a variance report: what changed, by how much,
/// and what was never counted at all (left untouched, not zeroed).
pub fn close(conn: &mut Connection, business_id: &str, user_id: &str, stock_take_id: &str) -> Result<Value> {
    crate::rbac::require(conn, user_id, "inventory", "stocktake")?;

    let inventory_module = crud::load_module(conn, business_id, "inventory")
        .map_err(|_| anyhow!("the Inventory module isn't enabled for this business"))?;
    let table = inventory_module.table_name();

    let tx = conn.transaction()?;

    let status: Option<String> = tx
        .query_row(
            "SELECT status FROM stock_takes WHERE id = ?1 AND business_id = ?2",
            params![stock_take_id, business_id],
            |r| r.get(0),
        )
        .optional()?;
    match status.as_deref() {
        None => return Err(anyhow!("stock take not found: {stock_take_id}")),
        Some("closed") => return Err(anyhow!("this stock take is already closed")),
        _ => {}
    }

    let items: Vec<(String, String, String, i64, Option<i64>)> = {
        let mut stmt = tx.prepare(
            "SELECT id, inventory_record_id, item_name, expected_qty, counted_qty
             FROM stock_take_items WHERE stock_take_id = ?1 ORDER BY item_name",
        )?;
        let rows = stmt.query_map(params![stock_take_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, Option<i64>>(4)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    let mut adjustments = Vec::new();
    let mut skipped = Vec::new();
    // Counted during this stock take, but the item was soft-deleted
    // before close() ran — distinct from `skipped` (never counted).
    // `initiate()`'s snapshot query filters `deleted_at IS NULL`, so
    // close() has to honor that same exclusion at write time, not
    // just at snapshot time, or a deleted row gets a fresh quantity
    // "resurrected" onto it and a variance report shows a confirmed
    // adjustment for an item that's actually gone.
    let mut skipped_deleted = Vec::new();
    let mut total_variance_units: i64 = 0;
    // Real dollar value of confirmed shrinkage this close — summed
    // from the FEFO write-off portions below, not estimated from a
    // single flat item-level cost.
    let mut total_write_off_cost: i64 = 0;

    for (item_id, inv_id, item_name, expected_qty, counted_qty) in &items {
        let Some(counted) = counted_qty else {
            // Never counted during this stock take — expected value
            // stands untouched, reported separately so it's visible
            // this item was skipped, not silently treated as "counted
            // and found unchanged."
            skipped.push(json!({ "inventory_record_id": inv_id, "item_name": item_name, "expected_qty": expected_qty }));
            continue;
        };

        let still_active: bool = tx
            .query_row(
                &format!("SELECT 1 FROM {table} WHERE id = ?1 AND business_id = ?2 AND deleted_at IS NULL"),
                params![inv_id, business_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !still_active {
            skipped_deleted.push(json!({
                "inventory_record_id": inv_id,
                "item_name": item_name,
                "expected_qty": expected_qty,
                "counted_qty": counted,
            }));
            continue;
        }

        let variance = counted - expected_qty;
        // `n/a`, not a divide-by-zero or a misleading 0%, when this
        // item's expected baseline was itself zero — any nonzero count
        // against a zero baseline is an infinite percentage, not a
        // real number worth charting.
        let variance_pct: Value = if *expected_qty != 0 {
            json!((variance as f64 / *expected_qty as f64) * 100.0)
        } else {
            json!("n/a")
        };
        let mut write_off_cost: i64 = 0;

        if variance < 0 {
            // Shrinkage: a stock take is a form of stock leaving,
            // exactly like a sale, just with no revenue attached — so
            // it draws down through the same strict FEFO order
            // checkout uses (dated batches soonest-first, then
            // undated, then legacy last) instead of a bare quantity
            // overwrite that would leave inventory_batches stale.
            let shrinkage_qty = -variance;
            // Read live `quantity` here, in this same transaction,
            // rather than trusting `*expected_qty` for it. The two are
            // guaranteed equal by `require_no_open_stock_take` (nothing
            // else can have moved `quantity` since `initiate()`'s
            // snapshot) — but `fefo_consume_in_tx` uses this value to
            // work out how much of `shrinkage_qty` legacy stock can
            // still cover once batches are exhausted, so getting it
            // from the row itself costs one extra column on a query
            // this loop already makes, and keeps that math right even
            // if the guard is ever loosened or bypassed elsewhere.
            let (live_qty, legacy_unit_cost, legacy_unit_price): (i64, i64, i64) = tx.query_row(
                &format!("SELECT quantity, unit_cost, unit_price FROM {table} WHERE id = ?1 AND business_id = ?2"),
                params![inv_id, business_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            let portions = crate::batches::fefo_consume_in_tx(
                &tx,
                business_id,
                inv_id,
                shrinkage_qty,
                live_qty,
                legacy_unit_cost,
                legacy_unit_price,
            )?;
            write_off_cost = portions.iter().map(|p| p.unit_cost * p.quantity).sum();
            tx.execute(
                &format!("UPDATE {table} SET quantity = ?1, updated_at = datetime('now') WHERE id = ?2 AND business_id = ?3 AND deleted_at IS NULL"),
                params![counted, inv_id, business_id],
            )?;
            total_variance_units += variance;
            total_write_off_cost += write_off_cost;

            // Ledger entry for the same quantity change, in the same
            // transaction — see stock_movement.rs. Costed from the
            // real FEFO write-off just computed above, not the item's
            // flat legacy cost, so the ledger and the write-off figure
            // profit.rs reports can never disagree.
            crate::stock_movement::record_in_tx(
                &tx,
                business_id,
                Some(user_id),
                inv_id,
                item_name,
                crate::stock_movement::STOCK_TAKE_SHRINKAGE,
                variance,
                if shrinkage_qty > 0 { write_off_cost / shrinkage_qty } else { 0 },
                Some(stock_take_id),
            )?;
        } else if variance > 0 {
            // THE FIX: this used to always just bump the legacy
            // `quantity` column, on the assumption (see this branch's
            // old comment, preserved in spirit but no longer accurate
            // for every item) that legacy quantity is always
            // already-priced, already-real stock. That's only true for
            // a genuinely pre-batch item. For any item created under
            // crud.rs's forced-zero rule (every new item's
            // unit_cost/unit_price starts at 0 — see that file's own
            // comment: a real price only ever enters through a priced
            // batch from that point on), the legacy unit_price column
            // is permanently 0, never a real price. Bumping legacy
            // quantity for one of THOSE items — one whose only-ever
            // batch has since been fully sold down to
            // quantity_remaining = 0 — left real, sellable-looking
            // stock with no price anywhere pos.rs's lookup_products
            // could find (its COALESCE falls straight through to that
            // permanently-zero legacy column once no batch has
            // anything left), showing as a genuine $0.00 at the POS
            // screen for every user — not a permissions or display
            // issue, an actual pricing gap this surplus itself created.
            //
            // So: if this item has EVER had a batch (even one fully
            // consumed by now), a surplus creates a NEW batch instead,
            // priced at the most recent batch's own cost/price —
            // "found stock, unknown origin" still needs *some* honest
            // price to sell at, and the last real price this exact
            // item actually sold at is a far better answer than
            // silently defaulting to $0. Only a genuinely pre-batch
            // item (one that has NEVER had a batch at all, so its
            // legacy unit_price really is live, real pricing) keeps
            // the original bare-legacy-quantity-bump behavior.
            //
            // `i.quantity` is still overwritten to `counted` below
            // exactly as before either way — it's the TOTAL count
            // (legacy portion is derived as quantity minus what
            // batches account for, see batches.rs's module doc
            // comment), not something this new batch would double up
            // against: adding `variance` units to a new batch's
            // quantity_remaining while separately setting `i.quantity`
            // to `counted` (= expected + variance) leaves the
            // *legacy* portion exactly where it was before this
            // close() call — only the surplus itself moves from
            // unpriced legacy stock to a priced batch.
            let last_batch_pricing: Option<(i64, i64)> = tx
                .query_row(
                    "SELECT unit_cost, unit_price FROM inventory_batches
                     WHERE inventory_record_id = ?1 AND business_id = ?2 AND deleted_at IS NULL
                     ORDER BY received_at DESC, id DESC LIMIT 1",
                    params![inv_id, business_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;

            tx.execute(
                &format!("UPDATE {table} SET quantity = ?1, updated_at = datetime('now') WHERE id = ?2 AND business_id = ?3 AND deleted_at IS NULL"),
                params![counted, inv_id, business_id],
            )?;
            total_variance_units += variance;

            let surplus_unit_cost = match last_batch_pricing {
                Some((last_cost, last_price)) => {
                    let received_at = chrono::Utc::now().to_rfc3339();
                    crate::batches::create_batch_in_tx(
                        &tx,
                        business_id,
                        inv_id,
                        None,
                        variance,
                        last_cost,
                        last_price,
                        None,
                        &received_at,
                        Some(user_id),
                    )?;
                    last_cost
                }
                // A genuinely pre-batch item: its legacy unit_price is
                // real, live pricing, so the original zero-cost
                // treatment stands — the surplus is real stock at a
                // real price already, nothing more to attach.
                None => 0,
            };

            crate::stock_movement::record_in_tx(
                &tx,
                business_id,
                Some(user_id),
                inv_id,
                item_name,
                crate::stock_movement::STOCK_TAKE_SURPLUS,
                variance,
                surplus_unit_cost,
                Some(stock_take_id),
            )?;
        }
        // variance == 0: nothing to write; still reported below so a
        // confirmed-correct count is visible, not just an unmentioned
        // absence.

        // Persisted onto the row itself — not just returned in this
        // response and the audit log — so profit.rs's shrinkage figure
        // (and anything else that ever needs to add this up later) has
        // a real, permanent, queryable number to read instead of
        // re-deriving or losing it. See db_migrations.rs's v34 for why
        // this column exists at all. Written even when it's 0 (a
        // surplus or an exact match), for the same "explicit, not
        // implied by a column's default" reasoning the rest of this
        // function already holds itself to.
        tx.execute(
            "UPDATE stock_take_items SET write_off_cost_cents = ?1 WHERE id = ?2",
            params![write_off_cost, item_id],
        )?;

        adjustments.push(json!({
            "inventory_record_id": inv_id,
            "item_name": item_name,
            "expected_qty": expected_qty,
            "counted_qty": counted,
            "variance": variance,
            "variance_pct": variance_pct,
            "write_off_cost": write_off_cost,
        }));
    }

    tx.execute(
        "UPDATE stock_takes SET status = 'closed', closed_at = datetime('now'), closed_by_user_id = ?1 WHERE id = ?2",
        params![user_id, stock_take_id],
    )?;

    // Same discipline as checkout()/receive()/repack(): nothing above
    // is durable until this line — every adjustment becomes real
    // together, or (if the process dies first) none of them do.
    tx.commit()?;

    let summary = json!({
        "stock_take_id": stock_take_id,
        "items_counted": adjustments.len(),
        "items_skipped": skipped.len(),
        "items_skipped_deleted": skipped_deleted.len(),
        "total_variance_units": total_variance_units,
        "total_write_off_cost": total_write_off_cost,
        "adjustments": adjustments,
        "skipped": skipped,
        "skipped_deleted": skipped_deleted,
    });

    // The traceability record — same reasoning as repack.rs's own
    // "_repack" pseudo-module log: this is what makes the reconciled
    // amounts checkable after the fact, not just true in the moment.
    let _ = crate::audit::log(conn, business_id, Some(user_id), "_stock_take", "close", Some(stock_take_id), Some(&summary));

    Ok(summary)
}

/// Fetches one stock take (open or closed) with its full item list —
/// used both right after `initiate()` and for viewing an in-progress
/// count's current state, or a past closed one's final numbers.
pub fn get(conn: &Connection, business_id: &str, user_id: &str, stock_take_id: &str) -> Result<Value> {
    crate::rbac::require(conn, user_id, "inventory", "stocktake")?;

    let head: Option<(String, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT status, created_at, closed_at FROM stock_takes WHERE id = ?1 AND business_id = ?2",
            params![stock_take_id, business_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((status, created_at, closed_at)) = head else {
        return Err(anyhow!("stock take not found: {stock_take_id}"));
    };

    let mut stmt = conn.prepare(
        "SELECT id, inventory_record_id, item_name, expected_qty, counted_qty
         FROM stock_take_items WHERE stock_take_id = ?1 ORDER BY item_name",
    )?;
    let items: Vec<Value> = stmt
        .query_map(params![stock_take_id], |r| {
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "inventory_record_id": r.get::<_, String>(1)?,
                "item_name": r.get::<_, String>(2)?,
                "expected_qty": r.get::<_, i64>(3)?,
                "counted_qty": r.get::<_, Option<i64>>(4)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(json!({
        "id": stock_take_id,
        "status": status,
        "created_at": created_at,
        "closed_at": closed_at,
        "items": items,
    }))
}

/// Returns the currently open stock take for this business, if any —
/// lets the frontend detect "resume this" vs "show a Start button"
/// without the caller needing to already know an id.
pub fn get_open(conn: &Connection, business_id: &str, user_id: &str) -> Result<Option<Value>> {
    crate::rbac::require(conn, user_id, "inventory", "stocktake")?;
    let id: Option<String> = conn
        .query_row(
            "SELECT id FROM stock_takes WHERE business_id = ?1 AND status = 'in_progress'",
            params![business_id],
            |r| r.get(0),
        )
        .optional()?;
    match id {
        Some(id) => Ok(Some(get(conn, business_id, user_id, &id)?)),
        None => Ok(None),
    }
}

/// Lists past stock takes (most recent first) for a simple history
/// view — summary only, not the full item list, matching how a list
/// screen should stay cheap regardless of catalog size.
pub fn list(conn: &Connection, business_id: &str, user_id: &str) -> Result<Value> {
    crate::rbac::require(conn, user_id, "inventory", "stocktake")?;
    // max/avg are over ABS(variance_pct) — "worst" stocktake means
    // largest swing in either direction, not largest net surplus. Only
    // counted items with a nonzero expected baseline contribute (an
    // uncounted item has no variance yet; a zero-expected baseline has
    // no meaningful percentage — see close()'s own "n/a" handling),
    // so both come back NULL/None for a stock take with nothing
    // countable yet, rather than a misleading 0.
    let mut stmt = conn.prepare(
        "SELECT st.id, st.status, st.created_at, st.closed_at,
                (SELECT COUNT(*) FROM stock_take_items sti WHERE sti.stock_take_id = st.id) AS item_count,
                (SELECT COUNT(*) FROM stock_take_items sti WHERE sti.stock_take_id = st.id AND sti.counted_qty IS NOT NULL) AS counted_count,
                (SELECT MAX(ABS((sti.counted_qty - sti.expected_qty) * 100.0 / sti.expected_qty))
                   FROM stock_take_items sti
                   WHERE sti.stock_take_id = st.id AND sti.counted_qty IS NOT NULL AND sti.expected_qty != 0) AS max_variance_pct,
                (SELECT AVG(ABS((sti.counted_qty - sti.expected_qty) * 100.0 / sti.expected_qty))
                   FROM stock_take_items sti
                   WHERE sti.stock_take_id = st.id AND sti.counted_qty IS NOT NULL AND sti.expected_qty != 0) AS avg_variance_pct
         FROM stock_takes st WHERE st.business_id = ?1 ORDER BY st.created_at DESC",
    )?;
    let rows: Vec<Value> = stmt
        .query_map(params![business_id], |r| {
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "status": r.get::<_, String>(1)?,
                "created_at": r.get::<_, String>(2)?,
                "closed_at": r.get::<_, Option<String>>(3)?,
                "item_count": r.get::<_, i64>(4)?,
                "counted_count": r.get::<_, i64>(5)?,
                "max_variance_pct": r.get::<_, Option<f64>>(6)?,
                "avg_variance_pct": r.get::<_, Option<f64>>(7)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(json!({ "stock_takes": rows }))
}
