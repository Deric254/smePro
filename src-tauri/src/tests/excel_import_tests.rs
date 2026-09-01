use super::common::*;
use serde_json::json;

/// Builds a minimal real .xlsx in memory with the given header row and
/// data rows, using the same rust_xlsxwriter crate this app already
/// depends on for exports — this exercises the REAL import() parsing
/// path against real spreadsheet bytes, not a hand-constructed
/// in-memory data structure standing in for one.
fn build_xlsx(headers: &[&str], rows: &[Vec<&str>]) -> Vec<u8> {
    let mut wb = rust_xlsxwriter::Workbook::new();
    let sheet = wb.add_worksheet();
    for (col, h) in headers.iter().enumerate() {
        sheet.write_string(0, col as u16, *h).unwrap();
    }
    for (row_idx, row) in rows.iter().enumerate() {
        for (col, val) in row.iter().enumerate() {
            sheet.write_string((row_idx + 1) as u32, col as u16, *val).unwrap();
        }
    }
    wb.save_to_buffer().unwrap()
}

#[test]
fn test_excel_import_parses_money_field_as_integer_cents() {
    // Proves a real gap is closed: cell_to_json had no "money" case at
    // all before this fix, so every money-field cell fell through to
    // a generic branch that produced a JSON STRING (e.g. "24.50")
    // instead of an integer — which module.validate() then correctly
    // rejected, meaning every row with a money value failed to import.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let module = crate::crud::load_module(&conn, &biz, "inventory").unwrap();

    let xlsx = build_xlsx(
        &["sku", "name", "quantity", "unit_cost", "unit_price"],
        &[vec!["FLOUR-001", "Flour", "10", "12.50", "24.50"]],
    );

    let result = crate::excel_import::import(&mut conn, &biz, &uid, &module, xlsx, "sku").unwrap();
    assert_eq!(result.created, 1);
    assert_eq!(result.errors.len(), 0, "errors: {:?}", result.errors);

    let list = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    let row = list.iter().find(|r| r["sku"] == json!("FLOUR-001")).unwrap();
    assert_eq!(row["unit_cost"].as_i64().unwrap(), 1250);
    assert_eq!(row["unit_price"].as_i64().unwrap(), 2450);
}

#[test]
fn test_excel_import_updates_existing_record_by_key_field() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let module = crate::crud::load_module(&conn, &biz, "inventory").unwrap();

    let mut record = serde_json::Map::new();
    record.insert("sku".into(), json!("RICE-001"));
    record.insert("name".into(), json!("Rice"));
    record.insert("quantity".into(), json!(5));
    record.insert("unit_cost".into(), json!(1000));
    record.insert("unit_price".into(), json!(1500));
    crate::crud::create(&conn, &biz, &uid, "inventory", &record).unwrap();

    // Re-importing the same SKU with a corrected price must UPDATE the
    // existing row, not create a duplicate.
    let xlsx = build_xlsx(
        &["sku", "name", "quantity", "unit_cost", "unit_price"],
        &[vec!["RICE-001", "Rice", "5", "10.00", "18.00"]],
    );
    let result = crate::excel_import::import(&mut conn, &biz, &uid, &module, xlsx, "sku").unwrap();
    assert_eq!(result.created, 0);
    assert_eq!(result.updated, 1);

    let list = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    assert_eq!(list.iter().filter(|r| r["sku"] == json!("RICE-001")).count(), 1, "must not create a duplicate row");
    let row = list.iter().find(|r| r["sku"] == json!("RICE-001")).unwrap();
    assert_eq!(row["unit_price"].as_i64().unwrap(), 1800);
}

#[test]
fn test_excel_import_reports_invalid_money_cell_as_a_clear_row_error() {
    // A garbage value in a money column must surface as a specific,
    // per-row error — not silently become 0, not crash the whole
    // import, and not silently get skipped without any indication.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let module = crate::crud::load_module(&conn, &biz, "inventory").unwrap();

    let xlsx = build_xlsx(
        &["sku", "name", "quantity", "unit_cost", "unit_price"],
        &[vec!["BAD-001", "Bad Row", "1", "not-a-price", "9.99"]],
    );
    let result = crate::excel_import::import(&mut conn, &biz, &uid, &module, xlsx, "sku").unwrap();
    assert_eq!(result.created, 0);
    assert_eq!(result.errors.len(), 1);
    assert_eq!(result.errors[0]["row"], json!(2)); // header is row 1

    let list = crate::crud::list(&conn, &biz, &uid, "inventory", None, 50, 0).unwrap();
    assert!(!list.iter().any(|r| r["sku"] == json!("BAD-001")), "an invalid row must not be partially imported");
}

