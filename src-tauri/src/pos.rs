//! Point of sale.
//!
//! Before this, Sales and Inventory were two completely unrelated
//! generic-module tables — selling something never touched stock at
//! all. This is the fix: `checkout()` links them for real, in one
//! database transaction. Either every line item's stock gets deducted
//! AND its sales record gets created, or — if anything fails partway,
//! most importantly running out of stock mid-cart — NONE of it does.
//! There is no possible state where a sale is recorded but stock wasn't
//! deducted, or the reverse.
//!
//! This module deliberately does NOT go through `crud::create` /
//! `crud::update` directly for its two writes — each of those enforces
//! its own separate permission ("create" on sales, "update" on
//! inventory), which would mean a cashier needs both grants just to
//! ring up a sale. Checkout uses one single, purpose-built permission
//! instead: "sell" on the Inventory module. It reuses the exact same
//! validation and insert logic those functions use internally
//! (`module.validate`, `reference_data::validate_field_references`,
//! `crud::insert_validated_record`) — just not their RBAC gate — so a
//! POS-created sale is held to precisely the same correctness standard
//! as one typed in by hand, with zero duplicated logic to drift out of
//! sync.

use crate::crud;
use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct CartItem {
    pub inventory_record_id: String,
    pub quantity: i64,
}

#[derive(Debug, Deserialize)]
pub struct CheckoutRequest {
    pub items: Vec<CartItem>,
    #[serde(default)]
    pub payment_method: Option<String>,
    #[serde(default)]
    pub customer: Option<String>,
    /// If false (the default) and any line item doesn't have enough
    /// stock, the ENTIRE checkout is rejected — nothing partially
    /// applied. Some businesses genuinely do sell on credit/backorder;
    /// this is how they opt into that per-checkout rather than it being
    /// silently allowed by default for everyone.
    #[serde(default)]
    pub allow_oversell: bool,
    /// Selling on credit — the customer owes the business, doesn't pay
    /// now. When true, this checkout ALSO creates a Debt & Credit
    /// record for the full subtotal, in the exact same transaction as
    /// the stock deduction and sales record. Before this, a credit sale
    /// had no connection to Debt & Credit at all — someone had to
    /// remember to go create that record by hand, with everything that
    /// implies for a business actually collecting on it later.
    #[serde(default)]
    pub on_credit: bool,
    #[serde(default)]
    pub due_date: Option<String>,
}

