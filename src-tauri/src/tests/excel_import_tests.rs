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
