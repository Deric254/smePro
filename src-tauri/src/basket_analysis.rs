//! "Frequently bought together" — which items appear in the same
//! order, and how often. This is not a new data source: pos.rs
//! already writes one shared `order_id` across every line item of a
//! single checkout (see checkout() and service_sale()), and has for
//! every sale made through the POS since that field existed. This
//! module just aggregates data that was already being recorded,
//! read-only, the same way report.rs aggregates the same table for
//! revenue/trend charts — it never writes to module_sales and never
//! changes what a sale record means.
//!
//! Deliberately excludes any row with no order_id (older imported
//! sales, or sales entered directly as records rather than through
//! checkout) rather than guessing at a basket for them — a "pair"
//! inferred from anything other than a real, shared order_id would be
//! a fabricated relationship, not a real one.

use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;

use crate::rbac;

#[derive(Debug, Serialize)]
pub struct BasketPair {
    pub item_a: String,
    pub item_b: String,
    /// Number of distinct orders containing both items. Deduplicated
    /// at the query level (see top_pairs) even if either item appears
    /// as more than one line within the same order.
    pub order_count: i64,
    /// Sum of the `revenue` field across every one of those matching
    /// line rows (both sides of the pair) — real recorded revenue,
    /// not a derived or estimated figure.
    pub combined_revenue_cents: f64,
}

/// Runs the "frequently bought together" query. `limit` is clamped to
/// [1, 50] — this is a discovery report meant to be skimmed, not a
/// full export, and an unbounded limit on a large sales history would
/// turn one dashboard request into an unbounded amount of work.
pub fn top_pairs(
    conn: &Connection,
    business_id: &str,
    user_id: &str,
    range_start: Option<&str>,
    range_end: Option<&str>,
    limit: i64,
) -> Result<Vec<BasketPair>> {
    // Same permission gate report.rs uses for the sales module, plus
    // the same "is this module actually enabled" check report::run
    // does before it ever builds a query — a disabled module having
    // its table still physically exist is not the same as it being a
    // valid data source right now.
    rbac::require(conn, user_id, "sales", "read")?;
    let enabled: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM modules WHERE business_id = ?1 AND id = 'sales' AND enabled = 1",
            rusqlite::params![business_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if enabled == 0 {
        anyhow::bail!("module 'sales' is not enabled for this business");
    }

    let limit = limit.clamp(1, 50);

    let mut where_clauses = vec![
        "business_id = ?1".to_string(),
        "deleted_at IS NULL".to_string(),
        "order_id IS NOT NULL".to_string(),
        "order_id != ''".to_string(),
    ];
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(business_id.to_string())];

    if let Some(start) = range_start {
        params.push(Box::new(start.to_string()));
        where_clauses.push(format!("created_at >= ?{}", params.len()));
    }
    if let Some(end) = range_end {
        params.push(Box::new(end.to_string()));
        where_clauses.push(format!("created_at <= ?{}", params.len()));
    }
    params.push(Box::new(limit));
    let limit_placeholder = params.len();

    // Two steps, not one direct self-join on the raw table: an item
    // can legitimately appear as more than one line within the same
    // order (e.g. two units rung up separately at different
    // discounts — see pos.rs). A direct join on module_sales would
    // then produce more than one matching row for that single order,
    // inflating both the order count AND the revenue total for that
    // pair. The `item_totals` CTE collapses each order down to at
    // most one row per item first (summing its revenue within that
    // order), so the join afterward can only ever match one row per
    // real item-pair-per-order — no COUNT(DISTINCT) workaround
    // needed, and no risk of double-counting revenue.
    //
    // a.item_name < b.item_name (a strict, deterministic ordering) is
    // what keeps each real pair counted exactly once: without it,
    // "Rice + Beans" and "Beans + Rice" would both match as two
    // separate rows, and an item paired with itself would too.
    let sql = format!(
        "WITH item_totals AS (
             SELECT order_id, item_name, SUM(revenue) AS item_revenue
             FROM module_sales
             WHERE {}
             GROUP BY order_id, item_name
         )
         SELECT a.item_name, b.item_name, COUNT(*) AS order_count,
                COALESCE(SUM(a.item_revenue + b.item_revenue), 0) AS combined_revenue
         FROM item_totals a
         JOIN item_totals b ON a.order_id = b.order_id AND a.item_name < b.item_name
         GROUP BY a.item_name, b.item_name
         ORDER BY order_count DESC
         LIMIT ?{}",
        where_clauses.join(" AND "),
        limit_placeholder
    );

    let mut stmt = conn.prepare(&sql)?;
    let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
    let rows = stmt.query_map(params_refs.as_slice(), |row| {
        Ok(BasketPair {
            item_a: row.get(0)?,
            item_b: row.get(1)?,
            order_count: row.get(2)?,
            combined_revenue_cents: row.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
        })
    })?;

    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
}