/// Runs the whole checkout as one atomic transaction. On success,
/// returns the order summary (order_id, subtotal, per-line detail) —
/// everything a receipt screen needs, computed from what was actually
/// written, not just echoed back from the request.
pub fn checkout(conn: &mut Connection, business_id: &str, user_id: &str, req: CheckoutRequest) -> Result<Value> {
    if req.items.is_empty() {
        return Err(anyhow!("the cart is empty"));
    }
    // One check, up front, for the whole operation — not per-line and
    // not split across two different modules' permissions.
    crate::rbac::require(conn, user_id, "inventory", "sell")?;

    let inventory_module = crud::load_module(conn, business_id, "inventory")
        .map_err(|_| anyhow!("the Inventory module isn't enabled for this business — checkout needs it"))?;
    let sales_module = crud::load_module(conn, business_id, "sales")
        .map_err(|_| anyhow!("the Sales module isn't enabled for this business — checkout needs it"))?;
    let debt_credit_module = if req.on_credit {
        Some(
            crud::load_module(conn, business_id, "debt_credit")
                .map_err(|_| anyhow!("selling on credit needs the Debt & Credit module enabled for this business"))?,
        )
    } else {
        None
    };
    if req.on_credit && req.customer.as_deref().unwrap_or("").trim().is_empty() {
        return Err(anyhow!("a customer name is required for a credit sale — Debt & Credit needs to know who owes it"));
    }
    let inventory_table = inventory_module.table_name();
    let sales_table = sales_module.table_name();

    let order_id = Uuid::new_v4().to_string();
    let mut lines = Vec::with_capacity(req.items.len());
    let mut subtotal = 0.0_f64;

    let tx = conn.transaction()?;

    for item in &req.items {
        if item.quantity <= 0 {
            return Err(anyhow!("quantity must be greater than zero"));
        }

        let row: Option<(String, i64, f64, String)> = tx
            .query_row(
                &format!("SELECT name, quantity, unit_price, sku FROM {inventory_table} WHERE id = ?1 AND business_id = ?2 AND deleted_at IS NULL"),
                params![item.inventory_record_id, business_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((name, current_qty, unit_price, sku)) = row else {
            return Err(anyhow!("product not found: {}", item.inventory_record_id));
        };

        if current_qty < item.quantity && !req.allow_oversell {
            return Err(anyhow!(
                "not enough stock for '{name}': {current_qty} available, {} requested",
                item.quantity
            ));
        }
        let new_qty = current_qty - item.quantity;
        tx.execute(
            &format!("UPDATE {inventory_table} SET quantity = ?1, updated_at = datetime('now') WHERE id = ?2 AND business_id = ?3"),
            params![new_qty, item.inventory_record_id, business_id],
        )?;

        let line_total = unit_price * item.quantity as f64;
        subtotal += line_total;

        let mut record: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
        record.insert("item_name".into(), json!(name));
        record.insert("quantity".into(), json!(item.quantity));
        record.insert("revenue".into(), json!(line_total));
        record.insert("unit_price".into(), json!(unit_price));
        record.insert("order_id".into(), json!(order_id));
        if let Some(c) = &req.customer {
            record.insert("customer".into(), json!(c));
        }
        if let Some(p) = &req.payment_method {
            record.insert("payment_method".into(), json!(p));
        }
        for f in &sales_module.fields {
            if !record.contains_key(&f.name) {
                if let Some(d) = &f.default {
                    record.insert(f.name.clone(), d.clone());
                }
            }
        }
        // Same validation a manually-typed sale goes through — no
        // special-casing for POS-originated records.
        sales_module.validate(&record)?;
        crate::reference_data::validate_field_references(&tx, business_id, &sales_module, &record)?;
        let sale_id = crud::insert_validated_record(&tx, business_id, &sales_module, &record)?;

        lines.push(json!({
            "sku": sku,
            "name": name,
            "quantity": item.quantity,
            "unit_price": unit_price,
            "line_total": line_total,
            "remaining_stock": new_qty,
            "sale_id": sale_id,
        }));
    }

    // If this is a credit sale, the debt is created here — still
    // inside `tx`, still nothing durable yet. Same all-or-nothing
    // guarantee extends to a third module now: stock deduction, sales
    // record, AND the debt record either all become real together at
    // the commit below, or none of them do.
    let mut debt_record_id: Option<String> = None;
    if let Some(debt_credit_module) = &debt_credit_module {
        let mut debt_record: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
        debt_record.insert("party_name".into(), json!(req.customer.as_deref().unwrap_or("")));
        debt_record.insert("direction".into(), json!("owed_to_business"));
        debt_record.insert("amount".into(), json!(subtotal));
        debt_record.insert("settled".into(), json!(false));
        debt_record.insert("notes".into(), json!(format!("Credit sale, POS order {order_id}")));
        if let Some(d) = &req.due_date {
            debt_record.insert("due_date".into(), json!(d));
        }
        for f in &debt_credit_module.fields {
            if !debt_record.contains_key(&f.name) {
                if let Some(d) = &f.default {
                    debt_record.insert(f.name.clone(), d.clone());
                }
            }
        }
        debt_credit_module.validate(&debt_record)?;
        crate::reference_data::validate_field_references(&tx, business_id, debt_credit_module, &debt_record)?;
        debt_record_id = Some(crud::insert_validated_record(&tx, business_id, debt_credit_module, &debt_record)?);
    }

    // Everything above happened inside `tx` and nothing is durable yet.
    // This is the one moment it all becomes real, together — if the
    // process died at any point before this line, every UPDATE and
    // INSERT above would simply not exist on next read, not exist
    // "partially."
    tx.commit()?;

    let summary = json!({
        "order_id": order_id,
        "customer": req.customer,
        "payment_method": req.payment_method,
        "subtotal": subtotal,
        "item_count": req.items.len(),
        "items": lines,
        "on_credit": req.on_credit,
        "debt_record_id": debt_record_id,
    });

    // Logged after commit, deliberately: the audit log recording a
    // checkout that turned out not to actually happen (had the commit
    // itself failed) would be worse than not logging at all.
    let _ = crate::audit::log(conn, business_id, Some(user_id), "_pos", "checkout", Some(&order_id), Some(&summary));

    let _ = sales_table; // kept for symmetry/clarity even though only inventory_table is queried directly above
    Ok(summary)
}

/// Fetches every sales line item belonging to one checkout, for a
/// receipt screen — grouped by the order_id `checkout()` generated.
pub fn get_order(conn: &Connection, business_id: &str, order_id: &str) -> Result<Value> {
    let sales_module = crud::load_module(conn, business_id, "sales")?;
    let table = sales_module.table_name();
    let mut stmt = conn.prepare(&format!(
        "SELECT id, item_name, quantity, revenue, unit_price, customer, payment_method, created_at
         FROM {table} WHERE business_id = ?1 AND order_id = ?2 AND deleted_at IS NULL ORDER BY created_at"
    ))?;
    let rows = stmt.query_map(params![business_id, order_id], |r| {
        Ok(json!({
            "sale_id": r.get::<_, String>(0)?,
            "item_name": r.get::<_, String>(1)?,
            "quantity": r.get::<_, i64>(2)?,
            "revenue": r.get::<_, f64>(3)?,
            "unit_price": r.get::<_, Option<f64>>(4)?,
            "customer": r.get::<_, Option<String>>(5)?,
            "payment_method": r.get::<_, Option<String>>(6)?,
            "created_at": r.get::<_, String>(7)?,
        }))
    })?;
    let items: Vec<Value> = rows.filter_map(|r| r.ok()).collect();
    if items.is_empty() {
        return Err(anyhow!("order not found"));
    }
    let subtotal: f64 = items.iter().filter_map(|v| v.get("revenue").and_then(|r| r.as_f64())).sum();
    Ok(json!({"order_id": order_id, "subtotal": subtotal, "items": items}))
}
