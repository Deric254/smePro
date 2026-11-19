//! Business Intelligence — the eight formal intelligence areas (Sales,
//! Profitability, Inventory, Customer, Expense, Cash Flow, Business
//! Performance, Business Growth), each producing structured insights.
//!
//! Read-only: this module writes nothing and adds no tables. Every figure
//! is computed from records the app already holds, using the same columns
//! the existing reports use (Sales `revenue` / `cost_at_sale` / `sale_date`,
//! Accounting `entry_type` / `category` / `amount` / `transaction_date`,
//! Debt & Credit, Refunds). Where an existing function already answers a
//! question (slow movers, stock runway, expiring batches, debt summary and
//! aging) it is called, not re-implemented.
//!
//! Insight shape (shared with Pharmacy Pro — see INTELLIGENCE.md):
//! what happened, evidence, reporting period, historical comparison, why it
//! matters, potential impact, recommended action, confidence with stated
//! limitations, and follow-up. A finding is emitted only when the data can
//! support it; otherwise the area reports what is missing instead. Nothing
//! is estimated to fill a gap.

use crate::{crud, debt_settlement, money, rbac, stock_health};
use anyhow::{anyhow, Result};
use chrono::{Duration, NaiveDate};
use rusqlite::{params, Connection};
use serde::Serialize;
use std::collections::HashMap;

// ---------------------------------------------------------------- shapes

