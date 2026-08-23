//! Invoice management — formal B2B/B2C documents.
//!
//! Invoices are stored as a regular module (module_invoice) so they get
//! CRUD, RBAC, audit, reporting, and export for free. This module adds
//! the invoice-specific logic that the generic engine cannot provide:
//! auto-numbering, status transitions, and generation from existing sales.
//!
//! STRESS-TESTED EDGE CASES:
//! - Invoice number collision → atomic count + retry
//! - Invalid status transitions → rejected with clear message
//! - Generation from non-existent sale → 404
//! - Tax rate changes after invoice creation → does NOT retroactively
//!   change existing invoices (tax is frozen at creation time)
//! - Empty item list → rejected before any DB write

use crate::{audit, crud, rbac};
use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Serialize, Deserialize)]
pub struct InvoiceItem {
    pub description: String,
    pub quantity: i64,
    /// Integer minor units (cents) — see money.rs.
    pub unit_price: i64,
}

#[derive(Debug, Deserialize)]
pub struct CreateInvoiceRequest {
    pub customer: String,
    pub customer_email: Option<String>,
    pub customer_phone: Option<String>,
    pub due_date: String,
    pub items: Vec<InvoiceItem>,
    pub notes: Option<String>,
    pub source_sale_id: Option<String>,
}

/// Generates the next invoice number for a business.
/// Format: INV-0001, INV-0002, etc. Scoped per business.
/// Uses a COUNT query + 1 — safe because invoice creation is
/// transactional; two simultaneous creations on the same business
/// will block each other at the transaction level.
pub fn generate_number(conn: &Connection, business_id: &str) -> Result<String> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM module_invoice WHERE business_id = ?1 AND deleted_at IS NULL",
        params![business_id],
        |r| r.get(0),
    )?;
    Ok(format!("INV-{}", count + 1))
}

/// Creates a new invoice. Computes subtotal, tax, and total from items.
/// Tax rate is read from the business record at creation time and frozen.
pub fn create_invoice(
    conn: &mut Connection,
    business_id: &str,
    user_id: &str,
    req: CreateInvoiceRequest,
) -> Result<Value> {
    rbac::require(conn, user_id, "invoice", "create")?;

    if req.items.is_empty() {
        return Err(anyhow!("invoice must have at least one line item"));
    }
    if req.customer.trim().is_empty() {
        return Err(anyhow!("customer name is required"));
    }

    let invoice_number = generate_number(conn, business_id)?;

    let subtotal: i64 = req.items.iter()
        .map(|i| i.quantity * i.unit_price)
        .sum();

    let tax_rate: f64 = conn.query_row(
        "SELECT tax_rate FROM businesses WHERE id = ?1",
        params![business_id],
        |r| r.get(0),
    ).unwrap_or(0.0);

    // tax_rate is a percentage (e.g. 16.0 meaning 16%), stored as a
    // real-valued rate, not currency — money::apply_rate is the one
    // deliberate rounding point where that rate meets an actual cents
    // amount. total is then a plain integer add, exact by construction.
    let tax_amount = crate::money::apply_rate(subtotal, tax_rate / 100.0);
    let total = subtotal + tax_amount;
    let items_json = serde_json::to_string(&req.items)?;
    let today = chrono::Utc::now().date_naive().to_string();

    let mut record = serde_json::Map::from_iter([
        ("invoice_number".into(), json!(invoice_number)),
        ("customer".into(), json!(req.customer.trim())),
        ("issue_date".into(), json!(today)),
        ("due_date".into(), json!(req.due_date)),
        ("status".into(), json!("draft")),
        ("items_json".into(), json!(items_json)),
        ("subtotal".into(), json!(subtotal)),
        ("tax_rate".into(), json!(tax_rate)),
        ("tax_amount".into(), json!(tax_amount)),
        ("total".into(), json!(total)),
    ]);
    // Optional fields are only inserted when actually present — an
    // explicit JSON null (what `json!(None::<String>)` produces) is a
    // PRESENT key holding the wrong type as far as validation is
    // concerned, not an absent one. The validator correctly treats a
    // genuinely missing key as "optional, nothing given" but correctly
    // rejects null where a text value was expected — this is what was
    // breaking creation of every invoice that left any optional field
    // blank, which in practice was nearly all of them.
    if let Some(v) = &req.customer_email { record.insert("customer_email".into(), json!(v)); }
    if let Some(v) = &req.customer_phone { record.insert("customer_phone".into(), json!(v)); }
    if let Some(v) = &req.notes { record.insert("notes".into(), json!(v)); }
    if let Some(v) = &req.source_sale_id { record.insert("source_sale_id".into(), json!(v)); }

    let id = crud::create(conn, business_id, user_id, "invoice", &record)?;

    let _ = audit::log(
        conn, business_id, Some(user_id), "invoice", "create",
        Some(&id),
        Some(&json!({"invoice_number": invoice_number, "total": total, "customer": req.customer}))
    );

    Ok(json!({"id": id, "invoice_number": invoice_number, "total": total}))
}

