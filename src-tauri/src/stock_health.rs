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

/// Items sitting in Inventory with a zero unit_cost, zero unit_price,
/// or both. Both fields are "money" (required, default 0 — see
/// inventory.json), so an item created without either one is
/// perfectly valid data as far as the schema and the "never sell below
/// cost" rule are concerned (0 is not less than 0) — this report is
/// how a business actually catches that before a cashier rings up a
/// $0 sale at the till, not a hard block on creation, which would
/// wrongly reject the (rarer, but real) case of a genuinely free
/// promotional give-away item.
///
/// Deliberately NOT filtered to `quantity > 0` the way slow_movers and
/// stock_runway are: those two are about capital efficiency on stock
/// that's actually on the shelf right now, but a zero-priced item with
/// no stock yet is just as capable of getting sold for $0 the moment
/// it's next received — this report exists to catch the pricing
/// mistake itself, before it matters, not just once it's already
/// costing money.
#[derive(Debug, Serialize)]
pub struct UnpricedItem {
    pub item_name: String,
    pub quantity: f64,
    pub unit_cost_cents: i64,
    pub unit_price_cents: i64,
    /// Which side of the pair is actually zero — "cost", "price", or
    /// "both" — so the UI can say precisely what's missing instead of
    /// a generic "check this item".
    pub missing: &'static str,
}

/// `limit` clamped to [1, 500] — same "discovery report, not an
/// unbounded export" reasoning as slow_movers/stock_runway above,
/// just a wider ceiling since every matching row here is actionable
/// (there's no long tail of merely-slow items to cut off early).
pub fn unpriced_items(
    conn: &Connection,
    business_id: &str,
    user_id: &str,
    limit: i64,
) -> Result<Vec<UnpricedItem>> {
    crate::rbac::require(conn, user_id, "inventory", "read")?;
    let inventory_module = crud::load_module(conn, business_id, "inventory")
        .map_err(|_| anyhow!("the Inventory module isn't enabled for this business"))?;
    let inv_table = inventory_module.table_name();
    let limit = limit.clamp(1, 500);

    let sql = format!(
        "SELECT name, quantity, unit_cost, unit_price
         FROM {inv_table}
         WHERE business_id = ?1 AND deleted_at IS NULL AND (unit_cost = 0 OR unit_price = 0)
         ORDER BY name ASC
         LIMIT ?2"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![business_id, limit], |r| {
        let quantity: f64 = r.get(1)?;
        let unit_cost: i64 = r.get(2)?;
        let unit_price: i64 = r.get(3)?;
        let missing = match (unit_cost == 0, unit_price == 0) {
            (true, true) => "both",
            (true, false) => "cost",
            (false, true) => "price",
            // Can't be reached — the WHERE clause above only ever
            // matches a row where at least one side is 0.
            (false, false) => "both",
        };
        Ok(UnpricedItem {
            item_name: r.get(0)?,
            quantity,
            unit_cost_cents: unit_cost,
            unit_price_cents: unit_price,
            missing,
        })
    })?;

    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
}

/// Purchasing order lines recorded with a zero unit_cost. Purchasing
/// has no unit_price field to compare against (it's a buying record,
/// not a selling one), and its own `unit_cost` floor is `min: 0`, not
/// `min: 1` — a genuinely free/donated delivery is real, legitimate
/// data, so this stays a report, not a hard block (same choice as
/// unpriced_items above, same reasoning).
///
/// Why this matters beyond Purchasing itself: `receiving.rs`'s
/// weighted-average cost recalculation folds a received PO's
/// unit_cost straight into the matching Inventory item's cost basis.
/// A $0 purchase — typo'd or genuine — pulls that cost basis toward
/// zero, which can make Inventory's own "price can't be below cost"
/// guard nearly meaningless on that item afterward (almost any price
/// clears an almost-zero cost). `received` is included in the result
/// so it's clear whether that cost-basis effect has already happened
/// for a given row, or the order is still pending.
#[derive(Debug, Serialize)]
pub struct ZeroCostPurchase {
    pub po_number: String,
    pub supplier: String,
    pub item_name: String,
    pub quantity: f64,
    pub received: bool,
}

/// `limit` clamped to [1, 500] — same reasoning as unpriced_items.
pub fn zero_cost_purchases(
    conn: &Connection,
    business_id: &str,
    user_id: &str,
    limit: i64,
) -> Result<Vec<ZeroCostPurchase>> {
    crate::rbac::require(conn, user_id, "purchasing", "read")?;
    let purchasing_module = crud::load_module(conn, business_id, "purchasing")
        .map_err(|_| anyhow!("the Purchasing module isn't enabled for this business"))?;
    let table = purchasing_module.table_name();
    let limit = limit.clamp(1, 500);

    let sql = format!(
        "SELECT po_number, supplier, item_name, quantity, received
         FROM {table}
         WHERE business_id = ?1 AND deleted_at IS NULL AND unit_cost = 0
         ORDER BY po_number ASC
         LIMIT ?2"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![business_id, limit], |r| {
        Ok(ZeroCostPurchase {
            po_number: r.get(0)?,
            supplier: r.get(1)?,
            item_name: r.get(2)?,
            quantity: r.get(3)?,
            received: r.get(4)?,
        })
    })?;

    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
}
