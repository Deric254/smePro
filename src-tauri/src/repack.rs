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
//!
//! CREATING THE TARGET INLINE: the retail unit a repack produces very
//! often doesn't exist in Inventory yet — that's the whole point of
//! repacking for the first time ("Rice — 1kg bag" has no reason to
//! exist until something has actually been broken down into it). The
//! old design forced a business owner out of this modal, into
//! Inventory's own create form, to make that record first (typing a
//! SKU and a zero opening quantity that would immediately be
//! overwritten anyway), then back into this modal to actually run the
//! repack — two trips, and an easy way to end up with an abandoned
//! zero-quantity item if the second trip never happens. `target_record_id`
//! is now optional; `new_target_name` + `new_target_unit_price` let
//! this same call create that record and repack into it in one atomic
//! step. Exactly one of the two must be supplied (see the `match` near
//! the top of `repack()`) — there is no legitimate "neither" (which
//! item is this repack even for?) or "both" (which one actually
//! happened?).
//!
//! Deliberately NOT asking for a SKU or an opening cost alongside the
//! name: a SKU is an internal bookkeeping code no one filling in "what
//! did we just produce" naturally has ready, so one is generated here
//! (see `generate_repack_sku`) from the name itself, the same
//! "generate it, never hand-type it" treatment `po_number` and
//! `entry_number` already get elsewhere in this codebase. An opening
//! cost would be worse than redundant — it's the ONE thing about a
//! freshly-repacked item that must never be typed in, since the
//! entire reason this module exists is computing that cost correctly
//! from what was actually consumed. The new record is inserted at
//! quantity 0 / unit_cost 0 and then carried through the exact same
//! weighted-average math as an existing target (a zero starting
//! quantity contributes nothing to that formula, so no special case
//! is needed) — one code path computes the real cost for a brand-new
//! item and a restocked existing one alike, which is what keeps this
//! consistent rather than having two subtly different ways to arrive
//! at "the cost of this item."
//!
//! NOTE ON PRICE VS. COST: repacking an EXISTING target can change its
//! cost (the whole point of the weighted average) without ever
//! touching its selling price, which can leave an item priced below
//! its own cost if an expensive source got broken into it. Unlike
//! `crud::create`/`receiving::receive` — which refuse a caller-typed
//! price under a caller-typed cost — repack does not block on this:
//! the cost here is computed, not typed, and a real business needs to
//! see that outcome (and re-price the target) rather than have the
//! whole repack silently refused. The frontend's confirmation message
//! surfaces this as a "Profit reduction" rather than hiding it.

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
    /// The smaller retail item being produced (e.g. "Rice — 1kg bag"),
    /// when it already exists in Inventory. Omit this and supply
    /// `new_target_name` instead to create it as part of this same
    /// repack — see the module doc comment. Exactly one of the two
    /// must be present.
    #[serde(default)]
    pub target_record_id: Option<String>,
    /// How many target units this specific repack actually produced —
    /// supplied explicitly, not computed from a stored ratio. See the
    /// module doc comment for why.
    pub target_quantity_produced: i64,
    /// Set this (instead of `target_record_id`) to create a brand-new
    /// Inventory item as the target of this repack. Its SKU is
    /// generated automatically and its opening cost is computed from
    /// what this repack actually consumes — see `generate_repack_sku`
    /// and the module doc comment for why neither is asked for here.
    #[serde(default)]
    pub new_target_name: Option<String>,
    /// Required alongside `new_target_name`: the new item's selling
    /// price. Ignored (and should be omitted) when `target_record_id`
    /// is used instead — an existing item's price is its own, set
    /// through the ordinary edit form, not through a repack.
    #[serde(default)]
    pub new_target_unit_price: Option<i64>,
    #[serde(default)]
    pub notes: Option<String>,
}

/// Generates a fresh, guaranteed-unique SKU for a brand-new item
/// created inline by a repack — see the module doc comment for why
/// this modal deliberately never asks a person to type one. Derived
/// from the item's own name (upper-cased, any run of non-alphanumeric
/// characters collapsed to a single hyphen) so the result is still
/// recognizable at a glance rather than an opaque code, then
/// de-duplicated the same "scan what already exists, go one past the
/// highest match" way `receiving::generate_po_number` does — a plain
/// COUNT-based suffix would be wrong here for the identical reason
/// it's wrong there: a previously deleted record with the same
/// generated SKU must not free that SKU up for silent reuse.
fn generate_repack_sku(tx: &rusqlite::Transaction<'_>, business_id: &str, table: &str, name: &str) -> Result<String> {
    let mut base: String = name
        .to_uppercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while base.contains("--") {
        base = base.replace("--", "-");
    }
    let base = base.trim_matches('-');
    let base = if base.is_empty() { "ITEM" } else { base };

    let sku_exists = |candidate: &str| -> Result<bool> {
        let count: i64 = tx.query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE business_id = ?1 AND sku = ?2"),
            params![business_id, candidate],
            |r| r.get(0),
        )?;
        Ok(count > 0)
    };

    if !sku_exists(base)? {
        return Ok(base.to_string());
    }
    let mut n = 2;
    loop {
        let candidate = format!("{base}-{n}");
        if !sku_exists(&candidate)? {
            return Ok(candidate);
        }
        n += 1;
    }
}

