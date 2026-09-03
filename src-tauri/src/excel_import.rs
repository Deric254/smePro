//! Excel-based bulk import — download a template matching a module's
//! real fields, fill it in a spreadsheet, upload it back.
//!
//! Deliberately serves two real needs with one mechanism rather than
//! building them as separate features: adding a batch of new products,
//! and doing a stock take (reconciling counted quantities against what
//! the system thinks is on the shelf). Both are "here's a spreadsheet
//! of items with values" — the only real difference is whether each
//! row's key matches something that already exists. If it does, this
//! UPDATES that record instead of creating a duplicate — exactly what
//! a stock take needs (correct the count on the item that's already
//! there), and exactly what re-uploading a template with a typo fixed
//! needs too (correct the row, not create a second copy of it).
//!
//! THE BUG THIS FIXES: "each row's key" used to mean, unconditionally,
//! whatever field the caller (or this module's own http_api.rs
//! fallback) passed as `key_field` — which in practice meant "the
//! module's first field," since that's what both defaulted to with no
//! further check. For `inventory`, the first field (`sku`) happens to
//! be genuinely unique, so that accident was harmless. For
//! `purchasing`, the first field used to be `supplier` — NOT unique,
//! since one supplier legitimately has many separate orders — so a
//! multi-row import all from one supplier matched every row onto
//! whichever ONE existing purchasing record already had that supplier,
//! tried to silently "update" it repeatedly, and got rejected on every
//! row by the blocked-field check further down (since that record's
//! real `received` almost never equals the sheet's default-filled
//! `received: false`). The visible symptom was "0 created, 0 updated,
//! every row an error," on a template that doesn't even have a
//! `received` column.
//!
//! The fix, and what "matches something that already exists" actually
//! means now: matching is only ever attempted when `key_field` is a
//! field the module itself declares `unique: true` — see
//! `key_field_is_unique` below. Purchasing didn't have one at all until
//! this fix also added `po_number` (generated at creation — see
//! crud.rs's purchasing block — never hand-typed), which both closes
//! the original hole (there's now a genuinely safe field to key on)
//! and restores the "correct a row via re-import" capability Purchasing
//! never actually had before.
//!
//! For `inventory` specifically, the two source files are no longer
//! header-identical: the blank "download template" (built by
//! `generate_template` below) omits `quantity` entirely, because a
//! new item is never allowed to seed its own opening count — the
//! `None` branch below forces it to 0 regardless of what's typed. The
//! "Export to Excel" file (built separately in ModuleView.tsx) still
//! carries real `quantity` values, because re-importing IT is the
//! sanctioned stock-take path. `import` tells the two apart by
//! whether the uploaded header row has a `quantity` column at all,
//! and rejects (rather than silently reconciling) any row from a
//! quantity-less upload that matches an existing item — see the
//! `Some(id)` branch's first check below.
//!
//! Every row goes through the EXACT SAME validation as a record typed
//! in by hand — `module.validate()` and
//! `reference_data::validate_field_references()`, the same functions
//! `crud::create` uses — not a parallel, simplified check that could
//! drift out of sync with the real rules. A bad row is reported with
//! its exact row number and reason and skipped; it does not abort the
//! whole batch, and it does not get silently coerced into something
//! technically valid but wrong.
//!
//! For `purchasing` specifically, two of the module's own fields never
//! appear as template columns at all — `generate_template` leaves them
//! out, same idea as `inventory`'s `quantity` above: `received` is
//! system-set only (via `receiving.rs::receive()`), and
//! `inventory_record_id` is an internal ID a spreadsheet-filling
//! business owner has no way to know. Both are things the manual
//! create/edit form already keeps out of a person's hands — `received`
//! by hiding it entirely, `inventory_record_id` by resolving it from a
//! NAME picked out of a dropdown (see ModuleView.tsx's
//! PurchaseItemSelector) instead of asking for the ID directly. `import`
//! below does that same name-to-id resolution itself, from the
//! `item_name` column that's already required, and rejects a row
//! outright if no Inventory item by that name exists — never creating
//! or leaving behind a purchasing record that isn't actually linked to
//! anything `receiving.rs` could ever receive against.

use crate::{audit, crud, module::ModuleDef, receiving, reference_data};
use anyhow::{anyhow, Result};
use calamine::{open_workbook_from_rs, Data, Reader, Xlsx};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::io::Cursor;

