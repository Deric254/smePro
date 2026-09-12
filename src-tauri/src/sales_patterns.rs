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
    offset_minutes: i64,
) -> Result<Vec<DayOfWeekPattern>> {
    crate::rbac::require(conn, user_id, "sales", "read")?;
    let sales_module = crud::load_module(conn, business_id, "sales")
        .map_err(|_| anyhow!("the Sales module isn't enabled for this business"))?;
    let table = sales_module.table_name();

    let lookback_days = lookback_days.clamp(7, 365);
    // Same [-720, 840]-minute clamp and same "shift the bucketing, not
    // the coarse window edge" reasoning as hour_of_day_pattern's own
    // doc comment — a weekday is exactly the kind of boundary that
    // silently mislabels a late-evening or just-after-midnight local
    // sale if bucketed on raw UTC instead.
    let offset_minutes = offset_minutes.clamp(-720, 840);
    let offset_modifier = format!("{offset_minutes:+} minutes");
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
        "SELECT CAST(strftime('%w', created_at, ?4) AS INTEGER) AS dow,
                COALESCE(SUM(revenue), 0), COUNT(DISTINCT order_id)
         FROM {table}
         WHERE business_id = ?1 AND deleted_at IS NULL
           AND created_at >= date(?2, '-' || ?3 || ' days')
         GROUP BY dow"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![business_id, today, lookback_days, offset_modifier], |r| {
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

/// One bucket of a weekly or monthly trend — deliberately the same
/// shape for both, since the honesty concerns are identical: real
/// totals, a real zero for a bucket with no sales (never just absent
/// from the list, which would look like missing data rather than a
/// quiet period), and whether the bucket has actually finished yet.
#[derive(Debug, Serialize)]
pub struct PeriodTrendPoint {
    /// Week: the Monday it starts, "YYYY-MM-DD". Month: "YYYY-MM".
    pub label: String,
    pub revenue_cents: i64,
    pub order_count: i64,
    /// False for the bucket `today` currently falls in — a week or
    /// month still in progress isn't comparable to a full one next to
    /// it (a Wednesday-only "this week" will always look slower than
    /// a full past week, not because business is actually down).
    /// Every OTHER bucket in the returned list is always true: this
    /// only ever applies to the single most recent point.
    pub is_complete: bool,
}

/// `weeks` clamped to [2, 104] — one week alone isn't a trend, and two
/// years is generous enough for a real trend view while still keeping
/// the query window bounded. Weeks start Monday, matching
/// report.rs's own `TimeBucket::Week` convention exactly, so a week
/// label here means the same calendar week a Reports-page chart would
/// show for it.
pub fn weekly_trend(
    conn: &Connection,
    business_id: &str,
    user_id: &str,
    today: &str,
    weeks: i64,
    offset_minutes: i64,
) -> Result<Vec<PeriodTrendPoint>> {
    crate::rbac::require(conn, user_id, "sales", "read")?;
    let sales_module = crud::load_module(conn, business_id, "sales")
        .map_err(|_| anyhow!("the Sales module isn't enabled for this business"))?;
    let table = sales_module.table_name();
    let weeks = weeks.clamp(2, 104);
    // Same reasoning as day_of_week_pattern's own offset handling —
    // which calendar week a sale belongs to is exactly the kind of
    // boundary a raw-UTC bucketing silently gets wrong near midnight.
    let offset_minutes = offset_minutes.clamp(-720, 840);
    let offset_modifier = format!("{offset_minutes:+} minutes");

    let today_date = NaiveDate::parse_from_str(today, "%Y-%m-%d").map_err(|_| anyhow!("invalid date"))?;
    // Same Monday-start convention as report.rs's TimeBucket::Week —
    // 'weekday 1' in SQLite's date() means "the next Monday on or
    // before this date", i.e. the start of this ISO week.
    let this_week_start = today_date - chrono::Duration::days((today_date.weekday().num_days_from_monday()) as i64);

    // Every week label that SHOULD appear in the window, oldest
    // first — same reasoning as day_of_week_pattern's `occurrences`
    // array: a week with zero sales must still show up as a real
    // zero, not silently vanish from a SQL GROUP BY that only ever
    // sees weeks with at least one row.
    let labels: Vec<NaiveDate> = (0..weeks).rev().map(|i| this_week_start - chrono::Duration::weeks(i)).collect();
    let earliest = labels[0];

    let sql = format!(
        "SELECT date(created_at, ?3, '-6 days', 'weekday 1') AS week_start,
                COALESCE(SUM(revenue), 0), COUNT(DISTINCT order_id)
         FROM {table}
         WHERE business_id = ?1 AND deleted_at IS NULL AND created_at >= ?2
         GROUP BY week_start"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![business_id, earliest.to_string(), offset_modifier], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
    })?;
    let mut by_label: std::collections::HashMap<String, (i64, i64)> = std::collections::HashMap::new();
    for row in rows {
        let (label, revenue, orders) = row?;
        by_label.insert(label, (revenue, orders));
    }

    Ok(labels
        .into_iter()
        .map(|week_start| {
            let label = week_start.to_string();
            let (revenue_cents, order_count) = by_label.get(&label).copied().unwrap_or((0, 0));
            PeriodTrendPoint { label, revenue_cents, order_count, is_complete: week_start < this_week_start }
        })
        .collect())
}

