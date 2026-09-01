//! Settling a debt or credit — the missing counterpart to
//! `debt_credit.json`'s own record.
//!
//! Before this, `settled` was a bare boolean field on a Debt & Credit
//! record, changeable only through the generic update endpoint — the
//! exact same way `notes` or `party_name` gets edited. That's the same
//! shape of gap `refund.rs` closed for Sales and `receiving.rs` closed
//! for Purchasing: a real cash event (a customer finally paying what
//! they owed, or the business finally paying off what it owed a
//! supplier) with nothing in the system that actually recorded the
//! cash side of it. Flipping the checkbox changed nothing in
//! Bookkeeping — the ledger stayed silent about money that had just
//! come in or gone out.
//!
//! This closes that the same way pos.rs/receiving.rs/refund.rs already
//! close their own versions of it: one purpose-built permission
//! ("settle" on Debt & Credit, not just "update"), one atomic
//! transaction, and the same best-effort Bookkeeping auto-post those
//! three already do — posted only when Bookkeeping happens to be
//! enabled for this business, never required for settling to work.
//!
//! CORRECTION to an earlier version of this comment, same correction
//! receiving.rs's own doc comment now states: this used to claim the
//! module engine has no field-level write restriction that could stop
//! someone with plain "update" on Debt & Credit from flipping `settled`
//! through the generic record-update endpoint instead of calling this
//! function. It does now — see crud.rs's `is_update_blocked_field`,
//! which unconditionally blocks `settled`, `payment_method`, and
//! `source_order_id` on every single-record update regardless of the
//! caller's role. The frontend (ModuleView.tsx) also hides `settled`
//! from the generic edit form, for the same reason it already hides
//! purchasing's `received` — belt AND suspenders, not just one or the
//! other.
//!
//! DIRECTION, not a fixed enum: `direction` is a free-text field (see
//! debt_credit.json) — the module engine has no enum/choice field type
//! to constrain it to. The only value this codebase itself ever
//! writes there is "owed_to_business" (pos.rs, for a credit sale).
//! Settling that direction means cash comes IN (income). Any other
//! value found in this field is treated as the business owing someone
//! else, so settling it means cash goes OUT (expense) — this is the
//! one place in the app that actually interprets this field's
//! meaning, rather than treating it as opaque text.

use crate::crud;
use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

/// Generates the next `entry_number` for a Debt & Credit record —
/// same shape and same reasoning as `receiving::generate_po_number`:
/// a real, business-scoped-unique identifier, read once as a max and
/// incremented, never taken from the caller.
///
/// THE BUG THIS FIXES: `debt_credit` used to have no field marked
/// `unique: true` at all, which meant Excel re-import could only ever
/// safely match against nothing — see excel_import.rs's own
/// `key_field_is_unique` comment. That's the intentionally SAFE
/// behavior for a field like `party_name` (one party can legitimately
/// have many separate, simultaneous debt/credit entries — pos.rs
/// creates one per credit sale, and two different anonymous walk-in
/// customers can both have an empty `party_name`), so `party_name`
/// itself must never become the unique key — doing that would put a
/// real `UNIQUE(business_id, party_name)` constraint in the way of a
/// second credit sale to the same repeat customer, an outright
/// regression on a core POS path.
///
/// `entry_number` is what `po_number` is for Purchasing: a field with
/// no other purpose than being a safe, always-present, never-hand-
/// edited identity a spreadsheet re-import can match rows against —
/// so "download an Export to Excel, correct a value, reimport it" can
/// finally work for Debt & Credit the same way it already does for
/// Purchasing and Inventory, without reopening the exact "matches
/// whichever ONE existing record happens to share a non-unique value"
/// hole this whole mechanism exists to close.
pub fn generate_entry_number(conn: &Connection, business_id: &str) -> Result<String> {
    let max_existing: i64 = conn.query_row(
        "SELECT COALESCE(MAX(CAST(SUBSTR(entry_number, 4) AS INTEGER)), 0)
         FROM module_debt_credit WHERE business_id = ?1 AND entry_number LIKE 'DC-%'",
        params![business_id],
        |r| r.get(0),
    )?;
    Ok(format!("DC-{}", max_existing + 1))
}

#[derive(Debug, Deserialize)]
pub struct SettleDebtRequest {
    pub debt_record_id: String,
    /// How the money actually moved — required, not optional: a
    /// settlement IS a real cash event (see the module doc comment
    /// above), and leaving this blank is exactly the "(not set)"
    /// ambiguity report.rs's own comment already has to work around
    /// for sales.payment_method. A credit sale's payment_method is
    /// honestly unset AT THE TIME OF SALE — that's a true fact, not a
    /// gap to fix. But by the time someone is settling it, the actual
    /// payment method is a known, real fact, and there's no reason for
    /// it to stay a blank in the ledger.
    pub payment_method: String,
}