/// Same helper as build_xlsx above, but for a sheet needing one real
/// Excel DATE cell mixed in among ordinary text cells — build_xlsx
/// itself always calls write_string for every cell, which cannot
/// produce this. Deliberately using rust_xlsxwriter's own
/// ExcelDateTime type here rather than enabling that crate's optional
/// "chrono" feature: ExcelDateTime alone is already enough to write a
/// real date cell, so there's no reason to widen this project's
/// dependency feature set just for a test fixture.
///
/// Deliberately applies a date-formatted `Format` via
/// `write_datetime_with_format` rather than the plain `write_datetime`
/// — rust_xlsxwriter only applies a date NUMBER FORMAT to a cell when
/// one is explicitly given; a bare `write_datetime` with no format
/// stores the exact same underlying serial number but leaves the
/// cell's number format at General (xf_index 0). calamine's own date
/// detection (see calamine::formats::detect_custom_number_format /
/// builtin_format_by_id) keys off THAT number format, not the cell's
/// storage type — so an unformatted date cell round-trips back as an
/// ordinary Data::Float, not Data::DateTime, same as any other number.
/// Without an explicit format here this fixture wasn't actually
/// reproducing "a real Excel date cell" (the ordinary way anyone
/// enters a date always carries an associated date format, whether
/// from the date picker or a pre-formatted column) — it was silently
/// building the untyped-number case instead, which happens to also
/// import as Null today but for a completely different, untested
/// reason than the one this test exists to prove closed.
fn build_xlsx_with_date_cell(
    headers: &[&str],
    text_cells: &[(u32, u16, &str)],
    date_cell: (u32, u16, i32, u8, u8),
) -> Vec<u8> {
    let mut wb = rust_xlsxwriter::Workbook::new();
    let sheet = wb.add_worksheet();
    for (col, h) in headers.iter().enumerate() {
        sheet.write_string(0, col as u16, *h).unwrap();
    }
    for (row, col, val) in text_cells {
        sheet.write_string(*row, *col, *val).unwrap();
    }
    let (row, col, year, month, day) = date_cell;
    let date = rust_xlsxwriter::ExcelDateTime::from_ymd(year as u16, month, day).unwrap();
    let date_format = rust_xlsxwriter::Format::new().set_num_format("yyyy-mm-dd");
    sheet.write_datetime_with_format(row, col, &date, &date_format).unwrap();
    wb.save_to_buffer().unwrap()
}

#[test]
fn test_excel_import_reads_a_real_excel_date_cell() {
    // THE ACTUAL BUG: cell_to_json previously handled ONLY
    // Data::DateTimeIso (a rare ISO-text representation) for "date"
    // fields — a normal, real Excel-formatted date cell (the ordinary
    // way anyone actually enters a date — pick it from the date
    // picker, or type it into a cell already formatted as a date)
    // comes back from calamine as the entirely different
    // Data::DateTime(ExcelDateTime) variant, which had no case at all
    // and fell straight through to `_ => Value::Null`. This is
    // "sometimes a date imports as null" from the user's own report —
    // not always, because which of the two representations you get
    // depends on exactly how the cell was entered, and only one of
    // them was ever handled.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let module = crate::crud::load_module(&conn, &biz, "debt_credit").unwrap();

    let xlsx = build_xlsx_with_date_cell(
        &["party_name", "direction", "amount", "due_date"],
        &[(1, 0, "Acme Ltd"), (1, 1, "owed_to_business"), (1, 2, "500.00")],
        (1, 3, 2026, 3, 15),
    );

    let result = crate::excel_import::import(&mut conn, &biz, &uid, &module, xlsx, "party_name").unwrap();
    assert_eq!(result.errors.len(), 0, "errors: {:?}", result.errors);
    assert_eq!(result.created, 1);

    let list = crate::crud::list(&conn, &biz, &uid, "debt_credit", None, 50, 0).unwrap();
    let row = list.iter().find(|r| r["party_name"] == json!("Acme Ltd")).unwrap();
    assert_eq!(row["due_date"], json!("2026-03-15"), "a real Excel date cell must import as this app's usual YYYY-MM-DD string, not null");
}