/// `months` clamped to [2, 36] — same reasoning as `weekly_trend`
/// above, just a longer natural ceiling since a month is a coarser
/// bucket (3 years of monthly points is still a manageable chart).
pub fn monthly_trend(
    conn: &Connection,
    business_id: &str,
    user_id: &str,
    today: &str,
    months: i64,
    offset_minutes: i64,
) -> Result<Vec<PeriodTrendPoint>> {
    crate::rbac::require(conn, user_id, "sales", "read")?;
    let sales_module = crud::load_module(conn, business_id, "sales")
        .map_err(|_| anyhow!("the Sales module isn't enabled for this business"))?;
    let table = sales_module.table_name();
    let months = months.clamp(2, 36);
    // Same reasoning as weekly_trend's own offset handling.
    let offset_minutes = offset_minutes.clamp(-720, 840);
    let offset_modifier = format!("{offset_minutes:+} minutes");

    let today_date = NaiveDate::parse_from_str(today, "%Y-%m-%d").map_err(|_| anyhow!("invalid date"))?;
    let this_month_label = format!("{:04}-{:02}", today_date.year(), today_date.month());

    // Same "every label must exist, even at zero" reasoning as
    // weekly_trend above — walk back `months` calendar months from
    // today rather than trusting SQL's GROUP BY to surface a month
    // with no sales at all.
    let labels: Vec<String> = (0..months)
        .rev()
        .map(|i| {
            let total_months = (today_date.year() as i64) * 12 + (today_date.month() as i64 - 1) - i;
            let year = total_months.div_euclid(12);
            let month = total_months.rem_euclid(12) + 1;
            format!("{year:04}-{month:02}")
        })
        .collect();
    let earliest = format!("{}-01", labels[0]);

    let sql = format!(
        "SELECT strftime('%Y-%m', created_at, ?3) AS month,
                COALESCE(SUM(revenue), 0), COUNT(DISTINCT order_id)
         FROM {table}
         WHERE business_id = ?1 AND deleted_at IS NULL AND created_at >= ?2
         GROUP BY month"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![business_id, earliest, offset_modifier], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
    })?;
    let mut by_label: std::collections::HashMap<String, (i64, i64)> = std::collections::HashMap::new();
    for row in rows {
        let (label, revenue, orders) = row?;
        by_label.insert(label, (revenue, orders));
    }

    Ok(labels
        .into_iter()
        .map(|label| {
            let (revenue_cents, order_count) = by_label.get(&label).copied().unwrap_or((0, 0));
            let is_complete = label != this_month_label;
            PeriodTrendPoint { label, revenue_cents, order_count, is_complete }
        })
        .collect())
}

