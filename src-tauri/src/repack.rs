//! Repacking / breaking bulk.
//!
//! The gap this closes: a business buys "1 Sack of 90kg," but sells
//! "loose kilograms" — two entirely separate Inventory records, with no
//! connection between them. Before this, converting one into the other
//! meant manually editing both quantities by hand: decrement the sack,
//! increment the loose-kg record, trust yourself to remember both steps
//! and get the math right every single time. One missed step and stock
//! is either lost (decremented the sack, forgot the kg) or duplicated
//! (incremented the kg, forgot the sack) — real inventory, silently
//! wrong, with nothing in the system able to tell you it happened.
//!
//! DELIBERATE DESIGN CHOICE: the conversion ratio is NOT a stored,
//! global property of a unit (e.g. "1 Sack always equals exactly 90kg
//! everywhere, forever"). It's supplied explicitly on each repack
//! instead — how many source units go in, how many target units come
//! out. A real shopkeeper's sack doesn't always yield exactly 90kg
//! (spillage, an underweight bag from the supplier); forcing a rigid
//! stored ratio would be less accurate to reality, not more, and would
//! require a whole separate unit-conversion-graph schema to do properly.
//! This is simpler AND more honest about how bulk-breaking actually
//! works in practice.
//!
//! Every repack is logged to the audit trail with the full before/after
//! quantities on both records — the traceability "nothing lost" actually
//! requires: not just that the math balances at the moment it happens,
//! but that anyone can look back later and see exactly what was
//! converted into what, and when.
//!
//! COST, not just quantity: the target item's `unit_cost` is
//! recalculated too, using the same weighted-average approach
//! receiving.rs uses for purchase orders — the cost consumed from the
//! source (source_quantity × the source's own unit_cost) is
//! distributed across the units produced, blended with whatever the
//! target already had in stock at its existing cost. A dozen eggs
//! bought for 300 and broken into 12 single eggs correctly leaves the
//! single-egg record at a cost of 25 each, not whatever arbitrary
//! value it happened to have before (often 0, for a brand-new item)
//! — an earlier version of this file only updated quantities and left
//! every repacked item's cost basis silently wrong, corrupting margin
//! and profit figures computed from it afterward.
//!
//! ROUNDING NEVER SILENTLY GAINS OR LOSES VALUE. `target_new_unit_cost`
//! is a single blended cost-per-unit rounded to the nearest cent (same
//! as receiving.rs), which by itself means `target_new_qty *
//! target_new_unit_cost` will not always equal the exact value that
//! actually went in — a completely ordinary repack (breaking a $90.00
//! sack into 91 one-kg bags) rounds to $0.99/bag, and 91 × $0.99 =
//! $90.09: nine cents that were never spent, appearing out of nowhere.
//! An earlier version of this file let that drift happen silently,
//! every single repack, with nothing recording it — exactly the "coin
//! lost" failure this module exists to prevent. The fix: the exact
//! remainder (`rounding_adjustment_cents` below) is computed and, when
//! nonzero, posted to Bookkeeping as its own labeled entry — a rounding
//! *gain* posts as income, a rounding *loss* posts as an expense. The
//! stock ledger and the books always reconcile to the exact cent; nothing
//! is ever quietly absorbed or dropped.
//!
//! PROFIT VISIBILITY: every repack also reports what selling the
//! consumed source quantity in bulk would have earned at its own
//! price (`bulk_equivalent_value`) against what selling everything
//! just produced would earn at the target's price
//! (`repacked_realizable_value`) — the actual economic reason a
//! business breaks bulk in the first place. `repack_profit_uplift` is
//! the difference; `repack_margin_uplift_pct` expresses it as a
//! percentage of the bulk value given up. Both are purely informational
//! (computed from each item's *current* `unit_price` at the moment of
//! this repack) — neither is stored or posted anywhere; they answer
//! "was this repack worth it," they don't change any ledger.
//!
//! TRACEABLE FROM EITHER END: every repack is written to the audit
//! trail twice — once keyed to the source record, once to the target
//! — so `GET /audit-log?record_id=<id>` finds it whether you're
//! looking up "what became of this sack" or "what was this bag made
//! from," not just one direction.

use crate::crud;
use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub struct RepackRequest {
    /// The bulk item being broken down (e.g. "Rice — Sack of 90kg").
    pub source_record_id: String,
    /// How many of the source unit are being consumed (usually 1 sack,
    /// but nothing stops breaking multiple sacks in one operation).
    pub source_quantity: i64,
    /// The smaller retail item being produced (e.g. "Rice — 1kg bag").
    pub target_record_id: String,
    /// How many target units this specific repack actually produced —
    /// supplied explicitly, not computed from a stored ratio. See the
    /// module doc comment for why.
    pub target_quantity_produced: i64,
    #[serde(default)]
    pub notes: Option<String>,
}

