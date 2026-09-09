//! Slow-moving / dead stock — inventory sitting on the shelf that
//! hasn't sold in a while, or ever. Answers a different question than
//! the existing low-stock flag in business_pulse.rs (which watches for
//! running OUT of an item): this watches for capital tied up in items
//! that aren't moving at all.
//!
//! IMPORTANT LIMITATION, stated plainly rather than hidden: Inventory
//! and Sales are only ever linked by matching `name` to `item_name` as
//! plain text (see pos.rs — a sale snapshots the item's display name
//! at the moment of sale; there is no foreign key between the two
//! tables, anywhere in this codebase, and basket_analysis.rs and
//! profit.rs already rely on that exact same name-matching
//! convention). If an inventory item is renamed after it was sold,
//! this can't find those older sales under the new name and will
//! under-count how recently it actually moved. This is an existing,
//! systemic property of how the two modules relate, not something
//! introduced here — fixing it would mean adding a real link between
//! Sales and Inventory records, which is a schema change well outside
//! this feature's scope.

use crate::crud;
use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct SlowMover {
    pub item_name: String,
    pub quantity: f64,
    /// quantity × unit_cost, at Inventory's current cost basis — 0 if
    /// unit_cost was never recorded for this item, never estimated.
    pub value_at_risk_cents: i64,
    /// None means never sold at all (under whatever name it has now —
    /// see the module doc comment above), not "sold infinitely long
    /// ago".
    pub last_sale_at: Option<String>,
    pub days_since_last_sale: Option<i64>,
}

/// `stale_after_days`: an item with no sale in at least this many days
/// (or no sale ever) is included. Clamped to [1, 365] — this is a
/// discovery report, not an unbounded export.
pub fn slow_movers(
    conn: &Connection,
    business_id: &str,
    user_id: &str,
    today: &str,
    stale_after_days: i64,
    limit: i64,
) -> Result<Vec<SlowMover>> {
    crate::rbac::require(conn, user_id, "inventory", "read")?;
    crate::rbac::require(conn, user_id, "sales", "read")?;
    let inventory_module = crud::load_module(conn, business_id, "inventory")
        .map_err(|_| anyhow!("the Inventory module isn't enabled for this business"))?;
    let sales_module = crud::load_module(conn, business_id, "sales")
        .map_err(|_| anyhow!("the Sales module isn't enabled for this business"))?;
    let inv_table = inventory_module.table_name();
    let sales_table = sales_module.table_name();

    let stale_after_days = stale_after_days.clamp(1, 365);
    let limit = limit.clamp(1, 100);

    let sql = format!(
        "WITH per_item AS (
             SELECT i.id, i.name, i.quantity, i.unit_cost, MAX(s.created_at) AS last_sale_at
             FROM {inv_table} i
             LEFT JOIN {sales_table} s
               ON s.business_id = i.business_id AND s.item_name = i.name AND s.deleted_at IS NULL
             WHERE i.business_id = ?1 AND i.deleted_at IS NULL AND i.quantity > 0
             GROUP BY i.id
         )
         SELECT name, quantity, unit_cost, last_sale_at,
                CASE WHEN last_sale_at IS NULL THEN NULL
                     ELSE CAST(julianday(?2) - julianday(last_sale_at) AS INTEGER) END AS days_since
         FROM per_item
         WHERE last_sale_at IS NULL OR last_sale_at < date(?2, '-' || ?3 || ' days')
         ORDER BY CASE WHEN last_sale_at IS NULL THEN 0 ELSE 1 END, last_sale_at ASC
         LIMIT ?4"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![business_id, today, stale_after_days, limit], |r| {
        let quantity: f64 = r.get(1)?;
        let unit_cost: i64 = r.get::<_, Option<i64>>(2)?.unwrap_or(0);
        Ok(SlowMover {
            item_name: r.get(0)?,
            quantity,
            value_at_risk_cents: (quantity * unit_cost as f64).round() as i64,
            last_sale_at: r.get(3)?,
            days_since_last_sale: r.get(4)?,
        })
    })?;

    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
}
