use anyhow::{anyhow, Result};
use rusqlite::Connection;
use serde::Serialize;

use crate::report::{self, Dimension, ReportPoint};

#[derive(Debug, Serialize)]
pub struct ForecastResult {
    pub history: Vec<ReportPoint>,
    pub forecast_next: f64,
    pub method: String,
}

/// Forecast the next period of a time-bucketed measure using a simple
/// moving average over the last `window` periods. This deliberately does
/// NOT ask an LLM to "do the math" — statistical forecasting is done
/// here in plain arithmetic, which is reliable and auditable. The AI's
/// job (see ai_assistant.rs) is to explain this number in plain language,
/// not compute it.
pub fn moving_average_forecast(
    conn: &Connection,
    business_id: &str,
    user_id: &str,
    module_id: &str,
    measure: &str,
    bucket: &str,
    window: usize,
) -> Result<ForecastResult> {
    // THE BUG THIS FIXES: window=0 used to slice `recent` down to an
    // empty window and then divide its sum by its own (zero) length —
    // a silent 0.0/0.0 = NaN that serde_json then serializes as a
    // bare `null` in the API response, with no error anywhere in the
    // chain. A caller reading `forecast_next: null` has no way to
    // tell that from "no data yet" vs. "you asked for a nonsensical
    // window." Rejected up front instead, before any division
    // happens, so the response is always either a real number or an
    // explicit error — never a silently-degenerate null.
    if window == 0 {
        return Err(anyhow!("'window' must be at least 1"));
    }
    let time_bucket = report::parse_time_bucket(bucket)?;
    let history = report::run(
        conn,
        business_id,
        user_id,
        module_id,
        report::ReportQuery {
            measure_field: Some(measure),
            aggregation: "sum",
            dimension: Dimension::Time { field: "created_at", bucket: time_bucket },
            range_start: None,
            range_end: None,
        },
    )?;

    let values: Vec<f64> = history.iter().map(|p| p.value).collect();
    let forecast = if values.is_empty() {
        0.0
    } else {
        let take = window.min(values.len());
        let recent = &values[values.len() - take..];
        recent.iter().sum::<f64>() / recent.len() as f64
    };

    Ok(ForecastResult {
        history,
        forecast_next: round2(forecast),
        method: format!("moving_average(window={window})"),
    })
}

/// Forecast using exponential smoothing (weights recent periods more
/// heavily than a flat moving average — better for data with a trend).
/// alpha in (0,1]: higher = more weight on the most recent period.
pub fn exponential_smoothing_forecast(
    conn: &Connection,
    business_id: &str,
    user_id: &str,
    module_id: &str,
    measure: &str,
    bucket: &str,
    alpha: f64,
) -> Result<ForecastResult> {
    // Same class of fix as moving_average_forecast's window==0 guard
    // just above: `alpha` reaches here straight from a query-string
    // `.parse::<f64>()`, which happily accepts "nan" and "inf" as
    // valid floats, and would otherwise accept 0 or a negative value
    // as "valid" input too. None of those are numerically nonsensical
    // in a way `s * v + (1.0 - s) * s` would ever surface as an
    // error — a NaN alpha poisons every future period once it enters
    // the running `s`, silently turning into another `null` in the
    // response (see the window==0 comment for why serde_json's
    // NaN-to-null behavior makes that failure mode particularly easy
    // to miss); alpha<=0 or >1 stays numerically well-defined but no
    // longer matches this function's own contract (this file's doc
    // comment: "alpha in (0,1]"), so it should be rejected rather
    // than silently accepted.
    if !alpha.is_finite() || alpha <= 0.0 || alpha > 1.0 {
        return Err(anyhow!("'alpha' must be a finite number in (0, 1]"));
    }
    let time_bucket = report::parse_time_bucket(bucket)?;
    let history = report::run(
        conn,
        business_id,
        user_id,
        module_id,
        report::ReportQuery {
            measure_field: Some(measure),
            aggregation: "sum",
            dimension: Dimension::Time { field: "created_at", bucket: time_bucket },
            range_start: None,
            range_end: None,
        },
    )?;

    let values: Vec<f64> = history.iter().map(|p| p.value).collect();
    let forecast = if values.is_empty() {
        0.0
    } else {
        let mut s = values[0];
        for &v in &values[1..] {
            s = alpha * v + (1.0 - alpha) * s;
        }
        s
    };

    Ok(ForecastResult {
        history,
        forecast_next: round2(forecast),
        method: format!("exponential_smoothing(alpha={alpha})"),
    })
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}