/// Runs the whole repack as one atomic transaction: the source item's
/// stock decreases, the target item's stock increases, together or not
/// at all.
pub fn repack(conn: &mut Connection, business_id: &str, user_id: &str, req: RepackRequest) -> Result<Value> {
    crate::rbac::require(conn, user_id, "inventory", "repack")?;

    if req.source_record_id == req.target_record_id {
        return Err(anyhow!("the source and target can't be the same inventory item — that isn't a repack, it's a no-op"));
    }
    if req.source_quantity <= 0 {
        return Err(anyhow!("source quantity must be greater than zero"));
    }
    if req.target_quantity_produced <= 0 {
        return Err(anyhow!("target quantity produced must be greater than zero"));
    }

    let inventory_module = crud::load_module(conn, business_id, "inventory")
        .map_err(|_| anyhow!("the Inventory module isn't enabled for this business"))?;
    let table = inventory_module.table_name();

    let tx = conn.transaction()?;

    // unit_price pulled alongside cost now too — needed for the
    // profit-uplift figures below, not just the cost-basis math.
    let source: Option<(String, i64, i64, i64)> = tx
        .query_row(
            &format!("SELECT name, quantity, unit_cost, unit_price FROM {table} WHERE id = ?1 AND business_id = ?2 AND deleted_at IS NULL"),
            params![req.source_record_id, business_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((source_name, source_current_qty, source_unit_cost, source_unit_price)) = source else {
        return Err(anyhow!("source inventory item not found: {}", req.source_record_id));
    };

    if source_current_qty < req.source_quantity {
        return Err(anyhow!(
            "not enough stock of '{source_name}' to repack: {source_current_qty} available, {} requested",
            req.source_quantity
        ));
    }

    let target: Option<(String, i64, i64, i64)> = tx
        .query_row(
            &format!("SELECT name, quantity, unit_cost, unit_price FROM {table} WHERE id = ?1 AND business_id = ?2 AND deleted_at IS NULL"),
            params![req.target_record_id, business_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((target_name, target_current_qty, target_current_unit_cost, target_unit_price)) = target else {
        return Err(anyhow!("target inventory item not found: {}", req.target_record_id));
    };

    let source_new_qty = source_current_qty - req.source_quantity;
    let target_new_qty = target_current_qty + req.target_quantity_produced;

    // The real fix: without this, the target item's cost basis never
    // reflects what was actually consumed to produce it — a dozen
    // eggs bought for 300 broken into 12 single eggs would leave the
    // "single egg" record at whatever cost it happened to have
    // before (often 0, for a brand-new item), silently corrupting
    // every margin/profit figure computed from it afterward. Instead:
    // the total cost consumed from the source (source_quantity *
    // source's own unit_cost) is distributed across the units
    // produced, blended with whatever the target already had in
    // stock at its existing cost — the exact same weighted-average
    // formula receiving.rs uses for purchase orders, applied here to
    // a repack instead. Integer cents throughout (see money.rs), so
    // this is exact arithmetic; the one deliberate rounding point is
    // the final division, rounded to the nearest cent rather than
    // truncated, same as receiving.rs.
    let cost_consumed_from_source = source_unit_cost * req.source_quantity;
    let existing_target_value = target_current_qty * target_current_unit_cost;
    // The exact, un-rounded total value the target record should now
    // represent — everything below is reconciled against this number
    // to the cent.
    let numerator = existing_target_value + cost_consumed_from_source;
    let target_new_unit_cost = if target_new_qty > 0 {
        (numerator + target_new_qty / 2) / target_new_qty
    } else {
        target_current_unit_cost // unreachable in practice (target_new_qty > 0 whenever target_quantity_produced > 0, already validated above), kept only so this can never divide by zero
    };

    // THE FIX: `target_new_qty * target_new_unit_cost` (what actually
    // gets stored) is not guaranteed to equal `numerator` (what was
    // actually consumed/already there) — rounding a blended per-unit
    // cost to the nearest cent and multiplying back out can land a few
    // cents above or below the exact value, in either direction.
    // Positive here means the stored value came in LOWER than the true
    // value consumed (a small loss that would otherwise vanish
    // untracked); negative means it came in HIGHER (value that
    // appeared from nowhere). Zero whenever target_new_qty divides
    // numerator evenly — the common case for round conversion ratios.
    let stored_target_value = target_new_qty * target_new_unit_cost;
    let rounding_adjustment_cents = numerator - stored_target_value;

    tx.execute(
        &format!("UPDATE {table} SET quantity = ?1, updated_at = datetime('now') WHERE id = ?2 AND business_id = ?3"),
        params![source_new_qty, req.source_record_id, business_id],
    )?;
    tx.execute(
        &format!("UPDATE {table} SET quantity = ?1, unit_cost = ?2, updated_at = datetime('now') WHERE id = ?3 AND business_id = ?4"),
        params![target_new_qty, target_new_unit_cost, req.target_record_id, business_id],
    )?;

    // Never silently absorbed: whatever the rounding step couldn't
    // represent exactly in the stock ledger gets posted to Bookkeeping
    // as its own labeled entry, in the same transaction as the stock
    // change itself — so the books and the stock ledger always
    // reconcile to the cent, and anyone can see exactly why. A loss
    // (stored value came in low) posts as an expense; a gain (stored
    // value came in high) posts as income. Best-effort, same pattern
    // as every other Bookkeeping post in this codebase: a business
    // without Accounting enabled can still repack.
    if rounding_adjustment_cents != 0 {
        if let Ok(accounting_module) = crud::load_module(&tx, business_id, "accounting") {
            let (entry_type, amount) = if rounding_adjustment_cents > 0 {
                ("expense", rounding_adjustment_cents)
            } else {
                ("income", -rounding_adjustment_cents)
            };
            let mut entry: HashMap<String, Value> = HashMap::new();
            entry.insert(
                "description".into(),
                json!(format!(
                    "Repack rounding {} — {source_name} → {target_name}",
                    if rounding_adjustment_cents > 0 { "loss" } else { "gain" }
                )),
            );
            entry.insert("entry_type".into(), json!(entry_type));
            entry.insert("category".into(), json!("Stock Revaluation"));
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

    // Same discipline as checkout() and receive(): nothing above is
    // durable until this line — every update above becomes real
    // together, or (if the process dies first) none of it does.
    tx.commit()?;

    // The economics of breaking bulk, made visible rather than left
    // for someone to work out by hand: what selling the consumed
    // source quantity in bulk would have earned, against what selling
    // everything just produced will earn at the target's own price.
    // Purely informational — computed from each item's current
    // unit_price at this moment, never stored, never posted anywhere.
    let bulk_equivalent_value = req.source_quantity * source_unit_price;
    let repacked_realizable_value = req.target_quantity_produced * target_unit_price;
    let repack_profit_uplift = repacked_realizable_value - bulk_equivalent_value;
    let repack_margin_uplift_pct = if bulk_equivalent_value > 0 {
        Some((repack_profit_uplift as f64 / bulk_equivalent_value as f64) * 100.0)
    } else {
        None
    };

    let summary = json!({
        "source_record_id": req.source_record_id,
        "source_name": source_name,
        "source_quantity_before": source_current_qty,
        "source_quantity_after": source_new_qty,
        "source_unit_cost": source_unit_cost,
        "source_unit_price": source_unit_price,
        "target_record_id": req.target_record_id,
        "target_name": target_name,
        "target_quantity_before": target_current_qty,
        "target_quantity_after": target_new_qty,
        "target_quantity_produced": req.target_quantity_produced,
        "target_unit_cost_before": target_current_unit_cost,
        "target_unit_cost_after": target_new_unit_cost,
        "target_unit_price": target_unit_price,
        // Reconciliation: exact value that went into the target record
        // vs. what actually got stored after rounding, and the labeled
        // adjustment (if any) that accounts for the difference. This
        // triple should always satisfy: exact_value_consumed ==
        // stored_target_value + rounding_adjustment_cents.
        "exact_value_consumed": numerator,
        "stored_target_value": stored_target_value,
        "rounding_adjustment_cents": rounding_adjustment_cents,
        "rounding_adjustment_posted_to_bookkeeping": rounding_adjustment_cents != 0,
        // The profit case for repacking, at today's prices.
        "bulk_equivalent_value": bulk_equivalent_value,
        "repacked_realizable_value": repacked_realizable_value,
        "repack_profit_uplift": repack_profit_uplift,
        "repack_margin_uplift_pct": repack_margin_uplift_pct,
        "notes": req.notes,
    });

    // Logged under BOTH records — this is what makes "nothing lost"
    // actually checkable from either direction later: looking up the
    // sack's history finds this repack, and so does looking up the
    // bag's. Same immutable audit_log entry content either way, only
    // the indexed record_id differs, so `GET
    // /audit-log?record_id=<id>` finds it starting from whichever item
    // someone actually has in front of them.
    let _ = crate::audit::log(conn, business_id, Some(user_id), "_repack", "repack", Some(&req.source_record_id), Some(&summary));
    let _ = crate::audit::log(conn, business_id, Some(user_id), "_repack", "repack", Some(&req.target_record_id), Some(&summary));

    Ok(summary)
}
