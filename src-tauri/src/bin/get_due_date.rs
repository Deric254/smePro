use anyhow::Result;
use core_engine::db;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: get_due_date <business_id>");
        std::process::exit(1);
    }
    let conn = db::open("erp.db")?;
    let due: String = conn.query_row(
        "SELECT next_due_date FROM licenses WHERE business_id = ?1",
        [&args[1]],
        |r| r.get(0),
    )?;
    println!("{due}");
    Ok(())
}