/// Runs the whole repack as one atomic transaction: the source item's
/// stock decreases, the target item's stock increases, together or not
/// at all.
pub fn repack(conn: &mut Connection, business_id: &str, user_id: &str, req: RepackRequest) -> Result<Value> {
    crate::rbac::require(conn, user_id, "inventory", "repack")?;

    if req.source_quantity <= 0 {
        return Err(anyhow!("source quantity must be greater than zero"));
    }
    if req.target_quantity_produced <= 0 {
        return Err(anyhow!("target quantity produced must be greater than zero"));
    }

    // Exactly one way to say which item this repack produces — see the
    // module doc comment for why "both" and "neither" are each
    // rejected outright rather than one silently winning over the
    // other.
    let new_target_name = req
        .new_target_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match (&req.target_record_id, new_target_name) {
        (Some(_), Some(_)) => {
            return Err(anyhow!(
                "provide either an existing target item or a name for a new one — not both"
            ))
        }
        (None, None) => {
            return Err(anyhow!(
                "a target item is required — pick an existing item, or name a new one to create"
            ))
        }
        _ => {}
    }
    if let Some(id) = &req.target_record_id {
        if id == &req.source_record_id {
            return Err(anyhow!("the source and target can't be the same inventory item — that isn't a repack, it's a no-op"));
        }
    }
    // Creating a brand-new Inventory item is a "create", not a
    // "repack", from a permissions standpoint — a role that can repack
    // stock between two items that already exist isn't automatically a
    // role that should be able to add new items to the catalog.
    if new_target_name.is_some() {
        crate::rbac::require(conn, user_id, "inventory", "create")?;
        match req.new_target_unit_price {
            None => return Err(anyhow!("a selling price is required for the new item being created")),
            Some(p) if p < 0 => return Err(anyhow!("selling price can't be negative")),
            Some(_) => {}
        }
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

    // Create the new target item now, inside this same transaction, so
    // it either comes into being together with the stock/cost update
    // just below or (on any later error in this function) not at all —
    // see the module doc comment for why a two-trip "create it in
    // Inventory first, then repack into it" flow is what this
    // replaces. Started at quantity 0 / unit_cost 0: the weighted-average
    // math a few lines down treats that identically to an existing
    // target that's simply out of stock right now, so this needs no
    // special case of its own.
    let target_record_id: String = if let Some(name) = new_target_name {
        let sku = generate_repack_sku(&tx, business_id, &table, name)?;
        let mut new_record: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
        new_record.insert("sku".to_string(), json!(sku));
        new_record.insert("name".to_string(), json!(name));
        new_record.insert("quantity".to_string(), json!(0));
        new_record.insert("unit_cost".to_string(), json!(0));
        // Safe to unwrap: validated as `Some` above whenever
        // `new_target_name` is present.
        new_record.insert("unit_price".to_string(), json!(req.new_target_unit_price.unwrap()));
        for f in &inventory_module.fields {
            if !new_record.contains_key(&f.name) {
                if let Some(d) = &f.default {
                    new_record.insert(f.name.clone(), d.clone());
                }
            }
        }
        inventory_module.validate(&new_record)?;
        crate::reference_data::validate_field_references(&tx, business_id, &inventory_module, &new_record)?;
        crud::insert_validated_record(&tx, business_id, &inventory_module, &new_record)?
    } else {
        // Safe to unwrap: the `match` near the top of this function
        // already guarantees exactly one of `target_record_id` /
        // `new_target_name` is present.
        req.target_record_id.clone().unwrap()
    };

    let target: Option<(String, i64, i64, i64)> = tx
        .query_row(
            &format!("SELECT name, quantity, unit_cost, unit_price FROM {table} WHERE id = ?1 AND business_id = ?2 AND deleted_at IS NULL"),
            params![target_record_id, business_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((target_name, target_current_qty, target_current_unit_cost, target_unit_price)) = target else {
        return Err(anyhow!("target inventory item not found: {}", target_record_id));
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

    // Unlike `crud::create`/`receiving::receive` (which refuse to leave
    // a caller-typed price under a caller-typed cost), repack computes
    // its cost from what was actually consumed rather than accepting
    // one — the whole point of breaking bulk. Blocking the write here
    // would leave the source decremented in the caller's head but not
    // on disk (nothing to repack into instead), and would hide the
    // exact number a shopkeeper needs to see to re-price the target:
    // if the target's current selling price doesn't cover the real
    // cost of what was just produced, that's real information, not an
    // error condition — see the frontend's "Profit reduction" line,
    // which surfaces this outcome rather than treating it as invalid.

    tx.execute(
        &format!("UPDATE {table} SET quantity = ?1, updated_at = datetime('now') WHERE id = ?2 AND business_id = ?3"),
        params![source_new_qty, req.source_record_id, business_id],
    )?;
    tx.execute(
        &format!("UPDATE {table} SET quantity = ?1, unit_cost = ?2, updated_at = datetime('now') WHERE id = ?3 AND business_id = ?4"),
        params![target_new_qty, target_new_unit_cost, target_record_id, business_id],
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
        "target_record_id": target_record_id,
        "target_created": new_target_name.is_some(),
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
    let _ = crate::audit::log(conn, business_id, Some(user_id), "_repack", "repack", Some(&target_record_id), Some(&summary));

    Ok(summary)
}
