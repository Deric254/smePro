use anyhow::{anyhow, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One field in a module's schema, as authored in modules/*.json
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FieldDef {
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: String, // "text" | "integer" | "real" | "money" | "date" | "boolean" | "unit" | "currency"
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub unique: bool,
    pub default: Option<Value>,
}

/// A module's own declaration of what single number best represents it
/// at a glance — "units in stock", "total revenue", "open debts" —
/// shown on its Dashboard tile. Data-driven on purpose, same as
/// everything else in this engine: a custom module a business defines
/// themselves gets exactly the same dashboard treatment as the built-in
/// ones, just by adding this to its own JSON, no engine code changes.
/// Optional and backward-compatible — a module without one just shows
/// its record count instead, same as before this existed.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DashboardMetric {
    /// Field to aggregate. Ignored (may be omitted) when aggregation is
    /// "count", same rule as the general report engine.
    #[serde(default)]
    pub measure: Option<String>,
    /// "sum" | "count" | "avg" — same three the report engine supports.
    pub aggregation: String,
    /// Shown next to the number, e.g. "in stock", "this month".
    pub label: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ModuleDef {
    pub id: String,
    pub display_name: String,
    pub fields: Vec<FieldDef>,
    pub actions: Vec<String>,
    pub default_roles: std::collections::HashMap<String, Vec<String>>,
    #[serde(default)]
    pub dashboard_metric: Option<DashboardMetric>,
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

        if let Some(metric) = &def.dashboard_metric {
            match metric.aggregation.as_str() {
                "sum" | "avg" => {
                    let field_name = metric.measure.as_deref().ok_or_else(|| {
                        anyhow!("module '{}': dashboard_metric aggregation '{}' requires a measure field", def.id, metric.aggregation)
                    })?;
                    let field = def.fields.iter().find(|f| f.name == field_name).ok_or_else(|| {
                        anyhow!("module '{}': dashboard_metric measure '{field_name}' is not a field on this module", def.id)
                    })?;
                    if field.field_type != "integer" && field.field_type != "real" && field.field_type != "money" {
                        return Err(anyhow!(
                            "module '{}': dashboard_metric measure '{field_name}' must be numeric (integer/real/money), got '{}'",
                            def.id, field.field_type
                        ));
                    }
                }
                "count" => {} // no measure needed
                other => return Err(anyhow!("module '{}': dashboard_metric aggregation must be sum/avg/count, got '{other}'", def.id)),
            }
        }

        Ok(def)
    }

    fn sql_type(field_type: &str) -> Result<&'static str> {
        match field_type {
            "text" | "date" | "unit" | "currency" => Ok("TEXT"),
            // "money" is deliberately its own type, distinct from
            // "integer": both map to INTEGER affinity, but "money"
            // carries the semantic meaning "this is minor-unit
            // currency" through to validation, the frontend formatter,
            // and xlsx export, none of which should guess based on a
            // field's name alone.
            "integer" | "boolean" | "money" => Ok("INTEGER"),
            "real" => Ok("REAL"),
            other => Err(anyhow!("unsupported field type: {other}")),
        }
    }

    /// Builds the `"name TYPE [NOT NULL] [UNIQUE]"` column definitions
    /// for this module's own fields (not the fixed system columns —
    /// id/business_id/created_at/updated_at/deleted_at are added by
    /// each caller, since a table rebuild needs to place them at
    /// specific positions matching the existing table). Shared by
    /// `create_table` (fresh tables) and the v8 migration's table
    /// rebuild (existing tables whose column affinity needs to
    /// change) so both are derived from exactly the same logic —
    /// nothing for the two to drift apart on.
    pub(crate) fn field_column_defs(&self) -> Result<Vec<String>> {
        self.fields
            .iter()
            .map(|f| {
                let sql_ty = Self::sql_type(&f.field_type)?;
                let mut col = format!("{} {}", f.name, sql_ty);
                if f.required {
                    col.push_str(" NOT NULL");
                }
                if f.unique {
                    col.push_str(" UNIQUE");
                }
                Ok(col)
            })
            .collect()
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
        cols.extend(self.field_column_defs()?);
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
<<<<<<< HEAD
                Some(v) => self.validate_field_value(f, v)?,
=======
                Some(v) => {
                    let ok = match f.field_type.as_str() {
                        "text" | "date" | "unit" | "currency" => v.is_string(),
                        "integer" => v.is_i64() || v.is_u64(),
                        // Money is ALWAYS integer minor units by the time
                        // it reaches storage — never a float. Any decimal
                        // dollar input from a human is converted via
                        // money::parse_money_input() at the API boundary,
                        // before it ever gets here. A float arriving at
                        // this point means something upstream skipped
                        // that conversion, which is exactly the bug this
                        // whole migration exists to prevent — so it's
                        // rejected here, not silently truncated.
                        "money" => v.is_i64() || v.is_u64(),
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
>>>>>>> 3071f825f10981753eb48b13f905fa2dd375c583
                None => {} // optional field, no value given — fine
            }
        }
        Ok(())
    }

    /// Same type-correctness checks as `validate`, but for a PATCH-style
    /// partial update: a field simply absent from `record` is never an
    /// error here, even if it's normally required — the existing stored
    /// value for that field isn't changing, so there's nothing to
    /// validate about it. Only the fields actually present in `record`
    /// are checked, and checked against exactly the same type rules as
    /// a fresh create — a "money" field being updated is just as
    /// forbidden from silently accepting a float as one being created.
    pub fn validate_partial(&self, record: &std::collections::HashMap<String, Value>) -> Result<()> {
        for f in &self.fields {
            if let Some(v) = record.get(&f.name) {
                self.validate_field_value(f, v)?;
            }
        }
        Ok(())
    }

    fn validate_field_value(&self, f: &FieldDef, v: &Value) -> Result<()> {
        let ok = match f.field_type.as_str() {
            "text" | "date" | "unit" | "currency" => v.is_string(),
            "integer" => v.is_i64() || v.is_u64(),
            // Money is ALWAYS integer minor units by the time it
            // reaches storage — never a float. Any decimal dollar
            // input from a human is converted via
            // money::parse_money_input() at the API boundary, before
            // it ever gets here. A float arriving at this point means
            // something upstream skipped that conversion, which is
            // exactly the bug this whole migration exists to prevent
            // — so it's rejected here, not silently truncated. This
            // applies identically whether the record is being created
            // or updated — there's no separate, weaker check for
            // edits that a float could sneak through.
            "money" => v.is_i64() || v.is_u64(),
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
        Ok(())
    }
}
