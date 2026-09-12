//! One-line Dashboard teasers for the fuller reports in Reports.tsx —
//! "here's something worth a look," not the full picture. Deliberately
//! a separate function from business_pulse.rs rather than added
//! fields on its struct: that struct and its tests already exist and
//! work; bolting more onto it risks it for no reason when a second,
//! small function does the job with zero risk to the first.
//!
//! Never errors outward — each field independently degrades to `None`
//! if its underlying computation fails or has nothing worth
//! surfacing, the same "a highlight bar should never be the reason
//! the Dashboard doesn't load" discipline business_pulse.rs already
//! follows for the exact same reason.

use rusqlite::Connection;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct UrgentItem {
    pub item_name: String,
    pub days_of_stock_left: f64,
}

#[derive(Debug, Serialize)]
pub struct BusiestDay {
    pub day_name: String,
    pub avg_revenue_cents: i64,
}

#[derive(Debug, Serialize)]
pub struct ReportHighlights {
    /// Only surfaced when genuinely soon (≤14 days) — an item with
    /// months of runway isn't a "highlight," it's just the smallest
    /// number in a list, and showing it here would train the reader
    /// to ignore this bar.
    pub most_urgent_item: Option<UrgentItem>,
    /// Only surfaced with at least 3 real occurrences of the winning
    /// weekday behind it — see sales_patterns.rs's own note on why a
    /// day average resting on 1–2 occurrences isn't a real pattern
    /// yet, just noise that happens to be the current maximum.
    pub busiest_day: Option<BusiestDay>,
}

pub fn compute(conn: &Connection, business_id: &str, user_id: &str, today: &str, offset_minutes: i64) -> ReportHighlights {
    let most_urgent_item = crate::stock_health::stock_runway(conn, business_id, user_id, today, 30, 1)
        .ok()
        .and_then(|v| v.into_iter().next())
        .and_then(|r| r.days_of_stock_left.map(|d| (r.item_name, d)))
        .filter(|(_, d)| *d <= 14.0)
        .map(|(item_name, days_of_stock_left)| UrgentItem { item_name, days_of_stock_left });

    let busiest_day = crate::sales_patterns::day_of_week_pattern(conn, business_id, user_id, today, 90, offset_minutes)
        .ok()
        .and_then(|days| {
            days.into_iter()
                .filter(|d| d.occurrences >= 3)
                .max_by_key(|d| d.avg_revenue_cents)
        })
        .filter(|d| d.avg_revenue_cents > 0)
        .map(|d| BusiestDay { day_name: d.day_name, avg_revenue_cents: d.avg_revenue_cents });

    ReportHighlights { most_urgent_item, busiest_day }
}
