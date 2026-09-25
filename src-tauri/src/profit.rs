//! Gross profit — computed directly from Sales' own `revenue` and
//! `cost_at_sale` columns (see sales.json and pos.rs::checkout()'s own
//! doc comment on why that field exists at all).
//!
//! Deliberately just one query against one table, not a join across
//! Sales, Inventory, and Purchasing: `cost_at_sale` is already the
//! exact, historical cost of what was sold, snapshotted the instant it
//! was sold, so `SUM(revenue) - SUM(cost_at_sale)` — equivalently,
//! `SUM(revenue - cost_at_sale)`, used directly below so the
//! subtraction happens once per row in integer cents rather than as
//! two separate sums that could each overflow or round differently
//! before being subtracted — is already the real, current, refund-
//! aware gross profit. Every write that touches either column
//! (checkout, refund) already keeps this table honest; this module
//! does no writing of its own, only reads what's already there.
//!
//! Same "degrade honestly rather than fabricate" standard
//! business_pulse.rs and debt_settlement::summary already hold
//! themselves to: a business without Sales enabled, or with no sales
//! at all yet, gets a real zeroed-out summary (or an error the caller
//! can choose to hide the KPI card on), never a made-up number.

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};

#[derive(Debug, serde::Serialize)]
pub struct GrossProfitSummary {
    pub revenue_cents: i64,
    pub cost_cents: i64,
    pub profit_cents: i64,
    /// `None` specifically when revenue is zero — a margin percentage
    /// against zero revenue is undefined, not "0%" (which would
    /// falsely claim break-even) and not an infinite/error value.
    /// Left as a real absence for the frontend to render as "-" or
    /// similar, the same way business_pulse.rs already treats a
    /// missing comparison period.
    pub margin_pct: Option<f64>,
    pub sales_count: i64,
    /// Real count of sales with cost data, not a boolean — so the
    /// frontend can show "42 of 50 sales have real cost data" instead
    /// of a flag that can't distinguish 2-of-50 from 50-of-50.
    pub cost_bearing_sales_count: i64,
    /// All-time confirmed shrinkage cost from CLOSED stock takes only
    /// (a cancelled one wrote nothing — see stock_take.rs::cancel) —
    /// a second, deliberately separate query against
    /// `stock_take_items`, NOT folded into `cost_cents`/`profit_cents`
    /// above. Sales' own cost_at_sale is "what was paid for something
    /// that generated revenue"; a write-off is "stock that left with
    /// no revenue at all" — a different kind of loss, not a discount
    /// on the ones this module was built to measure. Kept separate so
    /// neither number silently absorbs the other: `profit_cents`
    /// still answers "what did selling things make," unchanged from
    /// before this field existed, and `profit_cents_after_shrinkage`
    /// below answers the fuller "what did the business actually keep"
    /// question this session's gap report asked for.
    pub shrinkage_cents: i64,
    pub profit_cents_after_shrinkage: i64,
    /// Same "undefined, not zero" rule as `margin_pct`, against the
    /// same revenue denominator — shrinkage has no revenue of its own
    /// to divide by.
    pub margin_pct_after_shrinkage: Option<f64>,
}

/// Sums confirmed write-off cost across every CLOSED stock take for
/// this business — a deliberate second, single-purpose query, not a
/// join bolted onto `summary()`'s own Sales-only query above: that one
/// stays exactly what its module doc comment already promises
/// ("just one query against one table"), and this is a completely
/// different table answering a completely different question.
/// Cancelled stock takes are excluded on purpose — see
/// stock_take.rs::cancel, nothing they touched ever reached
/// inventory.quantity, so there is no real cost to attribute to one.
fn shrinkage_cents(conn: &Connection, business_id: &str) -> Result<i64> {
    conn.query_row(
        "SELECT COALESCE(SUM(sti.write_off_cost_cents), 0)
         FROM stock_take_items sti
         JOIN stock_takes st ON st.id = sti.stock_take_id
         WHERE st.business_id = ?1 AND st.status = 'closed'",
        params![business_id],
        |r| r.get(0),
    ).map_err(Into::into)
}

