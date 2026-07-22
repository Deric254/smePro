use anyhow::Result;
use core_engine::db;

/// Dev/test-only: directly overwrites next_due_date for a business, to
/// simulate time having passed without needing to wait real days or
/// fake the system clock. Used only for testing license edge cases.
fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: set_due_date <business_id> <YYYY-MM-DD>");
        std::process::exit(1);
    }
    let conn = db::open("erp.db")?;
    conn.execute(
        "UPDATE licenses SET next_due_date = ?1 WHERE business_id = ?2",
        rusqlite::params![args[2], args[1]],
    )?;
    println!("set next_due_date = {} for business {}", args[2], args[1]);
    Ok(())
}
