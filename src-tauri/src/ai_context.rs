use anyhow::Result;
use rusqlite::Connection;
use serde_json::{json, Value};

use crate::module::ModuleDef;
use crate::report::{self, Dimension};

/// Builds a structured, bounded snapshot of a business's current state
/// across every enabled module. This is deliberately NOT a dump of raw
/// rows — an LLM prompt built from thousands of raw records is slow,
/// expensive, and prone to the model losing track of what matters. This
/// function does the aggregation work itself (reusing the same reporting
/// engine as the report screens) and only sends summarized numbers.
pub fn build_snapshot(conn: &Connection, business_id: &str, user_id: &str) -> Result<Value> {
    let mut modules_summary = serde_json::Map::new();

    let mut stmt = conn.prepare(
        "SELECT id, schema_json FROM modules WHERE business_id = ?1 AND enabled = 1",
    )?;
    let module_rows: Vec<(String, String)> = stmt
        .query_map(rusqlite::params![business_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    for (module_id, schema_json) in module_rows {
        let module = ModuleDef::from_json_str(&schema_json)?;
        let table = module.table_name();

        let record_count: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE business_id = ?1 AND deleted_at IS NULL"),
            rusqlite::params![business_id],
            |r| r.get(0),
        )?;

        // Sum every numeric field — gives a free "totals" view (revenue,
        // quantity, unit_cost, whatever the module happens to define)
        // without the context builder needing to know module-specific
        // field names in advance.
        let mut totals = serde_json::Map::new();
        for f in &module.fields {
            if f.field_type == "integer" || f.field_type == "real" {
                let points = report::run(
                    conn, business_id, user_id, &module_id, Some(&f.name), "sum", Dimension::None, None, None,
                );
                if let Ok(points) = points {
                    if let Some(p) = points.first() {
                        totals.insert(f.name.clone(), json!(p.value));
                    }
                }
            }
        }

        // Generic low-stock-style flag: if a module happens to define
        // both `quantity` and `reorder_level`, surface anything at or
        // below its reorder point. This is the one place the context
        // builder leans on a naming convention rather than pure
        // genericity — worth it for how common this pattern is in SME
        // inventory-style modules.
        let low_stock = if module.fields.iter().any(|f| f.name == "quantity")
            && module.fields.iter().any(|f| f.name == "reorder_level")
        {
            let mut low_stmt = conn.prepare(&format!(
                "SELECT name, quantity, reorder_level FROM {table}
                 WHERE business_id = ?1 AND deleted_at IS NULL AND quantity <= reorder_level
                 ORDER BY quantity ASC LIMIT 10"
            ))?;
            let has_name_field = module.fields.iter().any(|f| f.name == "name");
            if has_name_field {
                let items: Vec<Value> = low_stmt
                    .query_map(rusqlite::params![business_id], |r| {
                        Ok(json!({
                            "name": r.get::<_, String>(0)?,
                            "quantity": r.get::<_, f64>(1)?,
                            "reorder_level": r.get::<_, f64>(2)?,
                        }))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap_or_default();
                items
            } else {
                vec![]
            }
        } else {
            vec![]
        };

        modules_summary.insert(
            module_id.clone(),
            json!({
                "display_name": module.display_name,
                "record_count": record_count,
                "totals": totals,
                "low_stock_alerts": low_stock,
            }),
        );
    }

    let business_name: String = conn.query_row(
        "SELECT name FROM businesses WHERE id = ?1",
        rusqlite::params![business_id],
        |r| r.get(0),
    )?;

    Ok(json!({
        "business_name": business_name,
        "generated_at": chrono::Utc::now().to_rfc3339(),
        "modules": modules_summary,
    }))
}
