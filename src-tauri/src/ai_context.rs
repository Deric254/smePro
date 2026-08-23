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

    // Needed to correctly present "money"-typed totals to the AI as
    // decimal currency (e.g. 4500.00) rather than raw integer cents
    // (450000) — an LLM prompt with an unlabeled 100x-inflated number
    // is exactly the kind of thing that produces a confidently wrong
    // answer about someone's own revenue.
    let currency: String = conn
        .query_row("SELECT currency FROM businesses WHERE id = ?1", rusqlite::params![business_id], |r| r.get(0))
        .unwrap_or_else(|_| "USD".to_string());

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
        // field names in advance. "money" fields are summed the same
        // way as "integer"/"real" (the underlying column is INTEGER
        // cents, and SQLite SUM over integers stays exact for any
        // realistic amount), but converted to decimal currency before
        // being handed to the AI — see the currency lookup above.
        let mut totals = serde_json::Map::new();
        for f in &module.fields {
            if f.field_type == "integer" || f.field_type == "real" || f.field_type == "money" {
                let points = report::run(
                    conn, business_id, user_id, &module_id,
                    report::ReportQuery {
                        measure_field: Some(&f.name),
                        aggregation: "sum",
                        dimension: Dimension::None,
                        range_start: None,
                        range_end: None,
                    },
                );
                if let Ok(points) = points {
                    if let Some(p) = points.first() {
                        if f.field_type == "money" {
                            let places = crate::money::decimal_places_for(&currency);
                            let scale = 10_i64.pow(places) as f64;
                            totals.insert(f.name.clone(), json!(p.value / scale));
                        } else {
                            totals.insert(f.name.clone(), json!(p.value));
                        }
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

        let operational_details = match module_id.as_str() {
            "inventory" => {
                let places = crate::money::decimal_places_for(&currency);
                let scale = 10_i64.pow(places) as f64;
                let mut detail_stmt = conn.prepare(&format!(
                    "SELECT sku, name, quantity, unit_cost, unit_price, reorder_level
                     FROM {table} WHERE business_id = ?1 AND deleted_at IS NULL
                     ORDER BY name LIMIT 100"
                ))?;
                let items: Vec<Value> = detail_stmt
                    .query_map(rusqlite::params![business_id], |r| {
                        Ok(json!({
                            "sku": r.get::<_, String>(0)?,
                            "name": r.get::<_, String>(1)?,
                            "quantity": r.get::<_, i64>(2)?,
                            "unit_cost": r.get::<_, i64>(3)? as f64 / scale,
                            "unit_price": r.get::<_, i64>(4)? as f64 / scale,
                            "reorder_level": r.get::<_, Option<i64>>(5)?,
                        }))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                json!(items)
            }
            "purchasing" => {
                let places = crate::money::decimal_places_for(&currency);
                let scale = 10_i64.pow(places) as f64;
                let mut detail_stmt = conn.prepare(&format!(
                    "SELECT supplier, item_name, inventory_record_id, quantity, unit_cost, received
                     FROM {table} WHERE business_id = ?1 AND deleted_at IS NULL
                     ORDER BY order_date DESC, created_at DESC LIMIT 100"
                ))?;
                let purchases: Vec<Value> = detail_stmt
                    .query_map(rusqlite::params![business_id], |r| {
                        Ok(json!({
                            "supplier": r.get::<_, String>(0)?,
                            "item_name": r.get::<_, String>(1)?,
                            "inventory_record_id": r.get::<_, Option<String>>(2)?,
                            "quantity": r.get::<_, i64>(3)?,
                            "unit_cost": r.get::<_, i64>(4)? as f64 / scale,
                            "received": r.get::<_, bool>(5)?,
                        }))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                json!(purchases)
            }
            _ => Value::Null,
        };

        modules_summary.insert(
            module_id.clone(),
            json!({
                "display_name": module.display_name,
                "record_count": record_count,
                "totals": totals,
                "low_stock_alerts": low_stock,
                "operational_details": operational_details,
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
        "currency": currency,
        "generated_at": chrono::Utc::now().to_rfc3339(),
        "modules": modules_summary,
    }))
}