/// One hour of the day (0–23, in the caller's real local time — see
/// `hour_of_day_pattern`'s own doc comment on why that conversion is
/// mandatory here specifically), grouped into the coarser named
/// period a dashboard would actually lead with.
#[derive(Debug, Serialize)]
pub struct HourOfDayPattern {
    pub hour: u32,
    /// e.g. "2 PM", "12 AM" — for the specific-hour drill-down view.
    pub hour_label: String,
    /// "Morning" (5–11), "Afternoon" (12–16), "Evening" (17–20), or
    /// "Night" (21–23 and 0–4, i.e. late evening through midnight
    /// into early morning) — the top-level bucket a caller groups by
    /// before drilling into `hour_label`.
    pub period: String,
    pub avg_revenue_cents: i64,
    pub avg_order_count: f64,
    /// Same meaning as `DayOfWeekPattern::occurrences`, but every hour
    /// of the day occurs exactly once per calendar day regardless of
    /// which day it is — so unlike the per-weekday array this is a
    /// single number, equal to `lookback_days`, shared by all 24 rows.
    pub occurrences: i64,
}

fn hour_label(h: u32) -> String {
    let display = if h == 0 { 12 } else if h > 12 { h - 12 } else { h };
    let suffix = if h < 12 { "AM" } else { "PM" };
    format!("{display} {suffix}")
}

fn period_for_hour(h: u32) -> &'static str {
    match h {
        5..=11 => "Morning",
        12..=16 => "Afternoon",
        17..=20 => "Evening",
        _ => "Night",
    }
}

/// `lookback_days` clamped to [7, 365], same reasoning as
/// `day_of_week_pattern`. `offset_minutes` is the caller's real local
/// UTC offset (e.g. +180 for Kenya/EAT) — NOT skippable here the way
/// a coarser day-level view might get away with it. Every `created_at`
/// in this database is SQLite's `datetime('now')`, always UTC (see
/// any of the many call sites across src-tauri/src/*.rs — none use
/// the 'localtime' modifier, the same fact src/lib/date.ts's own doc
/// comment already documents, and the exact bug TimeSlicer.tsx was
/// fixed for on the date-range side). Bucketing by the raw UTC hour
/// instead of the real local one would silently mislabel every sale
/// near a business's midnight — precisely the boundary this feature
/// exists to distinguish (a 12:30am local sale is a very different
/// signal from an 11:30pm one, but in UTC+3 those two both land
/// within the same UTC evening block if uncorrected). Clamped to
/// [-720, 840] minutes (UTC-12:00 .. UTC+14:00, the real range of
/// offsets in use anywhere) to reject nonsense before it becomes an
/// SQLite modifier string.
pub fn hour_of_day_pattern(
    conn: &Connection,
    business_id: &str,
    user_id: &str,
    today: &str,
    lookback_days: i64,
    offset_minutes: i64,
) -> Result<Vec<HourOfDayPattern>> {
    crate::rbac::require(conn, user_id, "sales", "read")?;
    let sales_module = crud::load_module(conn, business_id, "sales")
        .map_err(|_| anyhow!("the Sales module isn't enabled for this business"))?;
    let table = sales_module.table_name();

    let lookback_days = lookback_days.clamp(7, 365);
    let offset_minutes = offset_minutes.clamp(-720, 840);
    // Bound as a parameter, not spliced into the SQL text — SQLite's
    // date/time functions accept a modifier passed as a normal bound
    // value the same as a literal, so this needs no more trust than
    // any other params![] value already used throughout this file.
    let offset_modifier = format!("{offset_minutes:+} minutes");

    let sql = format!(
        "SELECT CAST(strftime('%H', created_at, ?4) AS INTEGER) AS hour,
                COALESCE(SUM(revenue), 0), COUNT(DISTINCT order_id)
         FROM {table}
         WHERE business_id = ?1 AND deleted_at IS NULL
           AND created_at >= date(?2, '-' || ?3 || ' days')
         GROUP BY hour"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![business_id, today, lookback_days, offset_modifier], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
    })?;

    let mut total_revenue = [0i64; 24];
    let mut total_orders = [0i64; 24];
    for row in rows {
        let (hour, revenue, orders) = row?;
        if (0..24).contains(&hour) {
            total_revenue[hour as usize] = revenue;
            total_orders[hour as usize] = orders;
        }
    }

    Ok((0..24)
        .map(|h| HourOfDayPattern {
            hour: h as u32,
            hour_label: hour_label(h as u32),
            period: period_for_hour(h as u32).to_string(),
            avg_revenue_cents: (total_revenue[h] as f64 / lookback_days as f64).round() as i64,
            avg_order_count: total_orders[h] as f64 / lookback_days as f64,
            occurrences: lookback_days,
        })
        .collect())
}

