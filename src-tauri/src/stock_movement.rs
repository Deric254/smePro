//! The stock movement trace: an append-only, signed ledger of every
//! quantity change to every inventory item.
//!
//! Written by the five modules that are allowed to move stock (pos,
//! receiving, refund, repack, stock_take) via `record_in_tx`, ALWAYS
//! inside the caller's own existing transaction — never on a bare
//! connection. That is the property that makes this trustworthy: a
//! movement row and the `inventory.quantity` change it describes commit
//! together or roll back together, so the ledger can never drift from
//! the stock it claims to explain.
//!
//! See db_migrations.rs's v35 for why this is a real table rather than a
//! view derived from audit_log.

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, Transaction};
use serde_json::{json, Value};

/// Every way stock is allowed to move. Kept as explicit constants rather
/// than free-form strings at the call sites so a typo becomes a compile
/// error instead of a movement type that silently never matches a filter.
pub const SALE: &str = "sale";
pub const REFUND_RESTOCK: &str = "refund_restock";
pub const RECEIVING: &str = "receiving";
pub const REPACK_CONSUMED: &str = "repack_consumed";
pub const REPACK_PRODUCED: &str = "repack_produced";
pub const STOCK_TAKE_SHRINKAGE: &str = "stock_take_shrinkage";
pub const STOCK_TAKE_SURPLUS: &str = "stock_take_surplus";

/// Human-facing label for one movement type. Centralised here so the
/// list endpoint, the Excel export and any future surface all name a
/// movement the same way rather than each inventing its own wording.
pub fn label_for(movement_type: &str) -> &'static str {
    match movement_type {
        SALE => "Sale",
        REFUND_RESTOCK => "Refund restock",
        RECEIVING => "Stock received",
        REPACK_CONSUMED => "Repack (consumed)",
        REPACK_PRODUCED => "Repack (produced)",
        STOCK_TAKE_SHRINKAGE => "Stock take shrinkage",
        STOCK_TAKE_SURPLUS => "Stock take surplus",
        _ => "Other",
    }
}

/// Appends one movement. `quantity_delta` is signed and must be non-zero:
/// negative leaves the business, positive enters it. A zero delta is
/// rejected rather than stored, because a movement that moved nothing is
/// noise in a ledger whose rows are meant to sum to a real quantity.
///
/// Takes a `&Transaction`, not a `&Connection`, specifically so this
/// cannot be called outside the caller's transaction by accident.
pub fn record_in_tx(
    tx: &Transaction,
    business_id: &str,
    user_id: Option<&str>,
    inventory_record_id: &str,
    item_name: &str,
    movement_type: &str,
    quantity_delta: i64,
    unit_cost_cents: i64,
    reference_id: Option<&str>,
) -> Result<()> {
    if quantity_delta == 0 {
        return Err(anyhow!("a stock movement must move a non-zero quantity"));
    }
    tx.execute(
        "INSERT INTO stock_movements
           (id, business_id, inventory_record_id, item_name, movement_type,
            quantity_delta, unit_cost_cents, reference_id, user_id, created_at)
         VALUES (lower(hex(randomblob(16))), ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, datetime('now'))",
        params![
            business_id,
            inventory_record_id,
            item_name,
            movement_type,
            quantity_delta,
            unit_cost_cents,
            reference_id,
            user_id,
        ],
    )?;
    Ok(())
}

/// Filters for the trace. Every field is optional; supplying none returns
/// the most recent movements business-wide.
///
/// `from`/`to` are inclusive date-or-datetime strings in the same format
/// the rest of this codebase stores timestamps in (`YYYY-MM-DD` or
/// `YYYY-MM-DD HH:MM:SS`). A bare `to` date is widened to the end of that
/// day before comparing, so "to 2026-03-01" means all of March 1st rather
/// than only its first instant — the behaviour someone picking a date in
/// a slicer actually expects.
#[derive(Debug, Default, Clone)]
pub struct MovementFilters {
    pub inventory_record_id: Option<String>,
    pub movement_type: Option<String>,
    pub user_id: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub limit: i64,
}

fn widen_to_end_of_day(to: &str) -> String {
    if to.len() == 10 {
        format!("{to} 23:59:59")
    } else {
        to.to_string()
    }
}

