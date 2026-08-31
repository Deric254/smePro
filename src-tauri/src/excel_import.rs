//! Excel-based bulk import — download a template matching a module's
//! real fields, fill it in a spreadsheet, upload it back.
//!
//! Deliberately serves two real needs with one mechanism rather than
//! building them as separate features: adding a batch of new products,
//! and doing a stock take (reconciling counted quantities against what
//! the system thinks is on the shelf). Both are "here's a spreadsheet
//! of items with values" — the only real difference is whether each
//! row's key (first field, e.g. SKU) matches something that already
//! exists. If it does, this UPDATES that record instead of creating a
//! duplicate — exactly what a stock take needs (correct the count on
//! the item that's already there), and exactly what re-uploading a
//! template with a typo fixed needs too (correct the row, not create
//! a second copy of it).
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

use crate::{crud, module::ModuleDef, reference_data};
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

    if !headers.iter().any(|h| h == key_field) {
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

        let key_value = record.get(key_field).and_then(|v| v.as_str()).map(|s| s.to_string());
        let existing_id = key_value
            .as_ref()
            .and_then(|kv| find_existing_by_key(&tx, business_id, module, key_field, kv).ok().flatten());

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
                    Ok(_) => created += 1,
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

fn cell_to_string(cell: &Data) -> String {
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
