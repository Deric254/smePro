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
/// `range_start`/`range_end`, when given, filter to `s.created_at` —
/// the same range field report.rs::run defaults to for a category
/// dimension with no explicit time field, which is exactly the path
/// AnalyticsSection's sibling category breakdowns (revenue by
/// item_name, revenue by payment_method) already run through. Filtering
/// on that same column keeps this chart's numbers on the same
/// TimeSlicer-defined "this period" as everything else next to it,
/// instead of quietly using a different date field. `None` for both
/// keeps the previous all-time behavior — used by ai_context.rs, which
/// wants the standing lifetime picture, not whatever period a Dashboard
/// slicer happens to be on.
pub fn by_category(
    conn: &Connection,
    business_id: &str,
    user_id: &str,
    range_start: Option<&str>,
    range_end: Option<&str>,
) -> Result<Vec<CategoryProfit>> {
    crate::rbac::require(conn, user_id, "sales", "read")?;
    let sales_module = crate::crud::load_module(conn, business_id, "sales")
        .map_err(|_| anyhow!("the Sales module isn't enabled for this business"))?;
    let inventory_module = crate::crud::load_module(conn, business_id, "inventory")
        .map_err(|_| anyhow!("the Inventory module isn't enabled for this business — category breakdown needs it"))?;
    let sales_table = sales_module.table_name();
    let inventory_table = inventory_module.table_name();

    let mut where_clauses = vec!["s.business_id = ?1".to_string(), "s.deleted_at IS NULL".to_string()];
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(business_id.to_string())];
    if let Some(start) = range_start {
        params.push(Box::new(start.to_string()));
        where_clauses.push(format!("s.created_at >= ?{}", params.len()));
    }
    if let Some(end) = range_end {
        params.push(Box::new(end.to_string()));
        where_clauses.push(format!("s.created_at <= ?{}", params.len()));
    }

    let sql = format!(
        "SELECT COALESCE(NULLIF(TRIM(i.category), ''), 'Uncategorized') AS category,
                COALESCE(SUM(s.revenue), 0), COALESCE(SUM(s.cost_at_sale), 0), COUNT(*),
                COALESCE(SUM(CASE WHEN s.cost_at_sale > 0 THEN 1 ELSE 0 END), 0)
         FROM {sales_table} s
         LEFT JOIN {inventory_table} i
           ON i.business_id = s.business_id AND i.name = s.item_name AND i.deleted_at IS NULL
         WHERE {}
         GROUP BY category
         ORDER BY (COALESCE(SUM(s.revenue), 0) - COALESCE(SUM(s.cost_at_sale), 0)) DESC",
        where_clauses.join(" AND ")
    );

    let mut stmt = conn.prepare(&sql)?;
    let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
    let rows = stmt.query_map(params_refs.as_slice(), |r| {
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

#[derive(Debug, serde::Serialize)]
pub struct ItemMarginTrend {
    pub item_name: String,
    pub current_revenue_cents: i64,
    pub current_cost_cents: i64,
    pub current_profit_cents: i64,
    /// Same "undefined, not zero" rule as ItemProfit::margin_pct
    /// above, same condition (revenue > 0) — deliberately NOT also
    /// gated on cost_bearing_sales_count, for the same reason
    /// GrossProfitSummary's own margin_pct isn't: that gap is
    /// reported honestly through cost_bearing count instead of
    /// silently hiding the number. `is_losing_money` below is the one
    /// field that DOES require real cost data, for a different
    /// reason — see its own doc comment.
    pub current_margin_pct: Option<f64>,
    pub current_sales_count: i64,
    pub current_cost_bearing_sales_count: i64,
    pub previous_revenue_cents: i64,
    pub previous_cost_cents: i64,
    pub previous_profit_cents: i64,
    pub previous_margin_pct: Option<f64>,
    pub previous_sales_count: i64,
    pub previous_cost_bearing_sales_count: i64,
    /// current_margin_pct minus previous_margin_pct, in percentage
    /// points. None whenever either side is itself None — a computed
    /// "improved" or "declined" reading built on a period with no
    /// revenue at all would be worse than no reading.
    pub margin_pct_change_pts: Option<f64>,
    /// Unambiguous only: cost exceeded revenue THIS period, AND at
    /// least one of this period's sales actually carried real cost
    /// data to base that on. This is the literal "secretly losing
    /// money" case an item can hit while still showing healthy
    /// revenue — and unlike margin_pct above, this is deliberately
    /// held back rather than computed from a 0-cost/0-revenue
    /// placeholder, because "you are losing money on this" is a
    /// strong, actionable claim that must not be made from missing
    /// data dressed up as a real zero.
    pub is_losing_money: bool,
}

/// "Which SKUs are secretly losers" — the same revenue-minus-cost
/// arithmetic as `by_item` above, computed over two adjacent,
/// equal-length windows (the last `period_days` days, and the
/// `period_days` before that) instead of one all-time total, so an
/// item whose margin just turned negative — or is quietly sliding —
/// doesn't stay hidden inside a lifetime number that still looks
/// fine.
///
/// Windowed on Sales' own `sale_date` (a plain business date, backfilled
/// for older rows — see db_migrations.rs — and set directly by
/// pos::checkout/create_service_sale on every sale since), not
/// `created_at`, for the same reason `sale_date` exists at all: a
/// business date an owner would actually recognize as "when this was
/// sold," not an insert timestamp.
///
/// Only items with at least one sale in the CURRENT window are
/// returned — an item nobody sold recently has nothing to trend.
/// Ordered by current profit ascending: the biggest current losses
/// first, since that's the most actionable ordering for a report
/// titled "which SKUs are secretly losers."
pub fn by_item_trend(
    conn: &Connection,
    business_id: &str,
    user_id: &str,
    today: &str,
    period_days: i64,
    limit: i64,
) -> Result<Vec<ItemMarginTrend>> {
    crate::rbac::require(conn, user_id, "sales", "read")?;
    let sales_module = crate::crud::load_module(conn, business_id, "sales")
        .map_err(|_| anyhow!("the Sales module isn't enabled for this business"))?;
    let table = sales_module.table_name();

    // Same clamp-not-reject rule as slow_movers' stale_after_days —
    // 180 days is a generous outer bound for a period comparison
    // report without letting a huge value make the query scan the
    // entire sales history twice for no real benefit.
    let period_days = period_days.clamp(1, 180);
    let limit = limit.clamp(1, 100);

    let sql = format!(
        "WITH bounds AS (
             SELECT date(?2) AS current_end,
                    date(?2, '-' || ?3 || ' days') AS current_start,
                    date(?2, '-' || (?3 * 2) || ' days') AS previous_start
         )
         SELECT item_name,
                COALESCE(SUM(CASE WHEN sale_date > bounds.current_start AND sale_date <= bounds.current_end THEN revenue ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN sale_date > bounds.current_start AND sale_date <= bounds.current_end THEN cost_at_sale ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN sale_date > bounds.current_start AND sale_date <= bounds.current_end THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN sale_date > bounds.current_start AND sale_date <= bounds.current_end AND cost_at_sale > 0 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN sale_date > bounds.previous_start AND sale_date <= bounds.current_start THEN revenue ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN sale_date > bounds.previous_start AND sale_date <= bounds.current_start THEN cost_at_sale ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN sale_date > bounds.previous_start AND sale_date <= bounds.current_start THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN sale_date > bounds.previous_start AND sale_date <= bounds.current_start AND cost_at_sale > 0 THEN 1 ELSE 0 END), 0)
         FROM {table}, bounds
         WHERE business_id = ?1 AND deleted_at IS NULL
         GROUP BY item_name
         HAVING SUM(CASE WHEN sale_date > bounds.current_start AND sale_date <= bounds.current_end THEN 1 ELSE 0 END) > 0
         ORDER BY (COALESCE(SUM(CASE WHEN sale_date > bounds.current_start AND sale_date <= bounds.current_end THEN revenue - cost_at_sale ELSE 0 END), 0)) ASC
         LIMIT ?4"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![business_id, today, period_days, limit], |r| {
        let item_name: String = r.get(0)?;
        let current_revenue_cents: i64 = r.get(1)?;
        let current_cost_cents: i64 = r.get(2)?;
        let current_sales_count: i64 = r.get(3)?;
        let current_cost_bearing_sales_count: i64 = r.get(4)?;
        let previous_revenue_cents: i64 = r.get(5)?;
        let previous_cost_cents: i64 = r.get(6)?;
        let previous_sales_count: i64 = r.get(7)?;
        let previous_cost_bearing_sales_count: i64 = r.get(8)?;

        let current_profit_cents = current_revenue_cents - current_cost_cents;
        let current_margin_pct = if current_revenue_cents > 0 {
            Some(current_profit_cents as f64 / current_revenue_cents as f64 * 100.0)
        } else {
            None
        };
        let previous_profit_cents = previous_revenue_cents - previous_cost_cents;
        let previous_margin_pct = if previous_revenue_cents > 0 {
            Some(previous_profit_cents as f64 / previous_revenue_cents as f64 * 100.0)
        } else {
            None
        };
        let margin_pct_change_pts = match (current_margin_pct, previous_margin_pct) {
            (Some(c), Some(p)) => Some(c - p),
            _ => None,
        };

        Ok(ItemMarginTrend {
            item_name,
            current_revenue_cents,
            current_cost_cents,
            current_profit_cents,
            current_margin_pct,
            current_sales_count,
            current_cost_bearing_sales_count,
            previous_revenue_cents,
            previous_cost_cents,
            previous_profit_cents,
            previous_margin_pct,
            previous_sales_count,
            previous_cost_bearing_sales_count,
            margin_pct_change_pts,
            is_losing_money: current_cost_bearing_sales_count > 0 && current_profit_cents < 0,
        })
    })?;

    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
}
