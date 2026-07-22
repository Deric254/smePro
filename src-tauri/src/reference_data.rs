//! User-addable master data: units of measure and currencies.
//!
//! Neither is a hardcoded enum anywhere in the engine. Both are ordinary
//! business-scoped rows, managed like any other data (create/list/delete
//! through the HTTP API), seeded with sensible starting defaults at
//! business creation but never locked to them — an Owner/admin-tier user
//! can rename, delete, or add to either list freely.
//!
//! A module field can declare `"type": "unit"` or `"type": "currency"`
//! (see module.rs) and `validate_field_references` below is what makes
//! that mean something: on every create/update, the value is checked
//! against exactly what THIS business has actually defined — not a fixed
//! list baked into Rust. This is the "every product accounted" piece:
//! a SKU's unit isn't a free-text field that silently drifts ("kg" vs
//! "Kg" vs "kilograms" all meaning different things to a report), it's a
//! reference to one canonical row.

use crate::module::ModuleDef;
use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use uuid::Uuid;

const DEFAULT_UNITS: &[(&str, &str)] = &[
    ("Piece", "pc"),
    ("Kilogram", "kg"),
    ("Gram", "g"),
    ("Litre", "L"),
    ("Millilitre", "ml"),
    ("Box", "box"),
    ("Dozen", "dz"),
    ("Carton", "ctn"),
];

const DEFAULT_CURRENCIES: &[(&str, &str, &str)] = &[
    ("USD", "$", "US Dollar"),
    ("KES", "KSh", "Kenyan Shilling"),
    ("EUR", "\u{20ac}", "Euro"),
    ("GBP", "\u{a3}", "British Pound"),
    ("NGN", "\u{20a6}", "Nigerian Naira"),
    ("TZS", "TSh", "Tanzanian Shilling"),
    ("UGX", "USh", "Ugandan Shilling"),
    ("GHS", "\u{20b5}", "Ghanaian Cedi"),
    ("ZAR", "R", "South African Rand"),
];

/// Called once at business creation. Purely a convenience starting
/// point — every row it inserts is exactly as deletable/editable as one
/// added later by hand. `INSERT OR IGNORE` makes this safe to call more
/// than once (e.g. re-run against an already-seeded business) without
/// erroring or duplicating rows.
pub fn seed_defaults(conn: &Connection, business_id: &str) -> Result<()> {
    for (name, abbr) in DEFAULT_UNITS {
        conn.execute(
            "INSERT OR IGNORE INTO units (id, business_id, name, abbreviation, created_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'))",
            params![Uuid::new_v4().to_string(), business_id, name, abbr],
        )?;
    }
    for (code, symbol, name) in DEFAULT_CURRENCIES {
        conn.execute(
            "INSERT OR IGNORE INTO currencies (id, business_id, code, symbol, name, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
            params![Uuid::new_v4().to_string(), business_id, code, symbol, name],
        )?;
    }
    Ok(())
}

// ---------------------------------------------------------------- units

pub fn list_units(conn: &Connection, business_id: &str) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, abbreviation FROM units WHERE business_id = ?1 ORDER BY name",
    )?;
    let rows = stmt.query_map(params![business_id], |r| {
        Ok(json!({
            "id": r.get::<_, String>(0)?,
            "name": r.get::<_, String>(1)?,
            "abbreviation": r.get::<_, Option<String>>(2)?,
        }))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn create_unit(conn: &Connection, business_id: &str, name: &str, abbreviation: Option<&str>) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(anyhow!("unit name cannot be empty"));
    }
    if name.len() > 64 {
        return Err(anyhow!("unit name is too long (max 64 characters)"));
    }
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO units (id, business_id, name, abbreviation, created_at) VALUES (?1, ?2, ?3, ?4, datetime('now'))",
        params![id, business_id, name, abbreviation],
    )
    .map_err(|e| classify_unique(e, "unit", name))?;
    Ok(id)
}

/// Refuses to delete a unit that's still referenced by any real record in
/// any enabled module — checked by actually scanning every module with a
/// "unit"-typed field, not assumed safe. Deleting the row out from under
/// live data would silently corrupt every report/export that groups or
/// displays by unit.
pub fn delete_unit(conn: &Connection, business_id: &str, unit_id: &str) -> Result<()> {
    let name: String = conn
        .query_row(
            "SELECT name FROM units WHERE id = ?1 AND business_id = ?2",
            params![unit_id, business_id],
            |r| r.get(0),
        )
        .map_err(|_| anyhow!("unit not found"))?;

    if let Some((module_id, count)) = find_usage(conn, business_id, "unit", &name)? {
        return Err(anyhow!(
            "cannot delete unit '{name}': still used by {count} record(s) in module '{module_id}' — reassign or remove those first"
        ));
    }

    conn.execute("DELETE FROM units WHERE id = ?1 AND business_id = ?2", params![unit_id, business_id])?;
    Ok(())
}

// ------------------------------------------------------------ currencies

