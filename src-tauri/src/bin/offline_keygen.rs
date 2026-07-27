//! Offline license key generator. Run by hand, on your own machine,
//! never shipped inside the app itself (see Cargo.toml — this binary
//! requires the "dev-tools" feature, same gate as demo_seed and the
//! other tools that must never end up in a customer's installer).
//!
//! Replaces the earlier server-based vendor_authority design: no
//! always-on server, no hosting cost, works completely offline. The
//! trade-off, stated plainly rather than hidden: without a central
//! server watching every install, this can't strictly guarantee a key
//! is used on only one device the way the server design could — it can
//! only guarantee a key is genuine (actually signed by you) and that
//! this app won't silently swap to a different key once one is bound.
//! For a flat one-time-purchase product, that's the right trade for
//! zero ongoing cost.
//!
//! Usage:
//!   offline_keygen generate-keypair
//!       Creates a new signing keypair. Prints BOTH halves — the
//!       private key (keep this secret, store it somewhere safe,
//!       NEVER commit it to git, losing it means you can never issue
//!       another valid key again) and the public key (safe to share —
//!       paste it into vendor_license.rs's LICENSE_PUBLIC_KEY_HEX
//!       constant before building the app).
//!
//!   offline_keygen issue --private-key <hex> [--note "text"]
//!       Signs and prints one new license key using your private key.
//!       The note is for YOUR OWN records only (who this key is for) —
//!       it is never embedded in the key or verifiable by the app.

use ed25519_dalek::{Signer, SigningKey};
use rand::rngs::OsRng;

const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ"; // Crockford base32

fn base32_encode(bytes: &[u8]) -> String {
    let mut bits = 0u32;
    let mut bit_count = 0u32;
    let mut out = String::new();
    for &byte in bytes {
        bits = (bits << 8) | byte as u32;
        bit_count += 8;
        while bit_count >= 5 {
            bit_count -= 5;
            let idx = (bits >> bit_count) & 0x1F;
            out.push(ALPHABET[idx as usize] as char);
        }
    }
    if bit_count > 0 {
        let idx = (bits << (5 - bit_count)) & 0x1F;
        out.push(ALPHABET[idx as usize] as char);
    }
    out
}

fn group_with_dashes(s: &str, group_size: usize) -> String {
    s.as_bytes()
        .chunks(group_size)
        .map(|c| String::from_utf8_lossy(c).to_string())
        .collect::<Vec<_>>()
        .join("-")
}

fn cmd_generate_keypair() {
    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();

    println!("New license signing keypair generated.\n");
    println!("PRIVATE KEY — keep this secret, store it somewhere safe (a password");
    println!("manager, an encrypted drive), and NEVER commit it to git. If you lose");
    println!("this, you can never issue a valid license key again:\n");
    println!("  {}\n", hex::encode(signing_key.to_bytes()));
    println!("PUBLIC KEY — safe to share, paste this into vendor_license.rs's");
    println!("LICENSE_PUBLIC_KEY_HEX constant before building the app:\n");
    println!("  {}\n", hex::encode(verifying_key.to_bytes()));
}

fn cmd_issue(private_key_hex: &str, note: &str) {
    let key_bytes = match hex::decode(private_key_hex.trim()) {
        Ok(b) if b.len() == 32 => b,
        Ok(_) => {
            eprintln!("Error: private key must be exactly 32 bytes (64 hex characters).");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: not valid hex: {e}");
            std::process::exit(1);
        }
    };
    let signing_key = SigningKey::from_bytes(key_bytes.as_slice().try_into().unwrap());

    // A short random nonce — its only job is to make each key unique
    // (so two keys issued for different customers never collide), not
    // to carry any security weight itself. All the actual security
    // comes from the signature being unforgeable without the private
    // key above.
    let mut nonce = [0u8; 8];
    use rand::RngCore;
    OsRng.fill_bytes(&mut nonce);

    let signature = signing_key.sign(&nonce);

    let mut payload = Vec::with_capacity(8 + 64);
    payload.extend_from_slice(&nonce);
    payload.extend_from_slice(&signature.to_bytes());

    let encoded = base32_encode(&payload);
    let grouped = group_with_dashes(&encoded, 5);
    let key = format!("SPK-{grouped}");

    println!("Issued a new license key{}:\n", if note.is_empty() { String::new() } else { format!(" for \"{note}\"") });
    println!("  {key}\n");
    println!("Send this to your customer. It is shown here once — this tool doesn't");
    println!("keep a record of it (there's no database to keep it in, by design —");
    println!("that's what makes this free and serverless). If you want your own");
    println!("record of who has which key, save the note yourself.");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("generate-keypair") => cmd_generate_keypair(),
        Some("issue") => {
            let mut private_key = None;
            let mut note = String::new();
            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "--private-key" => { private_key = args.get(i + 1).cloned(); i += 2; }
                    "--note" => { note = args.get(i + 1).cloned().unwrap_or_default(); i += 2; }
                    _ => { i += 1; }
                }
            }
            let Some(private_key) = private_key else {
                eprintln!("Usage: offline_keygen issue --private-key <hex> [--note \"text\"]");
                std::process::exit(1);
            };
            cmd_issue(&private_key, &note);
        }
        _ => {
            println!("Offline license key generator\n");
            println!("Usage:");
            println!("  offline_keygen generate-keypair");
            println!("  offline_keygen issue --private-key <hex> [--note \"text\"]");
        }
    }
}