/// All-time totals — same scope `DebtSummary`'s KPI-card numbers use,
/// not a date-range report (that's what a dedicated Profit report
/// screen is for; this is the single Dashboard card).
pub fn summary(conn: &Connection, business_id: &str, user_id: &str) -> Result<GrossProfitSummary> {
    crate::rbac::require(conn, user_id, "sales", "read")?;
    let sales_module = crate::crud::load_module(conn, business_id, "sales")
        .map_err(|_| anyhow!("the Sales module isn't enabled for this business"))?;
    let table = sales_module.table_name();

    let (revenue_cents, cost_cents, sales_count, cost_bearing_sales_count): (i64, i64, i64, i64) = conn.query_row(
        &format!(
            "SELECT COALESCE(SUM(revenue), 0), COALESCE(SUM(cost_at_sale), 0), COUNT(*),
                    COALESCE(SUM(CASE WHEN cost_at_sale > 0 THEN 1 ELSE 0 END), 0)
             FROM {table} WHERE business_id = ?1 AND deleted_at IS NULL"
        ),
        params![business_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;

    let profit_cents = revenue_cents - cost_cents;
    let margin_pct = if revenue_cents > 0 {
        Some(profit_cents as f64 / revenue_cents as f64 * 100.0)
    } else {
        None
    };

    let shrinkage_cents = shrinkage_cents(conn, business_id)?;
    let profit_cents_after_shrinkage = profit_cents - shrinkage_cents;
    let margin_pct_after_shrinkage = if revenue_cents > 0 {
        Some(profit_cents_after_shrinkage as f64 / revenue_cents as f64 * 100.0)
    } else {
        None
    };

    Ok(GrossProfitSummary {
        revenue_cents,
        cost_cents,
        profit_cents,
        margin_pct,
        sales_count,
        cost_bearing_sales_count,
        shrinkage_cents,
        profit_cents_after_shrinkage,
        margin_pct_after_shrinkage,
    })
}

#[derive(Debug, serde::Serialize)]
pub struct ItemProfit {
    pub item_name: String,
    pub revenue_cents: i64,
    pub cost_cents: i64,
    pub profit_cents: i64,
    /// Same "undefined, not zero" rule as GrossProfitSummary::margin_pct.
    pub margin_pct: Option<f64>,
    pub sales_count: i64,
    /// Same per-row meaning as GrossProfitSummary's own field, just
    /// scoped to this one item — an item's margin built on 1 of its 20
    /// sales having real cost data is exactly the kind of thing that
    /// should stay visible, not average away into a single blended
    /// business-wide number.
    pub cost_bearing_sales_count: i64,
    /// Same meaning and same CLOSED-only scope as
    /// GrossProfitSummary::shrinkage_cents, just for this one item's
    /// `item_name` — an item can be both a strong seller AND a
    /// frequent write-off (breakage, spoilage, theft), and blending
    /// those into one number would hide exactly the item an owner
    /// most needs to see both halves of.
    pub shrinkage_cents: i64,
    pub profit_cents_after_shrinkage: i64,
}

/// Same all-time, same-table computation as summary() above — just
/// GROUP BY item_name instead of collapsing straight to one row.
/// Deliberately not a new query shape: same columns, same
/// revenue-minus-cost arithmetic, same honest cost-coverage count,
/// just sliced one dimension further. Ordered by profit (not
/// revenue) descending — a high-revenue, thin-margin item and a
/// low-revenue, fat-margin item both matter to an owner deciding what
/// to push, and profit is the number that answers that question
/// directly rather than revenue alone.
pub fn by_item(conn: &Connection, business_id: &str, user_id: &str, limit: i64) -> Result<Vec<ItemProfit>> {
    crate::rbac::require(conn, user_id, "sales", "read")?;
    let sales_module = crate::crud::load_module(conn, business_id, "sales")
        .map_err(|_| anyhow!("the Sales module isn't enabled for this business"))?;
    let table = sales_module.table_name();
    let limit = limit.clamp(1, 100);

    let sql = format!(
        "SELECT item_name, COALESCE(SUM(revenue), 0), COALESCE(SUM(cost_at_sale), 0), COUNT(*),
                COALESCE(SUM(CASE WHEN cost_at_sale > 0 THEN 1 ELSE 0 END), 0)
         FROM {table}
         WHERE business_id = ?1 AND deleted_at IS NULL
         GROUP BY item_name
         ORDER BY (COALESCE(SUM(revenue), 0) - COALESCE(SUM(cost_at_sale), 0)) DESC
         LIMIT ?2"
    );

    // Same reasoning as shrinkage_cents() above, GROUP BY item_name
    // instead of collapsed to a single total. A HashMap, not a second
    // SQL join against the sales table above: stock_take_items'
    // item_name is a plain frozen-at-count-time string (see this
    // file's own struct — it's not a foreign key into Inventory), so
    // matching it to Sales' own item_name has to happen the same
    // string-equality way either query would do it — doing that in
    // Rust after one simple query, rather than in a cross-table SQL
    // join, keeps both queries exactly as single-purpose as
    // shrinkage_cents() itself already is.
    let mut shrinkage_by_item: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT sti.item_name, COALESCE(SUM(sti.write_off_cost_cents), 0)
             FROM stock_take_items sti
             JOIN stock_takes st ON st.id = sti.stock_take_id
             WHERE st.business_id = ?1 AND st.status = 'closed'
             GROUP BY sti.item_name",
        )?;
        let rows = stmt.query_map(params![business_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            let (item_name, cents) = row?;
            shrinkage_by_item.insert(item_name, cents);
        }
    }

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![business_id, limit], |r| {
        let item_name: String = r.get(0)?;
        let revenue_cents: i64 = r.get(1)?;
        let cost_cents: i64 = r.get(2)?;
        let profit_cents = revenue_cents - cost_cents;
        let margin_pct = if revenue_cents > 0 {
            Some(profit_cents as f64 / revenue_cents as f64 * 100.0)
        } else {
            None
        };
        let shrinkage_cents = shrinkage_by_item.get(&item_name).copied().unwrap_or(0);
        Ok(ItemProfit {
            profit_cents_after_shrinkage: profit_cents - shrinkage_cents,
            item_name,
            revenue_cents,
            cost_cents,
            profit_cents,
            margin_pct,
            sales_count: r.get(3)?,
            cost_bearing_sales_count: r.get(4)?,
            shrinkage_cents,
        })
    })?;

    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
}