pub fn list_currencies(conn: &Connection, business_id: &str) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT id, code, symbol, name FROM currencies WHERE business_id = ?1 ORDER BY code",
    )?;
    let rows = stmt.query_map(params![business_id], |r| {
        Ok(json!({
            "id": r.get::<_, String>(0)?,
            "code": r.get::<_, String>(1)?,
            "symbol": r.get::<_, Option<String>>(2)?,
            "name": r.get::<_, Option<String>>(3)?,
        }))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn create_currency(conn: &Connection, business_id: &str, code: &str, symbol: Option<&str>, name: Option<&str>) -> Result<String> {
    let code = code.trim().to_uppercase();
    if code.is_empty() {
        return Err(anyhow!("currency code cannot be empty"));
    }
    if code.len() > 12 {
        return Err(anyhow!("currency code is too long (max 12 characters) — use the symbol/name fields for anything longer"));
    }
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO currencies (id, business_id, code, symbol, name, created_at) VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
        params![id, business_id, code, symbol, name],
    )
    .map_err(|e| classify_unique(e, "currency", &code))?;
    Ok(id)
}

/// Same "actually check, don't assume" protection as `delete_unit`, plus
/// refuses to delete the business's own currently-configured default
/// currency (`businesses.currency`) — that would leave the business in
/// an inconsistent state where its own default no longer resolves.
pub fn delete_currency(conn: &Connection, business_id: &str, currency_id: &str) -> Result<()> {
    let code: String = conn
        .query_row(
            "SELECT code FROM currencies WHERE id = ?1 AND business_id = ?2",
            params![currency_id, business_id],
            |r| r.get(0),
        )
        .map_err(|_| anyhow!("currency not found"))?;

    let default_currency: String = conn.query_row(
        "SELECT currency FROM businesses WHERE id = ?1",
        params![business_id],
        |r| r.get(0),
    )?;
    if default_currency.eq_ignore_ascii_case(&code) {
        return Err(anyhow!(
            "cannot delete '{code}': it's this business's current default currency — change the default first (business panel)"
        ));
    }

    if let Some((module_id, count)) = find_usage(conn, business_id, "currency", &code)? {
        return Err(anyhow!(
            "cannot delete currency '{code}': still used by {count} record(s) in module '{module_id}' — reassign or remove those first"
        ));
    }

    conn.execute("DELETE FROM currencies WHERE id = ?1 AND business_id = ?2", params![currency_id, business_id])?;
    Ok(())
}

// --------------------------------------------------------------- shared

/// Scans every enabled module for fields of the given reference type
/// (`"unit"` or `"currency"`) and returns the first module where `value`
/// is actually present in live (non-deleted) data, along with a count.
fn find_usage(conn: &Connection, business_id: &str, field_type: &str, value: &str) -> Result<Option<(String, i64)>> {
    let mut stmt = conn.prepare("SELECT id, schema_json FROM modules WHERE business_id = ?1 AND enabled = 1")?;
    let mods: Vec<(String, String)> = stmt
        .query_map(params![business_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();

    for (module_id, schema_json) in mods {
        let module = match ModuleDef::from_json_str(&schema_json) {
            Ok(m) => m,
            Err(_) => continue,
        };
        for f in &module.fields {
            if f.field_type != field_type {
                continue;
            }
            let table = module.table_name();
            let sql = format!("SELECT COUNT(*) FROM {table} WHERE {} = ?1 AND deleted_at IS NULL", f.name);
            let count: i64 = conn.query_row(&sql, params![value], |r| r.get(0)).unwrap_or(0);
            if count > 0 {
                return Ok(Some((module_id, count)));
            }
        }
    }
    Ok(None)
}

fn classify_unique(e: rusqlite::Error, kind: &str, value: &str) -> anyhow::Error {
    if e.to_string().to_uppercase().contains("UNIQUE") {
        anyhow!("a {kind} '{value}' already exists")
    } else {
        anyhow!(e)
    }
}

/// Called from crud::create/crud::update after `module.validate()` — the
/// type-level check (is it a string?) already passed; this is the
/// content-level check (is it a value this business actually defined?).
/// Skips fields the caller didn't supply (optional fields, or an update
/// that isn't touching that field) since those are already handled by
/// `validate()`'s required-field logic.
pub fn validate_field_references(
    conn: &Connection,
    business_id: &str,
    module: &ModuleDef,
    record: &std::collections::HashMap<String, Value>,
) -> Result<()> {
    for f in &module.fields {
        if f.field_type != "unit" && f.field_type != "currency" {
            continue;
        }
        let Some(v) = record.get(&f.name) else { continue };
        let Some(s) = v.as_str() else { continue }; // type check already caught non-strings
        let table = if f.field_type == "unit" { "units" } else { "currencies" };
        let column = if f.field_type == "unit" { "name" } else { "code" };
        let exists: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE business_id = ?1 AND {column} = ?2"),
            params![business_id, s],
            |r| r.get(0),
        )?;
        if exists == 0 {
            return Err(anyhow!(
                "field '{}': unknown {} '{}' — add it under {} first",
                f.name,
                f.field_type,
                s,
                if f.field_type == "unit" { "Units" } else { "Currencies" }
            ));
        }
    }
    Ok(())
}
