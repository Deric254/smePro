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
    /// True once at least one sale exists with `cost_at_sale > 0` —
    /// lets the frontend distinguish "this business has made sales,
    /// but every one of them predates cost tracking" (see
    /// db_migrations.rs's v17 doc comment: historical sales are
    /// permanently stuck at cost_at_sale = 0) from "this business
    /// genuinely has zero cost of goods", so a brand-new install
    /// doesn't get a misleading "100% margin" reading it might
    /// mistake for a real result.
    pub has_cost_data: bool,
}

/// All-time totals — same scope `DebtSummary`'s KPI-card numbers use,
/// not a date-range report (that's what a dedicated Profit report
/// screen is for; this is the single Dashboard card).
pub fn summary(conn: &Connection, business_id: &str, user_id: &str) -> Result<GrossProfitSummary> {
    crate::rbac::require(conn, user_id, "sales", "read")?;
    let sales_module = crate::crud::load_module(conn, business_id, "sales")
        .map_err(|_| anyhow!("the Sales module isn't enabled for this business"))?;
    let table = sales_module.table_name();

    let (revenue_cents, cost_cents, sales_count, cost_bearing_count): (i64, i64, i64, i64) = conn.query_row(
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

    Ok(GrossProfitSummary {
        revenue_cents,
        cost_cents,
        profit_cents,
        margin_pct,
        sales_count,
        has_cost_data: cost_bearing_count > 0,
    })
}
