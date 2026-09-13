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
    /// Inclusive floor for "integer"/"money" fields, in the same unit
    /// the field is stored in (whole units for integer, minor units —
    /// cents — for money). `None` means no floor at all, which is the
    /// deliberate default: negative is legitimate for fields like a
    /// sale's `discount_amount` (see money.rs's `parse_money_input`
    /// doc comment on why negative is allowed there), so this is never
    /// a blanket "reject all negative money" switch — each module's
    /// own JSON opts a specific field in with e.g. `"min": 0`, which
    /// every quantity/cost/price field in the shipped modules now
    /// does. Checked in `validate_field_value` alongside the existing
    /// type check, so a value must be both the right type AND within
    /// this floor before it's accepted — a raw API call bypassing the
    /// UI can no longer write e.g. -500 units or a negative unit_cost
    /// on any module, built-in or custom.
    #[serde(default)]
    pub min: Option<i64>,
    /// Cross-field floor: this field's value may never be lower than
    /// the value of the OTHER field named here, in the same record.
    /// Unlike `min` (a fixed number), the floor here is itself another
    /// field's value — the canonical case is inventory's `unit_price`
    /// declaring `unit_cost` as its `min_field`, so a selling price can
    /// never be saved below its own cost price. Declared per-field, in
    /// the module's own JSON, so any module — built-in or custom — gets
    /// this protection for any comparable pair of `integer`/`money`
    /// fields just by setting it; the engine (see
    /// `ModuleDef::validate_cross_field_floors`) has never heard of
    /// "inventory" specifically. `None` (the default) means no
    /// cross-field constraint. Only meaningful for `integer`/`money`
    /// fields; ignored otherwise.
    #[serde(default)]
    pub min_field: Option<String>,
    /// Optional human-readable error to use instead of the generic
    /// "'X' cannot be lower than 'Y'" message when `min_field`'s check
    /// fails — e.g. inventory's actual message, "selling price cannot
    /// be lower than the cost price — this would sell at a loss", is
    /// far clearer than a field-name-only fallback would be. Ignored
    /// when `min_field` is None.
    #[serde(default)]
    pub min_field_message: Option<String>,
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
    /// Which of this module's own `date`-typed fields represents when
    /// the record actually happened, for reporting/forecasting
    /// purposes — as opposed to `created_at`, which is only ever when
    /// the row was typed into this app. Optional and backward-
    /// compatible: a module without one (the common case, where the
    /// two are effectively the same thing) just uses `created_at`,
    /// same as every module did before this existed. Declare this
    /// when a module lets someone backdate a record — accounting.json
    /// sets it to "transaction_date" for exactly that reason: a
    /// bookkeeper entering last Tuesday's expense today needs it to
    /// land in last Tuesday's numbers, not today's, or every report
    /// and forecast built on this module silently misattributes it.
    /// See forecast.rs's `time_field_for` for where this gets read.
    #[serde(default)]
    pub date_field: Option<String>,
}

impl ModuleDef {
    /// The field reports/forecasts should bucket this module's records
    /// by — `date_field` if the module declares one, `created_at`
    /// otherwise. Doesn't validate the field exists or is actually a
    /// `date` field itself; `report::run`'s own field-existence check
    /// (it already validates any `Dimension::Time` field against this
    /// module's real field list) is the enforcement point for that,
    /// so a typo'd `date_field` fails loudly as "not a field on this
    /// module" rather than silently falling back to created_at.
    pub fn time_field(&self) -> &str {
        self.date_field.as_deref().unwrap_or("created_at")
    }
}

