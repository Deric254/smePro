use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::module::ModuleDef;
use crate::{audit, rbac};

/// Loads a module's schema back out of the `modules` registry table (not
/// from a file) — this is what makes CRUD generic at request time: any
/// module enabled for the business, past or present, can be operated on
/// purely from what's stored in the DB.
pub fn load_module(conn: &Connection, business_id: &str, module_id: &str) -> Result<ModuleDef> {
    // Belt-and-suspenders: module_id ultimately becomes part of a raw
    // SQL table name (see ModuleDef::table_name), and while the
    // existence-gated query below already means an attacker can't
    // reach that with arbitrary content (a nonexistent module_id just
    // returns "not enabled" before any table name is ever built), this
    // is the one chokepoint every module resolution passes through —
    // cheap insurance against that invariant ever being relied on
    // incorrectly by some future code path that isn't as careful.
    crate::security::validate_table_name(module_id)?;
    let raw: String = conn
        .query_row(
            "SELECT schema_json FROM modules WHERE business_id = ?1 AND id = ?2 AND enabled = 1",
            rusqlite::params![business_id, module_id],
            |r| r.get(0),
        )
        .map_err(|_| anyhow!("module '{module_id}' is not enabled for this business"))?;
    ModuleDef::from_json_str(&raw)
}

/// Fields that must never change through a single-record PATCH, no
/// matter which role is calling it — the backend-side enforcement of
/// the same boundary the frontend's own `isActionManagedField()` (see
/// ModuleView.tsx) draws for `purchasing`'s `received` and
/// `debt_credit`'s `settled`: those fields, and this one, are only
/// ever supposed to change as a side effect of a specific,
/// purpose-built action, never as a standalone field edit.
///
/// `inventory`'s `quantity` belongs here for the same reason: once an
/// item exists, every legitimate way to change how much of it there is
/// already has its own dedicated, atomic, audited action — sell
/// (pos.rs), receive (receiving.rs), refund (refund.rs), repack
/// (repack.rs) — each of which enforces things a plain field edit
/// never could (oversell protection, weighted-average cost
/// recalculation, a floor at zero). Letting a single-record PATCH
/// touch `quantity` too would mean the stock level for an item could
/// be silently overwritten to anything — including a negative number,
/// or zero, wiping it out — indistinguishable in the audit log from
/// correcting a typo in the item's name. Hiding the field from React's
/// edit form is not enough on its own: that's a UI nicety that a raw
/// API call bypasses entirely, so the real boundary has to live here,
/// checked by the single-record HTTP route in http_api.rs before it
/// ever calls `update()` below.
///
/// This function only concerns `update()` — a later edit of an
/// existing record. `create()` has its own, separate rule: every
/// inventory item is forced to `quantity: 0` at creation time, no
/// exceptions, so there's no "caller-supplied opening count" case for
/// this function to worry about exempting (see `create()`'s own doc
/// comment on why).
///
/// Also deliberately does NOT apply when `update()`'s own `bulk_import`
/// flag is set — excel_import.rs's own doc comment explains why a
/// spreadsheet re-upload is a real, sanctioned way to change quantity:
/// it exists specifically to serve "a stock take (reconciling counted
/// quantities against what the system thinks is on the shelf)". That's
/// a deliberate, validated, whole-batch reconciliation workflow, not
/// the same danger as a single ad-hoc field edit slipping through the
/// generic one-record form — so it gets to bypass this specific block
/// (and only this one; excel_import still goes through every other
/// validation `update()` applies, same as any other caller).
fn is_single_record_edit_blocked_field(module_id: &str, field_name: &str) -> bool {
    module_id == "inventory" && field_name == "quantity"
}