/// Builds a downloadable .xlsx template for a module: one header row
/// with the module's own field names, in order, so it's unambiguous
/// which column is which when it comes back.
///
/// `inventory`'s `quantity` is deliberately left out of this blank
/// template. This template exists for one job — adding brand-new
/// items — and a new item is never allowed to seed its own opening
/// stock count (see crud.rs::create and this file's `import`, `None`
/// branch). Printing a quantity column on a sheet whose values are
/// never actually honored invites exactly the confusion the two
/// screenshots this fix was written against showed: someone typing a
/// real count into that column, reasonably expecting it to land, and
/// it silently not doing so. The *export* of real records (built
/// separately by `exportModule` in ModuleView.tsx, not this
/// function) still carries `quantity`, because that file's job is
/// different — reconciling counts on items that already exist, the
/// sanctioned stock-take path — and this function has no part in
/// producing it.
pub fn generate_template(module: &ModuleDef) -> Result<Vec<u8>> {
    let mut wb = rust_xlsxwriter::Workbook::new();
    let sheet = wb.add_worksheet().set_name("Import")?;

    let header_format = rust_xlsxwriter::Format::new()
        .set_bold()
        .set_background_color("#EAE3CE");

    let template_fields: Vec<_> = module
        .fields
        .iter()
        .filter(|f| !(module.id == "inventory" && f.name == "quantity"))
        // Same reasoning as inventory's `quantity` just above, for two
        // different purchasing fields:
        // - `received` is set only by receiving.rs::receive(), never by
        //   hand or by import (see is_update_blocked_field and this
        //   file's own forced-false default on the create path below).
        //   The generic create/edit form already hides it for the same
        //   reason (ModuleView.tsx's isActionManagedField) — printing it
        //   on a sheet whose values are never honored is the exact
        //   "invites someone to type a real value there, reasonably
        //   expecting it to land" trap the quantity fix above closes.
        // - `inventory_record_id` is an internal database ID, not
        //   something a business owner filling in a spreadsheet could
        //   know or type. The manual form never asks for it either —
        //   ModuleView.tsx's PurchaseItemSelector has the person pick
        //   the item by NAME from existing Inventory records and fills
        //   the ID in for them. `import` below does the same lookup
        //   itself, from the `item_name` column that's already on the
        //   template and already required.
        // - `po_number` is generated once, at creation, the same way
        //   invoice_number is (see crud::create's purchasing block) —
        //   a business owner filling in a BLANK template for brand-new
        //   orders has no real number to type yet, so this column
        //   isn't offered here. It DOES appear on a real "Export to
        //   Excel" of existing records (that file isn't built by this
        //   function — see ModuleView.tsx's exportModule), which is
        //   exactly what makes re-uploading THAT file able to correct
        //   an existing, not-yet-received order: `import` below
        //   matches rows by `po_number` when it's present, and only
        //   generates a fresh one when it's genuinely missing.
        .filter(|f| !(module.id == "purchasing" && (f.name == "received" || f.name == "inventory_record_id" || f.name == "po_number")))
        // Same reasoning as purchasing's `po_number` just above,
        // applied to debt_credit's own generated identity: a blank
        // "new entries" template has no real `entry_number` to offer
        // yet (see crud::create's debt_credit block), so it isn't a
        // column here — but it DOES appear on a real "Export to Excel"
        // of existing records, which is what lets `import` below match
        // rows by `entry_number` on a genuine re-upload.
        .filter(|f| !(module.id == "debt_credit" && f.name == "entry_number"))
        .collect();

    for (col, field) in template_fields.iter().enumerate() {
        let label = if field.required {
            format!("{} *", field.name)
        } else {
            field.name.clone()
        };
        sheet.write_string_with_format(0, col as u16, &label, &header_format)?;
    }

    // One example row, greyed-out-by-convention (a leading '#'), shows
    // the expected shape without the user having to guess — deleted
    // by them before their real data, same convention as most
    // downloadable import templates.
    for (col, field) in template_fields.iter().enumerate() {
        let example = match field.field_type.as_str() {
            "integer" => "0".to_string(),
            "real" => "0.00".to_string(),
            // Shown as a plain decimal amount, matching exactly what a
            // person should type — the integer-cents conversion this
            // app does internally for "money" fields is not something
            // a spreadsheet-filling business owner should ever need to
            // know about or type themselves.
            "money" => "0.00".to_string(),
            "boolean" => "false".to_string(),
            "date" => "2026-01-31".to_string(),
            _ => format!("example {}", field.name),
        };
        sheet.write_string(1, col as u16, format!("# {example}"))?;
    }

    sheet.autofit();
    let bytes = wb.save_to_buffer()?;
    Ok(bytes)
}

pub struct ImportResult {
    pub created: usize,
    pub updated: usize,
    pub errors: Vec<Value>,
}

