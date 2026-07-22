use anyhow::Result;
use core_engine::db;

/// Dev/test-only: proves the index added to module.rs's create_table()
/// is actually used by the query planner, not just present in the
/// schema. `EXPLAIN QUERY PLAN` on a representative query (the same
/// WHERE clause shape crud::list/report::run/xlsx export all use) will
/// say "SCAN" if it's doing a full table scan, or "SEARCH ... USING
/// INDEX" if the index is actually helping.
fn main() -> Result<()> {
    let conn = db::open("erp.db")?;

    let table = std::env::args().nth(1).unwrap_or_else(|| "module_inventory".to_string());
    let sql = format!(
        "EXPLAIN QUERY PLAN SELECT id FROM {table} WHERE business_id = 'x' AND deleted_at IS NULL"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |r| {
        let detail: String = r.get(3)?;
        Ok(detail)
    })?;

    println!("Query plan for: SELECT ... FROM {table} WHERE business_id = ? AND deleted_at IS NULL");
    for row in rows {
        println!("  {}", row?);
    }
    Ok(())
}
