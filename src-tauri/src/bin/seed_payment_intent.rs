use anyhow::Result;
use core_engine::db;
use rusqlite::params;

/// Dev/test-only utility: seeds a fake pending payment_intent row so the
/// webhook handlers (which look up a pending intent by provider
/// reference) can be exercised end-to-end without actually reaching
/// Stripe or Safaricom's servers — this is testing OUR code's reaction
/// to a webhook, not testing their APIs. Not part of the shipped app.
fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!("usage: seed_payment_intent <business_id> <provider> <provider_reference> <purpose> [amount]");
        std::process::exit(1);
    }
    let business_id = &args[1];
    let provider = &args[2];
    let provider_reference = &args[3];
    let purpose = &args[4];
    let amount: f64 = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(10.0);

    let conn = db::open("erp.db")?;
    conn.execute(
        "INSERT INTO payment_intents (id, business_id, provider, provider_reference, purpose, amount, currency, status, created_at)
         VALUES (lower(hex(randomblob(16))), ?1, ?2, ?3, ?4, ?5, 'usd', 'pending', datetime('now'))",
        params![business_id, provider, provider_reference, purpose, amount],
    )?;
    println!("seeded pending {provider} intent: reference={provider_reference} purpose={purpose} amount={amount}");
    Ok(())
}
