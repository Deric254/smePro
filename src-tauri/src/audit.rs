use anyhow::Result;
use rusqlite::Connection;
use serde_json::Value;

/// Writes one immutable audit entry. Never call UPDATE or DELETE against
/// audit_log anywhere in the codebase — if that ever feels necessary,
/// it's a sign something upstream is wrong.
pub fn log(
    conn: &Connection,
    business_id: &str,
    user_id: Option<&str>,
    module_id: &str,
    action: &str,
    record_id: Option<&str>,
    details: Option<&Value>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO audit_log (id, business_id, user_id, module_id, action, record_id, details_json, timestamp)
         VALUES (lower(hex(randomblob(16))), ?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))",
        rusqlite::params![
            business_id,
            user_id,
            module_id,
            action,
            record_id,
            details.map(|d| d.to_string()),
        ],
    )?;
    Ok(())
}