/// One calendar month's real, cross-year seasonal signal — "does
/// December actually run hot for this business, or does it just look
/// that way from one good December?" `years_seen` is the whole point
/// of this struct: it's the honest answer to that question, shown
/// alongside the average rather than hidden behind it. A caller
/// deciding whether to trust "December runs 40% above average" should
/// look here first — 1 means don't, regardless of how large the
/// percentage looks.
#[derive(Debug, Serialize)]
pub struct SeasonalMonthPattern {
    pub month_name: String,
    pub avg_revenue_cents: i64,
    /// How many distinct calendar years contributed a real data point
    /// to this month's average — NOT how many sales rows existed. One
    /// year with 30 sales in December still means `years_seen: 1`: it
    /// tells you nothing about whether December itself runs hot for
    /// this business, only that this one December did.
    pub years_seen: i64,
}

/// Deliberately takes no `lookback_days`/window parameter the way the
/// day-of-week and weekly/monthly trends above do — seasonality is
/// inherently a question about ALL the history a business has, not a
/// recent slice of it. Returns real per-year contributions rather
/// than pretending a single year of data already IS the seasonal
/// pattern — see `SeasonalMonthPattern::years_seen`'s own doc comment,
/// and this module's own top-of-file note on why year-over-year
/// seasonality needs materially more history than a "busiest weekday"
/// question does.
pub fn seasonal_month_pattern(
    conn: &Connection,
    business_id: &str,
    user_id: &str,
    offset_minutes: i64,
) -> Result<Vec<SeasonalMonthPattern>> {
    crate::rbac::require(conn, user_id, "sales", "read")?;
    let sales_module = crud::load_module(conn, business_id, "sales")
        .map_err(|_| anyhow!("the Sales module isn't enabled for this business"))?;
    let table = sales_module.table_name();
    // Same reasoning as this file's other offset handling — which
    // calendar month (and, at year boundaries, which year) a sale
    // belongs to is exactly the kind of thing raw UTC bucketing gets
    // wrong near midnight on the last/first day of a month.
    let offset_minutes = offset_minutes.clamp(-720, 840);
    let offset_modifier = format!("{offset_minutes:+} minutes");

    // Per (calendar-month, year) totals first — this is what makes
    // `years_seen` real: it's a COUNT of these rows per month, not a
    // guess derived from however many individual sales happened to
    // land in that month.
    let sql = format!(
        "SELECT CAST(strftime('%m', created_at, ?2) AS INTEGER) AS month,
                strftime('%Y', created_at, ?2) AS year,
                COALESCE(SUM(revenue), 0)
         FROM {table}
         WHERE business_id = ?1 AND deleted_at IS NULL
         GROUP BY month, year"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![business_id, offset_modifier], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
    })?;

    let mut year_totals_by_month: [Vec<i64>; 12] = Default::default();
    for row in rows {
        let (month, _year, revenue) = row?;
        if (1..=12).contains(&month) {
            year_totals_by_month[(month - 1) as usize].push(revenue);
        }
    }

    const MONTH_NAMES: [&str; 12] = [
        "January", "February", "March", "April", "May", "June",
        "July", "August", "September", "October", "November", "December",
    ];
    Ok((0..12)
        .map(|i| {
            let totals = &year_totals_by_month[i];
            let years_seen = totals.len() as i64;
            let avg_revenue_cents = if years_seen > 0 {
                (totals.iter().sum::<i64>() as f64 / years_seen as f64).round() as i64
            } else {
                0
            };
            SeasonalMonthPattern { month_name: MONTH_NAMES[i].to_string(), avg_revenue_cents, years_seen }
        })
        .collect())
}
