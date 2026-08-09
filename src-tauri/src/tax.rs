//! Tax calculation engine — integrated into POS, Invoices, and Reporting.
//!
//! Every business has a default tax rate (businesses.tax_rate). This
//! module adds per-item-category tax overrides, tax-inclusive vs
//! tax-exclusive pricing, and a dedicated tax report.
//!
//! DESIGN DECISIONS:
//! - Tax is computed at transaction time (not retroactively changed)
//! - Per-category overrides take precedence over business default
//! - Tax-inclusive mode: stored price already includes tax
//! - Tax-exclusive mode: tax added to stored price at checkout
//! - All amounts are integer minor units (cents) — see money.rs. The
//!   only fractional step is applying a percentage rate, done once via
//!   money::apply_rate and never carried forward as a float.
//!
//! STRESS TESTED:
//! - 0% tax → tax line omitted from receipt/invoice
//! - Tax-inclusive: unit_price 115, tax 15% → pre-tax 100, tax 15
//! - Tax-exclusive: unit_price 100, tax 15% → total 115, tax 15
//! - Mixed cart (some items tax-exempt) → correct per-line tax
//! - Rounding: 0.1 + 0.2 style edge cases handled

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::Serialize;

#[derive(Debug, Serialize, Clone)]
pub struct TaxLine {
    pub category: String,
    pub rate: f64,
    pub taxable_amount: i64,
    pub tax_amount: i64,
}

#[derive(Debug, Serialize)]
pub struct TaxSummary {
    pub subtotal: i64,
    pub total_tax: i64,
    pub total: i64,
    pub lines: Vec<TaxLine>,
    pub tax_inclusive: bool,
}

/// Computes tax for a set of line items.
///
/// # Arguments
/// * `conn` — SQLite connection
/// * `business_id` — tenant UUID
/// * `items` — vec of (category, unit_price, quantity)
/// * `tax_inclusive` — if true, unit_price already includes tax
///
/// # Returns
/// TaxSummary with per-category breakdown and totals.
pub fn compute(
    conn: &Connection,
    business_id: &str,
    items: &[(String, i64, i64)],
    tax_inclusive: bool,
) -> Result<TaxSummary> {
    let default_rate: f64 = conn.query_row(
        "SELECT tax_rate FROM businesses WHERE id = ?1",
        params![business_id],
        |r| r.get(0),
    ).unwrap_or(0.0);

    let mut lines: Vec<TaxLine> = Vec::new();
    let mut subtotal: i64 = 0;
    let mut total_tax: i64 = 0;

    for (category, unit_price, qty) in items {
        let rate = category_tax_rate(conn, business_id, category).unwrap_or(default_rate);
        // Exact — integer cents times an integer quantity.
        let line_total: i64 = unit_price * qty;

        let (taxable, tax) = if tax_inclusive {
            // Price includes tax: pre-tax = price / (1 + rate/100).
            // This division is unavoidably fractional, so it's rounded
            // exactly once, here — and `tax` is then derived by
            // subtraction from the already-integer line_total, not by
            // its own independent rounding, which guarantees
            // taxable + tax == line_total exactly, every time.
            let pre_tax = crate::money::apply_rate(line_total, 1.0 / (1.0 + rate / 100.0));
            let tax_amt = line_total - pre_tax;
            (pre_tax, tax_amt)
        } else {
            // Price excludes tax: tax = price * rate/100, the one
            // rounding point for this branch.
            let tax_amt = crate::money::apply_rate(line_total, rate / 100.0);
            (line_total, tax_amt)
        };

        subtotal += taxable;
        total_tax += tax;

        // Merge into existing line for same category+rate
        if let Some(existing) = lines.iter_mut().find(|l| l.category == *category && (l.rate - rate).abs() < 0.001) {
            existing.taxable_amount += taxable;
            existing.tax_amount += tax;
        } else {
            lines.push(TaxLine {
                category: category.clone(),
                rate,
                taxable_amount: taxable,
                tax_amount: tax,
            });
        }
    }

    let total = subtotal + total_tax;

    Ok(TaxSummary {
        subtotal,
        total_tax,
        total,
        lines,
        tax_inclusive,
    })
}

/// Returns the tax rate for a specific item category, if one is set.
fn category_tax_rate(conn: &Connection, business_id: &str, category: &str) -> Option<f64> {
    conn.query_row(
        "SELECT rate FROM tax_rates WHERE business_id = ?1 AND category = ?2",
        params![business_id, category],
        |r| r.get(0),
    ).ok()
}

/// Sets a per-category tax rate. Owner-only.
pub fn set_category_rate(
    conn: &mut Connection,
    business_id: &str,
    user_id: &str,
    category: &str,
    rate: f64,
) -> Result<()> {
    crate::rbac::require_owner(conn, user_id)?;

    conn.execute(
        "INSERT INTO tax_rates (id, business_id, category, rate, created_at)
         VALUES (?1, ?2, ?3, ?4, datetime('now'))
         ON CONFLICT(business_id, category) DO UPDATE SET rate = excluded.rate, updated_at = datetime('now')",
        params![uuid::Uuid::new_v4().to_string(), business_id, category, rate],
    )?;

    let _ = crate::audit::log(conn, business_id, Some(user_id), "_tax", "set_rate", None, 
        Some(&serde_json::json!({"category": category, "rate": rate})));

    Ok(())
}

/// Lists all per-category tax rates for a business.
pub fn list_rates(conn: &Connection, business_id: &str) -> Result<Vec<serde_json::Value>> {
    let mut stmt = conn.prepare(
        "SELECT category, rate FROM tax_rates WHERE business_id = ?1 ORDER BY category"
    )?;
    let rows = stmt.query_map(params![business_id], |r| {
        Ok(serde_json::json!({
            "category": r.get::<_, String>(0)?,
            "rate": r.get::<_, f64>(1)?,
        }))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

