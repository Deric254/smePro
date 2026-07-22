use anyhow::Result;
use core_engine::{db, http_api};

/// Dev/test-only: serves the API against a fresh, unseeded database —
/// exactly the state a real user's machine is in immediately after
/// installing the app, before first-run setup has happened. Used to
/// test the first-run flow specifically; `demo_seed` always pre-creates
/// a business, which is the wrong starting state for this.
fn main() -> Result<()> {
    let conn = db::open("erp.db")?;
    println!("[empty_server] serving with zero businesses — first-run setup flow should trigger");
    http_api::serve(conn, "127.0.0.1:8080");
    Ok(())
}