/// Parses an uploaded .xlsx, validates every row against the module's
/// real schema, and creates or updates records accordingly — all in
/// one transaction, so a batch that dies partway through (a genuine
/// crash, not a per-row validation failure, which is handled per-row
/// and doesn't abort anything) leaves nothing half-applied.
pub fn import(
    conn: &mut Connection,
    business_id: &str,
    user_id: &str,
    module: &ModuleDef,
    xlsx_bytes: Vec<u8>,
    key_field: &str,
) -> Result<ImportResult> {
    // One check up front for the whole batch. Rows that end up being
    // updates (an existing key match) get their own additional
    // "update" check inside crud::update below — so a user with only
    // "create" can add new items via import but a row matching an
    // existing key will correctly fail for them individually, not
    // silently succeed as an unauthorized overwrite.
    crate::rbac::require(conn, user_id, &module.id, "create")?;

    // Importing a Purchasing order now also receives it immediately
    // (see the big comment on the auto-receive block further down for
    // why) — which means this import can increase Inventory stock,
    // the exact same effect `receiving::receive` has, so it needs that
    // same permission checked up front for the whole batch. Without
    // this, someone with only "create" on Purchasing could use a bulk
    // import to do something a plain single receive() call would
    // correctly have refused them.
    if module.id == "purchasing" {
        crate::rbac::require(conn, user_id, "inventory", "receive")?;
    }

    // Needed to correctly parse any "money"-typed column — a human
    // typing "19.99" into a spreadsheet cell means something
    // different depending on the business's currency (2 decimal
    // places for USD/KES, 0 for JPY, 3 for KWD — see
    // money::decimal_places_for), and the cell has no way to carry
    // that context itself.
    let currency: String = conn
        .query_row("SELECT currency FROM businesses WHERE id = ?1", rusqlite::params![business_id], |r| r.get(0))
        .unwrap_or_else(|_| "USD".to_string());

    let cursor = Cursor::new(xlsx_bytes);
    let mut workbook: Xlsx<_> =
        open_workbook_from_rs(cursor).map_err(|e| anyhow!("could not read this file as an Excel spreadsheet: {e}"))?;
    let range = workbook
        .worksheet_range_at(0)
        .ok_or_else(|| anyhow!("this spreadsheet has no sheets"))?
        .map_err(|e| anyhow!("could not read the first sheet: {e}"))?;

    let mut rows = range.rows();
    let header_row = rows.next().ok_or_else(|| anyhow!("the spreadsheet is empty — no header row found"))?;
    let headers: Vec<String> = header_row
        .iter()
        .map(|c| cell_to_string(c).trim_end_matches(" *").to_string())
        .collect();

    // THE BUG THIS FIXES: matching an uploaded row against an existing
    // record is only ever safe on a field the module itself declares
    // `unique: true` (see module.rs::business_scoped_unique_constraints)
    // — that's the one guarantee that "this row's key equals that
    // record's key" really means "these are the same real-world thing".
    // `key_field` used to be trusted blindly, whatever the caller (or
    // http_api.rs's own fallback of "the module's first field") passed
    // in. For `inventory`, the first field (`sku`) is genuinely unique,
    // so that fallback happened to be safe by accident. For
    // `purchasing`, the first field is `supplier` — NOT unique, since
    // one supplier legitimately has many separate orders — and
    // `debt_credit`/any other module with no unique field at all has
    // the exact same exposure. Matching on a non-unique field means
    // every row whose value happens to equal an existing record's
    // silently collapses onto that ONE existing record instead of
    // creating its own: a five-row Purchasing import all from the same
    // supplier would match all five rows to whichever one PO already
    // existed for that supplier, attempt to overwrite it five times,
    // and — since that existing PO's own `received` almost certainly
    // differs from the sheet's default-filled `received: false` — get
    // rejected by the blocked-field check below on every single row.
    // The visible symptom was "0 created, 0 updated, every row failed
    // with a `received` error" on a template that doesn't even have a
    // `received` column.
    //
    // The fix: only ever attempt to match-and-update when `key_field`
    // is a field this module actually marked unique. When a module has
    // no unique field at all (purchasing, and any future module in the
    // same shape — an append-only log of transactions, not a catalog
    // of named things), there is no such thing as a legitimate "this
    // row is the same as that one" — every row is necessarily a brand
    // new record, so importing just creates, the same as it always
    // should have.
    let key_field_is_unique = module.fields.iter().any(|f| f.name == key_field && f.unique);

    // THE BUG THIS FIXES (round 2, caught only by re-tracing this
    // logic against the actual new po_number feature rather than
    // testing each piece in isolation): `purchasing.po_number` is now
    // the module's only unique field, which makes it the smart
    // default `key_field` (see http_api.rs) — but unlike every other
    // unique key field this engine has ever had, its blank "new
    // orders" template DELIBERATELY never carries it as a column at
    // all (see generate_template — it's system-generated, not
    // something a business owner filling in a spreadsheet could type).
    // Without this exemption, the header check just below would reject
    // every legitimate new-order import outright, before a single row
    // is even read, the moment the default key_field started
    // resolving to po_number — the exact opposite of what adding
    // po_number was supposed to fix. `inventory.sku` doesn't have this
    // problem: a person must always type a SKU themselves, even for a
    // brand-new item, so its absence really does mean the wrong file.
    let key_field_can_be_generated = (module.id == "purchasing" && key_field == "po_number")
        || (module.id == "debt_credit" && key_field == "entry_number");

    if key_field_is_unique && !key_field_can_be_generated && !headers.iter().any(|h| h == key_field) {
        return Err(anyhow!(
            "this spreadsheet's header row doesn't have a '{key_field}' column — download a fresh template and keep the header row exactly as it is"
        ));
    }

    // Distinguishes the two files this one importer accepts for
    // `inventory` (see the module doc comment above, and
    // generate_template's comment on why the blank template no
    // longer has this column at all): a real "Export to Excel" of
    // existing records always carries `quantity`; the blank
    // new-item template deliberately never does. That single
    // difference is what tells this function, per upload, which of
    // the two jobs it's doing — it's checked once here rather than
    // per-row because it's a property of the *file*, not of any one
    // row in it.
    let inventory_sheet_has_quantity_column = headers.iter().any(|h| h == "quantity");

    let tx = conn.transaction()?;
    let mut created = 0usize;
    let mut updated = 0usize;
    let mut errors = Vec::new();

    // THE BUG THIS FIXES: `generate_po_number` was called fresh, once
    // per row, inside the loop below — and it works by scanning every
    // po_number this business already has to find the current max
    // (see its own doc comment for why that's the correct logic, not
    // a plain COUNT). Correct in isolation, but calling it once per
    // row turns a single import into O(rows_imported × existing_POs)
    // table scans — fine for five rows, genuinely slow for the
    // thousand-row reconciliation a business doing this seriously
    // will eventually run. The starting point only needs to be read
    // from disk ONCE per import; every row after that just needs
    // "one more than the last row used," which is a plain in-memory
    // increment. Only fetched for `purchasing` at all — every other
    // module pays nothing for this.
    let mut purchasing_next_po_seq: Option<i64> = if module.id == "purchasing" {
        Some(tx.query_row(
            "SELECT COALESCE(MAX(CAST(SUBSTR(po_number, 4) AS INTEGER)), 0)
             FROM module_purchasing WHERE business_id = ?1 AND po_number LIKE 'PO-%'",
            rusqlite::params![business_id],
            |r| r.get(0),
        )?)
    } else {
        None
    };

    // Same fix, same reason, as `purchasing_next_po_seq` just above,
    // for debt_credit's own generated identity — see
    // debt_settlement::generate_entry_number's doc comment for why
    // this exists at all.
    let mut debt_credit_next_entry_seq: Option<i64> = if module.id == "debt_credit" {
        Some(tx.query_row(
            "SELECT COALESCE(MAX(CAST(SUBSTR(entry_number, 4) AS INTEGER)), 0)
             FROM module_debt_credit WHERE business_id = ?1 AND entry_number LIKE 'DC-%'",
            rusqlite::params![business_id],
            |r| r.get(0),
        )?)
    } else {
        None
    };

    for (i, row) in rows.enumerate() {
        let row_num = i + 2; // +1 for 0-index, +1 for the header row itself
        let cells = row;

        // A completely blank row (common at the end of a spreadsheet a
        // person has been editing) is skipped silently — not an error,
        // just nothing to import.
        if cells.iter().all(|c| matches!(c, Data::Empty)) {
            continue;
        }

        let mut record: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
        for (col_idx, header) in headers.iter().enumerate() {
            let Some(field) = module.fields.iter().find(|f| &f.name == header) else { continue };
            let Some(cell) = cells.get(col_idx) else { continue };
            if matches!(cell, Data::Empty) { continue; }
            record.insert(field.name.clone(), cell_to_json(cell, &field.field_type, &currency));
        }

        for f in &module.fields {
            if !record.contains_key(&f.name) {
                if let Some(d) = &f.default {
                    record.insert(f.name.clone(), d.clone());
                }
            }
        }

        if let Err(e) = module.validate(&record) {
            errors.push(json!({"row": row_num, "error": e.to_string()}));
            continue;
        }
        if let Err(e) = reference_data::validate_field_references(&tx, business_id, module, &record) {
            errors.push(json!({"row": row_num, "error": e.to_string()}));
            continue;
        }

        // `inventory_record_id` isn't a template column for `purchasing`
        // (see generate_template) — it's resolved here instead, the
        // same way ModuleView.tsx's PurchaseItemSelector resolves it for
        // a hand-typed record: look up the Inventory item whose `name`
        // matches this row's `item_name`, for THIS business, and use
        // its id. This runs for both new rows and updates to existing
        // ones — an update whose `item_name` was corrected to point at
        // a different existing item gets relinked to match, the same
        // way re-picking a different item in the dropdown would. No
        // match means there's nothing to link to yet, exactly the case
        // the dropdown's own "Create the item in Inventory first" note
        // covers — so the row is rejected with the same guidance,
        // rather than silently creating (or leaving) an orphaned
        // purchase order that receiving.rs can never actually receive
        // against.
        if module.id == "purchasing" {
            let item_name = record.get("item_name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            if item_name.is_empty() {
                errors.push(json!({"row": row_num, "error": "'item_name' is required"}));
                continue;
            }
            match find_inventory_id_by_name(&tx, business_id, &item_name) {
                Ok(Some(inv_id)) => {
                    record.insert("inventory_record_id".to_string(), json!(inv_id));
                }
                Ok(None) => {
                    errors.push(json!({
                        "row": row_num,
                        "error": format!(
                            "no Inventory item named '{item_name}' was found — create the item in Inventory first, then re-import"
                        )
                    }));
                    continue;
                }
                Err(e) => {
                    errors.push(json!({"row": row_num, "error": e.to_string()}));
                    continue;
                }
            }
        }

        // `po_number` has no static default that would make sense (it
        // can't be a fixed value — that's exactly what would make it
        // NOT unique), so `purchasing.json` gives it `default: ""`
        // purely so the fill-in-defaults loop above lets a row with no
        // po_number column pass `module.validate()`'s required-field
        // check — that placeholder is never the real value stored.
        // Two real cases land here:
        //   - The blank "new orders" template (see generate_template)
        //     never had a po_number column at all, so every one of its
        //     rows carries the "" placeholder — replaced here with a
        //     freshly generated real number, same as crud::create's
        //     purchasing block does for a hand-typed record.
        //   - A re-uploaded "Export to Excel" file DOES carry each
        //     row's real po_number (export includes every field) — so
        //     the placeholder check below is false, nothing is
        //     regenerated, and that real value is exactly what the key
        //     match right after this uses to find and correct the
        //     matching existing order (is_update_blocked_field then
        //     protects po_number itself from being changed by that
        //     same re-import, same as `received`).
        // Generated in row order from the single starting sequence
        // fetched once before this loop — see
        // `purchasing_next_po_seq`'s own comment for why that matters
        // for import performance on a large batch of new rows.
        if module.id == "purchasing" {
            let has_real_po_number = record
                .get("po_number")
                .and_then(|v| v.as_str())
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            if !has_real_po_number {
                match purchasing_next_po_seq.as_mut() {
                    Some(seq) => {
                        *seq += 1;
                        record.insert("po_number".to_string(), json!(format!("PO-{seq}")));
                    }
                    // Can't happen — `purchasing_next_po_seq` is always
                    // `Some` inside `if module.id == "purchasing"` — but
                    // a hard error here is still strictly better than
                    // silently skipping po_number generation and letting
                    // a later NOT NULL failure produce a confusing error
                    // pointing nowhere near the real cause.
                    None => {
                        errors.push(json!({"row": row_num, "error": "internal error: no PO sequence available"}));
                        continue;
                    }
                }
            }
        }

        // Same "generate once for a blank new-entries sheet, keep
        // whatever's already there on a genuine re-upload" logic as
        // purchasing's `po_number` just above, for debt_credit's
        // `entry_number`.
        if module.id == "debt_credit" {
            let has_real_entry_number = record
                .get("entry_number")
                .and_then(|v| v.as_str())
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            if !has_real_entry_number {
                match debt_credit_next_entry_seq.as_mut() {
                    Some(seq) => {
                        *seq += 1;
                        record.insert("entry_number".to_string(), json!(format!("DC-{seq}")));
                    }
                    None => {
                        errors.push(json!({"row": row_num, "error": "internal error: no entry sequence available"}));
                        continue;
                    }
                }
            }
        }

        // See the `key_field_is_unique` comment above `import()`'s
        // header check: matching against anything other than a truly
        // unique field isn't a stricter-but-imperfect check, it's
        // actively wrong, so it isn't attempted at all — `existing_id`
        // just stays `None` and this row always creates. This is what
        // makes a Purchasing (or any no-unique-field module) import
        // always append new rows, matching what re-uploading a
        // transaction log should actually do.
        let existing_id = if key_field_is_unique {
            record
                .get(key_field)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .and_then(|kv| find_existing_by_key(&tx, business_id, module, key_field, &kv).ok().flatten())
        } else {
            None
        };

        match existing_id {
            Some(id) => {
                // The blank new-item template no longer has a
                // `quantity` column (see generate_template), so a row
                // on this file that matches an existing item isn't a
                // legitimate stock-take edit — there's no real counted
                // value behind it, just this loop's own required-field
                // default of 0 (see the default-fill loop above). The
                // two screenshots this fix was written against are
                // exactly this: purchase orders hand-created as
                // already `received`, inventory left at the
                // create()-forced 0, and re-running a same-SKU import
                // would otherwise have silently reconciled that 0
                // right back into a record that actually needs a real
                // Receive or a real stock-take count. Reject the row
                // instead of guessing — loud and specific, same
                // standard the blocked-field check just below holds
                // every other protected field to.
                if module.id == "inventory" && !inventory_sheet_has_quantity_column {
                    errors.push(json!({
                        "row": row_num,
                        "error": format!(
                            "an item with this '{key_field}' already exists — this template is for adding new items only and has no quantity column; to correct an existing item's stock count, use \"Export to Excel\", edit the counted quantities, and reimport that file instead"
                        )
                    }));
                    continue;
                }

                // THE BUG THIS FIXES (round 2): every field the module
                // defines (including purchasing's `received` and
                // debt_credit's `settled`/`payment_method`/
                // `source_order_id`) is a column in the downloadable
                // template, so any row that simply carries the same
                // value already stored — the normal case for a
                // re-uploaded spreadsheet that only actually changed
                // one or two other columns — used to hard-fail the
                // whole row the instant crud::update() saw one of
                // those keys present at all, regardless of whether its
                // value had even changed. That made re-importing an
                // existing purchasing or debt_credit spreadsheet
                // effectively impossible.
                //
                // The first fix for that made the mistake of simply
                // never sending these fields as part of the update —
                // unconditionally, without checking whether the
                // incoming value actually differed from what's stored.
                // That reopened the exact hole this module exists to
                // close: a spreadsheet with `settled=true` on a row
                // that already exists would silently have `settled`
                // stripped from the payload and the rest of the row
                // (party_name, direction, amount, ...) applied with a
                // clean `Ok` and no error — indistinguishable, from the
                // caller's side, from the debt actually having been
                // settled through debt_settlement::settle(). Silently
                // dropping a field a person deliberately typed a new
                // value into is not "ignoring an unrelated column
                // that happened to be in the template", it's silently
                // discarding an edit — which is its own kind of
                // surprising, audit-defeating behavior for exactly the
                // fields this list exists to protect.
                //
                // So: compare each blocked field's incoming value
                // against what's actually stored first. Unchanged →
                // drop it from the payload same as before (a genuine
                // re-upload of an untouched column is not an edit at
                // all). Changed → reject the whole row with the same
                // error crud::update() would have given a single-record
                // caller, rather than quietly applying every other
                // field and hiding the one that mattered.
                let table = module.table_name();
                let mut blocked_change: Option<String> = None;
                for k in record.keys() {
                    if !crud::is_update_blocked_field(&module.id, k, true) {
                        continue;
                    }
                    let is_boolean = module.fields.iter().any(|f| &f.name == k && f.field_type == "boolean");
                    let stored = stored_field_value(&tx, &table, business_id, &id, k, is_boolean)
                        .unwrap_or(Value::Null);
                    if record.get(k) != Some(&stored) {
                        blocked_change = Some(k.clone());
                        break;
                    }
                }

                if let Some(field) = blocked_change {
                    errors.push(json!({
                        "row": row_num,
                        "error": format!(
                            "'{field}' cannot be edited directly on '{}' — use the sell, receive, refund, or repack action instead",
                            module.id
                        )
                    }));
                    continue;
                }

                // NEW CONSISTENCY CHECK, added alongside auto-receive
                // (see the `None` branch below): a purchasing row is
                // now received the moment it's first created by this
                // importer, which means by the time anyone re-imports
                // a correction, `quantity`/`unit_cost` have almost
                // always already been consumed into Inventory's stock
                // level and weighted-average cost. Neither of those
                // two fields is in the permanently-blocked list above
                // (they're ordinary editable columns on an UNRECEIVED
                // order, same as before this change), but editing
                // either one AFTER receipt would silently rewrite what
                // the purchase order claims happened without touching
                // the Inventory numbers that were already derived from
                // the old values — exactly the kind of drift this
                // importer exists to prevent elsewhere. Checked the
                // same way the blocked-field comparison just above is:
                // only rejected if the incoming value actually differs
                // from what's stored, so a re-upload that merely
                // carries the same received order's existing figures
                // unchanged (the ordinary case) still goes through.
                if module.id == "purchasing" {
                    let already_received = stored_field_value(&tx, &table, business_id, &id, "received", true)
                        .unwrap_or(Value::Bool(false))
                        == Value::Bool(true);
                    let mut rejected_field: Option<&str> = None;
                    if already_received {
                        for field in ["quantity", "unit_cost"] {
                            let Some(incoming) = record.get(field) else { continue };
                            let stored = stored_field_value(&tx, &table, business_id, &id, field, false).unwrap_or(Value::Null);
                            if incoming != &stored {
                                rejected_field = Some(field);
                                break;
                            }
                        }
                    }
                    if let Some(field) = rejected_field {
                        errors.push(json!({
                            "row": row_num,
                            "error": format!(
                                "'{field}' cannot be changed on a purchase order that's already been received — its stock and cost are already applied to Inventory; adjust Inventory directly (or use Repack) instead"
                            )
                        }));
                        continue;
                    }
                }

                // `inventory`'s `quantity` is deliberately NOT filtered
                // here even when changed: that field's whole reason
                // for being in the template is the sanctioned
                // stock-take reconciliation workflow (see the module
                // doc comment above and crud.rs's
                // is_update_blocked_field), so it's left in the
                // payload and crud::update's own bulk_import=true
                // narrows the block for exactly that field.
                let body_map: serde_json::Map<String, Value> = record
                    .clone()
                    .into_iter()
                    .filter(|(k, _)| !crud::is_update_blocked_field(&module.id, k, true))
                    .collect();
                match crud::update(&tx, business_id, user_id, &module.id, &id, &body_map, true) {
                    Ok(_) => updated += 1,
                    Err(e) => errors.push(json!({"row": row_num, "error": e.to_string()})),
                }
            }
            None => {
                // THE BUG THIS FIXES: this insert path calls
                // insert_validated_record() directly, not
                // crud::create() — so it never got create()'s own
                // "every inventory item starts at zero stock, full
                // stop" rule (see crud.rs) applied to it. A spreadsheet
                // adding a batch of brand-new products could carry
                // whatever quantity was typed into that column straight
                // into stock, silently bypassing the one invariant this
                // whole app is built around: stock only ever enters
                // through Purchasing receiving an order
                // (receiving.rs::receive()). The sanctioned exception —
                // a spreadsheet reconciling counts for items that
                // already EXIST (a real stock take) — is exactly the
                // `Some(id)` branch above, which correctly keeps
                // `quantity` in its update payload; this `None` branch
                // is "create a new item", which is never allowed to
                // seed its own opening count, on this path or any
                // other.
                if module.id == "inventory" {
                    record.insert("quantity".to_string(), json!(0));
                }
                // Same "starts at a forced, correct baseline, no
                // exceptions" rule crud::create() applies to a brand-new
                // debt/credit record — see crud.rs's own comment on why.
                // This insert path skips crud::create() entirely, so
                // without this a spreadsheet adding new debt_credit rows
                // could hand-create one already marked settled (with a
                // payment_method/source_order_id attached), the exact
                // gap crud::create() already closes for the single-record
                // form.
                if module.id == "debt_credit" {
                    record.insert("settled".to_string(), json!(false));
                    record.remove("payment_method");
                    record.remove("source_order_id");
                }
                // Same fix, same reason, as crud.rs::create()'s new
                // `if module_id == "purchasing"` block — this insert
                // path skips create() entirely (it calls
                // insert_validated_record() directly), so without this
                // a spreadsheet adding new purchasing rows could carry
                // `received: true` straight into a brand-new order,
                // same unguarded hole a raw API call had, just reached
                // through Excel instead.
                if module.id == "purchasing" {
                    record.insert("received".to_string(), json!(false));
                }
                // Same "never sell at a loss" rule crud::create()/update()
                // enforce for the single-record form — a new inventory
                // item created via spreadsheet import is just as capable
                // of carrying a bad price as one typed in by hand, and
                // this insert path calls insert_validated_record()
                // directly rather than crud::create(), so it doesn't get
                // that check for free.
                if module.id == "inventory" {
                    let unit_cost = record.get("unit_cost").and_then(|v| v.as_i64()).unwrap_or(0);
                    let unit_price = record.get("unit_price").and_then(|v| v.as_i64()).unwrap_or(0);
                    if unit_price < unit_cost {
                        errors.push(json!({"row": row_num, "error": "selling price cannot be lower than the cost price — this would sell at a loss"}));
                        continue;
                    }
                }
                match crud::insert_validated_record(&tx, business_id, module, &record) {
                    Ok(new_id) => {
                        created += 1;
                        // THE ACTUAL FIX Deric asked for: a purchase
                        // order imported from Excel is, in every real
                        // case this template exists for, stock that
                        // has ALREADY arrived — the whole reason
                        // someone is recording it now is that it's
                        // sitting in front of them. Requiring a
                        // separate manual "Receive" click per row
                        // afterward (150 of them, for a 150-row
                        // import) added a whole second pass over the
                        // same data with zero new information in it.
                        // So: the moment a new purchasing row is
                        // created here, it's received immediately, in
                        // this SAME transaction — `receive_in_tx` is
                        // the exact mechanics `receiving::receive`
                        // itself runs (weighted-average cost, the
                        // Purchasing expense post, the rounding
                        // reconciliation, all of it), just reused
                        // directly against the transaction this import
                        // already has open, rather than duplicating
                        // that logic here or opening a second nested
                        // transaction (which rusqlite can't do against
                        // the same connection anyway). Quantity
                        // received is always the row's own ordered
                        // `quantity` — an Excel import has no separate
                        // "partial delivery" column, so there's no
                        // other number it could mean.
                        //
                        // Deliberately only for freshly-CREATED rows
                        // (this `Ok(new_id)` arm), never for the
                        // `Some(id)` update branch above: re-uploading
                        // a spreadsheet to correct an existing order's
                        // details is not a second delivery of the same
                        // stock, and `received` is already a
                        // permanently blocked field on that path (see
                        // crud.rs) for exactly that reason.
                        if module.id == "purchasing" {
                            let quantity = record.get("quantity").and_then(|v| v.as_i64()).unwrap_or(0);
                            let purchasing_table = module.table_name();
                            match receiving::receive_in_tx(&tx, business_id, &purchasing_table, "module_inventory", &new_id, Some(quantity)) {
                                Ok(summary) => {
                                    let _ = audit::log(&tx, business_id, Some(user_id), "_receiving", "receive", Some(&new_id), Some(&summary));
                                }
                                Err(e) => {
                                    // Should not happen — this row's
                                    // inventory_record_id was already
                                    // resolved successfully above, and
                                    // a row that was just inserted
                                    // can't already be "received". If
                                    // it somehow does, surface it
                                    // plainly against this row rather
                                    // than leaving a purchase order
                                    // silently stuck unreceived, right
                                    // next to rows that succeeded, with
                                    // no indication anything was
                                    // different about it.
                                    errors.push(json!({"row": row_num, "error": format!("created purchase order but could not automatically receive it: {e}")}));
                                }
                            }
                        }
                    }
                    Err(e) => errors.push(json!({"row": row_num, "error": e.to_string()})),
                }
            }
        }
    }

    // Everything above happened inside `tx` — this is the one moment
    // all of it becomes durable at once, same discipline as every
    // other multi-step write in this app.
    tx.commit()?;

    Ok(ImportResult { created, updated, errors })
}

