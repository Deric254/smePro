use anyhow::Result;
use rusqlite::Connection;
use serde_json::{json, Value};

use crate::module::ModuleDef;
use crate::rbac;
use crate::report::{self, Dimension};

/// Builds a structured, bounded snapshot of a business's current state
/// across every enabled module. This is deliberately NOT a dump of raw
/// rows — an LLM prompt built from thousands of raw records is slow,
/// expensive, and prone to the model losing track of what matters. This
/// function does the aggregation work itself (reusing the same reporting
/// engine as the report screens) and only sends summarized numbers.
pub fn build_snapshot(conn: &Connection, business_id: &str, user_id: &str) -> Result<Value> {
    let mut modules_summary = serde_json::Map::new();

    // THE BUG THIS FIXES: every field below (record_count, low-stock
    // alerts, operational_details) was built for EVERY enabled module
    // regardless of whether the person asking the AI a question could
    // read that module at all — `totals` was the only piece that
    // happened to be safe, purely as a side effect of report::run
    // enforcing rbac internally and this function silently swallowing
    // its error. A cashier without Debt & Credit's own `read`
    // permission could still ask the assistant a question and have it
    // answer from that module's row count, low-stock list, or full
    // item-level detail. Same standard as everywhere else in this
    // app: no `read` on a module means that module doesn't exist for
    // this request, full stop.
    //
    // `can_view_reports` is a second, coarser gate on top of that —
    // see roles::set_reports_flag's own doc comment for why it's
    // separate from `read`. It governs `totals` specifically (the
    // aggregate, business-performance-style numbers: total revenue,
    // total outstanding debt, and so on) — not whether a module
    // appears at all, and not its low-stock alerts or item-level
    // detail, both of which stay tied to plain `read` the same way
    // the POS screen's own restocking awareness does (see
    // pos::low_stock_items). A cashier who can read Inventory to ring
    // up sales can still ask the assistant what's running low; they
    // just won't get handed a revenue figure through the back door
    // now that the Dashboard and Reports page no longer show them
    // one directly.
    let can_view_reports = rbac::require_reports_access(conn, user_id).is_ok();

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
        if !rbac::is_allowed(conn, user_id, &module_id, "read").unwrap_or(false) {
            continue;
        }
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
        // Gated on can_view_reports (see this function's own doc
        // comment above) — skipped entirely, not just hidden after
        // the fact, for a role that can't view reports.
        let mut totals = serde_json::Map::new();
        if can_view_reports {
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
            // THE ACCURACY GAP THIS CLOSES: debt_credit previously fell
            // through to the `_ => Value::Null` catch-all below, so the
            // assistant had zero per-record visibility into Debt &
            // Credit — not who owes what, and critically not `settled`
            // (paid vs unpaid) at all. Asked "has X paid yet," it had
            // nothing to answer from but an aggregate total that
            // doesn't even separate paid from unpaid, owed-to from
            // owed-by — the exact setup that produces a confidently
            // wrong guess instead of a real answer, or (better, but
            // still not useful) an "I don't have that" every time.
            // `party_name` and `settled` are the two fields that
            // actually answer "who, and paid or not" — everything else
            // here is already shown in the totals a role with `read`
            // on this module can already see.
            "debt_credit" => {
                let places = crate::money::decimal_places_for(&currency);
                let scale = 10_i64.pow(places) as f64;
                let mut detail_stmt = conn.prepare(&format!(
                    "SELECT party_name, direction, amount, settled, due_date
                     FROM {table} WHERE business_id = ?1 AND deleted_at IS NULL
                     ORDER BY settled ASC, due_date ASC LIMIT 100"
                ))?;
                let debts: Vec<Value> = detail_stmt
                    .query_map(rusqlite::params![business_id], |r| {
                        Ok(json!({
                            "party_name": r.get::<_, String>(0)?,
                            "direction": r.get::<_, String>(1)?,
                            "amount": r.get::<_, i64>(2)? as f64 / scale,
                            "settled": r.get::<_, bool>(3)?,
                            "due_date": r.get::<_, Option<String>>(4)?,
                        }))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                json!(debts)
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

    // Same coarser can_view_reports gate as `totals` above — these are
    // both insight-style computations (a lookback-window average, a
    // day-of-week breakdown), not a raw operational fact like
    // low-stock, so they follow `totals`'s rule, not `low_stock`'s.
    // Both degrade to an empty array on any failure (module not
    // enabled, no permission, whatever) rather than surfacing an
    // error into the AI's own prompt — the assistant just won't
    // mention them, the same as it already does for anything else
    // ai_context.rs doesn't manage to fetch.
    let today = chrono::Utc::now().date_naive().to_string();
    let stock_runway = if can_view_reports {
        crate::stock_health::stock_runway(conn, business_id, user_id, &today, 30, 15).unwrap_or_default()
    } else {
        Vec::new()
    };
    let day_of_week_pattern = if can_view_reports {
        crate::sales_patterns::day_of_week_pattern(conn, business_id, user_id, &today, 90).unwrap_or_default()
    } else {
        Vec::new()
    };

    Ok(json!({
        "business_name": business_name,
        "currency": currency,
        "generated_at": chrono::Utc::now().to_rfc3339(),
        "modules": modules_summary,
        "stock_runway": stock_runway,
        "day_of_week_sales_pattern": day_of_week_pattern,
    }))
}