#[test]
fn test_excel_import_rejects_a_wrongly_shaped_text_date_instead_of_accepting_it_silently() {
    // The other half of the same fix: a date typed as plain text into
    // a "General"/"Text"-formatted cell (no date formatting applied
    // at all) comes through as an ordinary Data::String, indistin-
    // guishable from any other text cell — calamine has no way to
    // know it was meant as a date. Before this fix, module.validate()
    // only checked that a "date" field IS a string, never that it's
    // actually shaped like one, so "31/01/2026" would have sailed
    // straight through as a "valid" date and silently corrupted every
    // date-range filter/sort elsewhere in the app that assumes this
    // app's own lexicographically-sortable YYYY-MM-DD shape.
    //
    // cell_to_json now rejects the malformed string to Value::Null —
    // and since that Null is inserted into the record as an explicit
    // value (not left absent), module.validate()'s "date" case
    // (v.is_string()) correctly fails it, same as the existing
    // invalid-money-cell test just above rejects a garbage price: the
    // whole row is turned away with a clear, specific error rather
    // than a partial import silently dropping the bad value.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let module = crate::crud::load_module(&conn, &biz, "debt_credit").unwrap();

    let xlsx = build_xlsx(
        &["party_name", "direction", "amount", "due_date"],
        &[vec!["Beta Co", "owed_by_business", "300.00", "31/01/2026"]],
    );

    let result = crate::excel_import::import(&mut conn, &biz, &uid, &module, xlsx, "party_name").unwrap();
    assert_eq!(result.created, 0);
    assert_eq!(result.errors.len(), 1);
    assert_eq!(result.errors[0]["row"], json!(2)); // header is row 1

    let list = crate::crud::list(&conn, &biz, &uid, "debt_credit", None, 50, 0).unwrap();
    assert!(!list.iter().any(|r| r["party_name"] == json!("Beta Co")), "a row with an invalid date must not be partially imported");
}

#[test]
fn test_excel_import_cannot_settle_a_debt_by_reimporting_a_settled_column() {
    // THE ACTUAL BUG THIS PROVES CLOSED: crud::update()'s `bulk_import`
    // flag (true for every excel_import.rs call, including this
    // "update an existing row by key" path) used to bypass its ENTIRE
    // blocked-fields list as one blanket flag — not just
    // inventory.quantity, the one field it was actually designed for.
    // That meant re-uploading a spreadsheet with a "settled" column
    // could mark an existing debt settled directly, completely
    // bypassing debt_settlement::settle()'s RBAC "settle" check, its
    // Bookkeeping post, and its sales-record payment_method backfill —
    // reachable by anyone holding plain "update" on debt_credit, which
    // both Manager and Staff have by default. This must still fail,
    // as a normal single-record edit already correctly does.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let module = crate::crud::load_module(&conn, &biz, "debt_credit").unwrap();

    let mut record = serde_json::Map::new();
    record.insert("party_name".into(), json!("Gamma Traders"));
    record.insert("direction".into(), json!("owed_to_business"));
    record.insert("amount".into(), json!(50000));
    let debt_id = crate::crud::create(&conn, &biz, &uid, "debt_credit", &record).unwrap();

    // `entry_number` — not `party_name` — is debt_credit's real,
    // system-generated identity (see
    // debt_settlement::generate_entry_number's own doc comment for why
    // `party_name` itself can never safely be an import-matching key:
    // one party can legitimately have many separate debt/credit
    // entries, e.g. repeat credit-sale customers in pos.rs). A genuine
    // "Export to Excel, fix a cell, reimport" of this record carries
    // its real entry_number, which is exactly what lets this re-import
    // match it at all.
    let before = crate::crud::list(&conn, &biz, &uid, "debt_credit", None, 50, 0).unwrap();
    let entry_number = before.iter().find(|r| r["id"] == json!(debt_id)).unwrap()["entry_number"]
        .as_str()
        .unwrap()
        .to_string();

    let xlsx = build_xlsx(
        &["entry_number", "party_name", "direction", "amount", "settled"],
        &[vec![&entry_number, "Gamma Traders", "owed_to_business", "500.00", "true"]],
    );
    let result = crate::excel_import::import(&mut conn, &biz, &uid, &module, xlsx, "entry_number").unwrap();
    assert_eq!(result.updated, 0);
    assert_eq!(result.errors.len(), 1, "an update touching a blocked field must be rejected, not silently applied");
    assert!(
        result.errors[0]["error"].as_str().unwrap().contains("cannot be edited directly"),
        "got: {:?}", result.errors[0]
    );

    let list = crate::crud::list(&conn, &biz, &uid, "debt_credit", None, 50, 0).unwrap();
    let row = list.iter().find(|r| r["party_name"] == json!("Gamma Traders")).unwrap();
    assert_eq!(row["settled"], json!(false), "settled must remain unchanged — no side channel around debt_settlement::settle()");
}