// Looks up an Inventory item's id by its `name`, for this business only,
// case- and whitespace-insensitively — matching how a person reads and
// picks names in the PurchaseItemSelector dropdown, not a byte-exact
// comparison a stray trailing space or capitalization difference would
// defeat. `module_inventory` is used directly rather than going through
// `ModuleDef`/`table_name()` here since this lookup is specific to one
// hardcoded module (inventory) from another module's (purchasing's) own
// import path, not a generic per-module operation.
//
// pub(crate) rather than private: crud::create() also calls this
// directly for a purchasing record's own `item_name` — this used to be
// resolved only on the Excel-import path, so any purchasing record
// created through the ordinary single-record create() (a raw API call,
// or any backend code that doesn't go through ModuleView.tsx's
// PurchaseItemSelector) could end up with no `inventory_record_id` at
// all, silently unlinked from anything receiving.rs::receive() could
// ever update. Same lookup, one implementation, both callers.
pub(crate) fn find_inventory_id_by_name(conn: &Connection, business_id: &str, name: &str) -> Result<Option<String>> {
    if name.is_empty() {
        return Ok(None);
    }
    let id: Option<String> = conn
        .query_row(
            "SELECT id FROM module_inventory
             WHERE business_id = ?1 AND deleted_at IS NULL AND LOWER(TRIM(name)) = LOWER(?2)
             LIMIT 1",
            rusqlite::params![business_id, name],
            |r| r.get(0),
        )
        .ok();
    Ok(id)
}

