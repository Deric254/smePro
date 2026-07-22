use anyhow::Result;
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
    let time_bucket = report::parse_time_bucket(bucket)?;
    let history = report::run(
        conn,
        business_id,
        user_id,
        module_id,
        Some(measure),
        "sum",
        Dimension::Time { field: "created_at", bucket: time_bucket },
        None,
        None,
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
    let time_bucket = report::parse_time_bucket(bucket)?;
    let history = report::run(
        conn,
        business_id,
        user_id,
        module_id,
        Some(measure),
        "sum",
        Dimension::Time { field: "created_at", bucket: time_bucket },
        None,
        None,
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