#[test]
fn test_purchasing_import_template_excludes_system_managed_columns() {
    // THE ACTUAL BUG: `received` and `inventory_record_id` used to be
    // ordinary template columns even though neither is something a
    // business owner filling in a spreadsheet can meaningfully supply —
    // `received` is set only by receiving.rs::receive(), and
    // `inventory_record_id` is an internal id the manual form never
    // asks for either (it resolves it from a name picked out of a
    // dropdown). Printing them invited exactly the "typed a real value,
    // reasonably expecting it to land" confusion this test guards
    // against. `po_number` joined this list later for the same reason —
    // see generate_template's own comment on it — so it belongs in the
    // same test rather than a separate one that could drift out of sync
    // with this list as it grows.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let module = crate::crud::load_module(&conn, &biz, "purchasing").unwrap();

    use calamine::Reader;
    let bytes = crate::excel_import::generate_template(&module).unwrap();
    let cursor = std::io::Cursor::new(bytes);
    let mut wb: calamine::Xlsx<_> = calamine::open_workbook_from_rs(cursor).unwrap();
    let range = wb.worksheet_range_at(0).unwrap().unwrap();
    let header_row = range.rows().next().unwrap();
    let headers: Vec<String> = header_row.iter().map(crate::excel_import::cell_to_string).collect();

    assert!(headers.iter().any(|h| h.starts_with("item_name")), "item_name must still be on the template: {headers:?}");
    assert!(!headers.iter().any(|h| h.starts_with("received")), "received must not be a template column: {headers:?}");
    assert!(!headers.iter().any(|h| h.starts_with("inventory_record_id")), "inventory_record_id must not be a template column: {headers:?}");
    assert!(!headers.iter().any(|h| h.starts_with("po_number")), "po_number must not be a template column: {headers:?}");
}

#[test]
fn test_purchasing_import_resolves_inventory_record_id_from_item_name() {
    // A person filling in the template only ever types the item's NAME
    // (the one column the template actually has) — this proves import
    // resolves that to the real Inventory id itself, the same lookup
    // the manual form's PurchaseItemSelector performs, rather than
    // requiring the id as a column at all.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let mut inv_record = serde_json::Map::new();
    inv_record.insert("sku".into(), json!("RICE-001"));
    inv_record.insert("name".into(), json!("Rice"));
    inv_record.insert("quantity".into(), json!(0));
    inv_record.insert("unit_cost".into(), json!(1000));
    inv_record.insert("unit_price".into(), json!(1500));
    let inv_id = crate::crud::create(&conn, &biz, &uid, "inventory", &inv_record).unwrap();

    let module = crate::crud::load_module(&conn, &biz, "purchasing").unwrap();
    let xlsx = build_xlsx(
        &["supplier", "item_name", "quantity", "unit_cost"],
        &[vec!["Acme Distributors", "Rice", "50", "9.50"]],
    );
    let result = crate::excel_import::import(&mut conn, &biz, &uid, &module, xlsx, "supplier").unwrap();
    assert_eq!(result.errors.len(), 0, "errors: {:?}", result.errors);
    assert_eq!(result.created, 1);

    let list = crate::crud::list(&conn, &biz, &uid, "purchasing", None, 50, 0).unwrap();
    let row = list.iter().find(|r| r["supplier"] == json!("Acme Distributors")).unwrap();
    assert_eq!(row["inventory_record_id"], json!(inv_id), "must resolve to the real Inventory item's id, not require it as a column");
    assert_eq!(row["received"], json!(false), "received must default false — never settable via import");
}