/// SQL column/table names are RESERVED — every module table already has
/// these; a field trying to reuse one of them would silently collide
/// with (or, combined with the injection risk below, deliberately
/// shadow) a real system column.
const RESERVED_COLUMN_NAMES: &[&str] = &["id", "business_id", "created_at", "updated_at", "deleted_at", "created_by"];

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

    /// Builds the `"name TYPE [NOT NULL]"` column definitions for this
    /// module's own fields (not the fixed system columns —
    /// id/business_id/created_at/updated_at/deleted_at are added by
    /// each caller, since a table rebuild needs to place them at
    /// specific positions matching the existing table). Shared by
    /// `create_table` (fresh tables) and the v8/v13 migrations' table
    /// rebuilds (existing tables whose column affinity or constraints
    /// need to change) so all three are derived from exactly the same
    /// logic — nothing for them to drift apart on.
    ///
    /// Deliberately does NOT emit a bare `UNIQUE` for a field marked
    /// `unique: true` — see `business_scoped_unique_constraints` below
    /// for why a plain column-level UNIQUE is wrong for this table
    /// shape and what's emitted instead.
    pub(crate) fn field_column_defs(&self) -> Result<Vec<String>> {
        self.fields
            .iter()
            .map(|f| {
                let sql_ty = Self::sql_type(&f.field_type)?;
                let mut col = format!("{} {}", f.name, sql_ty);
                if f.required {
                    col.push_str(" NOT NULL");
                }
                Ok(col)
            })
            .collect()
    }

    /// Table-level `UNIQUE(business_id, field)` constraints for every
    /// field marked `unique: true` — e.g. inventory's `sku`.
    ///
    /// THE BUG THIS FIXES: every module table is one single physical
    /// table shared across every business in this install (rows
    /// distinguished by the `business_id` column — see `table_name`
    /// and every query in crud.rs/pos.rs/etc, all of which filter on
    /// it), the same as every hand-written table in db_migrations.rs
    /// already does correctly (tax_rates: UNIQUE(business_id,
    /// category), customers: UNIQUE(business_id, phone)). A bare
    /// column-level `UNIQUE` on `sku` alone — what this code used to
    /// emit — enforces uniqueness across ALL businesses at once, not
    /// within one: two completely unrelated businesses on the same
    /// install could never both have a product called "SKU-001",
    /// which is exactly the kind of collision a real multi-business
    /// install would hit immediately and have no way to explain to
    /// either owner. Scoping the constraint by business_id is strictly
    /// weaker than what it replaces, never stronger — it can only
    /// permit combinations the old constraint wrongly forbade, never
    /// the reverse, so migrating existing data into it (see v13) can
    /// never itself create a new constraint violation.
    pub(crate) fn business_scoped_unique_constraints(&self) -> Vec<String> {
        self.fields
            .iter()
            .filter(|f| f.unique)
            .map(|f| format!("UNIQUE(business_id, {})", f.name))
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
        // Nullable, deliberately: not every record has a real human
        // author to attribute — see excel_import.rs bulk imports,
        // system-generated Bookkeeping entries created as a
        // side-effect of some other action (a sale, a receiving), and
        // every record that existed before this column did (v25's
        // migration leaves those NULL rather than guessing). Where it
        // IS known — a person filling out this module's own create
        // form, a cashier ringing up a sale — crud::insert_validated_
        // record's `created_by` parameter records it. See rbac::
        // ReadScope::Own for the one thing this column exists to
        // support: a `read_own` permission that can honestly filter
        // "records this user created" only where that's actually true.
        cols.push("created_by TEXT".to_string());
        cols.push("deleted_at TEXT".to_string()); // soft delete, keeps audit trail meaningful
        // Business-scoped, not global — see that method's own doc
        // comment for the bug this avoids.
        cols.extend(self.business_scoped_unique_constraints());

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

        // Inventory-specific: `excel_import::find_inventory_id_by_name`
        // (used on every single row of a Purchasing import, to resolve
        // `item_name` to the actual Inventory record) looks records up
        // by `LOWER(TRIM(name))`, and nothing above gives it anything
        // to use — the business-scoped unique index only exists for
        // fields actually marked `unique` (sku), and name isn't one of
        // those. Without this, that lookup is a full scan of every
        // inventory row this business has, for every single row of
        // every Purchasing import — fine at a handful of SKUs,
        // genuinely slow at the thousands a real catalog reaches.
        //
        // THE ACTUAL FIX Deric asked for: this index used to just be a
        // plain (non-unique) lookup accelerator — it made the lookup
        // above fast, but did nothing to stop two DIFFERENT SKUs from
        // sharing the same item name, which is real, confusing
        // duplication a shop owner never wants (two "Rice" rows with
        // different SKUs is a data-entry mistake almost every time, not
        // a legitimate distinct product). Making it a genuine UNIQUE
        // index — on the exact same `LOWER(TRIM(name))` expression the
        // lookup query above already uses, so it still serves that
        // lookup exactly as fast as the plain index did — closes that
        // gap at the one place it can never be forgotten or bypassed:
        // the database itself, covering create, update, AND import
        // alike, not just whichever single code path remembered to
        // check first.
        //
        // `WHERE deleted_at IS NULL` is deliberate, not decorative: a
        // soft-deleted "Rice" must not permanently block a real, later
        // "Rice" from ever being added again — every other query
        // against this table already treats a soft-deleted row as
        // gone (see idx_..._business above, crud::list, etc.); this
        // index holds itself to the same standard. Deliberately
        // case- and whitespace-insensitive (`LOWER(TRIM(...))`) for
        // the same reason `find_inventory_id_by_name` already resolves
        // names that way: "Rice", "rice", and " Rice " are the same
        // real-world item to a cashier typing fast, and treating them
        // as three different products would be its own kind of
        // inconsistency.
        if self.id == "inventory" {
            tx.execute(
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_module_inventory_unique_name
                 ON module_inventory(business_id, LOWER(TRIM(name)))
                 WHERE deleted_at IS NULL;",
                [],
            )?;
        }

        // Refunds-specific: refund.rs looks up "everything already
        // refunded against this sale" TWICE per refund now (a
        // SUM(quantity_refunded) that already existed, and a
        // SUM(cost_reversed) added alongside gross profit tracking —
        // see refund.rs's own doc comment on why that second lookup
        // exists), both filtered on `sale_id` — a column the
        // business-wide `idx_..._business` index above does nothing
        // for, since it only covers `business_id`/`deleted_at`.
        // Without this, refunding a sale on a business with a long
        // refund history means scanning every refund that business has
        // ever recorded, twice, on every single refund going forward —
        // exactly the kind of thing that's invisible on a fresh install
        // and only starts to bite once real data has piled up.
        if self.id == "refunds" {
            tx.execute(
                "CREATE INDEX IF NOT EXISTS idx_module_refunds_sale_id
                 ON module_refunds(business_id, sale_id);",
                [],
            )?;
        }

        // Sales-specific: basket_analysis.rs's "frequently bought
        // together" report self-joins this table on order_id (every
        // POS/service-sale checkout writes one order_id across all its
        // line items — see pos.rs) to find which items appear in the
        // same order. Same reasoning as the refunds index just above:
        // without a dedicated index on order_id, that self-join has
        // nothing but the business-wide index to lean on and degrades
        // to scanning the whole sales table on every request once a
        // business has real history. Same pair on new installs (via
        // this code path) and existing ones (via db_migrations.rs'
        // v20) so behavior never depends on which path created the
        // table.
        if self.id == "sales" {
            tx.execute(
                "CREATE INDEX IF NOT EXISTS idx_module_sales_order_id
                 ON module_sales(business_id, order_id);",
                [],
            )?;
        }

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
                Some(v) => self.validate_field_value(f, v)?,
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

    /// Checks every field that declares a `min_field` against the field
    /// it points at, for whichever of the two are actually present in
    /// `record` this call. Schema-driven and module-agnostic on
    /// purpose — this used to be a block hand-written in `crud.rs` that
    /// only ever fired `if module_id == "inventory"`, which meant a
    /// business's own custom module with its own cost/price-style pair
    /// got no such protection no matter what its JSON said. Moving the
    /// rule here, keyed off `min_field` instead of a hardcoded module
    /// id, gives every module — built-in or custom — the identical
    /// check for free.
    ///
    /// `record` is whatever the caller is actually writing this call —
    /// a fresh create (which already has every field, defaults
    /// included) or a PATCH-style update (which may have only one side
    /// of the pair). `lookup_stored` is how a caller supplies whichever
    /// side isn't in `record`: `create()` has nothing stored yet, so it
    /// passes a closure that always returns `None` (falling through to
    /// 0, exactly as both fields' own `default: 0` already implies);
    /// `update()` passes a closure that queries the field's current
    /// value from the database, since an absent field there means
    /// "unchanged," not "zero."
    ///
    /// A pair is only checked at all when at least one side is present
    /// in `record` — an update that touches neither field has nothing
    /// new to validate, and skipping it also means `lookup_stored`
    /// (which may hit the database) is never called needlessly.
    pub fn validate_cross_field_floors(
        &self,
        record: &std::collections::HashMap<String, Value>,
        lookup_stored: impl Fn(&str) -> Option<i64>,
    ) -> Result<()> {
        for f in &self.fields {
            let Some(floor_field) = &f.min_field else { continue };
            if !matches!(f.field_type.as_str(), "integer" | "money") {
                continue;
            }
            if !record.contains_key(&f.name) && !record.contains_key(floor_field) {
                continue;
            }
            let value = record
                .get(&f.name)
                .and_then(|v| v.as_i64())
                .or_else(|| lookup_stored(&f.name))
                .unwrap_or(0);
            let floor = record
                .get(floor_field)
                .and_then(|v| v.as_i64())
                .or_else(|| lookup_stored(floor_field))
                .unwrap_or(0);
            if value < floor {
                return Err(anyhow!(
                    "{}",
                    f.min_field_message.clone().unwrap_or_else(|| format!(
                        "'{}' cannot be lower than '{}'",
                        f.name, floor_field
                    ))
                ));
            }
        }
        Ok(())
    }

    fn validate_field_value(&self, f: &FieldDef, v: &Value) -> Result<()> {
        let ok = match f.field_type.as_str() {
            // "text"/"unit"/"currency" need only be a string — no
            // further shape implied by the type. "date" is a string
            // too at the JSON level, but gets its own real
            // calendar-date check below, past this initial type gate.
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

        // THE GAP THIS CLOSES: the check above only ever confirmed
        // "is this the right JSON type" — a "money" field being an
        // integer at all, never whether that integer made sense.
        // -50 is exactly as valid an i64 as 50 was, so a negative
        // unit_cost or unit_price sailed straight through, and two
        // negatives together could even pass inventory's own separate
        // "price >= cost" check (-50 >= -100 is true). This is the
        // floor half of that fix — see `FieldDef::min`'s own doc
        // comment for why it's opt-in per field rather than a global
        // "no negative money" rule.
        if let Some(min) = f.min {
            if matches!(f.field_type.as_str(), "integer" | "money") {
                let n = v.as_i64().or_else(|| v.as_u64().map(|u| u as i64));
                if let Some(n) = n {
                    if n < min {
                        return Err(anyhow!(
                            "field '{}' must be at least {min}, got {n}",
                            f.name
                        ));
                    }
                }
            }
        }

        // THE GAP THIS CLOSES: `required: true` only ever meant "the
        // key is present in the record" — an empty or whitespace-only
        // string satisfied that and nothing else, so a customer name
        // or item name of "" (or "   ") was accepted as a complete,
        // valid required field. A value simply isn't present here
        // (see `validate`/`validate_partial` above) is a different,
        // already-handled case; this is specifically about a value
        // that IS present but carries no real content.
        if f.required && f.field_type == "text" {
            if let Some(s) = v.as_str() {
                if s.trim().is_empty() {
                    return Err(anyhow!("field '{}' is required and cannot be empty", f.name));
                }
            }
        }

        // THE GAP THIS CLOSES: a "date" field's type check was just
        // `is_string()` — any string at all, including something like
        // "tomorrow" or "13/45/2026", passed silently and only
        // misbehaved much later (wrong bucket, wrong sort order) in
        // every date-range report that reads it back, with no error
        // anywhere close to where the bad value was actually entered.
        // Every date this app itself ever produces is the same
        // `chrono` ISO shape (see e.g. `Utc::now().date_naive()`
        // callers throughout http_api.rs/pos.rs), so that's the one
        // shape accepted here too — not a looser format that would
        // just move the "does this actually parse" problem elsewhere.
        // An empty, optional date field is left alone: `""` for a
        // field that isn't required is "not set", not "an invalid
        // date", and the `required`-empty-string check above already
        // handles the case where that's not allowed.
        if f.field_type == "date" {
            if let Some(s) = v.as_str() {
                if !(s.is_empty() && !f.required)
                    && chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_err()
                {
                    return Err(anyhow!(
                        "field '{}' must be a valid date in YYYY-MM-DD form, got '{}'",
                        f.name,
                        s
                    ));
                }
            }
        }

        // Storage-bloat guard, not a correctness rule: no module field
        // ever legitimately needs more than this many characters, and
        // an unbounded TEXT column otherwise has no ceiling at all — a
        // raw API call (or a pasted document) could write megabytes
        // into a single field. Generous enough that no real "notes" or
        // free-text field anyone actually writes should ever hit it.
        const MAX_TEXT_LEN: usize = 20_000;
        if f.field_type == "text" {
            if let Some(s) = v.as_str() {
                if s.len() > MAX_TEXT_LEN {
                    return Err(anyhow!(
                        "field '{}' is too long ({} characters, max {MAX_TEXT_LEN})",
                        f.name,
                        s.len()
                    ));
                }
            }
        }

        Ok(())
    }
}
