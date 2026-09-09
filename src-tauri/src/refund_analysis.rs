//! Refund rate by item — which items get returned more than others,
//! as a fraction of how much was actually sold. A raw refund count
//! means little on its own (your best-seller will rack up the most
//! refunds by volume alone); the rate against units sold is the
//! actual quality/fit signal.
//!
//! Same name-based link as stock_health.rs and basket_analysis.rs:
//! Refunds already carries its own `item_name` field directly (see
//! refunds.json), snapshotted the same way Sales' own `item_name` is
//! (see refund.rs), so no join through Inventory is even needed here
//! — this compares Sales and Refunds directly against each other.

use crate::crud;
use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct RefundRate {
    pub item_name: String,
    pub sold_quantity: f64,
    pub refunded_quantity: f64,
    pub refunded_amount_cents: i64,
    pub refund_count: i64,
    /// refunded_quantity / sold_quantity × 100. None only in the
    /// (should-be-impossible, but never assumed) case of zero
    /// recorded sold quantity for an item that nonetheless has
    /// refunds against its name — reported as "unknown", never a
    /// fabricated 0% or 100%.
    pub refund_rate_pct: Option<f64>,
}

/// Only items with at least one refunded unit are returned — an item
/// with zero refunds doesn't need to appear in a report about which
/// items get returned. `limit` clamped to [1, 100].
pub fn by_item(conn: &Connection, business_id: &str, user_id: &str, limit: i64) -> Result<Vec<RefundRate>> {
    crate::rbac::require(conn, user_id, "sales", "read")?;
    crate::rbac::require(conn, user_id, "refunds", "read")?;
    let sales_module = crud::load_module(conn, business_id, "sales")
        .map_err(|_| anyhow!("the Sales module isn't enabled for this business"))?;
    let refunds_module = crud::load_module(conn, business_id, "refunds")
        .map_err(|_| anyhow!("the Refunds module isn't enabled for this business"))?;
    let sales_table = sales_module.table_name();
    let refunds_table = refunds_module.table_name();

    let limit = limit.clamp(1, 100);

    let sql = format!(
        "WITH sold_totals AS (
             SELECT item_name, COALESCE(SUM(quantity), 0) AS sold_qty
             FROM {sales_table}
             WHERE business_id = ?1 AND deleted_at IS NULL
             GROUP BY item_name
         ),
         refund_totals AS (
             SELECT item_name, COALESCE(SUM(quantity_refunded), 0) AS refunded_qty,
                    COALESCE(SUM(refund_amount), 0) AS refunded_amount, COUNT(*) AS refund_count
             FROM {refunds_table}
             WHERE business_id = ?1 AND deleted_at IS NULL
             GROUP BY item_name
         )
         SELECT r.item_name, COALESCE(s.sold_qty, 0), r.refunded_qty, r.refunded_amount, r.refund_count
         FROM refund_totals r
         LEFT JOIN sold_totals s ON s.item_name = r.item_name
         WHERE r.refunded_qty > 0
         ORDER BY CASE WHEN COALESCE(s.sold_qty, 0) > 0
                       THEN CAST(r.refunded_qty AS REAL) / s.sold_qty ELSE 0 END DESC
         LIMIT ?2"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![business_id, limit], |row| {
        let sold_quantity: f64 = row.get(1)?;
        let refunded_quantity: f64 = row.get(2)?;
        Ok(RefundRate {
            item_name: row.get(0)?,
            sold_quantity,
            refunded_quantity,
            refunded_amount_cents: row.get(3)?,
            refund_count: row.get(4)?,
            refund_rate_pct: if sold_quantity > 0.0 {
                Some(refunded_quantity / sold_quantity * 100.0)
            } else {
                None
            },
        })
    })?;

    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
}