#[test]
fn test_purchasing_import_rejects_item_name_with_no_matching_inventory_item() {
    // The manual form's dropdown can only ever pick an item that
    // already exists in Inventory ("Create the item in Inventory
    // first" is its own guidance for the empty-list case) — import
    // must hold a spreadsheet row to that same standard rather than
    // creating (or silently leaving unlinked) a purchase order nothing
    // in Inventory backs, which receiving.rs could never actually
    // receive against.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let module = crate::crud::load_module(&conn, &biz, "purchasing").unwrap();

    let xlsx = build_xlsx(
        &["supplier", "item_name", "quantity", "unit_cost"],
        &[vec!["Acme Distributors", "Nonexistent Widget", "10", "5.00"]],
    );
    let result = crate::excel_import::import(&mut conn, &biz, &uid, &module, xlsx, "supplier").unwrap();
    assert_eq!(result.created, 0);
    assert_eq!(result.errors.len(), 1);
    assert!(
        result.errors[0]["error"].as_str().unwrap().contains("no Inventory item named"),
        "got: {:?}", result.errors[0]
    );

    let list = crate::crud::list(&conn, &biz, &uid, "purchasing", None, 50, 0).unwrap();
    assert!(!list.iter().any(|r| r["supplier"] == json!("Acme Distributors")), "an unlinkable row must not be partially imported");
}

#[test]
fn test_purchasing_import_cannot_mark_a_new_order_received_via_a_hand_added_column() {
    // Defense in depth: even if someone hand-adds a "received" column
    // to a re-uploaded spreadsheet (the template itself no longer has
    // one, but nothing stops a determined edit), a brand-new purchasing
    // row must still never be created already `received: true` —
    // that's the exact hole this app is built around closing (see
    // crud.rs::create's own matching purchasing block).
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let mut inv_record = serde_json::Map::new();
    inv_record.insert("sku".into(), json!("OIL-001"));
    inv_record.insert("name".into(), json!("Cooking Oil"));
    inv_record.insert("quantity".into(), json!(0));
    inv_record.insert("unit_cost".into(), json!(2000));
    inv_record.insert("unit_price".into(), json!(3000));
    crate::crud::create(&conn, &biz, &uid, "inventory", &inv_record).unwrap();

    let module = crate::crud::load_module(&conn, &biz, "purchasing").unwrap();
    let xlsx = build_xlsx(
        &["supplier", "item_name", "quantity", "unit_cost", "received"],
        &[vec!["Acme Distributors", "Cooking Oil", "20", "18.00", "true"]],
    );
    let result = crate::excel_import::import(&mut conn, &biz, &uid, &module, xlsx, "supplier").unwrap();
    assert_eq!(result.errors.len(), 0, "errors: {:?}", result.errors);
    assert_eq!(result.created, 1);

    let list = crate::crud::list(&conn, &biz, &uid, "purchasing", None, 50, 0).unwrap();
    let row = list.iter().find(|r| r["supplier"] == json!("Acme Distributors")).unwrap();
    assert_eq!(row["received"], json!(false), "a new order must never be created already received, even via a hand-added column");
}

