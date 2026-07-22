use anyhow::{anyhow, Result};
use rusqlite::Connection;

use crate::business_panel;

/// Maps a business type to a sensible default module set. This is the
/// entire "wizard" — no separate onboarding state machine needed, since
/// enabling a module is already idempotent and safe to call repeatedly.
/// Adding a new business type is a one-line change here, not a new
/// screen or workflow.
fn preset_modules(business_type: &str) -> Result<Vec<&'static str>> {
    match business_type {
        "retail" => Ok(vec!["inventory", "sales", "debt_credit", "accounting"]),
        "food" => Ok(vec!["inventory", "sales", "purchasing", "debt_credit", "accounting"]),
        "services" => Ok(vec!["hr", "sales", "debt_credit", "accounting"]),
        "manufacturing" => Ok(vec!["inventory", "purchasing", "hr", "sales", "accounting"]),
        other => Err(anyhow!(
            "unknown business type '{other}', expected one of: retail, food, services, manufacturing"
        )),
    }
}

/// Applies the preset for a business type: enables each module in the
/// preset (idempotent — safe even if some are already on) and returns
/// the list that ended up enabled, for the wizard UI to display back to
/// the owner as a confirmation step.
pub fn apply_business_type(
    conn: &mut Connection,
    business_id: &str,
    business_type: &str,
) -> Result<Vec<String>> {
    let modules = preset_modules(business_type)?;
    let mut enabled = Vec::new();
    for module_id in modules {
        let path = format!("modules/{module_id}.json");
        if std::path::Path::new(&path).exists() {
            business_panel::enable_module(conn, business_id, &path)?;
            enabled.push(module_id.to_string());
        }
        // Silently skips a module whose JSON file isn't present on disk —
        // keeps this forward-compatible with presets that reference
        // modules not yet shipped, without breaking the wizard.
    }
    Ok(enabled)
}