#[derive(Debug, serde::Serialize)]
pub struct CategoryProfit {
    pub category: String,
    pub revenue_cents: i64,
    pub cost_cents: i64,
    pub profit_cents: i64,
    /// Same "undefined, not zero" rule as GrossProfitSummary::margin_pct.
    pub margin_pct: Option<f64>,
    pub sales_count: i64,
    /// Same per-row meaning as GrossProfitSummary's own field, just
    /// scoped to this one category.
    pub cost_bearing_sales_count: i64,
}

/// "Which line of the business is actually making money" — the same
/// revenue-minus-cost arithmetic as `by_item` above, just grouped one
/// level higher, by Inventory's own `category` field instead of by
/// item name.
///
/// Sales and Inventory are only ever linked by matching `item_name` to
/// `name` as plain text — there is no foreign key between the two
/// tables anywhere in this codebase (see stock_health.rs's own module
/// doc comment, which documents this exact convention and its one
/// limitation: a renamed item's older sales won't match under the new
/// name). This function relies on that same convention via a LEFT
/// JOIN, not a new one. A sale whose item_name matches no current
/// Inventory item — a Service Sale (no inventory link at all), a
/// discontinued or renamed item, or a category left blank — is
/// grouped honestly under "Uncategorized" rather than silently
/// dropped or guessed at.
///
/// Requires both Sales and Inventory enabled, same as `slow_movers` in
/// stock_health.rs requires both for the same reason: without
/// Inventory there is no `category` to group by at all.
pub fn by_category(conn: &Connection, business_id: &str, user_id: &str) -> Result<Vec<CategoryProfit>> {
    crate::rbac::require(conn, user_id, "sales", "read")?;
    let sales_module = crate::crud::load_module(conn, business_id, "sales")
        .map_err(|_| anyhow!("the Sales module isn't enabled for this business"))?;
    let inventory_module = crate::crud::load_module(conn, business_id, "inventory")
        .map_err(|_| anyhow!("the Inventory module isn't enabled for this business — category breakdown needs it"))?;
    let sales_table = sales_module.table_name();
    let inventory_table = inventory_module.table_name();

    let sql = format!(
        "SELECT COALESCE(NULLIF(TRIM(i.category), ''), 'Uncategorized') AS category,
                COALESCE(SUM(s.revenue), 0), COALESCE(SUM(s.cost_at_sale), 0), COUNT(*),
                COALESCE(SUM(CASE WHEN s.cost_at_sale > 0 THEN 1 ELSE 0 END), 0)
         FROM {sales_table} s
         LEFT JOIN {inventory_table} i
           ON i.business_id = s.business_id AND i.name = s.item_name AND i.deleted_at IS NULL
         WHERE s.business_id = ?1 AND s.deleted_at IS NULL
         GROUP BY category
         ORDER BY (COALESCE(SUM(s.revenue), 0) - COALESCE(SUM(s.cost_at_sale), 0)) DESC"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![business_id], |r| {
        let revenue_cents: i64 = r.get(1)?;
        let cost_cents: i64 = r.get(2)?;
        let profit_cents = revenue_cents - cost_cents;
        let margin_pct = if revenue_cents > 0 {
            Some(profit_cents as f64 / revenue_cents as f64 * 100.0)
        } else {
            None
        };
        Ok(CategoryProfit {
            category: r.get(0)?,
            revenue_cents,
            cost_cents,
            profit_cents,
            margin_pct,
            sales_count: r.get(3)?,
            cost_bearing_sales_count: r.get(4)?,
        })
    })?;

    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
}
