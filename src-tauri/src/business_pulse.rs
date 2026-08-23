//! "Business pulse" — a real, computed performance readout appended to
//! every AI chat response, not something the AI model is asked to
//! guess or narrate from memory. Every number here comes from
//! forecast.rs's existing auditable arithmetic (see its own doc
//! comment on why forecasting is done in plain code, not by an LLM)
//! and the business's real sales history — the same principle applied
//! one level further: if accuracy matters for a single forecast
//! number, it matters just as much for "how is my business doing"
//! appearing after every single chat message, including ones as
//! simple as "good morning."
//!
//! Degrades honestly rather than fabricating: a business with no sales
//! module enabled, or no sales yet, gets `has_data: false` and a plain
//! "not enough history yet" message — never an invented trend.

use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;

use crate::forecast;

#[derive(Debug, Serialize, Clone)]
pub struct BusinessPulse {
    pub has_data: bool,
    /// Integer cents, same unit as everywhere else in this app (see
    /// money.rs) — the frontend formats these with the business's own
    /// currency via formatMoney, the same as every other money value
    /// already on screen, rather than this Rust code trying to render
    /// a currency string itself.
    pub revenue_this_period_cents: i64,
    pub revenue_last_period_cents: i64,
    /// None specifically when last period was zero — a percentage
    /// change against zero is undefined, not "infinite growth," and
    /// reporting it as such would be a fabricated number wearing a
    /// real one's clothes.
    pub pct_change: Option<f64>,
    pub forecast_next_period_cents: i64,
    pub low_stock_count: i64,
    pub recommendations: Vec<String>,
    /// So the frontend can call formatMoney(cents, currency) directly
    /// without needing the "what can AI see" panel to have been opened
    /// first (that's a separate, on-demand fetch — this pulse has to
    /// render correctly on its own, every single time, standalone).
    pub currency: String,
}

impl BusinessPulse {
    fn no_data(currency: String) -> Self {
        Self {
            has_data: false,
            revenue_this_period_cents: 0,
            revenue_last_period_cents: 0,
            pct_change: None,
            forecast_next_period_cents: 0,
            low_stock_count: 0,
            recommendations: vec!["Not enough sales history yet to show a trend — check back after a few more sales.".to_string()],
            currency,
        }
    }
}

/// Computes the pulse. Never returns an `Err` to its caller in
/// practice — every internal failure (sales module not enabled, no
/// history, a query error) degrades to `BusinessPulse::no_data(currency)`
/// rather than breaking the chat response it's attached to. A missing
/// performance summary is a minor omission; failing someone's actual
/// question because the OPTIONAL summary underneath it hit an error
/// would not be.
pub fn compute(conn: &Connection, business_id: &str, user_id: &str) -> BusinessPulse {
    let currency: String = conn
        .query_row("SELECT currency FROM businesses WHERE id = ?1", rusqlite::params![business_id], |r| r.get(0))
        .unwrap_or_else(|_| "USD".to_string());

    let forecast_result = match forecast::exponential_smoothing_forecast(
        conn, business_id, user_id, "sales", "revenue", "month", 0.5,
    ) {
        Ok(r) => r,
        Err(_) => return BusinessPulse::no_data(currency),
    };

    if forecast_result.history.len() < 2 {
        // Zero or one month of history — a trend needs at least two
        // points to compare, and reporting "flat" or "up" off a
        // single data point would be a guess dressed as an insight.
        return BusinessPulse::no_data(currency);
    }

    let this_period = forecast_result.history[forecast_result.history.len() - 1].value;
    let last_period = forecast_result.history[forecast_result.history.len() - 2].value;

    let pct_change = if last_period.abs() > 0.01 {
        Some(((this_period - last_period) / last_period.abs()) * 100.0)
    } else {
        None
    };

    let low_stock_count = count_low_stock(conn, business_id, user_id);

    let mut recommendations = Vec::new();
    match pct_change {
        Some(p) if p >= 5.0 => recommendations.push(format!(
            "Revenue is up {p:.0}% from last month — worth noting what changed and doing more of it."
        )),
        Some(p) if p <= -5.0 => recommendations.push(format!(
            "Revenue is down {:.0}% from last month — worth a look at what changed.",
            p.abs()
        )),
        Some(_) => recommendations.push("Revenue is holding roughly steady month over month.".to_string()),
        None => {}
    }
    if low_stock_count > 0 {
        recommendations.push(format!(
            "{low_stock_count} item{} low on stock — restocking {} could prevent missed sales.",
            if low_stock_count == 1 { " is" } else { "s are" },
            if low_stock_count == 1 { "it" } else { "them" },
        ));
    }
    if recommendations.is_empty() {
        recommendations.push("Steady, no urgent flags right now.".to_string());
    }

    BusinessPulse {
        has_data: true,
        revenue_this_period_cents: this_period.round() as i64,
        revenue_last_period_cents: last_period.round() as i64,
        pct_change,
        forecast_next_period_cents: forecast_result.forecast_next.round() as i64,
        low_stock_count,
        recommendations,
        currency,
    }
}

/// Total low-stock count across every module that has both `quantity`
/// and `reorder_level` fields — reuses ai_context's own snapshot
/// (already-audited logic for exactly this) rather than duplicating
/// the low-stock query here a second time. Falls back to 0 on any
/// error, consistent with `compute`'s own "never break the chat
/// response" rule above.
fn count_low_stock(conn: &Connection, business_id: &str, user_id: &str) -> i64 {
    let snapshot = match crate::ai_context::build_snapshot(conn, business_id, user_id) {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let modules = match snapshot.get("modules").and_then(Value::as_object) {
        Some(m) => m,
        None => return 0,
    };
    modules
        .values()
        .filter_map(|m| m.get("low_stock_alerts").and_then(Value::as_array))
        .map(|arr| arr.len() as i64)
        .sum()
}