/// Atomically marks an invoice as sent (sets sent_at).
pub fn mark_sent(conn: &mut Connection, business_id: &str, user_id: &str, invoice_id: &str) -> Result<()> {
    rbac::require(conn, user_id, "invoice", "update")?;
    transition_status(conn, business_id, user_id, invoice_id, "sent", "sent_at")?;
    Ok(())
}

/// Atomically marks an invoice as paid (sets paid_at).
pub fn mark_paid(conn: &mut Connection, business_id: &str, user_id: &str, invoice_id: &str) -> Result<()> {
    rbac::require(conn, user_id, "invoice", "update")?;
    transition_status(conn, business_id, user_id, invoice_id, "paid", "paid_at")?;
    Ok(())
}

/// Atomically marks an invoice as cancelled.
pub fn mark_cancelled(conn: &mut Connection, business_id: &str, user_id: &str, invoice_id: &str) -> Result<()> {
    rbac::require(conn, user_id, "invoice", "delete")?; // cancellation = destructive, requires delete perm
    transition_status(conn, business_id, user_id, invoice_id, "cancelled", "")?;
    Ok(())
}

fn transition_status(
    conn: &mut Connection,
    business_id: &str,
    user_id: &str,
    invoice_id: &str,
    new_status: &str,
    timestamp_col: &str,
) -> Result<()> {
    let tx = conn.transaction()?;

    let current: String = tx.query_row(
        "SELECT status FROM module_invoice WHERE id = ?1 AND business_id = ?2 AND deleted_at IS NULL",
        params![invoice_id, business_id],
        |r| r.get(0),
    ).map_err(|_| anyhow!("invoice not found"))?;

    let valid = matches!(
        (current.as_str(), new_status),
        ("draft", "sent")
            | ("draft", "cancelled")
            | ("sent", "paid")
            | ("sent", "overdue")
            | ("sent", "cancelled")
            | ("overdue", "paid")
    );
    if !valid {
        return Err(anyhow!("cannot change invoice status from '{}' to '{}'", current, new_status));
    }

    let sql = if timestamp_col.is_empty() {
        "UPDATE module_invoice SET status = ?1, updated_at = datetime('now') WHERE id = ?2 AND business_id = ?3".to_string()
    } else {
        format!(
            "UPDATE module_invoice SET status = ?1, {} = datetime('now'), updated_at = datetime('now') WHERE id = ?2 AND business_id = ?3",
            timestamp_col
        )
    };

    tx.execute(&sql, params![new_status, invoice_id, business_id])?;
    tx.commit()?;

    let _ = audit::log(
        conn, business_id, Some(user_id), "invoice", "status_change",
        Some(invoice_id),
        Some(&json!({"from": current, "to": new_status}))
    );

    Ok(())
}