/// Runs the whole settlement as one atomic transaction: marks the
/// record settled and — best-effort, only when Bookkeeping is enabled
/// — posts the real cash movement to it. Either both happen together,
/// or neither does.
pub fn settle(conn: &mut Connection, business_id: &str, user_id: &str, req: SettleDebtRequest) -> Result<Value> {
    // One purpose-built permission, matching checkout()'s "sell",
    // receive()'s "receive", and refund's "refund" — not a reuse of
    // plain "update" for what is really a distinct financial action.
    crate::rbac::require(conn, user_id, "debt_credit", "settle")?;

    let payment_method = req.payment_method.trim();
    if payment_method.is_empty() {
        return Err(anyhow!("select how this was paid before settling — a settlement must record its payment method"));
    }

    let debt_module = crud::load_module(conn, business_id, "debt_credit")
        .map_err(|_| anyhow!("the Debt & Credit module isn't enabled for this business"))?;
    let debt_table = debt_module.table_name();

    let tx = conn.transaction()?;

    let row: Option<(String, String, i64, bool, Option<String>)> = tx
        .query_row(
            &format!(
                "SELECT party_name, direction, amount, settled, source_order_id
                 FROM {debt_table} WHERE id = ?1 AND business_id = ?2 AND deleted_at IS NULL"
            ),
            params![req.debt_record_id, business_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let Some((party_name, direction, amount, already_settled, source_order_id)) = row else {
        return Err(anyhow!("debt/credit record not found: {}", req.debt_record_id));
    };

    // The same integrity discipline as refund.rs's double-refund
    // guard: settling twice would silently post the same cash
    // movement to Bookkeeping a second time.
    if already_settled {
        return Err(anyhow!(
            "this record is already marked settled — settling it again would double-post the cash movement to Bookkeeping"
        ));
    }

    tx.execute(
        &format!("UPDATE {debt_table} SET settled = 1, payment_method = ?3, updated_at = datetime('now') WHERE id = ?1 AND business_id = ?2"),
        params![req.debt_record_id, business_id, payment_method],
    )?;

    // Closes the exact ambiguity report.rs's own comment names: a
    // credit sale's original sales row has payment_method = NULL
    // (honestly — no cash had moved yet), which groups into the
    // payment-method breakdown chart's "(not set)" bucket forever,
    // even after the debt is fully paid off. Now that it genuinely
    // has been paid, and we know how, the sales row that started this
    // debt gets that real, final answer recorded on it — not a rewrite
    // of what happened at sale time (item, quantity, price, date all
    // stay exactly as they were), just filling in the one fact that
    // wasn't yet knowable then. Scoped tightly: only the specific
    // order this debt came from, and only if it's still genuinely
    // unset, so this can never overwrite a real payment_method that
    // (for whatever reason) already exists on that row.
    if let Some(order_id) = &source_order_id {
        if let Ok(sales_module) = crud::load_module(&tx, business_id, "sales") {
            let sales_table = sales_module.table_name();
            tx.execute(
                &format!(
                    "UPDATE {sales_table} SET payment_method = ?3, updated_at = datetime('now')
                     WHERE business_id = ?1 AND order_id = ?2 AND deleted_at IS NULL AND payment_method IS NULL"
                ),
                params![business_id, order_id, payment_method],
            )?;
        }
    }

    // See the module doc comment above: "owed_to_business" is the
    // only direction this codebase itself ever writes (pos.rs, a
    // credit sale) — settling it is cash coming IN. Anything else
    // found in this free-text field is treated as the business owing
    // someone else, so settling it is cash going OUT.
    let is_income = direction == "owed_to_business";

    // Same Bookkeeping auto-post as checkout()/receive()/
    // process_refund(). Skipped for a zero-amount record (nothing to
    // log on the cash ledger) and best-effort: a business without
    // Bookkeeping enabled can still settle a debt.
    if amount > 0 {
        if let Ok(accounting_module) = crud::load_module(&tx, business_id, "accounting") {
            let mut entry: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
            entry.insert(
                "description".into(),
                json!(format!("{} settled — {party_name}", if is_income { "Debt" } else { "Credit" })),
            );
            entry.insert("entry_type".into(), json!(if is_income { "income" } else { "expense" }));
            entry.insert("category".into(), json!("Debt & Credit"));
            entry.insert("amount".into(), json!(amount));
            entry.insert("payment_method".into(), json!(payment_method));
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

    // Same "commit is the one moment this becomes real" discipline as
    // checkout()/receive()/process_refund() — nothing above is
    // durable until this line.
    tx.commit()?;

    let summary = json!({
        "debt_record_id": req.debt_record_id,
        "party_name": party_name,
        "direction": direction,
        "amount": amount,
        "settled": true,
        "payment_method": payment_method,
        "posted_to_bookkeeping_as": if amount > 0 { Some(if is_income { "income" } else { "expense" }) } else { None },
    });

    let _ = crate::audit::log(conn, business_id, Some(user_id), "_debt_credit", "settle", Some(&req.debt_record_id), Some(&summary));

    Ok(summary)
}

/// Aggregate totals for the Debt & Credit dashboard widget — computed
/// directly from the full table with real SQL SUM/COUNT, not from
/// whatever page of records the generic list endpoint happens to have
/// returned (that endpoint caps at 1000 rows; a business with more
/// open debt records than that would get a silently wrong total from
/// a client-side sum). "Truthful" numbers here specifically means
/// numbers computed over every unsettled row, every time, not a
/// cached or partial view of them.
///
/// `today` is passed in from the caller (http_api.rs) rather than
/// computed here with `date('now')` in SQL, so this always compares
/// against the exact same real, current calendar date the rest of the
/// app is using at request time — one source of truth for "what day
/// is it", not two clocks (the database's and the request's) that
/// could disagree.
#[derive(Debug, serde::Serialize)]
pub struct DebtSummary {
    pub owed_to_business_unpaid: i64,
    pub owed_to_business_unpaid_count: i64,
    pub owed_by_business_unpaid: i64,
    pub owed_by_business_unpaid_count: i64,
    pub overdue_amount: i64,
    pub overdue_count: i64,
    /// Due within the next 7 days (inclusive), not yet overdue —
    /// the early-warning tier before something actually becomes
    /// overdue.
    pub due_soon_amount: i64,
    pub due_soon_count: i64,
}

pub fn summary(conn: &Connection, business_id: &str, user_id: &str, today: &str) -> Result<DebtSummary> {
    crate::rbac::require(conn, user_id, "debt_credit", "read")?;
    let debt_module = crud::load_module(conn, business_id, "debt_credit")
        .map_err(|_| anyhow!("the Debt & Credit module isn't enabled for this business"))?;
    let table = debt_module.table_name();

    let (owed_to_business_unpaid, owed_to_business_unpaid_count): (i64, i64) = conn.query_row(
        &format!(
            "SELECT COALESCE(SUM(amount), 0), COUNT(*) FROM {table}
             WHERE business_id = ?1 AND deleted_at IS NULL AND settled = 0 AND direction = 'owed_to_business'"
        ),
        params![business_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;

    // Same interpretation of `direction` as settle() above: anything
    // that isn't literally "owed_to_business" means the business owes
    // someone else.
    let (owed_by_business_unpaid, owed_by_business_unpaid_count): (i64, i64) = conn.query_row(
        &format!(
            "SELECT COALESCE(SUM(amount), 0), COUNT(*) FROM {table}
             WHERE business_id = ?1 AND deleted_at IS NULL AND settled = 0 AND direction != 'owed_to_business'"
        ),
        params![business_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;

    let (overdue_amount, overdue_count): (i64, i64) = conn.query_row(
        &format!(
            "SELECT COALESCE(SUM(amount), 0), COUNT(*) FROM {table}
             WHERE business_id = ?1 AND deleted_at IS NULL AND settled = 0
             AND due_date IS NOT NULL AND due_date != '' AND due_date < ?2"
        ),
        params![business_id, today],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;

    let (due_soon_amount, due_soon_count): (i64, i64) = conn.query_row(
        &format!(
            "SELECT COALESCE(SUM(amount), 0), COUNT(*) FROM {table}
             WHERE business_id = ?1 AND deleted_at IS NULL AND settled = 0
             AND due_date IS NOT NULL AND due_date != '' AND due_date >= ?2 AND due_date <= date(?2, '+7 days')"
        ),
        params![business_id, today],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;

    Ok(DebtSummary {
        owed_to_business_unpaid,
        owed_to_business_unpaid_count,
        owed_by_business_unpaid,
        owed_by_business_unpaid_count,
        overdue_amount,
        overdue_count,
        due_soon_amount,
        due_soon_count,
    })
}
