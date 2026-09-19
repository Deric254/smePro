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
/// `module_def` is what makes this currency-correct as to WHICH
/// columns are money (integer minor units — see money.rs): without
/// knowing that, this function would otherwise write the raw cents
/// integer straight into the spreadsheet — a business owner opening
/// the export would see "1250" where they should see "12.50". The
/// module's own field list is the single source of truth for that,
/// same as it is for validation and SQL column types.
///
/// `currency` is what makes it correct as to HOW MANY decimal places
/// and what scale — THE BUG THIS FIXES: this used to hardcode
/// `cents / 100.0` with a fixed "0.00" format regardless of currency,
/// which is only right for 2-decimal currencies. For a 0-decimal
/// currency (JPY, UGX, RWF...) every exported value came out 100x too
/// small; for a 3-decimal currency (BHD, KWD, OMR, JOD...) 10x too
/// large — and since excel_import.rs's own cell_to_json already
/// parses money correctly per-currency on the way back in, the
/// classic "export, tweak a row, re-import" workflow silently
/// corrupted every money field by that same factor on a round trip.
/// Mirrors money::decimal_places_for exactly (same table, same
/// source of truth as the Rust and TS money-parsing code both use).
pub fn records_to_xlsx(records: &[Value], sheet_name: &str, module_def: &ModuleDef, currency: &str) -> Result<Vec<u8>> {
    let mut workbook = Workbook::new();
    let sheet = workbook.add_worksheet().set_name(sheet_name)?;

    let header_format = Format::new().set_bold().set_background_color("#D9E1F2");
    let places = crate::money::decimal_places_for(currency);
    let scale = 10f64.powi(places as i32);
    let num_format_str = if places == 0 { "0".to_string() } else { format!("0.{}", "0".repeat(places as usize)) };
    let money_format = Format::new().set_num_format(num_format_str.as_str());

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

    // Inventory's unit_cost/unit_price are frozen legacy history once an
    // item has a real purchase batch, and even on batch-less items
    // they're no longer meant to be edited through this file — the
    // export/re-import round trip exists for the sanctioned stock-take
    // workflow (reconciling `quantity`), not for touching price (see
    // excel_import.rs). Dropping both columns here removes the only
    // place someone could type a new value in and have it silently
    // stripped or rejected on re-import. Per Deric: these two fields
    // belong to Purchasing, not this file.
    if module_def.id == "inventory" {
        columns.retain(|c| c != "unit_cost" && c != "unit_price");
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
                        // Integer minor units -> decimal currency value,
                        // scaled per this business's currency (see
                        // this function's own doc comment) — the ONLY
                        // place this division happens: purely for
                        // display in the exported file, never fed back
                        // into any calculation.
                        let cents = n.as_i64().unwrap_or(0);
                        sheet.write_number_with_format(row, col, cents as f64 / scale, &money_format)?;
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

/// Writes an explicit, caller-supplied table (fixed headers + rows) into
/// an .xlsx file. Used where the columns are a deliberate, human-facing
/// layout rather than whatever keys a record happens to carry — the
/// stock movement trace and the audit log, whose useful columns are a
/// curated subset presented in a specific order, not a raw row dump.
///
/// `money_columns` holds the indices of columns whose values are integer
/// minor units and must be written as a real decimal number with a
/// currency number-format — the exact same cents-to-decimal concern
/// records_to_xlsx documents above, expressed by position because these
/// tables have no ModuleDef to look field types up in. `currency` fixes
/// the same latent bug records_to_xlsx had: a hardcoded /100.0 and
/// "0.00" format is only correct for 2-decimal currencies. This
/// function's only current caller (audit-log export) passes no money
/// columns, so the bug was never actually live here — but a future
/// caller with money columns would have hit it immediately, so it's
/// fixed now rather than left as a trap. Numbers stay real numbers
/// (never pre-formatted strings) so the person who opens the file can
/// sum, sort, filter and chart them.
pub fn rows_to_xlsx(
    sheet_name: &str,
    headers: &[&str],
    rows: &[Vec<Value>],
    money_columns: &[usize],
    currency: &str,
) -> Result<Vec<u8>> {
    let mut workbook = Workbook::new();
    let sheet = workbook.add_worksheet().set_name(sheet_name)?;

    let header_format = Format::new().set_bold().set_background_color("#D9E1F2");
    let places = crate::money::decimal_places_for(currency);
    let scale = 10f64.powi(places as i32);
    let num_format_str = if places == 0 { "0".to_string() } else { format!("0.{}", "0".repeat(places as usize)) };
    let money_format = Format::new().set_num_format(num_format_str.as_str());
    let money: std::collections::HashSet<usize> = money_columns.iter().copied().collect();

    for (col_idx, header) in headers.iter().enumerate() {
        sheet.write_string_with_format(0, col_idx as u16, *header, &header_format)?;
    }

    for (row_idx, row_values) in rows.iter().enumerate() {
        let row = (row_idx + 1) as u32;
        for (col_idx, value) in row_values.iter().enumerate() {
            let col = col_idx as u16;
            match value {
                Value::String(s) => { sheet.write_string(row, col, s)?; }
                Value::Number(n) if money.contains(&col_idx) => {
                    let cents = n.as_i64().unwrap_or(0);
                    sheet.write_number_with_format(row, col, cents as f64 / scale, &money_format)?;
                }
                Value::Number(n) => { sheet.write_number(row, col, n.as_f64().unwrap_or(0.0))?; }
                Value::Bool(b) => { sheet.write_boolean(row, col, *b)?; }
                _ => { sheet.write_blank(row, col, &Format::new())?; }
            }
        }
    }

    // Widen to the header text, then to the widest cell beneath it, so a
    // long item name or timestamp isn't delivered as "#####".
    for (col_idx, header) in headers.iter().enumerate() {
        let widest_cell = rows
            .iter()
            .filter_map(|r| r.get(col_idx))
            .map(|v| match v {
                Value::String(s) => s.chars().count(),
                other => other.to_string().chars().count(),
            })
            .max()
            .unwrap_or(0);
        let width = (header.chars().count().max(widest_cell) as f64 + 4.0).clamp(12.0, 60.0);
        sheet.set_column_width(col_idx as u16, width)?;
    }

    Ok(workbook.save_to_buffer()?)
}