/// Builds the shared WHERE clause + bound parameters for both `list` and
/// `export_xlsx`, so the spreadsheet can never contain a different set of
/// rows than the screen it was exported from.
fn build_query(
    business_id: &str,
    f: &MovementFilters,
) -> (String, Vec<Box<dyn rusqlite::ToSql>>) {
    let mut sql = String::from(
        "SELECT id, inventory_record_id, item_name, movement_type, quantity_delta,
                unit_cost_cents, reference_id, user_id, created_at
         FROM stock_movements WHERE business_id = ?",
    );
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(business_id.to_string())];

    if let Some(v) = &f.inventory_record_id {
        sql.push_str(" AND inventory_record_id = ?");
        args.push(Box::new(v.clone()));
    }
    if let Some(v) = &f.movement_type {
        sql.push_str(" AND movement_type = ?");
        args.push(Box::new(v.clone()));
    }
    if let Some(v) = &f.user_id {
        sql.push_str(" AND user_id = ?");
        args.push(Box::new(v.clone()));
    }
    if let Some(v) = &f.from {
        sql.push_str(" AND created_at >= ?");
        args.push(Box::new(v.clone()));
    }
    if let Some(v) = &f.to {
        sql.push_str(" AND created_at <= ?");
        args.push(Box::new(widen_to_end_of_day(v)));
    }
    sql.push_str(" ORDER BY created_at DESC, id DESC LIMIT ?");
    args.push(Box::new(f.limit.clamp(1, 5000)));
    (sql, args)
}

/// Owner-only, for the same reason the audit log is: this is oversight
/// data covering everything every user in the business has moved.
pub fn list(conn: &Connection, business_id: &str, user_id: &str, f: &MovementFilters) -> Result<Value> {
    crate::rbac::require_owner(conn, user_id)?;
    let (sql, args) = build_query(business_id, f);
    let mut stmt = conn.prepare(&sql)?;
    let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();

    let rows = stmt.query_map(refs.as_slice(), |r| {
        let movement_type: String = r.get(3)?;
        let quantity_delta: i64 = r.get(4)?;
        let unit_cost_cents: i64 = r.get(5)?;
        Ok(json!({
            "id": r.get::<_, String>(0)?,
            "inventory_record_id": r.get::<_, String>(1)?,
            "item_name": r.get::<_, String>(2)?,
            "movement_type": movement_type,
            "movement_label": label_for(&movement_type),
            "quantity_delta": quantity_delta,
            "unit_cost_cents": unit_cost_cents,
            // Signed to match quantity_delta, so a spreadsheet total of
            // this column is the real net cost effect rather than an
            // absolute value that counts stock leaving and stock
            // arriving as if they were the same direction.
            "total_cost_cents": unit_cost_cents * quantity_delta,
            "reference_id": r.get::<_, Option<String>>(6)?,
            "user_id": r.get::<_, Option<String>>(7)?,
            "created_at": r.get::<_, String>(8)?,
        }))
    })?;
    let movements: Vec<Value> = rows.collect::<rusqlite::Result<Vec<_>>>()?;

    let total_in: i64 = movements.iter().filter_map(|m| m["quantity_delta"].as_i64()).filter(|d| *d > 0).sum();
    let total_out: i64 = movements.iter().filter_map(|m| m["quantity_delta"].as_i64()).filter(|d| *d < 0).sum();

    Ok(json!({
        "movements": movements,
        "total_quantity_in": total_in,
        "total_quantity_out": total_out,
        "net_quantity_change": total_in + total_out,
    }))
}

/// The same rows `list` returns, as a real .xlsx file. Shares
/// `build_query` with `list` on purpose — see that function's comment.
pub fn export_xlsx(conn: &Connection, business_id: &str, user_id: &str, f: &MovementFilters) -> Result<Vec<u8>> {
    let data = list(conn, business_id, user_id, f)?;
    let movements = data["movements"].as_array().cloned().unwrap_or_default();
    let rows: Vec<Vec<Value>> = movements
        .iter()
        .map(|m| {
            vec![
                m["created_at"].clone(),
                m["item_name"].clone(),
                m["movement_label"].clone(),
                m["quantity_delta"].clone(),
                m["unit_cost_cents"].clone(),
                m["total_cost_cents"].clone(),
                m["reference_id"].clone(),
                m["user_id"].clone(),
            ]
        })
        .collect();
    crate::xlsx_export::rows_to_xlsx(
        "Stock Movements",
        &["When", "Item", "Movement", "Quantity change", "Unit cost", "Total cost", "Reference", "User"],
        &rows,
        // Unit cost and total cost are the integer-cents columns.
        &[4, 5],
    )
}
