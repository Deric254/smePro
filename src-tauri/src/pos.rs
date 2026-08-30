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
    /// Optional — most sales stay anonymous, this only activates when
    /// a phone number is actually given. When present, finds-or-creates
    /// a customer record and links this sale to it (see customers.rs),
    /// which is what makes lifetime value tracking possible at all.
    #[serde(default)]
    pub customer_phone: Option<String>,
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
    // Parallel to `lines` above but in the exact shape
    // invoice::create_invoice_for_order needs — built alongside it so
    // the auto-generated invoice reflects precisely what was actually
    // sold, never a second, independently-reconstructed list that
    // could drift from it.
    let mut invoice_items: Vec<crate::invoice::InvoiceItem> = Vec::with_capacity(req.items.len());
    // Integer cents throughout — see money.rs. This sum is exact by
    // construction; there is no fractional cent that could ever need
    // rounding here, unlike the f64 subtotal this replaced.
    let mut subtotal: i64 = 0;

    let tx = conn.transaction()?;

    // Same transaction as everything else below — if the customer
    // gets created/updated but the sale itself fails partway through,
    // the whole thing rolls back together, not a customer record left
    // behind with no matching purchase.
    //
    // Triggered by EITHER a name or a phone — not phone alone. A
    // cashier who only got a name (no phone offered/asked) still gets
    // that customer tracked (weaker, name-only matching — see
    // customers.rs's own doc comment on the trade-off), rather than
    // silently skipping tracking entirely just because there was no
    // phone number this particular visit.
    let has_customer_info = req.customer.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false)
        || req.customer_phone.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false);
    let customer_id = if has_customer_info {
        Some(crate::customers::find_or_create(&tx, business_id, req.customer.as_deref(), req.customer_phone.as_deref())?)
    } else {
        None
    };

    for item in &req.items {
        if item.quantity <= 0 {
            return Err(anyhow!("quantity must be greater than zero"));
        }

        let row: Option<(String, i64, i64, String)> = tx
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

        // Exact — integer cents times an integer quantity is still an
        // exact integer, no rounding step needed or allowed here.
        let line_total: i64 = unit_price * item.quantity;
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
        if let Some(phone) = &req.customer_phone {
            // Same normalization as customers::find_or_create — this
            // field is what customers::list/detail JOIN against
            // customers.phone to compute lifetime value. Storing
            // anything other than the identical normalized form here
            // would silently break that join for this specific sale,
            // even though the customer record itself was created
            // correctly — the sale would just never show up in that
            // customer's history or LTV total.
            let normalized = crate::customers::normalize_phone(phone);
            if !normalized.is_empty() {
                record.insert("customer_phone".into(), json!(normalized));
            }
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
        invoice_items.push(crate::invoice::InvoiceItem {
            description: name,
            quantity: item.quantity,
            unit_price,
        });
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
        // Structured pointer back to the sale this debt came from —
        // the notes text above is for a human reading the record, this
        // is for settle() to actually find and update the matching
        // sales row's payment_method once the debt is paid off (see
        // debt_settlement.rs). Kept as its own field rather than
        // parsed back out of `notes` because notes is free text a
        // person can edit later; this isn't.
        debt_record.insert("source_order_id".into(), json!(order_id));
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

    // Bookkeeping used to be a completely disconnected, hand-typed
    // ledger — nothing a sale did ever showed up there automatically,
    // which is exactly why it tends to go stale. This posts one
    // income entry per completed order, same transaction as
    // everything above.
    //
    // Deliberately skipped for a credit sale (req.on_credit): no cash
    // has actually come in yet — that's exactly what the Debt &
    // Credit record above already represents (money owed, not money
    // received). Posting it as income here too would double-count it
    // in Bookkeeping the moment it's created. The cash side gets
    // posted later, for real, when the debt is actually paid off —
    // see debt_settlement::settle(), which does exactly that (and
    // also backfills this sale's own payment_method once it knows
    // it).
    // Best-effort by design, not required: a business that hasn't
    // enabled Bookkeeping can still ring up a sale.
    if !req.on_credit {
        if let Ok(accounting_module) = crud::load_module(&tx, business_id, "accounting") {
            let mut entry: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
            entry.insert("description".into(), json!(format!("POS sale — order {order_id}")));
            entry.insert("entry_type".into(), json!("income"));
            entry.insert("category".into(), json!("Sales"));
            entry.insert("amount".into(), json!(subtotal));
            if let Some(p) = &req.payment_method {
                entry.insert("payment_method".into(), json!(p));
            }
            for f in &accounting_module.fields {
                if !entry.contains_key(&f.name) {
                    if let Some(d) = &f.default {
                        entry.insert(f.name.clone(), d.clone());
                    }
                }
            }
            accounting_module.validate(&entry)?;
            crate::reference_data::validate_field_references(&tx, business_id, &accounting_module, &entry)?;
            crud::insert_validated_record(&tx, business_id, &accounting_module, &entry)?;
        }
    }

    // Every completed order gets a real, numbered invoice automatically
    // — see invoice::create_invoice_for_order's own doc comment for why
    // this exists and what it replaces (a person having to remember to
    // create one by hand afterward). Best-effort, same pattern as the
    // Bookkeeping post just above: a business that hasn't enabled the
    // Invoice module can still ring up a sale.
    if crud::load_module(&tx, business_id, "invoice").is_ok() {
        crate::invoice::create_invoice_for_order(
            &tx,
            business_id,
            &order_id,
            req.customer.as_deref(),
            req.customer_phone.as_deref(),
            &invoice_items,
            subtotal,
            req.on_credit,
            req.due_date.as_deref(),
        )?;
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
        "customer_id": customer_id,
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

/// One line item for a service sale (see `create_service_sale` below)
/// — no `inventory_record_id`, since a service business by definition
/// has no stock to reference.
#[derive(Debug, Deserialize)]
pub struct ServiceLine {
    pub description: String,
    pub unit_price: i64, // integer cents
    pub quantity: i64,
}

#[derive(Debug, Deserialize)]
pub struct ServiceSaleRequest {
    pub lines: Vec<ServiceLine>,
    #[serde(default)]
    pub payment_method: Option<String>,
    #[serde(default)]
    pub customer: Option<String>,
    #[serde(default)]
    pub customer_phone: Option<String>,
}

/// The service-business counterpart to `checkout` above — same three
/// guarantees, minus inventory: (1) every line commits together in one
/// transaction or none do, (2) a customer phone given here creates
/// or updates a real `customers` row exactly like checkout does,
/// through the same `customers::find_or_create`, and (3) it posts to
/// Bookkeeping exactly like checkout does for a non-credit sale.
///
/// This didn't always exist: ServiceSale.tsx used to call the plain
/// generic `crud::create` once per line in a loop with no shared
/// transaction (a failure partway through the loop could leave some
/// lines saved and others not), never touched `customers` at all —
/// a service business's repeat customers never appeared in the
/// Customers list or had any lifetime value tracked, despite the phone
/// number being recorded right there on every sale — and never posted
/// to Bookkeeping at all, unlike a goods sale through checkout(). All
/// three gaps closed here the same way checkout() already closes them
/// for goods sales.
pub fn create_service_sale(conn: &mut Connection, business_id: &str, user_id: &str, req: ServiceSaleRequest) -> Result<Value> {
    if req.lines.is_empty() {
        return Err(anyhow!("add at least one line"));
    }
    crate::rbac::require(conn, user_id, "sales", "create")?;
    let sales_module = crud::load_module(conn, business_id, "sales")
        .map_err(|_| anyhow!("the Sales module isn't enabled for this business"))?;

    let order_id = Uuid::new_v4().to_string();
    let mut lines_out = Vec::with_capacity(req.lines.len());
    let mut invoice_items: Vec<crate::invoice::InvoiceItem> = Vec::with_capacity(req.lines.len());
    let mut subtotal: i64 = 0;

    let tx = conn.transaction()?;

    let has_customer_info = req.customer.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false)
        || req.customer_phone.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false);
    let customer_id = if has_customer_info {
        Some(crate::customers::find_or_create(&tx, business_id, req.customer.as_deref(), req.customer_phone.as_deref())?)
    } else {
        None
    };
    let normalized_phone = req.customer_phone.as_deref().map(crate::customers::normalize_phone);

    for line in &req.lines {
        if line.quantity <= 0 {
            return Err(anyhow!("quantity must be greater than zero"));
        }
        if line.unit_price < 0 {
            return Err(anyhow!("price cannot be negative"));
        }
        if line.description.trim().is_empty() {
            return Err(anyhow!("every line needs a description"));
        }

        let line_total: i64 = line.unit_price * line.quantity;
        subtotal += line_total;

        let mut record: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
        record.insert("item_name".into(), json!(line.description.trim()));
        record.insert("quantity".into(), json!(line.quantity));
        record.insert("revenue".into(), json!(line_total));
        record.insert("unit_price".into(), json!(line.unit_price));
        record.insert("order_id".into(), json!(order_id));
        if let Some(c) = &req.customer {
            record.insert("customer".into(), json!(c));
        }
        if let Some(p) = &normalized_phone {
            if !p.is_empty() {
                record.insert("customer_phone".into(), json!(p));
            }
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
        sales_module.validate(&record)?;
        crate::reference_data::validate_field_references(&tx, business_id, &sales_module, &record)?;
        let sale_id = crud::insert_validated_record(&tx, business_id, &sales_module, &record)?;

        lines_out.push(json!({
            "description": line.description,
            "quantity": line.quantity,
            "unit_price": line.unit_price,
            "line_total": line_total,
            "sale_id": sale_id,
        }));
        invoice_items.push(crate::invoice::InvoiceItem {
            description: line.description.trim().to_string(),
            quantity: line.quantity,
            unit_price: line.unit_price,
        });
    }

    // Same reasoning and same shape as checkout()'s own Bookkeeping
    // post above — a service sale is real revenue too, and before
    // this it never showed up in Bookkeeping at all, unlike a goods
    // sale through checkout(). Unconditional here (no `if
    // !req.on_credit` guard): ServiceSaleRequest has no credit-sale
    // concept at all — there's no due date, no linked Debt & Credit
    // record, nothing distinguishing "paid" from "owed" the way
    // CheckoutRequest.on_credit does — so a service sale is always
    // treated as paid at the time it's rung up, same as any other POS
    // sale without on_credit set. Best-effort by design, not required,
    // matching checkout(): a business that hasn't enabled Bookkeeping
    // can still record a service sale.
    if let Ok(accounting_module) = crud::load_module(&tx, business_id, "accounting") {
        let mut entry: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
        entry.insert("description".into(), json!(format!("Service sale — order {order_id}")));
        entry.insert("entry_type".into(), json!("income"));
        // Its own category, not "Sales" — a service sale is a
        // distinct revenue line from a goods sale through checkout(),
        // and keeping them separately labeled is exactly the kind of
        // "clear, not ambiguous" breakdown a Bookkeeping report needs.
        entry.insert("category".into(), json!("Service Sales"));
        entry.insert("amount".into(), json!(subtotal));
        if let Some(p) = &req.payment_method {
            entry.insert("payment_method".into(), json!(p));
        }
        for f in &accounting_module.fields {
            if !entry.contains_key(&f.name) {
                if let Some(d) = &f.default {
                    entry.insert(f.name.clone(), d.clone());
                }
            }
        }
        accounting_module.validate(&entry)?;
        crate::reference_data::validate_field_references(&tx, business_id, &accounting_module, &entry)?;
        crud::insert_validated_record(&tx, business_id, &accounting_module, &entry)?;
    }

    // Every completed service sale gets a real, numbered invoice
    // automatically too — same reasoning and same best-effort pattern
    // as checkout()'s own call to this just above. A service sale has
    // no on_credit concept (see ServiceSaleRequest's own doc comment),
    // so it's always invoiced as immediately paid.
    if crud::load_module(&tx, business_id, "invoice").is_ok() {
        crate::invoice::create_invoice_for_order(
            &tx,
            business_id,
            &order_id,
            req.customer.as_deref(),
            req.customer_phone.as_deref(),
            &invoice_items,
            subtotal,
            false,
            None,
        )?;
    }

    tx.commit()?;

    let summary = json!({
        "order_id": order_id,
        "customer": req.customer,
        "customer_id": customer_id,
        "payment_method": req.payment_method,
        "subtotal": subtotal,
        "item_count": req.lines.len(),
        "items": lines_out,
    });

    let _ = crate::audit::log(conn, business_id, Some(user_id), "_pos", "service_sale", Some(&order_id), Some(&summary));
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
            "revenue": r.get::<_, i64>(3)?,
            "unit_price": r.get::<_, Option<i64>>(4)?,
            "customer": r.get::<_, Option<String>>(5)?,
            "payment_method": r.get::<_, Option<String>>(6)?,
            "created_at": r.get::<_, String>(7)?,
        }))
    })?;
    let items: Vec<Value> = rows.filter_map(|r| r.ok()).collect();
    if items.is_empty() {
        return Err(anyhow!("order not found"));
    }
    let subtotal: i64 = items.iter().filter_map(|v| v.get("revenue").and_then(|r| r.as_i64())).sum();
    Ok(json!({"order_id": order_id, "subtotal": subtotal, "items": items}))
}