#[test]
fn test_purchasing_import_creates_every_row_even_when_supplier_repeats() {
    // THE EXACT BUG REPORTED IN PRODUCTION: `supplier` is purchasing's
    // first field but is NOT declared `unique` (many separate orders
    // legitimately share one supplier) — yet both the frontend's
    // default and http_api.rs's own fallback used to hand it to
    // import() as the match key anyway. The result: a 5-row sheet all
    // from the same supplier matched every row against whichever ONE
    // purchasing record already had that supplier, tried to "update"
    // it five times, and — since that existing row's real `received`
    // value almost never equals the sheet's default-filled
    // `received: false` — every single row got rejected with a
    // "'received' cannot be edited directly" error, on a template that
    // doesn't even have a `received` column. 0 created, 0 updated,
    // every row an error. This test seeds exactly that pre-existing
    // same-supplier record, then imports a multi-row sheet that also
    // shares its supplier, and asserts every row still creates its own
    // new purchase order.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    for (sku, name) in [("ITEM-1", "item1"), ("ITEM-2", "item2"), ("ITEM-3", "item3")] {
        let mut inv_record = serde_json::Map::new();
        inv_record.insert("sku".into(), json!(sku));
        inv_record.insert("name".into(), json!(name));
        inv_record.insert("quantity".into(), json!(0));
        inv_record.insert("unit_cost".into(), json!(500));
        inv_record.insert("unit_price".into(), json!(800));
        crate::crud::create(&conn, &biz, &uid, "inventory", &inv_record).unwrap();
    }

    // A prior purchase order from this exact supplier already exists
    // and has already been received — this is the record the bug used
    // to silently (and repeatedly) collide every new row against.
    let mut prior_po = serde_json::Map::new();
    prior_po.insert("supplier".into(), json!("sup1"));
    prior_po.insert("item_name".into(), json!("item1"));
    prior_po.insert("quantity".into(), json!(20));
    prior_po.insert("unit_cost".into(), json!(1000));
    let prior_id = crate::crud::create(&conn, &biz, &uid, "purchasing", &prior_po).unwrap();
    let module = crate::crud::load_module(&conn, &biz, "purchasing").unwrap();
    crate::receiving::receive(
        &mut conn,
        &biz,
        &uid,
        crate::receiving::ReceiveRequest { purchase_record_id: prior_id, quantity_received: None },
    )
    .unwrap();

    // Same shape as the reported spreadsheet: three more rows, same
    // supplier, no `received` column at all (matching the real
    // download template — see generate_template).
    let xlsx = build_xlsx(
        &["supplier", "item_name", "quantity", "unit_cost", "order_date"],
        &[
            vec!["sup1", "item1", "20", "10", "2026-08-31"],
            vec!["sup1", "item2", "20", "10", "2026-08-31"],
            vec!["sup1", "item3", "20", "10", "2026-08-31"],
        ],
    );
    // Passing "supplier" explicitly, matching what the buggy frontend
    // default used to send — proving the fix holds even when a caller
    // still asks for a non-unique key, not just when the default is
    // fixed on the caller's side too.
    let result = crate::excel_import::import(&mut conn, &biz, &uid, &module, xlsx, "supplier").unwrap();
    assert_eq!(result.errors.len(), 0, "errors: {:?}", result.errors);
    assert_eq!(result.created, 3, "every row must create its own new order, not collide on shared supplier");
    assert_eq!(result.updated, 0);

    let list = crate::crud::list(&conn, &biz, &uid, "purchasing", None, 50, 0).unwrap();
    assert_eq!(list.iter().filter(|r| r["supplier"] == json!("sup1")).count(), 4, "the 1 pre-existing + 3 new rows, none merged together");
}

