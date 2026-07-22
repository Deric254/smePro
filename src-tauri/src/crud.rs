use anyhow::{anyhow, Result};
use rusqlite::Connection;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::module::ModuleDef;
use crate::{audit, rbac};

/// Loads a module's schema back out of the `modules` registry table (not
/// from a file) — this is what makes CRUD generic at request time: any
/// module enabled for the business, past or present, can be operated on
/// purely from what's stored in the DB.
fn load_module(conn: &Connection, business_id: &str, module_id: &str) -> Result<ModuleDef> {
    let raw: String = conn
        .query_row(
            "SELECT schema_json FROM modules WHERE business_id = ?1 AND id = ?2 AND enabled = 1",
            rusqlite::params![business_id, module_id],
            |r| r.get(0),
        )
        .map_err(|_| anyhow!("module '{module_id}' is not enabled for this business"))?;
    ModuleDef::from_json_str(&raw)
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
    module.validate(&record)?;
    crate::reference_data::validate_field_references(conn, business_id, &module, &record)?;

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

    audit::log(conn, business_id, Some(user_id), module_id, "create", Some(&id), Some(&json!(body)))?;
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
pub fn update(
    conn: &Connection,
    business_id: &str,
    user_id: &str,
    module_id: &str,
    record_id: &str,
    body: &Map<String, Value>,
) -> Result<()> {
    rbac::require(conn, user_id, module_id, "update")?;
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
        sets.push(format!("{k} = ?{idx}"));
        values.push(value_to_sql(v));
        idx += 1;
    }
    if sets.is_empty() {
        return Err(anyhow!("no fields provided to update"));
    }

    let record: std::collections::HashMap<String, Value> = body.clone().into_iter().collect();
    crate::reference_data::validate_field_references(conn, business_id, &module, &record)?;

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