fn find_existing_by_key(
    conn: &Connection,
    business_id: &str,
    module: &ModuleDef,
    key_field: &str,
    key_value: &str,
) -> Result<Option<String>> {
    let table = module.table_name();
    let sql = format!("SELECT id FROM {table} WHERE business_id = ?1 AND {key_field} = ?2 AND deleted_at IS NULL LIMIT 1");
    let id: Option<String> = conn
        .query_row(&sql, rusqlite::params![business_id, key_value], |r| r.get(0))
        .ok();
    Ok(id)
}

// Reads a single column's currently-stored value for one record, using
// the same boolean-awareness as crud::list's row-to-JSON conversion
// (SQLite has no native boolean type, so a "boolean"-typed field must
// be read back as INTEGER 0/1 and converted to a JSON bool, or a
// stored `false`/0 would never compare equal to the JSON `false` the
// spreadsheet cell parses into above it). Used only to decide whether
// an incoming blocked-field value actually differs from what's on
// file — never to build the update payload itself.
fn stored_field_value(
    conn: &Connection,
    table: &str,
    business_id: &str,
    id: &str,
    field_name: &str,
    is_boolean: bool,
) -> Result<Value> {
    let sql = format!("SELECT {field_name} FROM {table} WHERE id = ?1 AND business_id = ?2 AND deleted_at IS NULL");
    conn.query_row(&sql, rusqlite::params![id, business_id], |row| {
        Ok(match row.get_ref(0)? {
            rusqlite::types::ValueRef::Null => Value::Null,
            rusqlite::types::ValueRef::Integer(n) if is_boolean => json!(n != 0),
            rusqlite::types::ValueRef::Integer(n) => json!(n),
            rusqlite::types::ValueRef::Real(f) => json!(f),
            rusqlite::types::ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
            rusqlite::types::ValueRef::Blob(_) => Value::Null,
        })
    })
    .map_err(Into::into)
}

