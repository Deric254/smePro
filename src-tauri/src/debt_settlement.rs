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
//! KNOWN LIMITATION, stated as plainly as receiving.rs states its own:
//! this closes the gap for the intended path. It does NOT prevent
//! someone with "update" on Debt & Credit from manually flipping
//! `settled` through the generic record-update endpoint without ever
//! calling this — the module engine has no field-level write
//! restriction that could enforce that. What this DOES guarantee:
//! every settlement made through the intended flow is atomically
//! correct and posts the real cash movement to Bookkeeping; a manual
//! edit bypassing it is a workaround, not a hole in the main path —
//! and the frontend (ModuleView.tsx) hides `settled` from the generic
//! edit form for the same reason it already hides purchasing's
//! `received`.
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

#[derive(Debug, Deserialize)]
pub struct SettleDebtRequest {
    pub debt_record_id: String,
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

    let debt_module = crud::load_module(conn, business_id, "debt_credit")
        .map_err(|_| anyhow!("the Debt & Credit module isn't enabled for this business"))?;
    let debt_table = debt_module.table_name();

    let tx = conn.transaction()?;

    let row: Option<(String, String, i64, bool)> = tx
        .query_row(
            &format!(
                "SELECT party_name, direction, amount, settled
                 FROM {debt_table} WHERE id = ?1 AND business_id = ?2 AND deleted_at IS NULL"
            ),
            params![req.debt_record_id, business_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((party_name, direction, amount, already_settled)) = row else {
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
        &format!("UPDATE {debt_table} SET settled = 1, updated_at = datetime('now') WHERE id = ?1 AND business_id = ?2"),
        params![req.debt_record_id, business_id],
    )?;

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
        "posted_to_bookkeeping_as": if amount > 0 { Some(if is_income { "income" } else { "expense" }) } else { None },
    });

    let _ = crate::audit::log(conn, business_id, Some(user_id), "_debt_credit", "settle", Some(&req.debt_record_id), Some(&summary));

    Ok(summary)
}
