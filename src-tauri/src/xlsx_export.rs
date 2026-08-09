use anyhow::Result;
use rust_xlsxwriter::{Format, Workbook};
use serde_json::Value;

use crate::module::ModuleDef;
use crate::report::ReportPoint;

/// Writes a list of module records (as returned by crud::list) into a
/// real, formatted .xlsx file and returns the raw bytes — ready to be
/// sent as an HTTP response body or written straight to disk. This is
/// the "export in Excel" promise: an actual spreadsheet, not a CSV
/// wearing an xlsx extension.
///
/// `module_def` is what makes this currency-correct: money fields are
/// stored as integer minor units (cents — see money.rs), and without
/// knowing which columns those are, this function would otherwise
/// write the raw cents integer straight into the spreadsheet — a
/// business owner opening the export would see "1250" where they
/// should see "12.50". The module's own field list is the single
/// source of truth for which columns need that conversion, same as
/// it is for validation and SQL column types.
pub fn records_to_xlsx(records: &[Value], sheet_name: &str, module_def: &ModuleDef) -> Result<Vec<u8>> {
    let mut workbook = Workbook::new();
    let sheet = workbook.add_worksheet().set_name(sheet_name)?;

    let header_format = Format::new().set_bold().set_background_color("#D9E1F2");
    let money_format = Format::new().set_num_format("0.00");

    let money_fields: std::collections::HashSet<&str> = module_def
        .fields
        .iter()
        .filter(|f| f.field_type == "money")
        .map(|f| f.name.as_str())
        .collect();

    // Collect a stable column order from the first record's keys.
    let mut columns: Vec<String> = Vec::new();
    if let Some(Value::Object(first)) = records.first() {
        columns = first.keys().cloned().collect();
        columns.sort(); // deterministic order regardless of HashMap iteration
    }

    for (col_idx, col_name) in columns.iter().enumerate() {
        sheet.write_string_with_format(0, col_idx as u16, col_name, &header_format)?;
    }

    for (row_idx, record) in records.iter().enumerate() {
        let row = (row_idx + 1) as u32;
        if let Value::Object(obj) = record {
            for (col_idx, col_name) in columns.iter().enumerate() {
                let col = col_idx as u16;
                let is_money = money_fields.contains(col_name.as_str());
                match obj.get(col_name) {
                    Some(Value::String(s)) => { sheet.write_string(row, col, s)?; }
                    Some(Value::Number(n)) if is_money => {
                        // Integer cents -> decimal currency value, the
                        // ONLY place this division happens: purely for
                        // display in the exported file, never fed back
                        // into any calculation.
                        let cents = n.as_i64().unwrap_or(0);
                        sheet.write_number_with_format(row, col, cents as f64 / 100.0, &money_format)?;
                    }
                    Some(Value::Number(n)) => { sheet.write_number(row, col, n.as_f64().unwrap_or(0.0))?; }
                    Some(Value::Bool(b)) => { sheet.write_boolean(row, col, *b)?; }
                    _ => { sheet.write_blank(row, col, &Format::new())?; }
                }
            }
        }
    }

    for (col_idx, col_name) in columns.iter().enumerate() {
        sheet.set_column_width(col_idx as u16, (col_name.len() as f64 + 4.0).max(12.0))?;
    }

    Ok(workbook.save_to_buffer()?)
}

/// Writes report/slicer output (label, value pairs) into an .xlsx file.
pub fn report_to_xlsx(points: &[ReportPoint], measure_label: &str) -> Result<Vec<u8>> {
    let mut workbook = Workbook::new();
    let sheet = workbook.add_worksheet().set_name("Report")?;
    let header_format = Format::new().set_bold().set_background_color("#D9E1F2");

    sheet.write_string_with_format(0, 0, "Label", &header_format)?;
    sheet.write_string_with_format(0, 1, measure_label, &header_format)?;

    for (i, p) in points.iter().enumerate() {
        let row = (i + 1) as u32;
        sheet.write_string(row, 0, &p.label)?;
        sheet.write_number(row, 1, p.value)?;
    }

    sheet.set_column_width(0, 20.0)?;
    sheet.set_column_width(1, 16.0)?;

    Ok(workbook.save_to_buffer()?)
}