#[derive(Debug, Serialize, Clone)]
pub struct Evidence {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct PeriodRange {
    pub label: String,
    /// First day included (YYYY-MM-DD).
    pub start: String,
    /// Last day included (YYYY-MM-DD).
    pub end: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct Comparison {
    pub label: String,
    pub period: PeriodRange,
    pub current: f64,
    pub previous: f64,
    pub change_pct: Option<f64>,
    /// "money" (minor units), "percent", "count" or "days".
    pub unit: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct Impact {
    pub amount_cents: i64,
    pub basis: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct Confidence {
    /// "high", "medium" or "low".
    pub level: String,
    pub limitations: Vec<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct Insight {
    pub area: String,
    pub kind: String,
    pub title: String,
    /// "problem", "watch", "info" or "good".
    pub severity: String,
    pub what_happened: String,
    pub evidence: Vec<Evidence>,
    pub period: PeriodRange,
    pub comparison: Option<Comparison>,
    pub why_it_matters: String,
    pub impact: Option<Impact>,
    pub recommended_action: String,
    pub confidence: Confidence,
    pub follow_up: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct Kpi {
    pub label: String,
    pub value: Option<f64>,
    pub previous: Option<f64>,
    pub change_pct: Option<f64>,
    /// "money" (minor units), "percent", "count" or "days".
    pub unit: String,
}

#[derive(Debug, Serialize)]
pub struct AreaReport {
    pub id: String,
    pub title: String,
    /// "full", "partial" (some essential information is not recorded) or
    /// "unavailable" (the module behind it is off or not readable).
    pub status: String,
    pub kpis: Vec<Kpi>,
    pub insights: Vec<Insight>,
    /// Information the app does not hold that this area would need.
    pub missing: Vec<String>,
    /// Plain statements about this period's data (e.g. "no sales yet").
    pub notes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct IntelligenceReport {
    pub as_of: String,
    pub currency: String,
    pub period: PeriodRange,
    pub previous_period: PeriodRange,
    pub areas: Vec<AreaReport>,
}

// ---------------------------------------------------------------- context

struct Ctx<'a> {
    conn: &'a Connection,
    business_id: &'a str,
    user_id: &'a str,
    today: String,
    days: i64,
    currency: String,
    cur_start: String,   // exclusive
    prev_start: String,  // exclusive
    prev2_start: String, // exclusive
    period: PeriodRange,
    prev_period: PeriodRange,
}

impl<'a> Ctx<'a> {
    /// Same grouping the screens use (12,000.00), so a sentence here and
    /// a figure beside it never read differently.
    fn money(&self, cents: i64) -> String {
        let raw = money::format_money(cents, &self.currency);
        let (sign, digits) = match raw.strip_prefix('-') { Some(d) => ("-", d), None => ("", raw.as_str()) };
        let (whole, frac) = match digits.split_once('.') { Some((w, f)) => (w, Some(f)), None => (digits, None) };
        let mut grouped = String::new();
        for (i, c) in whole.chars().enumerate() {
            if i > 0 && (whole.len() - i) % 3 == 0 { grouped.push(','); }
            grouped.push(c);
        }
        match frac {
            Some(f) => format!("{} {sign}{grouped}.{f}", self.currency),
            None => format!("{} {sign}{grouped}", self.currency),
        }
    }
}

fn day_offset(date: &str, delta: i64) -> Result<String> {
    let d = NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| anyhow!("invalid date"))?;
    Ok((d + Duration::days(delta)).to_string())
}

fn pct_change(cur: f64, prev: f64) -> Option<f64> {
    if prev == 0.0 { None } else { Some((cur - prev) / prev.abs() * 100.0) }
}

fn pct(part: i64, whole: i64) -> Option<f64> {
    if whole <= 0 { None } else { Some(part as f64 / whole as f64 * 100.0) }
}

fn kpi(label: &str, unit: &str, value: Option<f64>, previous: Option<f64>) -> Kpi {
    let change_pct = match (value, previous) {
        (Some(c), Some(p)) if unit != "percent" => pct_change(c, p),
        _ => None,
    };
    Kpi { label: label.into(), value, previous, change_pct, unit: unit.into() }
}

fn ev(label: &str, value: String) -> Evidence {
    Evidence { label: label.into(), value }
}

fn new_insight(ctx: &Ctx, area: &str, kind: &str, title: &str, severity: &str) -> Insight {
    Insight {
        area: area.into(),
        kind: kind.into(),
        title: title.into(),
        severity: severity.into(),
        what_happened: String::new(),
        evidence: vec![],
        period: ctx.period.clone(),
        comparison: None,
        why_it_matters: String::new(),
        impact: None,
        recommended_action: String::new(),
        confidence: Confidence { level: "high".into(), limitations: vec![] },
        follow_up: None,
    }
}

/// Confidence is derived, never asserted: any stated limitation lowers it
/// to medium; `weak` (history too short / coverage too thin to lean on)
/// lowers it to low; a small sample adds its own limitation.
fn confidence(mut limitations: Vec<String>, weak: bool, sample: i64) -> Confidence {
    if sample < 20 {
        limitations.push(format!("Based on only {sample} record{}.", if sample == 1 { "" } else { "s" }));
    }
    let level = if weak { "low" } else if !limitations.is_empty() { "medium" } else { "high" };
    Confidence { level: level.into(), limitations }
}

fn comparison(ctx: &Ctx, label: &str, unit: &str, current: f64, previous: f64) -> Comparison {
    Comparison {
        label: label.into(),
        period: ctx.prev_period.clone(),
        current,
        previous,
        change_pct: if unit == "percent" { None } else { pct_change(current, previous) },
        unit: unit.into(),
    }
}

fn area(id: &str, title: &str) -> AreaReport {
    AreaReport { id: id.into(), title: title.into(), status: "full".into(), kpis: vec![], insights: vec![], missing: vec![], notes: vec![] }
}

fn unavailable(id: &str, title: &str, reason: String) -> AreaReport {
    let mut a = area(id, title);
    a.status = "unavailable".into();
    a.missing.push(reason);
    a
}

/// Table of an enabled module the caller is allowed to read, or the plain
/// reason it can't be used.
fn table_for(ctx: &Ctx, module: &str, label: &str) -> std::result::Result<String, String> {
    let m = crud::load_module(ctx.conn, ctx.business_id, module)
        .map_err(|_| format!("The {label} module isn't enabled for this business."))?;
    rbac::require(ctx.conn, ctx.user_id, module, "read")
        .map_err(|_| format!("Your role can't read {label}."))?;
    Ok(m.table_name())
}

// ------------------------------------------------------------ sales data

#[derive(Debug, Default, Clone)]
struct Win {
    revenue: i64,
    cost: i64,
    rows: i64,
    orders: i64,
    units: i64,
    /// Revenue of sale rows that carry a real cost_at_sale.
    cost_covered_revenue: i64,
}

impl Win {
    fn profit(&self) -> i64 { self.revenue - self.cost }
    fn margin(&self) -> Option<f64> { pct(self.profit(), self.revenue) }
    fn cost_coverage(&self) -> Option<f64> { pct(self.cost_covered_revenue, self.revenue) }
    fn aov(&self) -> Option<f64> { if self.orders > 0 { Some(self.revenue as f64 / self.orders as f64) } else { None } }
}

#[derive(Debug, Default, Clone)]
struct Itm {
    revenue: i64,
    cost: i64,
    cost_rows: i64,
}

#[derive(Debug, Default, Clone)]
struct Cust {
    name: String,
    revenue: i64,
    orders: i64,
}

/// Everything the sales-based areas share, computed once so every area
/// uses identical definitions and identical numbers.
struct Data {
    table: String,
    cur: Win,
    prev: Win,
    prev2: Win,
    items_cur: HashMap<String, Itm>,
    items_prev: HashMap<String, Itm>,
    cust_cur: HashMap<String, Cust>,
    cust_prev: HashMap<String, Cust>,
    cust_first: HashMap<String, String>,
    item_first: HashMap<String, String>,
    history_days: i64,
    /// True when the previous period is fully covered by recorded history.
    prev_ok: bool,
    /// True when the period before that is also fully covered.
    prev2_ok: bool,
}

const CUST_KEY: &str = "COALESCE(NULLIF(TRIM(customer_phone), ''), LOWER(NULLIF(TRIM(customer), '')))";

fn win_agg(ctx: &Ctx, table: &str, start: &str, end: &str) -> Result<Win> {
    let sql = format!(
        "SELECT COALESCE(SUM(revenue),0), COALESCE(SUM(cost_at_sale),0), COUNT(*),
                COUNT(DISTINCT COALESCE(NULLIF(order_id,''), id)), COALESCE(SUM(quantity),0),
                COALESCE(SUM(CASE WHEN cost_at_sale > 0 THEN revenue ELSE 0 END),0)
         FROM {table} WHERE business_id = ?1 AND deleted_at IS NULL AND sale_date > ?2 AND sale_date <= ?3"
    );
    Ok(ctx.conn.query_row(&sql, params![ctx.business_id, start, end], |r| {
        Ok(Win {
            revenue: r.get(0)?, cost: r.get(1)?, rows: r.get(2)?, orders: r.get(3)?, units: r.get(4)?,
            cost_covered_revenue: r.get(5)?,
        })
    })?)
}

fn item_agg(ctx: &Ctx, table: &str, start: &str, end: &str) -> Result<HashMap<String, Itm>> {
    let sql = format!(
        "SELECT item_name, COALESCE(SUM(revenue),0), COALESCE(SUM(cost_at_sale),0),
                COALESCE(SUM(CASE WHEN cost_at_sale > 0 THEN 1 ELSE 0 END),0)
         FROM {table} WHERE business_id = ?1 AND deleted_at IS NULL AND sale_date > ?2 AND sale_date <= ?3
         GROUP BY item_name"
    );
    let mut stmt = ctx.conn.prepare(&sql)?;
    let rows = stmt.query_map(params![ctx.business_id, start, end], |r| {
        Ok((r.get::<_, String>(0)?, Itm { revenue: r.get(1)?, cost: r.get(2)?, cost_rows: r.get(3)? }))
    })?;
    Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
}

fn cust_agg(ctx: &Ctx, table: &str, start: &str, end: &str) -> Result<HashMap<String, Cust>> {
    let sql = format!(
        "SELECT {CUST_KEY} AS k, MAX(NULLIF(TRIM(customer), '')), COALESCE(SUM(revenue),0),
                COUNT(DISTINCT COALESCE(NULLIF(order_id,''), id))
         FROM {table} WHERE business_id = ?1 AND deleted_at IS NULL AND sale_date > ?2 AND sale_date <= ?3
           AND {CUST_KEY} IS NOT NULL GROUP BY k"
    );
    let mut stmt = ctx.conn.prepare(&sql)?;
    let rows = stmt.query_map(params![ctx.business_id, start, end], |r| {
        let key: String = r.get(0)?;
        let name: Option<String> = r.get(1)?;
        Ok((key.clone(), Cust { name: name.unwrap_or(key), revenue: r.get(2)?, orders: r.get(3)? }))
    })?;
    Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
}

fn first_seen(ctx: &Ctx, table: &str, key_sql: &str) -> Result<HashMap<String, String>> {
    let sql = format!(
        "SELECT {key_sql} AS k, MIN(sale_date) FROM {table}
         WHERE business_id = ?1 AND deleted_at IS NULL AND sale_date IS NOT NULL AND {key_sql} IS NOT NULL GROUP BY k"
    );
    let mut stmt = ctx.conn.prepare(&sql)?;
    let rows = stmt.query_map(params![ctx.business_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
}

fn load_data(ctx: &Ctx, table: &str) -> Result<Data> {
    let first_sale: Option<String> = ctx.conn.query_row(
        &format!("SELECT MIN(sale_date) FROM {table} WHERE business_id = ?1 AND deleted_at IS NULL AND sale_date IS NOT NULL"),
        params![ctx.business_id],
        |r| r.get(0),
    )?;
    let today = NaiveDate::parse_from_str(&ctx.today, "%Y-%m-%d").map_err(|_| anyhow!("invalid date"))?;
    let history_days = first_sale
        .as_deref()
        .and_then(|f| NaiveDate::parse_from_str(f, "%Y-%m-%d").ok())
        .map(|f| (today - f).num_days() + 1)
        .unwrap_or(0);
    // A period is "covered" when the first recorded sale is on or before
    // its first day.
    let covered = |exclusive_start: &str| -> bool {
        match (&first_sale, day_offset(exclusive_start, 1)) {
            (Some(f), Ok(first_day)) => f.as_str() <= first_day.as_str(),
            _ => false,
        }
    };
    Ok(Data {
        table: table.to_string(),
        cur: win_agg(ctx, table, &ctx.cur_start, &ctx.today)?,
        prev: win_agg(ctx, table, &ctx.prev_start, &ctx.cur_start)?,
        prev2: win_agg(ctx, table, &ctx.prev2_start, &ctx.prev_start)?,
        items_cur: item_agg(ctx, table, &ctx.cur_start, &ctx.today)?,
        items_prev: item_agg(ctx, table, &ctx.prev_start, &ctx.cur_start)?,
        cust_cur: cust_agg(ctx, table, &ctx.cur_start, &ctx.today)?,
        cust_prev: cust_agg(ctx, table, &ctx.prev_start, &ctx.cur_start)?,
        cust_first: first_seen(ctx, table, CUST_KEY)?,
        item_first: first_seen(ctx, table, "item_name")?,
        history_days,
        prev_ok: covered(&ctx.prev_start),
        prev2_ok: covered(&ctx.prev2_start),
    })
}

fn weekday_limitation(ctx: &Ctx) -> Option<String> {
    if ctx.days % 7 != 0 {
        Some("The two periods are not a whole number of weeks, so their weekday mix can differ.".into())
    } else {
        None
    }
}

fn history_limitation(d: &Data, ctx: &Ctx) -> Option<String> {
    if d.prev_ok { None } else {
        Some(format!("Only {} day(s) of sales history exist, less than the {} needed for a full previous period.", d.history_days, ctx.days * 2))
    }
}

fn top_by<F: Fn(&(String, i64)) -> i64>(mut v: Vec<(String, i64)>, key: F, n: usize) -> Vec<(String, i64)> {
    v.sort_by(|a, b| key(b).cmp(&key(a)).then(a.0.cmp(&b.0)));
    v.truncate(n);
    v
}

// ------------------------------------------------------------------ sales

fn sales_area(ctx: &Ctx, d: &Data) -> AreaReport {
    let mut a = area("sales", "Sales Intelligence");
    let (cur, prev) = (&d.cur, &d.prev);
    let prev_known = d.prev_ok;
    let pv = |x: Option<f64>| if prev_known { x } else { None };

    a.kpis = vec![
        kpi("Revenue", "money", Some(cur.revenue as f64), pv(Some(prev.revenue as f64))),
        kpi("Orders", "count", Some(cur.orders as f64), pv(Some(prev.orders as f64))),
        kpi("Average order value", "money", cur.aov(), pv(prev.aov())),
        kpi("Units sold", "count", Some(cur.units as f64), pv(Some(prev.units as f64))),
    ];
    if cur.rows == 0 {
        a.notes.push("No sales are recorded in this period.".into());
    }
    if let Some(l) = history_limitation(d, ctx) { a.notes.push(l); }

    // 1. Unusual change in sales vs the equal previous period, with drivers.
    if prev_known && prev.revenue > 0 && cur.rows + prev.rows >= 10 {
        if let Some(change) = pct_change(cur.revenue as f64, prev.revenue as f64) {
            if change.abs() >= 15.0 {
                let mut deltas: Vec<(String, i64)> = d.items_cur.keys().chain(d.items_prev.keys()).collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .map(|n| {
                        let c = d.items_cur.get(n).map(|i| i.revenue).unwrap_or(0);
                        let p = d.items_prev.get(n).map(|i| i.revenue).unwrap_or(0);
                        (n.clone(), c - p)
                    })
                    .filter(|(_, delta)| if change < 0.0 { *delta < 0 } else { *delta > 0 })
                    .collect();
                deltas = top_by(deltas, |x| x.1.abs(), 3);
                let severity = if change <= -30.0 { "problem" } else if change < 0.0 { "watch" } else { "good" };
                let mut i = new_insight(ctx, "sales", "sales_change", if change < 0.0 { "Sales fell against the previous period" } else { "Sales rose against the previous period" }, severity);
                i.what_happened = format!(
                    "Revenue was {} in this period against {} in the equal period before it ({:+.1}%).",
                    ctx.money(cur.revenue), ctx.money(prev.revenue), change
                );
                i.evidence = vec![
                    ev("Revenue this period", ctx.money(cur.revenue)),
                    ev("Revenue previous period", ctx.money(prev.revenue)),
                    ev("Orders", format!("{} vs {}", cur.orders, prev.orders)),
                ];
                for (n, delta) in &deltas {
                    i.evidence.push(ev(&format!("Biggest driver: {n}"), format!("{}{}", if *delta < 0 { "-" } else { "+" }, ctx.money(delta.abs()))));
                }
                i.comparison = Some(comparison(ctx, "Revenue", "money", cur.revenue as f64, prev.revenue as f64));
                i.why_it_matters = if change < 0.0 {
                    "A sustained drop in sales reduces the cash and profit available to cover costs.".into()
                } else {
                    "A real rise shows what is working and what stock and cash it will need to keep up.".into()
                };
                i.impact = Some(Impact { amount_cents: cur.revenue - prev.revenue, basis: "Revenue difference between the two periods".into() });
                i.recommended_action = if change < 0.0 {
                    "Look at the items driving the drop first: check stock availability, price changes and whether regular customers stopped buying.".into()
                } else {
                    "Check the items driving the rise have enough stock and a healthy margin before relying on it continuing.".into()
                };
                let mut lim = vec![];
                if let Some(l) = weekday_limitation(ctx) { lim.push(l); }
                i.confidence = confidence(lim, false, cur.rows.min(prev.rows));
                i.follow_up = Some(format!("Re-check after the next {} days: does revenue stay near {}?", ctx.days, ctx.money(cur.revenue)));
                a.insights.push(i);
            }
        }
    }

    // 2. Individual products that fell sharply.
    if prev_known && prev.revenue > 0 {
        let mut falling: Vec<(String, i64)> = d.items_prev.iter()
            .filter(|(_, p)| p.revenue as f64 >= prev.revenue as f64 * 0.03)
            .filter_map(|(n, p)| {
                let c = d.items_cur.get(n).map(|i| i.revenue).unwrap_or(0);
                if (c as f64) <= p.revenue as f64 * 0.6 { Some((n.clone(), p.revenue - c)) } else { None }
            })
            .collect();
        if !falling.is_empty() {
            falling = top_by(falling, |x| x.1, 5);
            let total: i64 = falling.iter().map(|x| x.1).sum();
            let mut i = new_insight(ctx, "sales", "product_decline", "Products with a sharp sales drop", "watch");
            i.what_happened = format!("{} product(s) that mattered last period lost at least 40% of their revenue this period.", falling.len());
            for (n, lost) in &falling {
                i.evidence.push(ev(n, format!("-{}", ctx.money(*lost))));
            }
            i.why_it_matters = "These were meaningful sellers; losing them explains part of any overall change.".into();
            i.impact = Some(Impact { amount_cents: -total, basis: "Revenue lost on these products versus the previous period".into() });
            i.recommended_action = "Check whether each is out of stock, repriced, or replaced by something customers now prefer.".into();
            let mut lim = vec!["Products are matched by name; a renamed product looks like a drop plus a new item.".to_string()];
            if let Some(l) = weekday_limitation(ctx) { lim.push(l); }
            i.confidence = confidence(lim, false, cur.rows.min(prev.rows));
            a.insights.push(i);
        }
    }

    // 3. Concentration of revenue in a few products.
    if d.items_cur.len() >= 4 && cur.revenue > 0 {
        let mut v: Vec<(String, i64)> = d.items_cur.iter().map(|(n, i)| (n.clone(), i.revenue)).collect();
        v = top_by(v, |x| x.1, 3);
        let top1 = pct(v[0].1, cur.revenue).unwrap_or(0.0);
        let top3 = pct(v.iter().map(|x| x.1).sum(), cur.revenue).unwrap_or(0.0);
        if top1 >= 40.0 || top3 >= 70.0 {
            let mut i = new_insight(ctx, "sales", "sales_concentration", "Sales depend on very few products", "watch");
            i.what_happened = format!("{} accounts for {:.0}% of revenue, and the top three products for {:.0}%.", v[0].0, top1, top3);
            for (n, r) in &v { i.evidence.push(ev(n, format!("{} ({:.0}%)", ctx.money(*r), pct(*r, cur.revenue).unwrap_or(0.0)))); }
            i.why_it_matters = "If one of these stops selling or runs out, a large part of revenue goes with it.".into();
            i.impact = Some(Impact { amount_cents: v[0].1, basis: "Revenue from the single largest product".into() });
            i.recommended_action = "Keep these well stocked and look for a second line of products that can carry part of the load.".into();
            i.confidence = confidence(vec![], false, cur.rows);
            a.insights.push(i);
        }
    }
    a
}

// ---------------------------------------------------------- profitability

fn opex_by_category(ctx: &Ctx, acc: &str, start: &str, end: &str) -> Result<HashMap<String, (i64, i64)>> {
    let sql = format!(
        "SELECT COALESCE(NULLIF(TRIM(category),''), 'Uncategorised'), COALESCE(SUM(amount),0), COUNT(*)
         FROM {acc} WHERE business_id = ?1 AND deleted_at IS NULL AND LOWER(TRIM(entry_type)) = 'expense'
           AND NOT {AUTO_POSTED_SQL}
           AND transaction_date > ?2 AND transaction_date <= ?3 GROUP BY 1"
    );
    let mut stmt = ctx.conn.prepare(&sql)?;
    let rows = stmt.query_map(params![ctx.business_id, start, end], |r| Ok((r.get::<_, String>(0)?, (r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))))?;
    Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
}

fn opex_total(m: &HashMap<String, (i64, i64)>) -> (i64, i64) {
    m.values().fold((0, 0), |a, v| (a.0 + v.0, a.1 + v.1))
}

fn profit_area(ctx: &Ctx, d: &Data, acc: Option<&str>) -> AreaReport {
    let mut a = area("profitability", "Profitability Intelligence");
    let (cur, prev) = (&d.cur, &d.prev);
    let cov_cur = cur.cost_coverage();
    let cov_prev = prev.cost_coverage();
    let pv = |x: Option<f64>| if d.prev_ok { x } else { None };

    a.kpis = vec![
        kpi("Gross profit", "money", Some(cur.profit() as f64), pv(Some(prev.profit() as f64))),
        kpi("Gross margin", "percent", cur.margin(), pv(prev.margin())),
        kpi("Cost data coverage", "percent", cov_cur, pv(cov_prev)),
    ];

    let opex = acc.and_then(|t| opex_by_category(ctx, t, &ctx.cur_start, &ctx.today).ok());
    let opex_prev = acc.and_then(|t| opex_by_category(ctx, t, &ctx.prev_start, &ctx.cur_start).ok());
    if let (Some(o), Some(op)) = (&opex, &opex_prev) {
        let (o_sum, o_n) = opex_total(o);
        if o_n > 0 {
            let est = cur.profit() - o_sum;
            let est_prev = if d.prev_ok { Some((prev.profit() - opex_total(op).0) as f64) } else { None };
            a.kpis.push(kpi("Estimated operating profit", "money", Some(est as f64), est_prev));
        }
    }
    if acc.is_none() {
        a.status = "partial".into();
        a.missing.push("Operating expenses are not available (Accounting is off or not readable), so profit after expenses can't be stated.".into());
    }
    if cur.rows == 0 { a.notes.push("No sales are recorded in this period.".into()); return a; }

    // 1. Cost data gaps make every profit figure less reliable.
    if let Some(cov) = cov_cur {
        if cov < 80.0 {
            let uncovered = cur.revenue - cur.cost_covered_revenue;
            let mut i = new_insight(ctx, "profitability", "cost_data_gap", "Profit figures rest on incomplete cost data", if cov < 50.0 { "problem" } else { "watch" });
            i.what_happened = format!("Only {:.0}% of this period's sales revenue has a recorded cost, so gross profit is overstated for the rest.", cov);
            i.evidence = vec![
                ev("Revenue with a recorded cost", ctx.money(cur.cost_covered_revenue)),
                ev("Revenue without a recorded cost", ctx.money(uncovered)),
            ];
            i.why_it_matters = "Sales with no cost look like pure profit, so margins and item rankings can be wrong.".into();
            i.impact = Some(Impact { amount_cents: uncovered, basis: "Revenue whose profit can't be measured".into() });
            i.recommended_action = "Record the unit cost on inventory items (or receive stock through purchasing) so every sale carries its cost.".into();
            i.confidence = confidence(vec![], false, cur.rows);
            a.insights.push(i);
        }
    }

    let reliable_now = cov_cur.map(|c| c >= 80.0).unwrap_or(false);
    let reliable_prev = cov_prev.map(|c| c >= 80.0).unwrap_or(false);

    // 2. Margin shift between equal periods, with the items behind it.
    if d.prev_ok && reliable_now && reliable_prev {
        if let (Some(m), Some(pm)) = (cur.margin(), prev.margin()) {
            let delta = m - pm;
            if delta.abs() >= 3.0 {
                let mut deltas: Vec<(String, i64)> = d.items_cur.keys().chain(d.items_prev.keys()).collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .filter(|n| d.items_cur.get(*n).map(|i| i.cost_rows > 0).unwrap_or(true) && d.items_prev.get(*n).map(|i| i.cost_rows > 0).unwrap_or(true))
                    .map(|n| {
                        let c = d.items_cur.get(n).map(|i| i.revenue - i.cost).unwrap_or(0);
                        let p = d.items_prev.get(n).map(|i| i.revenue - i.cost).unwrap_or(0);
                        (n.clone(), c - p)
                    })
                    .filter(|(_, x)| if delta < 0.0 { *x < 0 } else { *x > 0 })
                    .collect();
                deltas = top_by(deltas, |x| x.1.abs(), 3);
                let severity = if delta <= -6.0 { "problem" } else if delta < 0.0 { "watch" } else { "good" };
                let mut i = new_insight(ctx, "profitability", "margin_shift", if delta < 0.0 { "Gross margin has narrowed" } else { "Gross margin has widened" }, severity);
                i.what_happened = format!("Gross margin was {:.1}% against {:.1}% in the previous period ({:+.1} points).", m, pm, delta);
                i.evidence = vec![
                    ev("Gross profit this period", ctx.money(cur.profit())),
                    ev("Gross profit previous period", ctx.money(prev.profit())),
                ];
                for (n, x) in &deltas { i.evidence.push(ev(&format!("Profit change: {n}"), format!("{}{}", if *x < 0 { "-" } else { "+" }, ctx.money(x.abs())))); }
                i.comparison = Some(comparison(ctx, "Gross margin", "percent", m, pm));
                i.why_it_matters = if delta < 0.0 { "Each sale now keeps less of its price, so the same revenue produces less profit.".into() } else { "Each sale now keeps more of its price.".into() };
                i.impact = Some(Impact { amount_cents: (delta / 100.0 * cur.revenue as f64).round() as i64, basis: "Margin change applied to this period's revenue".into() });
                i.recommended_action = if delta < 0.0 { "Check whether supplier costs rose or discounts deepened on the items listed, and reprice or renegotiate.".into() } else { "Confirm the gain comes from pricing or cost savings you can keep, not a one-off.".into() };
                let mut lim = vec![];
                if let Some(l) = weekday_limitation(ctx) { lim.push(l); }
                i.confidence = confidence(lim, false, cur.rows.min(prev.rows));
                i.follow_up = Some(format!("Next check: does margin hold near {:.1}% over the following {} days?", m, ctx.days));
                a.insights.push(i);
            }
        }
    }

    // 3. Items selling below cost.
    let mut losers: Vec<(String, i64)> = d.items_cur.iter()
        .filter(|(_, i)| i.cost_rows > 0 && i.revenue - i.cost < 0)
        .map(|(n, i)| (n.clone(), i.revenue - i.cost))
        .collect();
    if !losers.is_empty() {
        losers = top_by(losers, |x| -x.1, 5);
        let total: i64 = losers.iter().map(|x| x.1).sum();
        let mut i = new_insight(ctx, "profitability", "loss_making_items", "Items selling below their cost", "problem");
        i.what_happened = format!("{} item(s) cost more than they earned this period.", losers.len());
        for (n, p) in &losers { i.evidence.push(ev(n, format!("-{}", ctx.money(p.abs())))); }
        i.why_it_matters = "Every sale of these items reduces total profit.".into();
        i.impact = Some(Impact { amount_cents: total, basis: "Gross loss on the listed items this period".into() });
        i.recommended_action = "Raise the price, renegotiate the cost, or stop selling the item below cost.".into();
        i.confidence = confidence(vec!["Only items with at least one recorded cost are judged.".into()], false, cur.rows);
        a.insights.push(i);
    }

    // 4. Profit concentrated in one item.
    let mut gainers: Vec<(String, i64)> = d.items_cur.iter().filter(|(_, i)| i.cost_rows > 0 && i.revenue - i.cost > 0).map(|(n, i)| (n.clone(), i.revenue - i.cost)).collect();
    let positive_total: i64 = gainers.iter().map(|x| x.1).sum();
    if gainers.len() >= 4 && positive_total > 0 {
        gainers = top_by(gainers, |x| x.1, 3);
        let share = pct(gainers[0].1, positive_total).unwrap_or(0.0);
        if share >= 50.0 {
            let mut i = new_insight(ctx, "profitability", "profit_concentration", "Most profit comes from one item", "watch");
            i.what_happened = format!("{} produced {:.0}% of the profit made on items with recorded costs.", gainers[0].0, share);
            for (n, p) in &gainers { i.evidence.push(ev(n, ctx.money(*p))); }
            i.why_it_matters = "Profit is fragile if this one item's price, cost or demand changes.".into();
            i.impact = Some(Impact { amount_cents: gainers[0].1, basis: "Gross profit from the single most profitable item".into() });
            i.recommended_action = "Protect its supply and price, and build up other profitable items.".into();
            i.confidence = confidence(vec!["Only items with recorded costs are counted.".into()], false, cur.rows);
            a.insights.push(i);
        }
    }

    // 5. Expenses against gross profit.
    if let Some(o) = &opex {
        let (o_sum, o_n) = opex_total(o);
        if o_n > 0 && reliable_now && cur.profit() > 0 {
            let ratio = o_sum as f64 / cur.profit() as f64 * 100.0;
            if ratio >= 70.0 {
                let est = cur.profit() - o_sum;
                let mut i = new_insight(ctx, "profitability", "operating_profit", if est < 0 { "Expenses exceed gross profit" } else { "Expenses are absorbing most gross profit" }, if est < 0 { "problem" } else { "watch" });
                i.what_happened = format!("Recorded operating expenses of {} equal {:.0}% of gross profit ({}), leaving {}.", ctx.money(o_sum), ratio, ctx.money(cur.profit()), ctx.money(est));
                i.evidence = vec![ev("Gross profit", ctx.money(cur.profit())), ev("Operating expenses recorded", ctx.money(o_sum)), ev("Estimated operating profit", ctx.money(est))];
                i.why_it_matters = "What is left after running costs is what the business actually keeps.".into();
                i.impact = Some(Impact { amount_cents: est, basis: "Gross profit minus recorded operating expenses".into() });
                i.recommended_action = "Review the largest expense categories and the margin on top-selling items.".into();
                i.confidence = confidence(vec!["Expenses are counted when recorded in Accounting, while profit is counted when sold, and an unrecorded expense would make this look better than it is.".into()], false, o_n);
                a.insights.push(i);
            }
        }
    }
    a
}

// -------------------------------------------------------------- inventory

fn inventory_area(ctx: &Ctx, d: Option<&Data>) -> AreaReport {
    let inv = match table_for(ctx, "inventory", "Inventory") {
        Ok(t) => t,
        Err(reason) => return unavailable("inventory", "Inventory Intelligence", reason),
    };
    let mut a = area("inventory", "Inventory Intelligence");

    // Same blended value slow_movers uses: live batches at their own cost,
    // plus any legacy (non-batch) quantity at the item's own unit_cost.
    let value_sql = format!(
        "WITH bt AS (SELECT inventory_record_id, SUM(quantity_remaining) AS q, SUM(quantity_remaining * unit_cost) AS v
                     FROM inventory_batches WHERE business_id = ?1 AND deleted_at IS NULL GROUP BY inventory_record_id)
         SELECT COALESCE(SUM(COALESCE(bt.v,0) + CAST(ROUND(MAX(i.quantity - COALESCE(bt.q,0), 0) * COALESCE(i.unit_cost,0)) AS INTEGER)),0), COUNT(*)
         FROM {inv} i LEFT JOIN bt ON bt.inventory_record_id = i.id
         WHERE i.business_id = ?1 AND i.deleted_at IS NULL AND i.quantity > 0"
    );
    let (stock_value, items_in_stock): (i64, i64) = ctx.conn
        .query_row(&value_sql, params![ctx.business_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap_or((0, 0));
    a.kpis = vec![
        kpi("Stock value at cost", "money", Some(stock_value as f64), None),
        kpi("Items in stock", "count", Some(items_in_stock as f64), None),
    ];

    let Some(d) = d else {
        a.status = "partial".into();
        a.missing.push("Sales isn't available to you, so findings about how stock moves (slow stock, stock-out risk) can't be produced.".into());
        return a;
    };
    let name_link = "Stock and sales are matched by item name; a renamed item loses its earlier sales history.".to_string();

    // 1. Sold this period but now out of stock.
    let names_sql = format!(
        "SELECT i.name FROM {inv} i WHERE i.business_id = ?1 AND i.deleted_at IS NULL AND i.quantity <= 0
           AND EXISTS (SELECT 1 FROM {s} s WHERE s.business_id = i.business_id AND s.item_name = i.name AND s.deleted_at IS NULL
                       AND s.sale_date > ?2 AND s.sale_date <= ?3)",
        s = d.table
    );
    if let Ok(mut stmt) = ctx.conn.prepare(&names_sql) {
        if let Ok(rows) = stmt.query_map(params![ctx.business_id, ctx.cur_start, ctx.today], |r| r.get::<_, String>(0)) {
            let names: Vec<String> = rows.filter_map(|x| x.ok()).collect();
            a.kpis.push(kpi("Selling items out of stock", "count", Some(names.len() as f64), None));
            if !names.is_empty() {
                let mut with_rev: Vec<(String, i64)> = names.iter().map(|n| (n.clone(), d.items_cur.get(n).map(|i| i.revenue).unwrap_or(0))).collect();
                let total: i64 = with_rev.iter().map(|x| x.1).sum();
                with_rev = top_by(with_rev, |x| x.1, 5);
                let mut i = new_insight(ctx, "inventory", "sold_out_sellers", "Items that were selling are now out of stock", "problem");
                i.what_happened = format!("{} item(s) that sold this period have no stock left.", names.len());
                for (n, r) in &with_rev { i.evidence.push(ev(n, format!("{} sold this period", ctx.money(*r)))); }
                i.why_it_matters = "Every day these stay empty, the sales they were making are lost.".into();
                i.impact = Some(Impact { amount_cents: total, basis: "Revenue these items earned this period, at risk while they are out of stock".into() });
                i.recommended_action = "Reorder these first.".into();
                i.confidence = confidence(vec![name_link.clone()], false, d.cur.rows);
                a.insights.push(i);
            }
        }
    }

    // 2. Capital tied up in stock that has stopped selling (existing slow_movers).
    if let Ok(items) = stock_health::slow_movers(ctx.conn, ctx.business_id, ctx.user_id, &ctx.today, 60, 100) {
        let total: i64 = items.iter().map(|x| x.value_at_risk_cents).sum();
        if total > 0 {
            let share = pct(total, stock_value).unwrap_or(0.0);
            let mut i = new_insight(ctx, "inventory", "dead_stock", "Money tied up in stock that isn't selling", if share >= 25.0 { "problem" } else { "watch" });
            i.what_happened = format!(
                "{}{} item(s) with no sale in 60 days hold {} at cost, {:.0}% of total stock value.",
                if items.len() >= 100 { "At least " } else { "" }, items.len(), ctx.money(total), share
            );
            for x in items.iter().take(5) {
                let since = x.days_since_last_sale.map(|d| format!("last sold {d} days ago")).unwrap_or_else(|| "never sold".into());
                i.evidence.push(ev(&x.item_name, format!("{} ({})", ctx.money(x.value_at_risk_cents), since)));
            }
            i.why_it_matters = "Cash sitting in unsold stock can't pay suppliers or expenses, and some of it may expire or go out of date.".into();
            i.impact = Some(Impact { amount_cents: total, basis: "Stock at cost with no sale in 60 days".into() });
            i.recommended_action = "Discount, bundle or return the largest items, and stop reordering them until they move.".into();
            let mut lim = vec![name_link.clone()];
            if d.history_days < 60 { lim.push(format!("Sales history covers only {} day(s), so \"no sale in 60 days\" can't yet be judged properly.", d.history_days)); }
            i.confidence = confidence(lim, d.history_days < 60, items.len() as i64);
            i.follow_up = Some("Re-run after 30 days: has the tied-up value fallen?".into());
            a.insights.push(i);
        }
    }

    // 3. Fast sellers about to run out (existing stock_runway).
    if let Ok(items) = stock_health::stock_runway(ctx.conn, ctx.business_id, ctx.user_id, &ctx.today, 30, 100) {
        let mut risky: Vec<&stock_health::StockRunway> = items.iter().filter(|x| x.days_of_stock_left.map(|d| d < 7.0).unwrap_or(false)).collect();
        risky.sort_by(|x, y| x.days_of_stock_left.partial_cmp(&y.days_of_stock_left).unwrap_or(std::cmp::Ordering::Equal));
        if !risky.is_empty() {
            let urgent = risky.iter().any(|x| x.days_of_stock_left.map(|d| d < 3.0).unwrap_or(false));
            let mut i = new_insight(ctx, "inventory", "stockout_risk", "Items that will run out within a week", if urgent { "problem" } else { "watch" });
            i.what_happened = format!("{} item(s) have under 7 days of stock at their recent selling pace.", risky.len());
            for x in risky.iter().take(5) {
                i.evidence.push(ev(&x.item_name, format!("{:.1} days left ({} on hand, about {:.1} sold per day)", x.days_of_stock_left.unwrap_or(0.0), x.quantity, x.avg_daily_sales)));
            }
            i.why_it_matters = "Running out of a steady seller means lost sales and customers going elsewhere.".into();
            i.recommended_action = "Reorder the items listed, starting with the fewest days left.".into();
            let mut lim = vec![name_link.clone(), "Based on the last 30 days of sales; stock already on order isn't considered.".to_string()];
            if d.history_days < 14 { lim.push("Under 14 days of sales history makes the selling pace unreliable.".into()); }
            i.confidence = confidence(lim, d.history_days < 14, d.cur.rows);
            a.insights.push(i);
        }
    }

    // 4. Expired and soon-to-expire batches (existing expiring_batches).
    if let Ok(rep) = stock_health::expiring_batches(ctx.conn, ctx.business_id, ctx.user_id, &ctx.today, 30, 50) {
        let s = &rep.summary;
        if s.expired_batches > 0 || s.expiring_batches > 0 {
            let mut i = new_insight(ctx, "inventory", "expiry_risk", "Stock past or close to its expiry date", if s.expired_batches > 0 { "problem" } else { "watch" });
            i.what_happened = format!("{} batch(es) have already expired ({} unit(s), {} at cost) and {} more expire within 30 days ({} unit(s)).",
                s.expired_batches, s.expired_units, ctx.money(s.expired_cost_value), s.expiring_batches, s.expiring_units);
            i.evidence = vec![ev("Expired batches", s.expired_batches.to_string()), ev("Expiring within 30 days", s.expiring_batches.to_string())];
            i.why_it_matters = "Expired stock can't be sold and is a direct write-off.".into();
            i.impact = Some(Impact { amount_cents: s.expired_cost_value, basis: "Cost of units already expired".into() });
            i.recommended_action = "Remove expired stock, and sell, discount or return the batches expiring soonest.".into();
            i.confidence = confidence(vec!["Only items received with an expiry date are covered.".into()], false, s.expired_batches + s.expiring_batches);
            a.insights.push(i);
        }
    }
    a
}

// --------------------------------------------------------------- customers

fn customer_area(ctx: &Ctx, d: &Data) -> AreaReport {
    let mut a = area("customers", "Customer Intelligence");
    let identified_rev: i64 = d.cust_cur.values().map(|c| c.revenue).sum();
    let share = pct(identified_rev, d.cur.revenue);
    let n_cur = d.cust_cur.len() as i64;
    let n_prev = d.cust_prev.len() as i64;
    let new_cur = d.cust_cur.keys().filter(|k| d.cust_first.get(*k).map(|f| f.as_str() > ctx.cur_start.as_str()).unwrap_or(false)).count() as i64;
    let repeat = d.cust_cur.values().filter(|c| c.orders >= 2).count() as i64;
    a.kpis = vec![
        kpi("Identified customers", "count", Some(n_cur as f64), if d.prev_ok { Some(n_prev as f64) } else { None }),
        kpi("New customers", "count", Some(new_cur as f64), None),
        kpi("Returning customers", "count", Some((n_cur - new_cur) as f64), None),
        kpi("Repeat buyers", "percent", pct(repeat, n_cur), None),
        kpi("Revenue from identified customers", "percent", share, None),
    ];
    let ident_note = "Customers are identified by phone number, otherwise by name; sales with neither are not counted here.".to_string();
    if let Some(s) = share {
        if s < 70.0 { a.notes.push(format!("Only {:.0}% of revenue has a customer name or phone, so these findings cover that part only.", s)); a.status = "partial".into(); a.missing.push("Customer name or phone on more sales, so more of the revenue can be analysed by customer.".into()); }
    }
    if n_cur == 0 { a.notes.push("No identified customers in this period.".into()); return a; }
    let weak_share = share.map(|s| s < 50.0).unwrap_or(true);
    let base_lim = |extra: Option<String>| { let mut v = vec![ident_note.clone()]; if let Some(e) = extra { v.push(e); } v };

    // 1. Concentration.
    if n_cur >= 4 && identified_rev > 0 {
        let mut v: Vec<(String, i64)> = d.cust_cur.values().map(|c| (c.name.clone(), c.revenue)).collect();
        v = top_by(v, |x| x.1, 3);
        let t1 = pct(v[0].1, identified_rev).unwrap_or(0.0);
        let t3 = pct(v.iter().map(|x| x.1).sum(), identified_rev).unwrap_or(0.0);
        if t1 >= 30.0 || t3 >= 60.0 {
            let mut i = new_insight(ctx, "customers", "customer_concentration", "A few customers account for most sales", "watch");
            i.what_happened = format!("{} brings {:.0}% of identified-customer revenue; the top three bring {:.0}%.", v[0].0, t1, t3);
            for (n, r) in &v { i.evidence.push(ev(n, ctx.money(*r))); }
            i.why_it_matters = "Losing one of them would remove a large share of sales.".into();
            i.impact = Some(Impact { amount_cents: v[0].1, basis: "Revenue from the largest customer this period".into() });
            i.recommended_action = "Look after these relationships and work on widening the customer base.".into();
            i.confidence = confidence(base_lim(None), weak_share, n_cur);
            a.insights.push(i);
        }
    }

    // 2. Customers who bought last period and not this one.
    if d.prev_ok && n_prev > 0 {
        let prev_rev: i64 = d.cust_prev.values().map(|c| c.revenue).sum();
        let mut lapsed: Vec<(String, i64)> = d.cust_prev.iter().filter(|(k, _)| !d.cust_cur.contains_key(*k)).map(|(_, c)| (c.name.clone(), c.revenue)).collect();
        let lapsed_n = lapsed.len() as i64;
        let lapsed_rev: i64 = lapsed.iter().map(|x| x.1).sum();
        if lapsed_n >= 3 && prev_rev > 0 && lapsed_rev as f64 >= prev_rev as f64 * 0.10 {
            lapsed = top_by(lapsed, |x| x.1, 5);
            let share_lost = pct(lapsed_rev, prev_rev).unwrap_or(0.0);
            let mut i = new_insight(ctx, "customers", "lapsed_customers", "Customers who stopped buying", if share_lost >= 30.0 { "problem" } else { "watch" });
            i.what_happened = format!("{} customer(s) who bought in the previous period have not bought in this one; they accounted for {:.0}% of that period's identified revenue.", lapsed_n, share_lost);
            for (n, r) in &lapsed { i.evidence.push(ev(n, format!("{} last period", ctx.money(*r)))); }
            i.comparison = Some(comparison(ctx, "Identified customers", "count", n_cur as f64, n_prev as f64));
            i.why_it_matters = "Existing customers are the cheapest sales to keep.".into();
            i.impact = Some(Impact { amount_cents: -lapsed_rev, basis: "Revenue these customers gave in the previous period".into() });
            i.recommended_action = "Contact the biggest ones to find out why, and offer a reason to come back.".into();
            let extra = if ctx.days < 30 { Some("A customer who normally buys less often than every period can look lapsed.".to_string()) } else { None };
            i.confidence = confidence(base_lim(extra), weak_share, lapsed_n);
            i.follow_up = Some(format!("Check next period: how many of these {} customers bought again?", lapsed_n));
            a.insights.push(i);
        }
    }

    // 3. Retention.
    if d.prev_ok && n_prev >= 5 {
        let retained = d.cust_prev.keys().filter(|k| d.cust_cur.contains_key(*k)).count() as i64;
        let rate = retained as f64 / n_prev as f64 * 100.0;
        if rate < 50.0 || rate >= 75.0 {
            let mut i = new_insight(ctx, "customers", "retention", if rate < 50.0 { "Fewer customers are coming back" } else { "Most customers are coming back" }, if rate < 50.0 { "watch" } else { "good" });
            i.what_happened = format!("{} of the {} customers from the previous period bought again this period ({:.0}%).", retained, n_prev, rate);
            i.evidence = vec![ev("Bought in both periods", retained.to_string()), ev("Bought only last period", (n_prev - retained).to_string()), ev("New this period", new_cur.to_string())];
            i.comparison = Some(comparison(ctx, "Identified customers", "count", n_cur as f64, n_prev as f64));
            i.why_it_matters = "Repeat buying is the steadiest source of revenue.".into();
            i.recommended_action = if rate < 50.0 { "Find out why customers don't return: stock gaps, price, service or competitors.".into() } else { "Keep doing what brings them back and note what it is.".into() };
            let extra = if ctx.days < 30 { Some("Over a short period, regular customers who buy less often than that look as if they left.".to_string()) } else { None };
            i.confidence = confidence(base_lim(extra), weak_share, n_prev);
            a.insights.push(i);
        }
    }
    a
}

// ---------------------------------------------------------------- finance

/// Entries smePro posts to Accounting by itself (purchases received,
/// refunds, debt settlements, repack rounding), recognised the way the
/// BRE agent profile recognises them: category AND the wording smePro
/// gives them. Everything else an owner types is an operating expense, so
/// a manual entry that merely uses the category "Purchasing" still counts.
const AUTO_POSTED_SQL: &str = "((LOWER(TRIM(COALESCE(category,''))) = 'purchasing' AND COALESCE(description,'') LIKE 'Purchase received — %') OR (LOWER(TRIM(COALESCE(category,''))) = 'refunds' AND COALESCE(description,'') LIKE 'Refund — %') OR (LOWER(TRIM(COALESCE(category,''))) = 'debt & credit' AND (COALESCE(description,'') LIKE 'Debt settled — %' OR COALESCE(description,'') LIKE 'Credit settled — %')) OR (LOWER(TRIM(COALESCE(category,''))) = 'stock revaluation' AND COALESCE(description,'') LIKE 'Repack rounding %'))";

fn acc_covers(ctx: &Ctx, acc: &str, exclusive_start: &str) -> bool {
    let first: Option<String> = ctx.conn
        .query_row(&format!("SELECT MIN(transaction_date) FROM {acc} WHERE business_id = ?1 AND deleted_at IS NULL AND transaction_date IS NOT NULL"), params![ctx.business_id], |r| r.get(0))
        .unwrap_or(None);
    match (first, day_offset(exclusive_start, 1)) {
        (Some(f), Ok(day)) => f.as_str() <= day.as_str(),
        _ => false,
    }
}

fn cash_totals(ctx: &Ctx, acc: &str, start: &str, end: &str) -> Result<(i64, i64, i64)> {
    let sql = format!(
        "SELECT COALESCE(SUM(CASE WHEN LOWER(TRIM(entry_type)) = 'income' THEN amount ELSE 0 END),0),
                COALESCE(SUM(CASE WHEN LOWER(TRIM(entry_type)) = 'expense' THEN amount ELSE 0 END),0), COUNT(*)
         FROM {acc} WHERE business_id = ?1 AND deleted_at IS NULL AND transaction_date > ?2 AND transaction_date <= ?3"
    );
    Ok(ctx.conn.query_row(&sql, params![ctx.business_id, start, end], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?)
}

fn exp_cats(ctx: &Ctx, acc: &str, start: &str, end: &str) -> Result<HashMap<String, (i64, i64)>> {
    let sql = format!(
        "SELECT COALESCE(NULLIF(TRIM(category),''), 'Uncategorised'), COALESCE(SUM(amount),0), COUNT(*)
         FROM {acc} WHERE business_id = ?1 AND deleted_at IS NULL AND LOWER(TRIM(entry_type)) = 'expense'
           AND transaction_date > ?2 AND transaction_date <= ?3 GROUP BY 1"
    );
    let mut stmt = ctx.conn.prepare(&sql)?;
    let rows = stmt.query_map(params![ctx.business_id, start, end], |r| Ok((r.get::<_, String>(0)?, (r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))))?;
    Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
}

fn expense_area(ctx: &Ctx, d: Option<&Data>, acc: &str) -> AreaReport {
    let mut a = area("expenses", "Expense Intelligence");
    let cur = opex_by_category(ctx, acc, &ctx.cur_start, &ctx.today).unwrap_or_default();
    let prev = opex_by_category(ctx, acc, &ctx.prev_start, &ctx.cur_start).unwrap_or_default();
    let (cur_sum, cur_n) = opex_total(&cur);
    let (prev_sum, _) = opex_total(&prev);
    let prev_ok = acc_covers(ctx, acc, &ctx.prev_start);
    let pv = |x: f64| if prev_ok { Some(x) } else { None };

    let auto = |category: &str, wording: &str, start: &str, end: &str| -> f64 {
        ctx.conn.query_row(
            &format!("SELECT COALESCE(SUM(amount),0) FROM {acc} WHERE business_id = ?1 AND deleted_at IS NULL AND LOWER(TRIM(entry_type)) = 'expense'
                      AND LOWER(TRIM(COALESCE(category,''))) = ?2 AND COALESCE(description,'') LIKE ?3 AND transaction_date > ?4 AND transaction_date <= ?5"),
            params![ctx.business_id, category, wording, start, end], |r| r.get::<_, i64>(0)).unwrap_or(0) as f64
    };

    let rev = d.map(|d| (d.cur.revenue, d.prev.revenue, d.prev_ok));
    a.kpis = vec![
        kpi("Operating expenses", "money", Some(cur_sum as f64), pv(prev_sum as f64)),
        kpi("Expenses as share of sales", "percent", rev.and_then(|r| pct(cur_sum, r.0)), rev.and_then(|r| if r.2 && prev_ok { pct(prev_sum, r.1) } else { None })),
        kpi("Refunds paid out", "money", Some(auto("refunds", "Refund — %", &ctx.cur_start, &ctx.today)), pv(auto("refunds", "Refund — %", &ctx.prev_start, &ctx.cur_start))),
        kpi("Supplier purchases paid", "money", Some(auto("purchasing", "Purchase received — %", &ctx.cur_start, &ctx.today)), pv(auto("purchasing", "Purchase received — %", &ctx.prev_start, &ctx.cur_start))),
    ];
    let cap = "Operating expenses are the expense entries recorded in Accounting other than those the app posts itself (purchases, refunds, debt settlement, stock revaluation).".to_string();

    // 5 (data). Sales continue but no operating expenses were ever recorded recently.
    let recent: i64 = ctx.conn.query_row(
        &format!("SELECT COUNT(*) FROM {acc} WHERE business_id = ?1 AND deleted_at IS NULL AND LOWER(TRIM(entry_type)) = 'expense' AND NOT {AUTO_POSTED_SQL} AND transaction_date > date(?2, '-90 days')"),
        params![ctx.business_id, ctx.today], |r| r.get(0)).unwrap_or(0);
    if recent == 0 {
        a.status = "partial".into();
        a.missing.push("No operating expenses (rent, wages, utilities and so on) were recorded in the last 90 days, so expense findings can't be produced.".into());
        if let Some(d) = d {
            if d.cur.rows >= 5 {
                let mut i = new_insight(ctx, "expenses", "expense_recording_gap", "No operating expenses recorded while sales continue", "watch");
                i.what_happened = "Sales were recorded this period, but no operating expenses have been entered in the last 90 days.".into();
                i.evidence = vec![ev("Sales recorded this period", d.cur.rows.to_string()), ev("Operating expense entries, last 90 days", "0".into())];
                i.why_it_matters = "Without expenses, profit looks higher than it really is and cost problems stay hidden.".into();
                i.recommended_action = "Record rent, wages, utilities and other running costs in Accounting.".into();
                i.confidence = confidence(vec!["The costs may be tracked somewhere outside this app.".into()], false, d.cur.rows);
                a.insights.push(i);
            }
        }
        return a;
    }

    let drivers = |ctx: &Ctx, up: bool| -> Vec<Evidence> {
        let mut v: Vec<(String, i64)> = cur.keys().chain(prev.keys()).collect::<std::collections::BTreeSet<_>>().into_iter()
            .map(|c| (c.clone(), cur.get(c).map(|x| x.0).unwrap_or(0) - prev.get(c).map(|x| x.0).unwrap_or(0)))
            .filter(|(_, x)| if up { *x > 0 } else { *x < 0 }).collect();
        v = top_by(v, |x| x.1.abs(), 3);
        v.into_iter().map(|(c, x)| ev(&format!("Biggest driver: {c}"), format!("{}{}", if x < 0 { "-" } else { "+" }, ctx.money(x.abs())))).collect()
    };

    // 1. Spending change.
    if prev_ok && prev_sum > 0 && cur_n >= 3 {
        if let Some(change) = pct_change(cur_sum as f64, prev_sum as f64) {
            if change >= 20.0 || change <= -30.0 {
                let up = change > 0.0;
                let mut i = new_insight(ctx, "expenses", "expense_change", if up { "Operating expenses rose" } else { "Operating expenses fell" }, if up { "watch" } else { "good" });
                i.what_happened = format!("Operating expenses were {} against {} in the previous period ({:+.1}%).", ctx.money(cur_sum), ctx.money(prev_sum), change);
                i.evidence = vec![ev("This period", ctx.money(cur_sum)), ev("Previous period", ctx.money(prev_sum))];
                i.evidence.extend(drivers(ctx, up));
                i.comparison = Some(comparison(ctx, "Operating expenses", "money", cur_sum as f64, prev_sum as f64));
                i.why_it_matters = if up { "Higher running costs cut into profit unless sales rise with them.".into() } else { "Lower costs improve profit if they don't hurt sales.".into() };
                i.impact = Some(Impact { amount_cents: cur_sum - prev_sum, basis: "Expense difference between the two periods".into() });
                i.recommended_action = if up { "Review the categories listed; separate one-off from recurring costs.".into() } else { "Confirm the saving is real and not an unrecorded or delayed bill.".into() };
                i.confidence = confidence(vec![cap.clone(), "Costs are counted on the date recorded; a late-entered bill moves between periods.".into()], false, cur_n);
                i.follow_up = Some(format!("Check next period: do expenses stay near {}?", ctx.money(cur_sum)));
                a.insights.push(i);
            }
        }
    }

    // 2. Expenses relative to sales.
    if let Some((r_cur, r_prev, r_ok)) = rev {
        if prev_ok && r_ok && r_cur > 0 && r_prev > 0 && prev_sum > 0 {
            let (rc, rp) = (cur_sum as f64 / r_cur as f64 * 100.0, prev_sum as f64 / r_prev as f64 * 100.0);
            if rc - rp >= 5.0 {
                let mut i = new_insight(ctx, "expenses", "expense_ratio", "Expenses are taking a bigger share of sales", "watch");
                i.what_happened = format!("Operating expenses were {:.1}% of sales, up from {:.1}% in the previous period.", rc, rp);
                i.evidence = vec![ev("Operating expenses", ctx.money(cur_sum)), ev("Sales", ctx.money(r_cur))];
                i.comparison = Some(comparison(ctx, "Expenses as share of sales", "percent", rc, rp));
                i.why_it_matters = "When costs grow faster than sales, each sale leaves less profit.".into();
                i.impact = Some(Impact { amount_cents: ((rc - rp) / 100.0 * r_cur as f64).round() as i64, basis: "Extra expense versus the previous period's share of sales".into() });
                i.recommended_action = "Find which cost grew without a matching rise in sales.".into();
                i.confidence = confidence(vec![cap.clone()], false, cur_n);
                a.insights.push(i);
            }
        }
    }

    // 3. Concentration in one category.
    if cur.len() >= 3 && cur_sum > 0 {
        let mut v: Vec<(String, i64)> = cur.iter().map(|(c, x)| (c.clone(), x.0)).collect();
        v = top_by(v, |x| x.1, 3);
        let share = pct(v[0].1, cur_sum).unwrap_or(0.0);
        if share >= 50.0 {
            let mut i = new_insight(ctx, "expenses", "expense_concentration", "One cost category dominates spending", "info");
            i.what_happened = format!("{} is {:.0}% of operating expenses this period.", v[0].0, share);
            for (c, x) in &v { i.evidence.push(ev(c, ctx.money(*x))); }
            i.why_it_matters = "The biggest category is where a saving or an increase matters most.".into();
            i.impact = Some(Impact { amount_cents: v[0].1, basis: "Spend in the largest category".into() });
            i.recommended_action = "Check whether this category can be renegotiated or reduced.".into();
            i.confidence = confidence(vec![cap.clone()], false, cur_n);
            a.insights.push(i);
        }
    }

    // 4. Unusually large single expenses.
    let hist_sql = format!(
        "SELECT amount FROM {acc} WHERE business_id = ?1 AND deleted_at IS NULL AND LOWER(TRIM(entry_type)) = 'expense'
           AND NOT {AUTO_POSTED_SQL}
           AND transaction_date > date(?2, '-180 days') AND transaction_date <= ?2 ORDER BY amount"
    );
    if let Ok(mut stmt) = ctx.conn.prepare(&hist_sql) {
        if let Ok(rows) = stmt.query_map(params![ctx.business_id, ctx.cur_start], |r| r.get::<_, i64>(0)) {
            let hist: Vec<i64> = rows.filter_map(|x| x.ok()).collect();
            if hist.len() >= 8 {
                let median = hist[hist.len() / 2];
                let cur_sql = format!(
                    "SELECT description, amount, transaction_date FROM {acc} WHERE business_id = ?1 AND deleted_at IS NULL AND LOWER(TRIM(entry_type)) = 'expense'
                       AND NOT {AUTO_POSTED_SQL}
                       AND transaction_date > ?2 AND transaction_date <= ?3 ORDER BY amount DESC LIMIT 5"
                );
                if median > 0 {
                    if let Ok(mut s2) = ctx.conn.prepare(&cur_sql) {
                        if let Ok(rs) = s2.query_map(params![ctx.business_id, ctx.cur_start, ctx.today], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?))) {
                            let big: Vec<(String, i64, String)> = rs.filter_map(|x| x.ok()).filter(|x| x.1 >= median * 3).collect();
                            if !big.is_empty() {
                                let total: i64 = big.iter().map(|x| x.1).sum();
                                let mut i = new_insight(ctx, "expenses", "unusual_expense", "Unusually large expenses", "watch");
                                i.what_happened = format!("{} expense(s) this period were at least three times the typical expense of {}.", big.len(), ctx.money(median));
                                for (desc, amt, date) in &big { i.evidence.push(ev(&format!("{desc} ({date})"), ctx.money(*amt))); }
                                i.why_it_matters = "A one-off or mistaken entry can distort profit for the whole period.".into();
                                i.impact = Some(Impact { amount_cents: total, basis: "Total of the unusually large expenses".into() });
                                i.recommended_action = "Confirm each is correct and whether it will repeat.".into();
                                i.confidence = confidence(vec![cap.clone(), format!("Typical expense is the median of {} entries from the previous 180 days.", hist.len())], false, hist.len() as i64);
                                a.insights.push(i);
                            }
                        }
                    }
                }
            }
        }
    }
    a
}

fn cash_area(ctx: &Ctx, acc: Option<&str>, debt: Option<&str>) -> AreaReport {
    let mut a = area("cash_flow", "Cash Flow Intelligence");
    a.status = "partial".into();
    a.missing.push("Opening cash or bank balance isn't recorded, so the cash position and how long cash will last can't be stated. Only cash movement is analysed.".into());
    let cap = "Cash movement comes from Accounting entries: credit sales appear when paid, and entries can only be as complete as what is recorded.".to_string();

    let mut net_cur = None;
    if let Some(acc) = acc {
        let (ci, co, n) = cash_totals(ctx, acc, &ctx.cur_start, &ctx.today).unwrap_or((0, 0, 0));
        let (pi, po, _) = cash_totals(ctx, acc, &ctx.prev_start, &ctx.cur_start).unwrap_or((0, 0, 0));
        let prev_ok = acc_covers(ctx, acc, &ctx.prev_start);
        let pv = |x: i64| if prev_ok { Some(x as f64) } else { None };
        a.kpis = vec![
            kpi("Cash in", "money", Some(ci as f64), pv(pi)),
            kpi("Cash out", "money", Some(co as f64), pv(po)),
            kpi("Net cash flow", "money", Some((ci - co) as f64), pv(pi - po)),
        ];
        net_cur = Some(ci - co);
        if n >= 3 {
            let net = ci - co;
            let net_prev = pi - po;
            let sev = if net < 0 { if prev_ok && net_prev < 0 { "problem" } else { "watch" } } else if prev_ok && net_prev > 0 && net as f64 >= net_prev as f64 * 1.2 { "good" } else { "" };
            if !sev.is_empty() {
                let mut i = new_insight(ctx, "cash_flow", "cash_flow_trend", if net < 0 { "More cash went out than came in" } else { "Cash generation improved" }, sev);
                i.what_happened = format!("Cash in was {} and cash out was {}, a net {} {}.", ctx.money(ci), ctx.money(co), if net < 0 { "outflow of" } else { "inflow of" }, ctx.money(net.abs()));
                i.evidence = vec![ev("Cash in", ctx.money(ci)), ev("Cash out", ctx.money(co))];
                if prev_ok { i.evidence.push(ev("Net, previous period", ctx.money(net_prev))); }
                if net < 0 {
                    let mut v: Vec<(String, i64)> = exp_cats(ctx, acc, &ctx.cur_start, &ctx.today).unwrap_or_default().into_iter().map(|(c, x)| (c, x.0)).collect();
                    v = top_by(v, |x| x.1, 3);
                    for (c, x) in v { i.evidence.push(ev(&format!("Largest outflow: {c}"), ctx.money(x))); }
                }
                if prev_ok { i.comparison = Some(comparison(ctx, "Net cash flow", "money", net as f64, net_prev as f64)); }
                i.why_it_matters = if net < 0 { "Spending more cash than comes in drains the business unless it is a planned investment.".into() } else { "Stronger cash generation gives room to pay suppliers and invest.".into() };
                i.impact = Some(Impact { amount_cents: net, basis: "Cash in minus cash out this period".into() });
                i.recommended_action = if net < 0 { "Check the largest outflows, chase unpaid customer debts, and time supplier payments.".into() } else { "Keep the discipline that produced it and set aside a buffer.".into() };
                i.confidence = confidence(vec![cap.clone()], false, n);
                a.insights.push(i);
            }
        }
    }

    if let Some(debt) = debt {
        if let Ok(sum) = debt_settlement::summary(ctx.conn, ctx.business_id, ctx.user_id, &ctx.today) {
            a.kpis.push(kpi("Owed to you (unpaid)", "money", Some(sum.owed_to_business_unpaid as f64), None));
            a.kpis.push(kpi("You owe (unpaid)", "money", Some(sum.owed_by_business_unpaid as f64), None));
            // Overdue customer debts (existing aging buckets).
            if let Ok(ag) = debt_settlement::aging_buckets(ctx.conn, ctx.business_id, ctx.user_id, &ctx.today) {
                if ag.total_overdue_amount > 0 {
                    let share = pct(ag.total_overdue_amount, sum.owed_to_business_unpaid).unwrap_or(0.0);
                    let old = ag.bucket_61_90_amount + ag.bucket_90_plus_amount;
                    let mut i = new_insight(ctx, "cash_flow", "receivables_overdue", "Customer debts are overdue", if ag.bucket_90_plus_amount > 0 { "problem" } else { "watch" });
                    i.what_happened = format!("{} owed by customers is past its due date ({:.0}% of everything owed to you).", ctx.money(ag.total_overdue_amount), share);
                    i.evidence = vec![
                        ev("1-30 days overdue", ctx.money(ag.bucket_1_30_amount)),
                        ev("31-60 days overdue", ctx.money(ag.bucket_31_60_amount)),
                        ev("61-90 days overdue", ctx.money(ag.bucket_61_90_amount)),
                        ev("Over 90 days overdue", ctx.money(ag.bucket_90_plus_amount)),
                    ];
                    i.why_it_matters = "Money you've already earned but can't spend; the older it gets, the less likely it is collected.".into();
                    i.impact = Some(Impact { amount_cents: ag.total_overdue_amount, basis: "Total overdue customer debt".into() });
                    i.recommended_action = if old > 0 { "Follow up the debts over 60 days first and consider stopping further credit to those customers.".into() } else { "Send reminders now, before the debts age further.".into() };
                    i.confidence = confidence(vec!["Debts with no due date can't be judged overdue and are left out.".into()], false, ag.total_overdue_count);
                    i.follow_up = Some("Re-check next period: has the overdue total fallen?".into());
                    a.insights.push(i);
                }
            }
            // Supplier payments due against cash generated.
            let due: (i64, i64, i64) = ctx.conn.query_row(
                &format!("SELECT COALESCE(SUM(amount),0), COUNT(*), COALESCE(SUM(CASE WHEN due_date < ?2 THEN amount ELSE 0 END),0) FROM {debt}
                          WHERE business_id = ?1 AND deleted_at IS NULL AND settled = 0 AND direction != 'owed_to_business'
                            AND due_date IS NOT NULL AND due_date <= date(?2, '+14 days')"),
                params![ctx.business_id, ctx.today], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap_or((0, 0, 0));
            if due.0 > 0 {
                let short = net_cur.map(|n| n < due.0).unwrap_or(false);
                if short || due.2 > 0 {
                    let mut i = new_insight(ctx, "cash_flow", "payables_due", "Payments you owe are due soon", if due.2 > 0 { "problem" } else { "watch" });
                    i.what_happened = format!("{} you owe is overdue or due within 14 days ({} payment(s), {} already overdue).", ctx.money(due.0), due.1, ctx.money(due.2));
                    i.evidence = vec![ev("Due or overdue", ctx.money(due.0)), ev("Already overdue", ctx.money(due.2))];
                    if let Some(n) = net_cur { i.evidence.push(ev("Net cash flow this period", ctx.money(n))); }
                    i.why_it_matters = "Late supplier payments can end credit terms or supply.".into();
                    i.impact = Some(Impact { amount_cents: due.0, basis: "Debts you owe that are due within 14 days or overdue".into() });
                    i.recommended_action = "Plan these payments against expected receipts and chase customer debts to fund them.".into();
                    i.confidence = confidence(vec!["No cash balance is recorded, so whether the money is available can't be confirmed.".into()], false, due.1);
                    a.insights.push(i);
                }
            }
        }
    }
    if a.kpis.is_empty() { a.status = "unavailable".into(); a.missing.insert(0, "Neither Accounting nor Debt & Credit is available to you.".into()); }
    a
}

// ------------------------------------------------- performance and growth

fn refunds_total(ctx: &Ctx, table: &str, start: &str, end: &str) -> i64 {
    ctx.conn.query_row(
        &format!("SELECT COALESCE(SUM(refund_amount),0) FROM {table} WHERE business_id = ?1 AND deleted_at IS NULL AND date(created_at) > ?2 AND date(created_at) <= ?3"),
        params![ctx.business_id, start, end], |r| r.get(0)).unwrap_or(0)
}

struct Metric { label: &'static str, unit: &'static str, cur: f64, prev: f64, higher_is_better: bool }

fn performance_area(ctx: &Ctx, d: &Data, acc: Option<&str>, refunds: Option<&str>) -> AreaReport {
    let mut a = area("performance", "Business Performance Intelligence");
    a.status = "partial".into();
    a.missing.push("No business targets are recorded, so performance against a target can't be assessed; results are compared with the previous equal period instead.".into());
    if !d.prev_ok {
        a.notes.push(history_limitation(d, ctx).unwrap_or_default());
        return a;
    }
    let mut m = vec![
        Metric { label: "Revenue", unit: "money", cur: d.cur.revenue as f64, prev: d.prev.revenue as f64, higher_is_better: true },
        Metric { label: "Orders", unit: "count", cur: d.cur.orders as f64, prev: d.prev.orders as f64, higher_is_better: true },
    ];
    if let (Some(c), Some(p)) = (d.cur.aov(), d.prev.aov()) { m.push(Metric { label: "Average order value", unit: "money", cur: c, prev: p, higher_is_better: true }); }
    let reliable = d.cur.cost_coverage().map(|c| c >= 80.0).unwrap_or(false) && d.prev.cost_coverage().map(|c| c >= 80.0).unwrap_or(false);
    if reliable {
        m.push(Metric { label: "Gross profit", unit: "money", cur: d.cur.profit() as f64, prev: d.prev.profit() as f64, higher_is_better: true });
        if let (Some(c), Some(p)) = (d.cur.margin(), d.prev.margin()) { m.push(Metric { label: "Gross margin", unit: "percent", cur: c, prev: p, higher_is_better: true }); }
    } else {
        a.notes.push("Profit measures are left out because under 80% of sales revenue has a recorded cost in one of the periods.".into());
    }
    if let Some(t) = acc {
        if acc_covers(ctx, t, &ctx.prev_start) {
            let (ci, co, _) = cash_totals(ctx, t, &ctx.cur_start, &ctx.today).unwrap_or((0, 0, 0));
            let (pi, po, _) = cash_totals(ctx, t, &ctx.prev_start, &ctx.cur_start).unwrap_or((0, 0, 0));
            let (oc, _) = opex_total(&opex_by_category(ctx, t, &ctx.cur_start, &ctx.today).unwrap_or_default());
            let (op, _) = opex_total(&opex_by_category(ctx, t, &ctx.prev_start, &ctx.cur_start).unwrap_or_default());
            if op > 0 { m.push(Metric { label: "Operating expenses", unit: "money", cur: oc as f64, prev: op as f64, higher_is_better: false }); }
            m.push(Metric { label: "Net cash flow", unit: "money", cur: (ci - co) as f64, prev: (pi - po) as f64, higher_is_better: true });
        }
    }
    let mut refund_pair: Option<(i64, i64)> = None;
    if let Some(t) = refunds {
        let rc = refunds_total(ctx, t, &ctx.cur_start, &ctx.today);
        let rp = refunds_total(ctx, t, &ctx.prev_start, &ctx.cur_start);
        refund_pair = Some((rc, rp));
        if let (Some(c), Some(p)) = (pct(rc, d.cur.revenue + rc), pct(rp, d.prev.revenue + rp)) {
            m.push(Metric { label: "Refunds as share of sales", unit: "percent", cur: c, prev: p, higher_is_better: false });
        }
    }

    let (mut better, mut worse, mut flat) = (0i64, 0i64, 0i64);
    let mut lines = vec![];
    for x in &m {
        let moved = if x.unit == "percent" { x.cur - x.prev } else { pct_change(x.cur, x.prev).unwrap_or(0.0) };
        let threshold = if x.unit == "percent" { 1.0 } else { 5.0 };
        let good = if x.higher_is_better { moved } else { -moved };
        if good >= threshold { better += 1 } else if good <= -threshold { worse += 1 } else { flat += 1 }
        let fmtv = |v: f64| match x.unit { "money" => ctx.money(v.round() as i64), "percent" => format!("{:.1}%", v), _ => format!("{:.0}", v) };
        lines.push(ev(x.label, format!("{} vs {} ({})", fmtv(x.cur), fmtv(x.prev), if x.unit == "percent" { format!("{:+.1} pts", moved) } else { format!("{:+.1}%", moved) })));
    }
    a.kpis = vec![
        kpi("Measures improved", "count", Some(better as f64), None),
        kpi("Measures worsened", "count", Some(worse as f64), None),
        kpi("Measures about level", "count", Some(flat as f64), None),
    ];

    if m.len() >= 3 {
        let sev = if worse > better { if worse >= 3 && better == 0 { "problem" } else { "watch" } } else if better > worse { "good" } else { "info" };
        let mut i = new_insight(ctx, "performance", "performance_scorecard", match sev { "good" => "Business performance improved", "info" => "Business performance is mixed", _ => "Business performance weakened" }, sev);
        i.what_happened = format!("Of {} measures compared with the previous period, {} improved, {} worsened and {} stayed about level.", m.len(), better, worse, flat);
        i.evidence = lines;
        i.comparison = Some(comparison(ctx, "Revenue", "money", d.cur.revenue as f64, d.prev.revenue as f64));
        i.why_it_matters = "One view across sales, profit, costs, cash and returns shows whether the business is moving the right way overall.".into();
        i.recommended_action = if worse > better { "Start with the measures that worsened most; the other areas in this report show the causes.".into() } else { "Keep what is working and check the measures that slipped.".into() };
        let mut lim = vec!["Compared with the previous equal period, not against targets (none are recorded).".to_string()];
        if let Some(l) = weekday_limitation(ctx) { lim.push(l); }
        i.confidence = confidence(lim, false, d.cur.rows.min(d.prev.rows));
        i.follow_up = Some(format!("Re-run after the next {} days to see whether the weaker measures recovered.", ctx.days));
        a.insights.push(i);
    }

    if let Some((rc, rp)) = refund_pair {
        if let (Some(c), Some(p)) = (pct(rc, d.cur.revenue + rc), pct(rp, d.prev.revenue + rp)) {
            if rc > 0 && c - p >= 2.0 {
                let mut i = new_insight(ctx, "performance", "refund_share", "Refunds are taking a bigger share of sales", "watch");
                i.what_happened = format!("Refunds were {:.1}% of sales value, up from {:.1}% in the previous period.", c, p);
                i.evidence = vec![ev("Refunds this period", ctx.money(rc)), ev("Refunds previous period", ctx.money(rp))];
                i.comparison = Some(comparison(ctx, "Refunds as share of sales", "percent", c, p));
                i.why_it_matters = "More returns point to product, description or service problems and reduce revenue.".into();
                i.impact = Some(Impact { amount_cents: rc - rp, basis: "Extra refunds versus the previous period".into() });
                i.recommended_action = "Check the Refunds report for the items and reasons behind the rise.".into();
                i.confidence = confidence(vec!["Refunds are counted when issued, sales when sold, so a refund can belong to an earlier period's sale.".into()], false, d.cur.rows);
                a.insights.push(i);
            }
        }
    }
    a
}

fn growth_area(ctx: &Ctx, d: &Data) -> AreaReport {
    let mut a = area("growth", "Business Growth Intelligence");
    a.status = "partial".into();
    a.missing.push("Growth drivers beyond sales history (marketing spend, capacity, targets) aren't recorded, so growth is described from sales, customers and products only.".into());
    let (c, p, p2) = (d.cur.revenue, d.prev.revenue, d.prev2.revenue);
    let g1 = if d.prev_ok { pct_change(c as f64, p as f64) } else { None };
    let new_prod: Vec<(&String, i64)> = if d.prev_ok { d.items_cur.iter().filter(|(n, _)| d.item_first.get(*n).map(|f| f.as_str() > ctx.cur_start.as_str()).unwrap_or(false)).map(|(n, i)| (n, i.revenue)).collect() } else { vec![] };
    let new_cust_cur = d.cust_cur.keys().filter(|k| d.cust_first.get(*k).map(|f| f.as_str() > ctx.cur_start.as_str()).unwrap_or(false)).count() as i64;
    a.kpis = vec![
        kpi("Revenue growth vs previous period", "percent", g1, None),
        kpi("New customers", "count", Some(new_cust_cur as f64), None),
        kpi("Products first sold this period", "count", if d.prev_ok { Some(new_prod.len() as f64) } else { None }, None),
    ];
    if !d.prev_ok { a.notes.push(history_limitation(d, ctx).unwrap_or_default()); return a; }

    // 1. Direction of revenue over three consecutive periods.
    if d.prev2_ok && p > 0 && p2 > 0 {
        if let (Some(now), Some(before)) = (pct_change(c as f64, p as f64), pct_change(p as f64, p2 as f64)) {
            let (sev, title) = if now >= 5.0 && before >= 5.0 { ("good", "Sales are growing period after period") }
                else if now <= -5.0 && before <= -5.0 { ("problem", "Sales have fallen two periods in a row") }
                else if now.abs() >= 10.0 && (now > 0.0) != (before > 0.0) { ("info", "The direction of sales growth changed") }
                else { ("", "") };
            if !sev.is_empty() {
                let mut i = new_insight(ctx, "growth", "revenue_growth", title, sev);
                i.what_happened = format!("Revenue moved {:+.1}% this period after {:+.1}% in the one before.", now, before);
                i.evidence = vec![ev("This period", ctx.money(c)), ev("Previous period", ctx.money(p)), ev("Period before that", ctx.money(p2))];
                i.comparison = Some(comparison(ctx, "Revenue", "money", c as f64, p as f64));
                i.why_it_matters = "Two periods in a row separates a real trend from one good or bad stretch.".into();
                i.impact = Some(Impact { amount_cents: c - p, basis: "Revenue change versus the previous period".into() });
                i.recommended_action = match sev { "good" => "Make sure stock, staff and cash can support continued growth.".into(), "problem" => "Treat this as a trend, not a blip: review pricing, stock gaps and customer losses.".into(), _ => "Find what changed between the periods before assuming it will continue.".into() };
                let mut lim = vec![];
                if let Some(l) = weekday_limitation(ctx) { lim.push(l); }
                i.confidence = confidence(lim, false, d.cur.rows.min(d.prev.rows).min(d.prev2.rows));
                i.follow_up = Some(format!("Re-run after the next {} days to see whether the trend holds.", ctx.days));
                a.insights.push(i);
            }
        }
    }

    // 2. Products growing fast.
    if c > 0 {
        let mut grow: Vec<(String, i64)> = d.items_cur.iter().filter_map(|(n, i)| {
            let before = d.items_prev.get(n).map(|x| x.revenue).unwrap_or(0);
            let delta = i.revenue - before;
            if before > 0 && i.revenue as f64 >= before as f64 * 1.3 && i.revenue as f64 >= c as f64 * 0.03 && delta as f64 >= c as f64 * 0.01 { Some((n.clone(), delta)) } else { None }
        }).collect();
        if !grow.is_empty() {
            grow = top_by(grow, |x| x.1, 5);
            let total: i64 = grow.iter().map(|x| x.1).sum();
            let mut i = new_insight(ctx, "growth", "growing_products", "Products growing fastest", "good");
            i.what_happened = format!("{} product(s) grew at least 30% and now matter to total sales.", grow.len());
            for (n, x) in &grow { i.evidence.push(ev(n, format!("+{}", ctx.money(*x)))); }
            i.why_it_matters = "These are where growth is coming from.".into();
            i.impact = Some(Impact { amount_cents: total, basis: "Extra revenue on these products versus the previous period".into() });
            i.recommended_action = "Keep them in stock and check their margin so growth adds profit, not just volume.".into();
            i.confidence = confidence(vec!["Products are matched by name.".into()], false, d.cur.rows.min(d.prev.rows));
            a.insights.push(i);
        }
    }

    // 3. New products contributing.
    let new_rev: i64 = new_prod.iter().map(|x| x.1).sum();
    if !new_prod.is_empty() && c > 0 && new_rev as f64 >= c as f64 * 0.02 {
        let mut v: Vec<(String, i64)> = new_prod.iter().map(|(n, r)| ((*n).clone(), *r)).collect();
        v = top_by(v, |x| x.1, 5);
        let mut i = new_insight(ctx, "growth", "new_products", "New products are contributing", "good");
        i.what_happened = format!("{} product(s) sold for the first time this period and brought {:.0}% of revenue.", new_prod.len(), pct(new_rev, c).unwrap_or(0.0));
        for (n, r) in &v { i.evidence.push(ev(n, ctx.money(*r))); }
        i.why_it_matters = "A widening range is a source of growth that doesn't depend on existing lines.".into();
        i.impact = Some(Impact { amount_cents: new_rev, basis: "Revenue from products first sold this period".into() });
        i.recommended_action = "Back the ones that sell and review the ones that don't.".into();
        i.confidence = confidence(vec!["A renamed product counts as new.".into()], false, d.cur.rows);
        a.insights.push(i);
    }

    // 4. New customers versus the previous period.
    if d.prev2_ok {
        let new_prev = d.cust_prev.keys().filter(|k| d.cust_first.get(*k).map(|f| f.as_str() > ctx.prev_start.as_str() && f.as_str() <= ctx.cur_start.as_str()).unwrap_or(false)).count() as i64;
        if new_cust_cur >= 3 && new_prev >= 3 {
            if let Some(ch) = pct_change(new_cust_cur as f64, new_prev as f64) {
                if ch >= 30.0 || ch <= -30.0 {
                    let up = ch > 0.0;
                    let mut i = new_insight(ctx, "growth", "customer_growth", if up { "More new customers than last period" } else { "Fewer new customers than last period" }, if up { "good" } else { "watch" });
                    i.what_happened = format!("{} new customers this period against {} in the previous one ({:+.0}%).", new_cust_cur, new_prev, ch);
                    i.evidence = vec![ev("New customers this period", new_cust_cur.to_string()), ev("New customers previous period", new_prev.to_string())];
                    i.comparison = Some(comparison(ctx, "New customers", "count", new_cust_cur as f64, new_prev as f64));
                    i.why_it_matters = "New customers are what grows the base beyond existing buyers.".into();
                    i.recommended_action = if up { "Find out where they came from and do more of it.".into() } else { "Review what brought customers in before, and whether it stopped.".into() };
                    i.confidence = confidence(vec!["Customers are identified by phone number, otherwise by name; sales with neither aren't counted.".into()], false, new_cust_cur + new_prev);
                    a.insights.push(i);
                }
            }
        }
    }

    // 5. Straight-line outlook from the last three periods.
    if d.prev2_ok && c > 0 && p > 0 && p2 > 0 {
        let avg = (c + p + p2) / 3;
        let (lo, hi) = (c.min(p).min(p2), c.max(p).max(p2));
        let mut i = new_insight(ctx, "growth", "revenue_outlook", "Revenue outlook if the recent pace holds", "info");
        i.what_happened = format!("If the next {} days look like the average of the last three periods, revenue would be about {}, within {} to {}.", ctx.days, ctx.money(avg), ctx.money(lo), ctx.money(hi));
        i.evidence = vec![ev("Last three periods, average", ctx.money(avg)), ev("Lowest of the three", ctx.money(lo)), ev("Highest of the three", ctx.money(hi))];
        i.why_it_matters = "A plain baseline to plan stock and cash against, and to judge whether the next period beat it.".into();
        i.recommended_action = "Plan stock and expenses against the low end; treat anything above the average as upside.".into();
        i.confidence = confidence(vec!["A simple average of three periods: it ignores seasonality, price changes and stock shortages. It is an outlook, not a promise.".into()], false, d.cur.rows.min(d.prev.rows).min(d.prev2.rows));
        i.follow_up = Some(format!("Compare the next {}-day result with {}.", ctx.days, ctx.money(avg)));
        a.insights.push(i);
    }
    a
}

// ------------------------------------------------------------------ report

fn severity_rank(s: &str) -> u8 {
    match s { "problem" => 0, "watch" => 1, "info" => 2, _ => 3 }
}

/// Builds the full intelligence report for the `days` ending `today`
/// (clamped to 7..=180), compared with the equal period before it.
/// Read-only. Callers are expected to have checked Reports access, as the
/// other analytics routes do.
pub fn report(conn: &Connection, business_id: &str, user_id: &str, today: &str, days: i64) -> Result<IntelligenceReport> {
    let days = days.clamp(7, 180);
    let currency: String = conn
        .query_row("SELECT currency FROM businesses WHERE id = ?1", params![business_id], |r| r.get(0))
        .map_err(|_| anyhow!("business not found"))?;
    let cur_start = day_offset(today, -days)?;
    let prev_start = day_offset(today, -2 * days)?;
    let prev2_start = day_offset(today, -3 * days)?;
    let range = |label: &str, excl_start: &str, end: &str| -> Result<PeriodRange> {
        Ok(PeriodRange { label: label.into(), start: day_offset(excl_start, 1)?, end: end.into() })
    };
    let period = range(&format!("Last {days} days"), &cur_start, today)?;
    let prev_period = range(&format!("Previous {days} days"), &prev_start, &cur_start)?;
    let ctx = Ctx {
        conn, business_id, user_id, today: today.to_string(), days, currency,
        cur_start, prev_start, prev2_start, period: period.clone(), prev_period: prev_period.clone(),
    };

    let sales_table = table_for(&ctx, "sales", "Sales");
    let data: std::result::Result<Data, String> = match &sales_table {
        Ok(t) => load_data(&ctx, t).map_err(|_| "Sales data couldn't be read.".to_string()),
        Err(reason) => Err(reason.clone()),
    };
    let acc = table_for(&ctx, "accounting", "Accounting");
    let debt = table_for(&ctx, "debt_credit", "Debt & Credit");
    let refunds = table_for(&ctx, "refunds", "Refunds");
    let no_sales = |id: &str, title: &str, why: &str| unavailable(id, title, why.to_string());

    let mut areas = vec![
        match &data { Ok(d) => sales_area(&ctx, d), Err(why) => no_sales("sales", "Sales Intelligence", why) },
        match &data { Ok(d) => profit_area(&ctx, d, acc.as_deref().ok()), Err(why) => no_sales("profitability", "Profitability Intelligence", why) },
        inventory_area(&ctx, data.as_ref().ok()),
        match &data { Ok(d) => customer_area(&ctx, d), Err(why) => no_sales("customers", "Customer Intelligence", why) },
        match &acc { Ok(t) => expense_area(&ctx, data.as_ref().ok(), t), Err(why) => no_sales("expenses", "Expense Intelligence", why) },
        cash_area(&ctx, acc.as_deref().ok(), debt.as_deref().ok()),
        match &data { Ok(d) => performance_area(&ctx, d, acc.as_deref().ok(), refunds.as_deref().ok()), Err(why) => no_sales("performance", "Business Performance Intelligence", why) },
        match &data { Ok(d) => growth_area(&ctx, d), Err(why) => no_sales("growth", "Business Growth Intelligence", why) },
    ];
    for a in areas.iter_mut() {
        a.insights.sort_by_key(|i| severity_rank(&i.severity));
    }
    Ok(IntelligenceReport { as_of: today.to_string(), currency: ctx.currency.clone(), period, previous_period: prev_period, areas })
}
