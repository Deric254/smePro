//! Customer capture and lifetime value.
//!
//! Deliberately optional at every step: a cashier types a name/phone at
//! checkout only if the customer offers one, or doesn't. Most sales
//! stay fully anonymous, same as before this existed — this only
//! activates for the ones where real customer details were actually
//! given.
//!
//! Lifetime value is NOT a stored, cached number — it's computed fresh
//! on every read, by summing the real sales records matching this
//! customer's phone number. A cached total would need updating every
//! time a sale happens, gets refunded, or gets corrected — and any
//! missed update point means a wrong number silently sitting there.
//! Computing it live means it can never disagree with the actual
//! transaction history, by construction, not by discipline.

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use uuid::Uuid;

/// Finds an existing customer by phone, or creates one. Called from
/// POS checkout — never called directly by an HTTP route, since a
/// customer record should only ever originate from an actual sale.
/// Updates the stored name if a different one was given this time
/// (people's names get typed slightly differently visit to visit;
/// phone is the actual stable identity here, not the name).
pub fn find_or_create(conn: &Connection, business_id: &str, name: Option<&str>, phone: &str) -> Result<String> {
    let phone = phone.trim();
    if phone.is_empty() {
        return Err(anyhow!("phone number is required to identify a customer"));
    }

    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM customers WHERE business_id = ?1 AND phone = ?2",
            params![business_id, phone],
            |r| r.get(0),
        )
        .optional()?;

    if let Some(id) = existing {
        if let Some(n) = name {
            if !n.trim().is_empty() {
                conn.execute(
                    "UPDATE customers SET name = ?1, updated_at = datetime('now') WHERE id = ?2",
                    params![n.trim(), id],
                )?;
            }
        }
        return Ok(id);
    }

    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO customers (id, business_id, name, phone, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, datetime('now'), datetime('now'))",
        params![id, business_id, name.map(|n| n.trim()), phone],
    )?;
    Ok(id)
}

/// Lists every customer with a real purchase, sorted by lifetime value
/// (highest first) — the naturally useful default: a business owner
/// glancing at this list sees their best customers first, not an
/// arbitrary alphabetical or chronological order.
pub fn list(conn: &Connection, business_id: &str) -> Result<Value> {
    let sales_table = crate::crud::load_module(conn, business_id, "sales")
        .map(|m| m.table_name())
        .unwrap_or_else(|_| "sales_records".to_string());

    let sql = format!(
        "SELECT c.id, c.name, c.phone, c.created_at,
                COALESCE(SUM(s.revenue), 0) as lifetime_value,
                COUNT(s.id) as order_count,
                MAX(s.created_at) as last_purchase_at
         FROM customers c
         LEFT JOIN {sales_table} s ON s.customer_phone = c.phone AND s.business_id = c.business_id AND s.deleted_at IS NULL
         WHERE c.business_id = ?1
         GROUP BY c.id
         ORDER BY lifetime_value DESC"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![business_id], |r| {
        Ok(json!({
            "id": r.get::<_, String>(0)?,
            "name": r.get::<_, Option<String>>(1)?,
            "phone": r.get::<_, String>(2)?,
            "customer_since": r.get::<_, String>(3)?,
            "lifetime_value": r.get::<_, f64>(4)?,
            "order_count": r.get::<_, i64>(5)?,
            "last_purchase_at": r.get::<_, Option<String>>(6)?,
        }))
    })?;

    let customers: Vec<Value> = rows.filter_map(|r| r.ok()).collect();
    Ok(json!({ "customers": customers }))
}

/// Full detail for one customer — their info plus every purchase,
/// most recent first, so clicking into a customer answers "what did
/// they buy, and when" directly, not just "how much have they spent."
pub fn detail(conn: &Connection, business_id: &str, customer_id: &str) -> Result<Value> {
    let (name, phone, since): (Option<String>, String, String) = conn
        .query_row(
            "SELECT name, phone, created_at FROM customers WHERE id = ?1 AND business_id = ?2",
            params![customer_id, business_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|_| anyhow!("customer not found"))?;

    let sales_table = crate::crud::load_module(conn, business_id, "sales")
        .map(|m| m.table_name())
        .unwrap_or_else(|_| "sales_records".to_string());

    let sql = format!(
        "SELECT item_name, quantity, revenue, order_id, created_at
         FROM {sales_table}
         WHERE business_id = ?1 AND customer_phone = ?2 AND deleted_at IS NULL
         ORDER BY created_at DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![business_id, phone], |r| {
        Ok(json!({
            "item_name": r.get::<_, String>(0)?,
            "quantity": r.get::<_, i64>(1)?,
            "revenue": r.get::<_, f64>(2)?,
            "order_id": r.get::<_, Option<String>>(3)?,
            "date": r.get::<_, String>(4)?,
        }))
    })?;
    let purchases: Vec<Value> = rows.filter_map(|r| r.ok()).collect();
    let lifetime_value: f64 = purchases.iter().filter_map(|p| p.get("revenue").and_then(|r| r.as_f64())).sum();

    Ok(json!({
        "id": customer_id,
        "name": name,
        "phone": phone,
        "customer_since": since,
        "lifetime_value": lifetime_value,
        "order_count": purchases.len(),
        "purchases": purchases,
    }))
}
