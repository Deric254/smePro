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

/// How many days of stock are left at recent selling pace — quantity
/// on hand divided by average units sold per day over the lookback
/// window. Answers "what do I need to reorder soon," which low-stock
/// (a fixed reorder_level threshold) can't: a reorder_level of 5 means
/// nothing if an item sells 50 units a day.
#[derive(Debug, Serialize)]
pub struct StockRunway {
    pub item_name: String,
    pub quantity: f64,
    /// Units sold per day, averaged over the lookback window. 0 means
    /// no sales recorded for this item in that window at all.
    pub avg_daily_sales: f64,
    /// None when avg_daily_sales is 0 — there is no rate to divide by,
    /// so "days until stockout" is genuinely undefined here, not
    /// infinite. Never fabricated as a large placeholder number.
    pub days_of_stock_left: Option<f64>,
}

/// Same name-based Inventory/Sales link as slow_movers above, same
/// limitation. `lookback_days` clamped to [7, 180] — under a week is
/// too noisy to average meaningfully; over 180 days starts averaging
/// in demand from long enough ago it may not reflect current pace.
pub fn stock_runway(
    conn: &Connection,
    business_id: &str,
    user_id: &str,
    today: &str,
    lookback_days: i64,
    limit: i64,
) -> Result<Vec<StockRunway>> {
    crate::rbac::require(conn, user_id, "inventory", "read")?;
    crate::rbac::require(conn, user_id, "sales", "read")?;
    let inventory_module = crud::load_module(conn, business_id, "inventory")
        .map_err(|_| anyhow!("the Inventory module isn't enabled for this business"))?;
    let sales_module = crud::load_module(conn, business_id, "sales")
        .map_err(|_| anyhow!("the Sales module isn't enabled for this business"))?;
    let inv_table = inventory_module.table_name();
    let sales_table = sales_module.table_name();

    let lookback_days = lookback_days.clamp(7, 180);
    let limit = limit.clamp(1, 100);

    let sql = format!(
        "SELECT i.name, i.quantity, COALESCE(s.qty_sold, 0)
         FROM {inv_table} i
         LEFT JOIN (
             SELECT item_name, SUM(quantity) AS qty_sold
             FROM {sales_table}
             WHERE business_id = ?1 AND deleted_at IS NULL
               AND created_at >= date(?2, '-' || ?3 || ' days')
             GROUP BY item_name
         ) s ON s.item_name = i.name
         WHERE i.business_id = ?1 AND i.deleted_at IS NULL AND i.quantity > 0"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![business_id, today, lookback_days], |r| {
        let quantity: f64 = r.get(1)?;
        let qty_sold: f64 = r.get(2)?;
        let avg_daily_sales = qty_sold / lookback_days as f64;
        Ok(StockRunway {
            item_name: r.get(0)?,
            quantity,
            avg_daily_sales,
            days_of_stock_left: if avg_daily_sales > 0.0 { Some(quantity / avg_daily_sales) } else { None },
        })
    })?;

    let mut out = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    // Soonest to run out first. An item with no recent sales at all
    // isn't "safe" — it's "unknown" — so it sorts last, not first: a
    // real, computable urgency always outranks an absence of data.
    out.sort_by(|a, b| match (a.days_of_stock_left, b.days_of_stock_left) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    out.truncate(limit as usize);
    Ok(out)
}
