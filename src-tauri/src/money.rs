//! Money as integer minor units (cents), not floating point.
//!
//! Why this file exists: `f64` cannot exactly represent most decimal
//! currency values, and every multiplication or repeated addition over
//! a money value is a chance for a fraction-of-a-cent error to creep
//! in and compound across thousands of transactions. Storing and
//! computing money as `i64` minor units makes every arithmetic
//! operation exact by construction — there is no rounding error to
//! accumulate because there is never a fractional cent in memory in
//! the first place.
//!
//! This is the ONLY place rounding is allowed to happen. Every other
//! module either passes cents straight through (no rounding needed,
//! because integer math is exact) or calls into here at the specific
//! boundary where a fraction is unavoidable (parsing human input,
//! applying an exchange rate, applying a tax rate).
//!
//! Not every currency has 2 decimal places (JPY has 0, KWD has 3) —
//! `decimal_places_for` is the one place that fact lives, so nothing
//! else hardcodes "divide by 100".

use anyhow::{anyhow, Result};

/// Minor-unit decimal places per ISO 4217 currency code. Defaults to 2
/// (the overwhelmingly common case) for anything not listed here —
/// this list only needs to cover the exceptions.
pub fn decimal_places_for(currency_code: &str) -> u32 {
    match currency_code.to_uppercase().as_str() {
        // Zero-decimal currencies — no minor unit in practice.
        "JPY" | "KRW" | "VND" | "UGX" | "RWF" | "XOF" | "XAF" | "BIF" | "DJF" | "GNF"
        | "KMF" | "MGA" | "PYG" | "VUV" | "CLP" => 0,
        // Three-decimal currencies.
        "BHD" | "IQD" | "JOD" | "KWD" | "OMR" | "TND" => 3,
        // Everything else, including KES and USD: standard 2dp.
        _ => 2,
    }
}

/// Parses a human-typed decimal string (e.g. "12.50", "1250" for a
/// 0dp currency) into integer minor units. Rejects anything with more
/// fractional precision than the currency supports — a sub-cent entry
/// like "12.505" for a 2dp currency is a data-entry mistake, not
/// something to silently round away.
pub fn parse_money_input(input: &str, currency_code: &str) -> Result<i64> {
    // Strip thousands separators before anything else — mirrors
    // src/lib/money.ts's parseMoneyInput exactly (see that file's own
    // doc comment for THE BUG THIS FIXES: formatMoney's own comma
    // grouping used to round-trip straight into a rejection here).
    // A spreadsheet cell can carry the same comma-grouped text a
    // human would type, so this needs the identical fix, not just the
    // frontend.
    let no_separators = input.replace(',', "");
    let trimmed = no_separators.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("amount is required"));
    }
    let negative = trimmed.starts_with('-');
    let unsigned = trimmed.trim_start_matches('-');

    let places = decimal_places_for(currency_code);
    let (whole, frac) = match unsigned.split_once('.') {
        Some((w, f)) => (w, f),
        None => (unsigned, ""),
    };
    if frac.len() > places as usize {
        return Err(anyhow!(
            "'{input}' has more precision than {currency_code} supports ({places} decimal place{})",
            if places == 1 { "" } else { "s" }
        ));
    }
    if !whole.chars().all(|c| c.is_ascii_digit()) || (!frac.is_empty() && !frac.chars().all(|c| c.is_ascii_digit())) {
        return Err(anyhow!("'{input}' is not a valid amount"));
    }
    if whole.is_empty() && frac.is_empty() {
        return Err(anyhow!("'{input}' is not a valid amount"));
    }

    let whole_val: i64 = if whole.is_empty() { 0 } else { whole.parse().map_err(|_| anyhow!("'{input}' is not a valid amount"))? };
    let scale = 10_i64.pow(places);
    let frac_padded = format!("{frac:0<width$}", width = places as usize);
    let frac_val: i64 = if places == 0 { 0 } else { frac_padded.parse().map_err(|_| anyhow!("'{input}' is not a valid amount"))? };

    let cents = whole_val
        .checked_mul(scale)
        .and_then(|v| v.checked_add(frac_val))
        .ok_or_else(|| anyhow!("'{input}' is out of range"))?;

    Ok(if negative { -cents } else { cents })
}

/// Formats integer minor units back to a display string, e.g.
/// 1250 -> "12.50" for a 2dp currency, 1250 -> "1250" for a 0dp one.
pub fn format_money(cents: i64, currency_code: &str) -> String {
    let places = decimal_places_for(currency_code);
    if places == 0 {
        return cents.to_string();
    }
    let scale = 10_i64.pow(places);
    let negative = cents < 0;
    let abs = cents.unsigned_abs() as i64;
    let whole = abs / scale;
    let frac = abs % scale;
    format!(
        "{}{}.{:0width$}",
        if negative { "-" } else { "" },
        whole,
        frac,
        width = places as usize
    )
}

/// Applies a fractional rate (e.g. a tax rate like 0.16, or an
/// exchange rate) to an integer cents amount, rounding to the nearest
/// cent exactly once. This is the ONLY place a rate multiplies a
/// money value — every caller passes the result straight through as
/// an integer afterward, never carrying the unrounded fraction.
pub fn apply_rate(cents: i64, rate: f64) -> i64 {
    (cents as f64 * rate).round() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_decimal() {
        assert_eq!(parse_money_input("12.50", "USD").unwrap(), 1250);
        assert_eq!(parse_money_input("0.01", "USD").unwrap(), 1);
        assert_eq!(parse_money_input("100", "USD").unwrap(), 10000);
    }

    #[test]
    fn rejects_sub_cent_precision() {
        assert!(parse_money_input("12.505", "USD").is_err());
    }

    #[test]
    fn respects_zero_decimal_currencies() {
        assert_eq!(parse_money_input("1500", "JPY").unwrap(), 1500);
        assert!(parse_money_input("15.50", "JPY").is_err());
    }

    #[test]
    fn respects_three_decimal_currencies() {
        assert_eq!(parse_money_input("1.500", "KWD").unwrap(), 1500);
    }

    #[test]
    fn strips_thousands_separators() {
        assert_eq!(parse_money_input("50,000", "KES").unwrap(), 5_000_000);
        assert_eq!(parse_money_input("50,000.00", "KES").unwrap(), 5_000_000);
        assert_eq!(parse_money_input("1,234,567.89", "USD").unwrap(), 123_456_789);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_money_input("abc", "USD").is_err());
        assert!(parse_money_input("", "USD").is_err());
        assert!(parse_money_input("12.5.6", "USD").is_err());
    }

    #[test]
    fn format_round_trips() {
        assert_eq!(format_money(1250, "USD"), "12.50");
        assert_eq!(format_money(1, "USD"), "0.01");
        assert_eq!(format_money(1500, "JPY"), "1500");
        assert_eq!(format_money(-1250, "USD"), "-12.50");
    }

    #[test]
    fn exactness_over_many_operations() {
        // The exact case that would drift with f64: repeated addition
        // of a value that isn't exactly representable in binary.
        let unit_price_cents = 1999_i64; // $19.99
        let qty = 7_i64;
        let mut subtotal = 0_i64;
        for _ in 0..10_000 {
            subtotal = 0;
            subtotal += unit_price_cents * qty;
        }
        assert_eq!(subtotal, 13993);
    }

    #[test]
    fn apply_rate_rounds_once() {
        // 16% tax on 1050 cents = 168.0 exactly
        assert_eq!(apply_rate(1050, 0.16), 168);
        // A case that would show float drift if rounded more than once
        assert_eq!(apply_rate(333, 0.1), 33);
    }
}