#[test]
fn test_purchasing_po_number_generated_sequentially_and_usable_for_correction() {
    // Covers the feature this bug report led to, end to end: brand-new
    // orders get real, sequential PO numbers with no DB round trip per
    // row (see `purchasing_next_po_seq` in excel_import.rs), and that
    // number is exactly what lets a genuine correction — re-importing
    // an exported file with one value fixed — land on the right order
    // instead of accidentally creating a duplicate or silently editing
    // the wrong one.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let module = crate::crud::load_module(&conn, &biz, "purchasing").unwrap();

    // Every row's `item_name` must already exist in Inventory (see
    // excel_import.rs's own `find_inventory_id_by_name` requirement,
    // exercised directly by
    // test_purchasing_import_rejects_item_name_with_no_matching_inventory_item)
    // — this test is about the po_number sequencing/correction
    // workflow, not that check, so it seeds the two items the sheet
    // below references first, same as the other purchasing import
    // tests do for the items they use.
    for (sku, name) in [("WIDGET-001", "widget"), ("GADGET-001", "gadget")] {
        let mut inv_record = serde_json::Map::new();
        inv_record.insert("sku".into(), json!(sku));
        inv_record.insert("name".into(), json!(name));
        inv_record.insert("quantity".into(), json!(0));
        inv_record.insert("unit_cost".into(), json!(400));
        inv_record.insert("unit_price".into(), json!(600));
        crate::crud::create(&conn, &biz, &uid, "inventory", &inv_record).unwrap();
    }

    // Blank "new orders" template shape — no po_number column, exactly
    // what generate_template() actually produces.
    let new_orders = build_xlsx(
        &["supplier", "item_name", "quantity", "unit_cost", "order_date"],
        &[
            vec!["Acme", "widget", "10", "500", "2026-08-01"],
            vec!["Acme", "gadget", "5", "1200", "2026-08-01"],
            vec!["Beta Co", "widget", "20", "480", "2026-08-02"],
        ],
    );
    let result = crate::excel_import::import(&mut conn, &biz, &uid, &module, new_orders, "po_number").unwrap();
    assert_eq!(result.errors.len(), 0, "errors: {:?}", result.errors);
    assert_eq!(result.created, 3);

    let list = crate::crud::list(&conn, &biz, &uid, "purchasing", None, 50, 0).unwrap();
    let mut po_numbers: Vec<String> = list.iter()
        .map(|r| r["po_number"].as_str().unwrap().to_string())
        .collect();
    po_numbers.sort();
    assert_eq!(po_numbers, vec!["PO-1", "PO-2", "PO-3"], "sequential, distinct — never repeated or skipped");

    // Now correct one order before it's received — exactly the "Export
    // to Excel, fix a cell, reimport" workflow the import dialog
    // describes for Purchasing. The exported file carries the real
    // po_number for every row (unlike the blank template), which is
    // what makes this an update, not a fourth new order.
    let gadget_po = list.iter().find(|r| r["item_name"] == json!("gadget")).unwrap();
    let gadget_po_number = gadget_po["po_number"].as_str().unwrap().to_string();
    let gadget_id = gadget_po["id"].as_str().unwrap().to_string();

    let correction = build_xlsx(
        &["po_number", "supplier", "item_name", "quantity", "unit_cost", "order_date"],
        &[vec![&gadget_po_number, "Acme", "gadget", "5", "1100", "2026-08-01"]], // unit_cost corrected 1200 -> 1100
    );
    let result2 = crate::excel_import::import(&mut conn, &biz, &uid, &module, correction, "po_number").unwrap();
    assert_eq!(result2.errors.len(), 0, "errors: {:?}", result2.errors);
    assert_eq!(result2.created, 0, "a real po_number must match, not create a duplicate order");
    assert_eq!(result2.updated, 1);

    let list2 = crate::crud::list(&conn, &biz, &uid, "purchasing", None, 50, 0).unwrap();
    let corrected = list2.iter().find(|r| r["id"] == json!(gadget_id)).unwrap();
    // unit_cost is a "money" field — stored as integer cents, so
    // "1100" (typed as $1100.00) round-trips to 110000, not 1100.
    assert_eq!(corrected["unit_cost"], json!(110000), "the correction applied");
    assert_eq!(corrected["po_number"], json!(gadget_po_number), "the identity used to find it didn't itself change");
    assert_eq!(list2.len(), 3, "still exactly 3 orders total, not 4");
}

#[test]
fn test_purchasing_po_number_cannot_be_hand_edited() {
    // po_number is this module's one real identity — see
    // crud::is_update_blocked_field's own comment for why letting it
    // be hand-edited would undermine the exact Excel-matching workflow
    // it exists to support.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    // `crud::create()` now resolves `item_name` against Inventory and
    // rejects the row if no match exists (see crud.rs's "THE BUG THIS
    // FIXES" comment on the purchasing block) — so the item has to be
    // seeded first, same as every other purchasing-create test does.
    let mut inv = serde_json::Map::new();
    inv.insert("sku".into(), json!("WIDGET-001"));
    inv.insert("name".into(), json!("widget"));
    inv.insert("quantity".into(), json!(0));
    inv.insert("unit_cost".into(), json!(400));
    inv.insert("unit_price".into(), json!(600));
    crate::crud::create(&conn, &biz, &uid, "inventory", &inv).unwrap();

    let mut po = serde_json::Map::new();
    po.insert("supplier".into(), json!("Acme"));
    po.insert("item_name".into(), json!("widget"));
    po.insert("quantity".into(), json!(10));
    po.insert("unit_cost".into(), json!(500));
    let id = crate::crud::create(&conn, &biz, &uid, "purchasing", &po).unwrap();

    let mut edit = serde_json::Map::new();
    edit.insert("po_number".into(), json!("PO-9999"));
    let err = crate::crud::update(&conn, &biz, &uid, "purchasing", &id, &edit, false).unwrap_err();
    assert!(err.to_string().contains("po_number"), "unexpected error: {err}");
}