/// CREATE — validates against the module's field rules, inserts, audits.
pub fn create(
    conn: &Connection,
    business_id: &str,
    user_id: &str,
    module_id: &str,
    body: &Map<String, Value>,
) -> Result<String> {
    rbac::require(conn, user_id, module_id, "create")?;
    let module = load_module(conn, business_id, module_id)?;

    let mut record: std::collections::HashMap<String, Value> = body.clone().into_iter().collect();
    // Apply defaults for any field the caller omitted.
    for f in &module.fields {
        if !record.contains_key(&f.name) {
            if let Some(d) = &f.default {
                record.insert(f.name.clone(), d.clone());
            }
        }
    }
    // Every inventory item starts at zero stock, full stop — no
    // exceptions, no caller-supplied opening count, on this single-record
    // path. Stock only ever enters the system one way: Purchasing
    // receiving an order against the item (receiving.rs::receive()),
    // which is the only place a real vendor, cost, and delivered
    // quantity all get recorded together. Letting a plain "create a new
    // item" call seed its own quantity would mean two different, silently
    // inconsistent ways for stock to appear — one traceable to a purchase,
    // one not traceable to anything. Whatever the caller sent for
    // "quantity" is discarded here, not validated-then-rejected, because
    // this is a normal, expected part of creating an inventory item, not
    // an error condition — the frontend doesn't even show the field on
    // this form (see ModuleView.tsx).
    //
    // Deliberately does NOT apply to insert_validated_record() below,
    // which is what excel_import.rs's bulk upload calls directly instead
    // of this function — see that module's own doc comment for why a
    // spreadsheet-driven initial catalog load (or a stock take) is a
    // real, sanctioned way to set a starting quantity, unlike a single
    // ad-hoc item creation through this generic form.
    if module_id == "inventory" {
        record.insert("quantity".to_string(), json!(0));
    }
    // Hard business rule, not just a UI nicety: an inventory item can
    // never be saved with a selling price below its cost price. Both
    // fields are "money" (required, default 0), so by the time we get
    // here `record` always has a value for each — either what the
    // caller sent or the default just applied above — so this is a
    // simple, complete comparison with nothing left to fall back to.
    if module_id == "inventory" {
        let unit_cost = record.get("unit_cost").and_then(|v| v.as_i64()).unwrap_or(0);
        let unit_price = record.get("unit_price").and_then(|v| v.as_i64()).unwrap_or(0);
        if unit_price < unit_cost {
            return Err(anyhow!(
                "selling price cannot be lower than the cost price — this would sell at a loss"
            ));
        }
    }
    module.validate(&record)?;
    crate::reference_data::validate_field_references(conn, business_id, &module, &record)?;

    let id = insert_validated_record(conn, business_id, &module, &record)?;

    audit::log(conn, business_id, Some(user_id), module_id, "create", Some(&id), Some(&json!(body)))?;
    Ok(id)
}

/// The actual INSERT, split out from `create()` above so any caller
/// that needs this exact insert wrapped in a LARGER transaction — most
/// notably `pos::checkout`, which must insert a sales record AND
/// deduct inventory atomically — reuses this precisely, instead of a
/// second, hand-copied INSERT that could quietly drift out of sync
/// with this one as the schema evolves. Validation is the caller's
/// responsibility (call `module.validate()` and
/// `reference_data::validate_field_references()` first) — this
/// function trusts `record` is already correct.
pub fn insert_validated_record(
    conn: &Connection,
    business_id: &str,
    module: &ModuleDef,
    record: &std::collections::HashMap<String, Value>,
) -> Result<String> {
    let table = module.table_name();
    let mut col_names = vec!["id".to_string(), "business_id".to_string()];
    let mut placeholders = vec!["?1".to_string(), "?2".to_string()];
    let mut values: Vec<Box<dyn rusqlite::ToSql>> = vec![];
    let id = Uuid::new_v4().to_string();
    values.push(Box::new(id.clone()));
    values.push(Box::new(business_id.to_string()));

    let mut idx = 3;
    for f in &module.fields {
        if let Some(v) = record.get(&f.name) {
            col_names.push(f.name.clone());
            placeholders.push(format!("?{idx}"));
            values.push(value_to_sql(v));
            idx += 1;
        }
    }
    col_names.push("created_at".into());
    col_names.push("updated_at".into());
    placeholders.push("datetime('now')".into());
    placeholders.push("datetime('now')".into());

    let sql = format!(
        "INSERT INTO {table} ({}) VALUES ({})",
        col_names.join(", "),
        placeholders.join(", ")
    );
    let params_refs: Vec<&dyn rusqlite::ToSql> = values.iter().map(|b| b.as_ref()).collect();
    conn.execute(&sql, params_refs.as_slice())?;
    Ok(id)
}

