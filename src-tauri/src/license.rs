use anyhow::{anyhow, Result};
use chrono::{Duration, NaiveDate, Utc};
use rusqlite::{params, Connection};
use uuid::Uuid;

const BILLING_CYCLE_DAYS: i64 = 30;
const GRACE_PERIOD_DAYS: i64 = 5;

#[derive(Debug, PartialEq)]
pub enum LicenseStatus {
    /// Never activated — the one-time activation fee hasn't been paid yet.
    Inactive,
    /// Paid and current. Full access, including export.
    Active,
    /// Payment is overdue but still within the 5-day warning window.
    /// Core operations continue uninterrupted; export is blocked.
    Grace { days_left: i64 },
    /// Grace period has fully elapsed with no payment. Core operations
    /// still continue (an SME can't be locked out of running their shop),
    /// but export stays blocked until payment resumes.
    Locked { days_overdue: i64 },
}

fn today() -> NaiveDate {
    Utc::now().date_naive()
}

fn parse_date(s: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|e| anyhow!("bad date '{s}': {e}"))
}

/// Activates a business's license after the one-time activation fee is
/// paid. Starts the first 30-day billing cycle immediately.
///
/// Refuses to run if the license is already activated — without this
/// guard, calling activate() a second time would blindly reset
/// next_due_date to today+30 regardless of how much time remained on
/// the current cycle, silently giving away free time. Activation is a
/// one-time event; renewing an active subscription is what
/// `record_payment` is for.
pub fn activate(conn: &Connection, business_id: &str) -> Result<()> {
    let already_activated: Option<i64> = conn
        .query_row(
            "SELECT activated FROM licenses WHERE business_id = ?1",
            params![business_id],
            |r| r.get(0),
        )
        .ok();
    if already_activated == Some(1) {
        return Err(anyhow!(
            "this license is already activated — use the pay/renew action instead of activating again"
        ));
    }

    let due = today() + Duration::days(BILLING_CYCLE_DAYS);
    let token = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO licenses (business_id, activated, activation_date, last_paid_date, next_due_date, status, license_token)
         VALUES (?1, 1, ?2, ?2, ?3, 'active', ?4)
         ON CONFLICT(business_id) DO UPDATE SET
            activated = 1, activation_date = excluded.activation_date,
            last_paid_date = excluded.last_paid_date,
            next_due_date = excluded.next_due_date,
            status = 'active', license_token = excluded.license_token",
        params![business_id, today().to_string(), due.to_string(), token],
    )?;
    Ok(())
}

/// Records a monthly payment: extends the due date another 30 days from
/// whichever is later — today, or the current due date. Using the later
/// of the two means paying early doesn't shorten a future cycle, and
/// paying late (during grace) doesn't compound extra days on top of the
/// missed ones.
pub fn record_payment(conn: &Connection, business_id: &str) -> Result<()> {
    let current_due: String = conn.query_row(
        "SELECT next_due_date FROM licenses WHERE business_id = ?1",
        params![business_id],
        |r| r.get(0),
    ).map_err(|_| anyhow!("business has no license record — activate first"))?;

    let current_due = parse_date(&current_due)?;
    let base = current_due.max(today());
    let new_due = base + Duration::days(BILLING_CYCLE_DAYS);

    conn.execute(
        "UPDATE licenses SET last_paid_date = ?1, next_due_date = ?2, status = 'active' WHERE business_id = ?3",
        params![today().to_string(), new_due.to_string(), business_id],
    )?;
    Ok(())
}

/// Computes the current license status by comparing today's date against
/// next_due_date, and writes the result back to `status` so any other
/// query reading that column directly stays consistent.
pub fn check_status(conn: &Connection, business_id: &str) -> Result<LicenseStatus> {
    let row: Option<(i64, String)> = conn
        .query_row(
            "SELECT activated, next_due_date FROM licenses WHERE business_id = ?1",
            params![business_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();

    let (activated, due_str) = match row {
        Some(r) => r,
        None => return Ok(LicenseStatus::Inactive),
    };
    if activated != 1 {
        return Ok(LicenseStatus::Inactive);
    }

    let due = parse_date(&due_str)?;
    let now = today();
    let grace_end = due + Duration::days(GRACE_PERIOD_DAYS);

    let status = if now <= due {
        LicenseStatus::Active
    } else if now <= grace_end {
        LicenseStatus::Grace { days_left: (grace_end - now).num_days() }
    } else {
        LicenseStatus::Locked { days_overdue: (now - due).num_days() }
    };

    let status_str = match &status {
        LicenseStatus::Active => "active",
        LicenseStatus::Grace { .. } => "grace",
        LicenseStatus::Locked { .. } => "locked",
        LicenseStatus::Inactive => "inactive",
    };
    conn.execute(
        "UPDATE licenses SET status = ?1 WHERE business_id = ?2",
        params![status_str, business_id],
    )?;

    Ok(status)
}

/// The single choke point export endpoints must call before returning
/// any data. Only a fully current license may export.
pub fn require_export_allowed(conn: &Connection, business_id: &str) -> Result<()> {
    match check_status(conn, business_id)? {
        LicenseStatus::Active => Ok(()),
        LicenseStatus::Inactive => Err(anyhow!("license not activated — export is unavailable until activation")),
        LicenseStatus::Grace { days_left } => Err(anyhow!(
            "payment overdue — export is locked. {days_left} day(s) remain in your grace period; please pay to restore export access"
        )),
        LicenseStatus::Locked { days_overdue } => Err(anyhow!(
            "payment overdue by {days_overdue} day(s) — export is locked. All other features keep working; please pay to restore export access"
        )),
    }
}
