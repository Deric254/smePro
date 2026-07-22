use anyhow::{anyhow, Result};
use rusqlite::Connection;
use serde::Serialize;

use crate::module::ModuleDef;
use crate::rbac;

#[derive(Debug, Clone, Copy)]
pub enum Aggregation {
    Sum,
    Count,
    Avg,
}

impl Aggregation {
    fn parse(s: &str) -> Result<Self> {
        match s {
            "sum" => Ok(Aggregation::Sum),
            "count" => Ok(Aggregation::Count),
            "avg" => Ok(Aggregation::Avg),
            other => Err(anyhow!("unknown aggregation '{other}', expected sum/count/avg")),
        }
    }
    fn sql_fn(&self, measure_col: &str) -> String {
        match self {
            Aggregation::Sum => format!("SUM({measure_col})"),
            Aggregation::Count => "COUNT(*)".to_string(),
            Aggregation::Avg => format!("AVG({measure_col})"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum TimeBucket {
    Day,
    Week,
    Month,
    Quarter,
    Year,
}

impl TimeBucket {
    fn parse(s: &str) -> Result<Self> {
        match s {
            "day" => Ok(TimeBucket::Day),
            "week" => Ok(TimeBucket::Week),
            "month" => Ok(TimeBucket::Month),
            "quarter" => Ok(TimeBucket::Quarter),
            "year" => Ok(TimeBucket::Year),
            other => Err(anyhow!("unknown time bucket '{other}', expected day/week/month/quarter/year")),
        }
    }

    /// Returns a SQL expression that buckets `time_col` into this
    /// granularity as a sortable, human-readable label.
    fn sql_expr(&self, time_col: &str) -> String {
        match self {
            TimeBucket::Day => format!("strftime('%Y-%m-%d', {time_col})"),
            TimeBucket::Week => format!("strftime('%Y-W%W', {time_col})"),
            TimeBucket::Month => format!("strftime('%Y-%m', {time_col})"),
            TimeBucket::Year => format!("strftime('%Y', {time_col})"),
            // SQLite has no native quarter bucket — derive it: (month-1)/3 + 1
            TimeBucket::Quarter => format!(
                "strftime('%Y', {time_col}) || '-Q' || ((CAST(strftime('%m', {time_col}) AS INTEGER) - 1) / 3 + 1)"
            ),
        }
    }
}

/// What to group the aggregated measure by: nothing (one grand total),
/// a time bucket over a date/datetime field, or any other field treated
/// as a plain category (e.g. group revenue by `category` or `branch`).
pub enum Dimension<'a> {
    None,
    Time { field: &'a str, bucket: TimeBucket },
    Category { field: &'a str },
}

#[derive(Debug, Serialize)]
pub struct ReportPoint {
    pub label: String,
    pub value: f64,
}

/// Runs a report: SUM/COUNT/AVG of `measure_field` (ignored for Count),
/// optionally grouped by a time bucket or category field, optionally
/// filtered to a date range on `time_field`. Every piece of this is
/// derived from the module's own field list at request time — this
/// function has never heard of "inventory" or "sales" specifically,
/// which is what lets one reporting engine serve every module.
pub fn run(
    conn: &Connection,
    business_id: &str,
    user_id: &str,
    module_id: &str,
    measure_field: Option<&str>,
    aggregation: &str,
    dimension: Dimension,
    range_start: Option<&str>,
    range_end: Option<&str>,
) -> Result<Vec<ReportPoint>> {
    rbac::require(conn, user_id, module_id, "read")?;

    let schema_json: String = conn.query_row(
        "SELECT schema_json FROM modules WHERE business_id = ?1 AND id = ?2 AND enabled = 1",
        rusqlite::params![business_id, module_id],
        |r| r.get(0),
    ).map_err(|_| anyhow!("module '{module_id}' is not enabled for this business"))?;
    let module = ModuleDef::from_json_str(&schema_json)?;
    let table = module.table_name();

    let field_names: std::collections::HashSet<&str> =
        module.fields.iter().map(|f| f.name.as_str()).collect();

    let agg = Aggregation::parse(aggregation)?;
    let measure_col = match aggregation {
        "count" => "*".to_string(),
        _ => {
            let m = measure_field.ok_or_else(|| anyhow!("'measure' is required for sum/avg"))?;
            if !field_names.contains(m) {
                return Err(anyhow!("'{m}' is not a field on module '{module_id}'"));
            }
            let ty = &module.fields.iter().find(|f| f.name == m).unwrap().field_type;
            if ty != "integer" && ty != "real" {
                return Err(anyhow!("field '{m}' is not numeric, cannot aggregate with {aggregation}"));
            }
            m.to_string()
        }
    };
    let agg_expr = agg.sql_fn(&measure_col);

    // Validate any field used in the dimension actually exists.
    if let Dimension::Time { field, .. } = &dimension {
        if *field != "created_at" && *field != "updated_at" && !field_names.contains(field) {
            return Err(anyhow!("'{field}' is not a field on module '{module_id}'"));
        }
    }
    if let Dimension::Category { field } = &dimension {
        if !field_names.contains(field) {
            return Err(anyhow!("'{field}' is not a field on module '{module_id}'"));
        }
    }

    let mut where_clauses = vec!["business_id = ?1".to_string(), "deleted_at IS NULL".to_string()];
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(business_id.to_string())];

    if let Dimension::Time { field, .. } = &dimension {
        if let Some(start) = range_start {
            params.push(Box::new(start.to_string()));
            where_clauses.push(format!("{field} >= ?{}", params.len()));
        }
        if let Some(end) = range_end {
            params.push(Box::new(end.to_string()));
            where_clauses.push(format!("{field} <= ?{}", params.len()));
        }
    }

    let (select_label, group_by, order_by) = match &dimension {
        Dimension::None => ("'total'".to_string(), String::new(), String::new()),
        Dimension::Time { field, bucket } => {
            let expr = bucket.sql_expr(field);
            (expr.clone(), format!("GROUP BY {expr}"), format!("ORDER BY {expr}"))
        }
        Dimension::Category { field } => (
            field.to_string(),
            format!("GROUP BY {field}"),
            format!("ORDER BY {agg_expr} DESC"),
        ),
    };

    let sql = format!(
        "SELECT {select_label} AS label, {agg_expr} AS value FROM {table} WHERE {} {} {}",
        where_clauses.join(" AND "),
        group_by,
        order_by
    );

    let mut stmt = conn.prepare(&sql)?;
    let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
    let rows = stmt.query_map(params_refs.as_slice(), |row| {
        let label: String = row.get(0)?;
        let value: f64 = row.get(1)?;
        Ok(ReportPoint { label, value })
    })?;

    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
}

pub fn parse_time_bucket(s: &str) -> Result<TimeBucket> {
    TimeBucket::parse(s)
}