// pub(crate) rather than private: exercised directly by
// excel_import_tests.rs to read back a generated template's own
// header row (via calamine) without duplicating this same
// Data-to-String conversion in the test itself.
pub(crate) fn cell_to_string(cell: &Data) -> String {
    match cell {
        Data::String(s) => s.clone(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) => f.to_string(),
        Data::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

fn cell_to_json(cell: &Data, field_type: &str, currency: &str) -> Value {
    match (cell, field_type) {
        (Data::Int(i), "integer") => json!(i),
        (Data::Float(f), "integer") => json!(*f as i64),
        // A numeric-looking cell stored as TEXT rather than a native
        // number — genuinely common in real spreadsheets (a column
        // formatted as Text, or data pasted/imported from a CSV,
        // often ends up this way even though a human reading it sees
        // "10", not text). Falling through to the generic string
        // branch below would silently produce the STRING "10" instead
        // of the integer 10, which module.validate() then rejects —
        // exactly the same class of bug the money-field fix above
        // exists to close, just for integer/real instead of money.
        (Data::String(s), "integer") => s.trim().parse::<i64>().map(|i| json!(i)).unwrap_or(Value::Null),
        (Data::Float(f), "real") => json!(f),
        (Data::Int(i), "real") => json!(*i as f64),
        (Data::String(s), "real") => s.trim().parse::<f64>().map(|f| json!(f)).unwrap_or(Value::Null),
        // A human types a decimal amount into the cell (19.99), same
        // as anywhere else money is typed in this app — parsed here
        // through the exact same strict parser (money::parse_money_input)
        // that every other money input in the app goes through, not a
        // separate, looser one just because this path is a spreadsheet.
        // An Excel float and an Excel string both need handling: Excel
        // itself decides which storage type a cell gets based on how
        // it was typed/formatted, and a person filling in a template
        // could produce either. A value that fails to parse (or is
        // simply garbage) becomes Null rather than a guess — the
        // existing per-row module.validate() call right after this
        // returns already rejects a Null "money" field with a clear
        // "expected type money but got Null" error, so this doesn't
        // need its own separate error-reporting path.
        (Data::Float(f), "money") => crate::money::parse_money_input(&f.to_string(), currency)
            .map(|c| json!(c))
            .unwrap_or(Value::Null),
        (Data::String(s), "money") => crate::money::parse_money_input(s.trim(), currency)
            .map(|c| json!(c))
            .unwrap_or(Value::Null),
        (Data::Int(i), "money") => crate::money::parse_money_input(&i.to_string(), currency)
            .map(|c| json!(c))
            .unwrap_or(Value::Null),
        (Data::Bool(b), "boolean") => json!(b),
        (Data::String(s), "boolean") => json!(s.eq_ignore_ascii_case("true") || s == "1"),
        (Data::DateTimeIso(s), "date") => json!(s),
        // The far more common case than the ISO-string variant just
        // above: when a cell is actually formatted as a date in Excel
        // or LibreOffice (the normal, expected way to enter a date —
        // pick it from the date picker, or type it and let the cell's
        // date format apply), calamine reads it back as this
        // ExcelDateTime variant — an internal day-count-since-1900
        // serial number, not a string at all. The code here previously
        // had no case for it whatsoever, so any such cell fell straight
        // through to the catch-all `_ => Value::Null` below — silently,
        // with no error of its own, since the per-row validate() call
        // right after this just reports "expected date, got Null"
        // rather than anything pointing at the real cause. This is
        // "sometimes a date imports as null" instead of always,
        // because which of these two representations you get for a
        // given cell depends on exactly how it was entered/formatted —
        // both are entirely normal, valid ways to put a date in a
        // spreadsheet, and only one of them was ever handled.
        // `as_datetime()` needs calamine's "dates" feature enabled
        // (see Cargo.toml) — it isn't part of the default feature set.
        (Data::DateTime(dt), "date") => match dt.as_datetime() {
            Some(naive) => json!(naive.date().to_string()), // "YYYY-MM-DD", same format every other "date" field in this app uses (see invoice.rs's own date_naive().to_string())
            None => Value::Null,
        },
        // A third way a date can arrive: typed directly as plain text
        // into a cell with no date formatting applied at all (a
        // "General" or "Text"-formatted cell) — calamine has no way to
        // tell that string was meant as a date, so it comes through as
        // an ordinary Data::String, same as any other text cell.
        // Restricted to exactly this app's own "YYYY-MM-DD" shape
        // (the same one every date field elsewhere in the app produces
        // — see the DateTime case just above) rather than accepted
        // as-is: module.validate() only checks that a "date" field IS
        // a string, not that it's actually a valid, correctly-shaped
        // date (see module.rs) — so "31/01/2026", "1/31/26", or "Jan
        // 31 2026" would otherwise sail straight through as a
        // "valid" date and silently corrupt every date-range filter,
        // sort, and comparison elsewhere in the app that assumes
        // dates are lexicographically sortable in this exact format.
        (Data::String(s), "date") => match chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d") {
            Ok(_) => json!(s.trim()),
            Err(_) => Value::Null,
        },
        (Data::String(s), _) => json!(s),
        (Data::Int(i), _) => json!(i.to_string()),
        (Data::Float(f), _) => json!(f.to_string()),
        _ => Value::Null,
    }
}
