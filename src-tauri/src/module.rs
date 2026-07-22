use anyhow::{anyhow, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One field in a module's schema, as authored in modules/*.json
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FieldDef {
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: String, // "text" | "integer" | "real" | "date" | "boolean"
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub unique: bool,
    pub default: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ModuleDef {
    pub id: String,
    pub display_name: String,
    pub fields: Vec<FieldDef>,
    pub actions: Vec<String>,
    pub default_roles: std::collections::HashMap<String, Vec<String>>,
}

/// SQL column/table names are RESERVED — every module table already has
/// these; a field trying to reuse one of them would silently collide
/// with (or, combined with the injection risk below, deliberately
/// shadow) a real system column.
const RESERVED_COLUMN_NAMES: &[&str] = &["id", "business_id", "created_at", "updated_at", "deleted_at"];

/// Validates that `name` is safe to interpolate directly into raw SQL
/// as an identifier (a table or column name) — which is exactly what
/// happens with every module id and field name, throughout
/// `module.rs`, `crud.rs`, and `report.rs`. None of those call sites
/// use parameterized queries for identifiers (SQL doesn't support
/// parameterizing identifiers the way it does values), so this
/// validation is the ONLY thing standing between a malicious or
/// malformed module definition and genuine SQL injection into DDL and
/// DML alike. Checked once here, at parse time, rather than needing
/// every individual SQL-building call site to remember to re-check it.
fn validate_identifier(name: &str, kind: &str) -> Result<()> {
    if name.is_empty() {
        return Err(anyhow!("{kind} name cannot be empty"));
    }
    if name.len() > 64 {
        return Err(anyhow!("{kind} name '{name}' is too long (max 64 characters)"));
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(anyhow!("{kind} name '{name}' must start with a letter or underscore"));
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(anyhow!(
            "{kind} name '{name}' may only contain letters, numbers, and underscores — \
             this is a hard requirement, not a style preference: this name gets used \
             directly as a SQL column/table name"
        ));
    }
    if RESERVED_COLUMN_NAMES.contains(&name) {
        return Err(anyhow!("{kind} name '{name}' is reserved by the engine and can't be reused"));
    }
    Ok(())
}

impl ModuleDef {
    pub fn from_json_str(raw: &str) -> Result<Self> {
        let def: ModuleDef = serde_json::from_str(raw)?;

        validate_identifier(&def.id, "module id")?;
        for f in &def.fields {
            validate_identifier(&f.name, "field")?;
        }
        // Field names must also be unique — a duplicate would make the
        // generated CREATE TABLE ambiguous or outright invalid.
        let mut seen = std::collections::HashSet::new();
        for f in &def.fields {
            if !seen.insert(f.name.as_str()) {
                return Err(anyhow!("duplicate field name '{}' in module '{}'", f.name, def.id));
            }
        }

        Ok(def)
    }

    fn sql_type(field_type: &str) -> Result<&'static str> {
        match field_type {
            "text" | "date" | "unit" | "currency" => Ok("TEXT"),
            "integer" | "boolean" => Ok("INTEGER"),
            "real" => Ok("REAL"),
            other => Err(anyhow!("unsupported field type: {other}")),
        }
    }

    /// Generates and runs `CREATE TABLE IF NOT EXISTS module_<id> (...)`
    /// derived entirely from the JSON field definitions. This is the
    /// mechanism that lets new modules be added with zero code changes:
    /// drop a new JSON file in modules/, call this once, done.
    pub fn create_table(&self, conn: &mut Connection, business_id: &str) -> Result<()> {
        let table_name = self.table_name();
        let mut cols = vec![
            "id TEXT PRIMARY KEY".to_string(),
            "business_id TEXT NOT NULL".to_string(),
        ];

        for f in &self.fields {
            let sql_ty = Self::sql_type(&f.field_type)?;
            let mut col = format!("{} {}", f.name, sql_ty);
            if f.required {
                col.push_str(" NOT NULL");
            }
            if f.unique {
                col.push_str(" UNIQUE");
            }
            cols.push(col);
        }
        cols.push("created_at TEXT NOT NULL".to_string());
        cols.push("updated_at TEXT NOT NULL".to_string());
        cols.push("deleted_at TEXT".to_string()); // soft delete, keeps audit trail meaningful

        // Everything below is one atomic unit: if index creation or the
        // registry insert fails for any reason, the table creation rolls
        // back too, rather than leaving a partially-created table behind
        // (exactly what happened before this fix, when a malformed
        // module definition left a truncated table in place even though
        // the overall operation reported failure).
        let tx = conn.transaction()?;

        let create_sql = format!(
            "CREATE TABLE IF NOT EXISTS {table_name} ({});",
            cols.join(", ")
        );
        tx.execute(&create_sql, [])?;

        // Every query against a module table filters on exactly this
        // pair (business_id, deleted_at) — crud::list, report::run,
        // ai_context's totals, xlsx export, forecast's history series,
        // all of them. Without this index every one of those is a full
        // table scan; with it, they're a direct lookup. Cheap to add,
        // and the kind of thing that only starts to visibly matter once
        // a business has been running long enough to accumulate real
        // data — exactly when it's most annoying to discover missing.
        let index_sql = format!(
            "CREATE INDEX IF NOT EXISTS idx_{table_name}_business ON {table_name}(business_id, deleted_at);"
        );
        tx.execute(&index_sql, [])?;

        // Register (or update) this module against the business in the
        // core `modules` registry table.
        tx.execute(
            "INSERT INTO modules (id, business_id, display_name, schema_json, enabled, table_created, created_at)
             VALUES (?1, ?2, ?3, ?4, 1, 1, datetime('now'))
             ON CONFLICT(business_id, id) DO UPDATE SET
                schema_json = excluded.schema_json,
                table_created = 1",
            rusqlite::params![
                self.id,
                business_id,
                self.display_name,
                serde_json::to_string(self)?,
            ],
        )?;

        tx.commit()?;
        Ok(())
    }

    pub fn table_name(&self) -> String {
        format!("module_{}", self.id)
    }

    /// Validates a record (field name -> value) against required/type rules
    /// before it's ever allowed to hit the database. Belt-and-suspenders
    /// alongside the SQL-level NOT NULL / UNIQUE constraints.
    pub fn validate(&self, record: &std::collections::HashMap<String, Value>) -> Result<()> {
        for f in &self.fields {
            match record.get(&f.name) {
                None if f.required && f.default.is_none() => {
                    return Err(anyhow!("missing required field: {}", f.name));
                }
                Some(v) => {
                    let ok = match f.field_type.as_str() {
                        "text" | "date" | "unit" | "currency" => v.is_string(),
                        "integer" => v.is_i64() || v.is_u64(),
                        "real" => v.is_f64() || v.is_i64(),
                        "boolean" => v.is_boolean(),
                        _ => true,
                    };
                    if !ok {
                        return Err(anyhow!(
                            "field '{}' expected type {} but got {:?}",
                            f.name,
                            f.field_type,
                            v
                        ));
                    }
                }
                None => {} // optional field, no value given — fine
            }
        }
        Ok(())
    }
}
