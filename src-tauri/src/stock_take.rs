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
//!   3. `close()` — for every item that WAS counted, applies the
//!      variance (counted - expected) directly to
//!      `inventory.quantity` in one atomic transaction, the same way
//!      receiving/refund/repack do, and returns a variance report.
//!      Uncounted items are untouched and separately reported as
//!      "skipped," not silently folded into "no change."
//!
//! ONLY ONE STOCK TAKE OPEN AT A TIME, per business — enforced by a
//! partial unique index in the schema (see db_migrations.rs's v11),
//! not just an application-level check, so a race between two
//! `initiate()` calls fails at the database level rather than
//! producing two simultaneously "current" counts with no way to tell
//! which one a given count belongs to.

use crate::crud;
use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

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
    let mut total_variance_units: i64 = 0;

    for (_item_id, inv_id, item_name, expected_qty, counted_qty) in &items {
        let Some(counted) = counted_qty else {
            // Never counted during this stock take — expected value
            // stands untouched, reported separately so it's visible
            // this item was skipped, not silently treated as "counted
            // and found unchanged."
            skipped.push(json!({ "inventory_record_id": inv_id, "item_name": item_name, "expected_qty": expected_qty }));
            continue;
        };
        let variance = counted - expected_qty;
        if variance != 0 {
            tx.execute(
                &format!("UPDATE {table} SET quantity = ?1, updated_at = datetime('now') WHERE id = ?2 AND business_id = ?3"),
                params![counted, inv_id, business_id],
            )?;
            total_variance_units += variance;
        }
        adjustments.push(json!({
            "inventory_record_id": inv_id,
            "item_name": item_name,
            "expected_qty": expected_qty,
            "counted_qty": counted,
            "variance": variance,
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
        "total_variance_units": total_variance_units,
        "adjustments": adjustments,
        "skipped": skipped,
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
    let mut stmt = conn.prepare(
        "SELECT st.id, st.status, st.created_at, st.closed_at,
                (SELECT COUNT(*) FROM stock_take_items sti WHERE sti.stock_take_id = st.id) AS item_count,
                (SELECT COUNT(*) FROM stock_take_items sti WHERE sti.stock_take_id = st.id AND sti.counted_qty IS NOT NULL) AS counted_count
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
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(json!({ "stock_takes": rows }))
}
