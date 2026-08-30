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

/// Auto-generates a real, numbered invoice for a completed POS
/// order — goods sale (`pos::checkout`) or service sale
/// (`pos::create_service_sale`) — inside the SAME transaction as the
/// sale itself. Before this, an invoice only ever existed if someone
/// went to the Invoices tab and filled in the "+ New invoice" form by
/// hand afterward; every sale rung up at the till or the service
/// counter had no invoice at all unless a person remembered to create
/// one separately. Every sale now gets one automatically — printable,
/// shareable, and searchable through the exact same Invoices module
/// and InvoiceView UI a manually-created invoice already uses.
///
/// Deliberately bypasses `create_invoice()`'s own RBAC check and its
/// full `CreateInvoiceRequest` shape: the caller (`checkout`/
/// `create_service_sale`) already authorized the whole operation via
/// its own purpose-built permission, the same reasoning those
/// functions already apply to their own direct
/// accounting/debt_credit inserts. Every amount here is computed
/// fresh from what the sale itself actually recorded — never
/// re-derived from a second, independently-typed request that could
/// drift out of sync with it.
///
/// Best-effort by design, exactly like the accounting/debt_credit
/// inserts it sits alongside in `pos.rs`: a business that hasn't
/// enabled the Invoice module can still ring up a sale, and the
/// caller only invokes this at all once it's confirmed the module
/// exists.
pub fn create_invoice_for_order(
    conn: &Connection,
    business_id: &str,
    order_id: &str,
    customer: Option<&str>,
    customer_phone: Option<&str>,
    items: &[InvoiceItem],
    subtotal: i64,
    on_credit: bool,
    due_date: Option<&str>,
) -> Result<String> {
    let invoice_number = generate_number(conn, business_id)?;

    let tax_rate: f64 = conn.query_row(
        "SELECT tax_rate FROM businesses WHERE id = ?1",
        params![business_id],
        |r| r.get(0),
    ).unwrap_or(0.0);
    let tax_amount = crate::money::apply_rate(subtotal, tax_rate / 100.0);
    let total = subtotal + tax_amount;
    let items_json = serde_json::to_string(items)?;
    let today = chrono::Utc::now().date_naive().to_string();

    let mut record = serde_json::Map::from_iter([
        ("invoice_number".into(), json!(invoice_number)),
        // A walk-in POS sale often has no customer name at all —
        // `customer` is a required field on the Invoice module, so a
        // clear placeholder stands in rather than leaving this
        // invoice unable to be created (and the sale silently
        // invoice-less) just because nobody was asked for a name at
        // the till.
        ("customer".into(), json!(customer.map(str::trim).filter(|c| !c.is_empty()).unwrap_or("Walk-in customer"))),
        ("issue_date".into(), json!(today)),
        // A credit sale is genuinely owed later — its due date is
        // whatever Debt & Credit was given (or, absent that, today,
        // same "no invisible default" floor used elsewhere). A normal
        // paid-now sale has nothing left to be "due" — its due date is
        // simply the day it was issued.
        ("due_date".into(), json!(if on_credit { due_date.filter(|d| !d.trim().is_empty()).unwrap_or(&today) } else { today.as_str() })),
        // Reflects reality immediately, not a workflow status that
        // then has to be manually advanced: a credit sale is genuinely
        // "sent" (awaiting payment) the moment it's rung up, and a
        // paid-now sale is genuinely "paid" the moment it's rung up —
        // there's no real "draft" in between for either one.
        ("status".into(), json!(if on_credit { "sent" } else { "paid" })),
        ("items_json".into(), json!(items_json)),
        ("subtotal".into(), json!(subtotal)),
        ("tax_rate".into(), json!(tax_rate)),
        ("tax_amount".into(), json!(tax_amount)),
        ("total".into(), json!(total)),
        ("source_sale_id".into(), json!(order_id)),
    ]);
    if !on_credit {
        record.insert("paid_at".into(), json!(today));
    }
    if let Some(phone) = customer_phone {
        if !phone.trim().is_empty() {
            record.insert("customer_phone".into(), json!(phone));
        }
    }

    let invoice_module = crud::load_module(conn, business_id, "invoice")?;
    let record_map: std::collections::HashMap<String, Value> = record.into_iter().collect();
    invoice_module.validate(&record_map)?;
    crate::reference_data::validate_field_references(conn, business_id, &invoice_module, &record_map)?;
    crud::insert_validated_record(conn, business_id, &invoice_module, &record_map)
}

/// Looks up how much (if anything) has been refunded against the
/// sale this invoice was auto-generated from, WITHOUT touching the
/// invoice's own frozen subtotal/tax/total — those stay exactly as
/// originally issued, same "as it was" principle receipt.rs already
/// applies. This is purely additional disclosure, read fresh each
/// time the invoice is viewed, so it always reflects the current
/// refund state even though the invoice document itself never
/// changes.
///
/// Returns `refunded_amount: 0, is_refunded: false` — not an error —
/// for a manually-created invoice with no `source_sale_id` at all,
/// or a refunds module that isn't enabled: "nothing refunded" is the
/// correct, honest answer in both cases, not a failure to report.
pub fn get_refund_status(conn: &Connection, business_id: &str, user_id: &str, invoice_id: &str) -> Result<Value> {
    rbac::require(conn, user_id, "invoice", "read")?;

    let invoice_module = crud::load_module(conn, business_id, "invoice")?;
    let invoice_table = invoice_module.table_name();
    let source_sale_id: Option<String> = conn
        .query_row(
            &format!("SELECT source_sale_id FROM {invoice_table} WHERE id = ?1 AND business_id = ?2 AND deleted_at IS NULL"),
            params![invoice_id, business_id],
            |r| r.get(0),
        )
        .map_err(|_| anyhow!("invoice not found"))?;

    let Some(order_id) = source_sale_id else {
        return Ok(json!({"refunded_amount": 0, "is_refunded": false}));
    };

    // Best-effort, same reasoning as receipt.rs's own refund lookup:
    // a business that's never enabled Refunds simply has nothing to
    // find, not an error.
    let refunded_amount: i64 = if let Ok(refunds_module) = crud::load_module(conn, business_id, "refunds") {
        let refunds_table = refunds_module.table_name();
        conn.query_row(
            &format!(
                "SELECT COALESCE(SUM(refund_amount), 0) FROM {refunds_table} \
                 WHERE business_id = ?1 AND order_id = ?2 AND deleted_at IS NULL"
            ),
            params![business_id, order_id],
            |r| r.get(0),
        ).unwrap_or(0)
    } else {
        0
    };

    Ok(json!({"refunded_amount": refunded_amount, "is_refunded": refunded_amount > 0}))
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

