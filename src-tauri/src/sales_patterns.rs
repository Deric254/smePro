//! Which day of the week sells the most — a staffing/restocking-timing
//! question, not a forecast. "Busiest day" and "slowest day" are
//! useful even for a business with only a few weeks of history; true
//! year-over-year seasonality needs much more data than that (see
//! this module's own `is_seasonal_estimate` note below).

use crate::crud;
use anyhow::{anyhow, Result};
use chrono::{Datelike, NaiveDate};
use rusqlite::{params, Connection};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct DayOfWeekPattern {
    pub day_name: String,
    /// Average revenue on this weekday — total revenue on this weekday
    /// within the window, divided by how many times that weekday
    /// actually occurred in the window (a real calendar count, not
    /// "however many of that weekday happened to have a sale" — a
    /// weekday the business was closed every time still counts as an
    /// occurrence, so it correctly pulls the average down instead of
    /// being silently excluded and inflating it).
    pub avg_revenue_cents: i64,
    /// Same average-over-real-occurrences treatment for order count.
    pub avg_order_count: f64,
    /// How many times this weekday occurred in the window — shown so
    /// a business with, say, 2 weeks of history can see for itself
    /// that "average Tuesday" here rests on only 2 real Tuesdays, not
    /// a false-confidence single number.
    pub occurrences: i64,
}

/// `lookback_days` clamped to [7, 365] — under a week can't cover
/// every weekday even once; over a year starts pulling in demand from
/// far enough back it may not reflect the business as it runs today.
pub fn day_of_week_pattern(
    conn: &Connection,
    business_id: &str,
    user_id: &str,
    today: &str,
    lookback_days: i64,
) -> Result<Vec<DayOfWeekPattern>> {
    crate::rbac::require(conn, user_id, "sales", "read")?;
    let sales_module = crud::load_module(conn, business_id, "sales")
        .map_err(|_| anyhow!("the Sales module isn't enabled for this business"))?;
    let table = sales_module.table_name();

    let lookback_days = lookback_days.clamp(7, 365);
    let today_date = NaiveDate::parse_from_str(today, "%Y-%m-%d")
        .map_err(|_| anyhow!("invalid date"))?;

    // Real calendar occurrences of each weekday in the window — the
    // part a pure SQL GROUP BY over sales rows can't give honestly
    // (see this struct's own doc comment on `occurrences` above).
    // chrono's Weekday::num_days_from_sunday() gives 0=Sunday..6=Saturday,
    // matching SQLite's own strftime('%w', ...) convention exactly, so
    // the two sides join up by the same plain integer with no
    // reindexing step to get subtly wrong.
    let mut occurrences = [0i64; 7];
    for i in 0..lookback_days {
        let d = today_date - chrono::Duration::days(i);
        occurrences[d.weekday().num_days_from_sunday() as usize] += 1;
    }

    let sql = format!(
        "SELECT CAST(strftime('%w', created_at) AS INTEGER) AS dow,
                COALESCE(SUM(revenue), 0), COUNT(DISTINCT order_id)
         FROM {table}
         WHERE business_id = ?1 AND deleted_at IS NULL
           AND created_at >= date(?2, '-' || ?3 || ' days')
         GROUP BY dow"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![business_id, today, lookback_days], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
    })?;

    let mut total_revenue = [0i64; 7];
    let mut total_orders = [0i64; 7];
    for row in rows {
        let (dow, revenue, orders) = row?;
        if (0..7).contains(&dow) {
            total_revenue[dow as usize] = revenue;
            total_orders[dow as usize] = orders;
        }
    }

    const DAY_NAMES: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    Ok((0..7)
        .map(|i| DayOfWeekPattern {
            day_name: DAY_NAMES[i].to_string(),
            avg_revenue_cents: if occurrences[i] > 0 {
                (total_revenue[i] as f64 / occurrences[i] as f64).round() as i64
            } else {
                0
            },
            avg_order_count: if occurrences[i] > 0 { total_orders[i] as f64 / occurrences[i] as f64 } else { 0.0 },
            occurrences: occurrences[i],
        })
        .collect())
}
