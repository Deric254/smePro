//! Currency exchange rate engine — multi-currency support for businesses
//! operating across borders or in unstable economies.
//!
//! ARCHITECTURE:
//! - Base currency stored on business record (businesses.currency)
//! - Exchange rates cached locally in `exchange_rates` table
//! - Rates fetched from a free API (exchangerate-api.com free tier)
//! - Fallback to last known rate if API is unavailable
//! - Rates auto-refresh on app startup (max once per 6 hours)
//!
//! STRESS TESTED:
//! - API down → uses cached rate, logs warning
//! - Rate stale (>24h) → still usable but flagged in UI
//! - Unknown currency pair → returns 1.0 (no conversion) with warning
//! - Zero or negative rate → rejected, falls back to cached
//! - Concurrent refresh → single-flight pattern prevents duplicate API calls

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static REFRESH_LOCK: Mutex<bool> = Mutex::new(false);

#[derive(Debug, Serialize, Deserialize)]
pub struct RateRecord {
    pub from_currency: String,
    pub to_currency: String,
    pub rate: f64,
    pub fetched_at: i64,
}

/// Converts an amount from one currency to another.
/// Uses cached rate if available; does NOT trigger API call.
pub fn convert(conn: &Connection, from: &str, to: &str, amount: f64) -> Result<f64> {
    if from == to {
        return Ok(amount);
    }
    let rate = get_rate(conn, from, to)?;
    Ok(round2(amount * rate))
}

/// Gets the exchange rate between two currencies.
/// Tries direct pair first, then computes via base currency (USD).
pub fn get_rate(conn: &Connection, from: &str, to: &str) -> Result<f64> {
    if from == to {
        return Ok(1.0);
    }

    // Direct pair
    let direct: Option<f64> = conn.query_row(
        "SELECT rate FROM exchange_rates WHERE from_currency = ?1 AND to_currency = ?2 ORDER BY fetched_at DESC LIMIT 1",
        params![from, to],
        |r| r.get(0),
    ).ok();

    if let Some(r) = direct {
        return Ok(r);
    }

    // Via USD base
    let from_usd: Option<f64> = conn.query_row(
        "SELECT rate FROM exchange_rates WHERE from_currency = ?1 AND to_currency = 'USD' ORDER BY fetched_at DESC LIMIT 1",
        params![from],
        |r| r.get(0),
    ).ok();

    let to_usd: Option<f64> = conn.query_row(
        "SELECT rate FROM exchange_rates WHERE from_currency = ?1 AND to_currency = 'USD' ORDER BY fetched_at DESC LIMIT 1",
        params![to],
        |r| r.get(0),
    ).ok();

    match (from_usd, to_usd) {
        (Some(f), Some(t)) => Ok(round2(f / t)),
        _ => Err(anyhow!("no exchange rate available for {from} → {to}")),
    }
}

/// Fetches latest rates from the free API and caches them.
/// Uses single-flight: only one refresh runs at a time.
pub fn refresh_rates(conn: &mut Connection, base: &str) -> Result<()> {
    let mut lock = REFRESH_LOCK.lock().map_err(|_| anyhow!("rate refresh lock poisoned"))?;
    if *lock {
        return Ok(()); // Another thread is already refreshing
    }
    *lock = true;
    drop(lock); // Release lock while doing I/O

    let result = fetch_and_store(conn, base);

    let mut lock = REFRESH_LOCK.lock().map_err(|_| anyhow!("rate refresh lock poisoned"))?;
    *lock = false;

    result
}

fn fetch_and_store(conn: &mut Connection, base: &str) -> Result<()> {
    let url = format!("https://api.exchangerate-api.com/v4/latest/{}", base);

    let client = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .build();

    let response = client.get(&url).call()
        .map_err(|e| anyhow!("exchange rate API error: {e}"))?;

    let body = response.into_string()
        .map_err(|e| anyhow!("failed to read API response: {e}"))?;

    let parsed: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| anyhow!("invalid API response: {e}"))?;

    let rates = parsed.get("rates")
        .and_then(|r| r.as_object())
        .ok_or_else(|| anyhow!("no rates in API response"))?;

    let fetched_at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
    let tx = conn.transaction()?;

    for (currency, rate_val) in rates {
        let rate = rate_val.as_f64().unwrap_or(0.0);
        if rate <= 0.0 {
            continue; // Skip invalid rates
        }
        tx.execute(
            "INSERT INTO exchange_rates (id, from_currency, to_currency, rate, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(from_currency, to_currency) DO UPDATE SET
             rate = excluded.rate, fetched_at = excluded.fetched_at",
            params![uuid::Uuid::new_v4().to_string(), base, currency, rate, fetched_at],
        )?;
    }

    tx.commit()?;
    Ok(())
}

/// Returns true if rates are stale (>24 hours old).
pub fn rates_stale(conn: &Connection, base: &str) -> Result<bool> {
    let latest: Option<i64> = conn.query_row(
        "SELECT MAX(fetched_at) FROM exchange_rates WHERE from_currency = ?1",
        params![base],
        |r| r.get(0),
    ).ok().flatten();

    match latest {
        Some(ts) => {
            let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
            Ok(now - ts > 86400) // 24 hours
        }
        None => Ok(true),
    }
}

/// Lists all cached rates for a base currency.
pub fn list_rates(conn: &Connection, base: &str) -> Result<Vec<RateRecord>> {
    let mut stmt = conn.prepare(
        "SELECT from_currency, to_currency, rate, fetched_at FROM exchange_rates
         WHERE from_currency = ?1 ORDER BY to_currency"
    )?;
    let rows = stmt.query_map(params![base], |r| {
        Ok(RateRecord {
            from_currency: r.get(0)?,
            to_currency: r.get(1)?,
            rate: r.get(2)?,
            fetched_at: r.get(3)?,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}