/// READ (list) — optional free-text search across all text fields, plus
/// standard pagination. This is generic: it doesn't know in advance which
/// fields exist, it reads them from the module definition.
pub fn list(
    conn: &Connection,
    business_id: &str,
    user_id: &str,
    module_id: &str,
    search: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<Value>> {
    rbac::require(conn, user_id, module_id, "read")?;
    let module = load_module(conn, business_id, module_id)?;
    let table = module.table_name();

    let mut sql = format!(
        "SELECT id, {} FROM {table} WHERE business_id = ?1 AND deleted_at IS NULL",
        module.fields.iter().map(|f| f.name.clone()).collect::<Vec<_>>().join(", ")
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(business_id.to_string())];

    if let Some(term) = search {
        let text_fields: Vec<&str> = module
            .fields
            .iter()
            .filter(|f| f.field_type == "text")
            .map(|f| f.name.as_str())
            .collect();
        if !text_fields.is_empty() {
            let start = params.len() + 1;
            let clauses: Vec<String> = text_fields
                .iter()
                .enumerate()
                .map(|(i, f)| format!("{f} LIKE ?{}", start + i))
                .collect();
            sql.push_str(&format!(" AND ({})", clauses.join(" OR ")));
            for _ in &text_fields {
                params.push(Box::new(format!("%{term}%")));
            }
        }
    }
    sql.push_str(&format!(" ORDER BY created_at DESC LIMIT {limit} OFFSET {offset}"));

    let mut stmt = conn.prepare(&sql)?;
    let col_names: Vec<String> = std::iter::once("id".to_string())
        .chain(module.fields.iter().map(|f| f.name.clone()))
        .collect();

    let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
    let rows = stmt.query_map(params_refs.as_slice(), |row| {
        let mut obj = Map::new();
        for (i, name) in col_names.iter().enumerate() {
            let v: Value = match row.get_ref(i)? {
                rusqlite::types::ValueRef::Null => Value::Null,
                rusqlite::types::ValueRef::Integer(n) => json!(n),
                rusqlite::types::ValueRef::Real(f) => json!(f),
                rusqlite::types::ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                rusqlite::types::ValueRef::Blob(_) => Value::Null,
            };
            obj.insert(name.clone(), v);
        }
        Ok(Value::Object(obj))
    })?;

    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
}

/// UPDATE — partial update of any subset of fields, validated, audited
/// with an old->new diff so the audit trail is actually useful.
/// `bulk_import` distinguishes a single-record PATCH (the generic
/// create/edit form, or any direct API call hitting that same route)
/// from excel_import.rs's own internal reconciliation-by-spreadsheet
/// path. `false` for every normal caller — pass `true` only from
/// excel_import.rs, and only because that call site's own doc comment
/// already explains, in detail, exactly why it's a sanctioned way to
/// change a field this function otherwise blocks (see
/// `is_single_record_edit_blocked_field` above).
pub fn update(
    conn: &Connection,
    business_id: &str,
    user_id: &str,
    module_id: &str,
    record_id: &str,
    body: &Map<String, Value>,
    bulk_import: bool,
) -> Result<()> {
    rbac::require(conn, user_id, module_id, "update")?;
    // record_id arrives straight from the URL path — every record ID
    // this system ever creates is a UUIDv4 (see insert_validated_record),
    // so anything else is either a typo'd URL or a probe, not a
    // legitimate request. Rejecting it here gives a clear error
    // immediately rather than a generic "record not found" further
    // down, and — same chokepoint reasoning as validate_table_name in
    // load_module — costs nothing and closes the gap between what
    // security.rs claims to validate and what actually gets checked.
    crate::security::validate_uuid(record_id)?;
    let module = load_module(conn, business_id, module_id)?;
    let table = module.table_name();

    let valid_fields: std::collections::HashSet<&str> =
        module.fields.iter().map(|f| f.name.as_str()).collect();

    let mut sets = vec![];
    let mut values: Vec<Box<dyn rusqlite::ToSql>> = vec![];
    let mut idx = 1;
    for (k, v) in body {
        if !valid_fields.contains(k.as_str()) {
            return Err(anyhow!("'{k}' is not a field on module '{module_id}'"));
        }
        if !bulk_import && is_single_record_edit_blocked_field(module_id, k) {
            return Err(anyhow!(
                "'{k}' cannot be edited directly on '{module_id}' — use the sell, receive, refund, or repack action instead"
            ));
        }
        sets.push(format!("{k} = ?{idx}"));
        values.push(value_to_sql(v));
        idx += 1;
    }
    if sets.is_empty() {
        return Err(anyhow!("no fields provided to update"));
    }

    let record: std::collections::HashMap<String, Value> = body.clone().into_iter().collect();
    // Same type enforcement as create — a "money" field being edited
    // is exactly as forbidden from accepting a float as one being
    // created. This was previously missing entirely: update() only
    // checked that field NAMES were valid, never that a provided
    // value's TYPE matched the field's declared type, which meant a
    // float dollar amount could bypass the integer-cents contract
    // simply by going through an edit instead of a create.
    module.validate_partial(&record)?;
    crate::reference_data::validate_field_references(conn, business_id, &module, &record)?;

    // Same "never sell at a loss" rule as create(), applied here too —
    // an edit is just as capable of putting a bad price on an item as
    // a create is. This is a PATCH, so unlike create() the value being
    // compared against might not be in `record` at all (e.g. someone
    // only edits unit_price and leaves unit_cost untouched) — for
    // whichever of the pair is missing from this update, fall back to
    // what's already stored for this record rather than treating an
    // absent field as zero, which would wrongly wave through a real
    // cost the caller just didn't happen to resend.
    if module_id == "inventory" && (record.contains_key("unit_cost") || record.contains_key("unit_price")) {
        let (stored_cost, stored_price): (i64, i64) = conn
            .query_row(
                &format!("SELECT unit_cost, unit_price FROM {table} WHERE id = ?1 AND business_id = ?2 AND deleted_at IS NULL"),
                params![record_id, business_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| anyhow!("record not found"))?;
        let unit_cost = record.get("unit_cost").and_then(|v| v.as_i64()).unwrap_or(stored_cost);
        let unit_price = record.get("unit_price").and_then(|v| v.as_i64()).unwrap_or(stored_price);
        if unit_price < unit_cost {
            return Err(anyhow!(
                "selling price cannot be lower than the cost price — this would sell at a loss"
            ));
        }
    }

    sets.push("updated_at = datetime('now')".to_string());

    let sql = format!(
        "UPDATE {table} SET {} WHERE id = ?{idx} AND business_id = ?{} AND deleted_at IS NULL",
        sets.join(", "),
        idx + 1
    );
    values.push(Box::new(record_id.to_string()));
    values.push(Box::new(business_id.to_string()));

    let params_refs: Vec<&dyn rusqlite::ToSql> = values.iter().map(|b| b.as_ref()).collect();
    let changed = conn.execute(&sql, params_refs.as_slice())?;
    if changed == 0 {
        return Err(anyhow!("record not found"));
    }

    audit::log(conn, business_id, Some(user_id), module_id, "update", Some(record_id), Some(&json!(body)))?;
    Ok(())
}

/// DELETE — soft delete only (sets deleted_at). Real destructive deletes
/// are deliberately not exposed here: an owner who "deleted by accident"
/// should be recoverable, and the audit trail should show what disappeared
/// and when, not just silently lose the row.
pub fn delete(
    conn: &Connection,
    business_id: &str,
    user_id: &str,
    module_id: &str,
    record_id: &str,
) -> Result<()> {
    rbac::require(conn, user_id, module_id, "delete")?;
    crate::security::validate_uuid(record_id)?;
    let module = load_module(conn, business_id, module_id)?;
    let table = module.table_name();

    let sql = format!(
        "UPDATE {table} SET deleted_at = datetime('now') WHERE id = ?1 AND business_id = ?2 AND deleted_at IS NULL"
    );
    let changed = conn.execute(&sql, rusqlite::params![record_id, business_id])?;
    if changed == 0 {
        return Err(anyhow!("record not found"));
    }

    audit::log(conn, business_id, Some(user_id), module_id, "delete", Some(record_id), None)?;
    Ok(())
}

fn value_to_sql(v: &Value) -> Box<dyn rusqlite::ToSql> {
    match v {
        Value::String(s) => Box::new(s.clone()),
        Value::Number(n) if n.is_i64() => Box::new(n.as_i64().unwrap()),
        Value::Number(n) => Box::new(n.as_f64().unwrap()),
        Value::Bool(b) => Box::new(*b as i64),
        Value::Null => Box::new(Option::<String>::None),
        other => Box::new(other.to_string()),
    }
}
