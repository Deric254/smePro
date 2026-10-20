//! Offline, per-computer licensing for the desktop and Android apps (Ed25519 signed licences).
//!
//! This is the Rust port of the `offline-app-licensing` kit, using the exact
//! same licence format as the owner tools in `licensing/` (see
//! `licensing/licensing.py`), so a licence issued with `issue_licence.py`
//! verifies here, byte for byte.
//!
//! How it works:
//!   * The owner holds a private owner key (a secret file kept OFF this repo).
//!     Its PUBLIC half is `owner_key::OWNER_PUBLIC_KEY`, built into every copy.
//!   * Each installed copy has an activation code (random, kept in
//!     `licensing.json` inside the app's own data folder).
//!   * The owner signs a licence for exactly that code, one person and an
//!     expiry date. The app only ever VERIFIES; a copy of the app can check a
//!     licence but cannot make one. No server, no internet.
//!   * The owner's own computer proves ownership once with the owner key (a
//!     signed proof bound to that install, never expires). The key itself is
//!     used in memory and never stored.
//!
//! Additive by design: it keeps its own tiny JSON file and touches neither the
//! database nor the users/sign-in system. Licence = "may this copy run";
//! sign-in = "who is using it".
//!
//! Where it is enforced: `http_api::serve` (all release builds, Android included) asks
//! `gate_block()` before routing ANY request, so the local API itself refuses
//! to serve an unlicensed copy: the window can't be used to bypass it. The
//! activation screen (`src/components/LicenceGate.tsx`) talks to the three
//! `/licence/*` routes handled by `handle_route`.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use chrono::{Duration as Days, Local, NaiveDate};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde_json::{json, Map, Value};

/// Stable id baked into every licence. Changing it invalidates all of them.
pub const APP_ID: &str = "SME Pro";
/// 2-6 capitals: tags the owner key text (`SMP-OWNER-...`) and licences (`SMP1.`).
pub const PREFIX: &str = "SMP";
const VERSION: u32 = 1;
const ROLLBACK_SECONDS: i64 = 86_400;
/// No look-alike characters (0/O/1/I) so codes can be read out over the phone.
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

fn unb64(s: &str) -> Option<Vec<u8>> {
    B64.decode(s.trim_end_matches('=')).ok()
}

/// Activation codes compare with everything but A-Z0-9 removed, upper-cased.
pub fn norm_code(code: &str) -> String {
    code.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect()
}

fn strip_ws(s: &str) -> String {
    s.split_whitespace().collect()
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() & 1 == 1 || !s.is_ascii() {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

// ---------------------------------------------------------------- storage

/// A tiny JSON-file store in the app's own data folder.
///
/// It never mistakes "couldn't read it right now" for "it's empty": that mistake
/// used to overwrite a customer's licence (and make `install_id` invent a new
/// activation code) whenever antivirus or a sync client held the file for a moment.
///   * a missing file is a fresh install; any other read failure is an ERROR and
///     nothing is written while it lasts;
///   * every change keeps the previous good copy as `licensing.json.bak`;
///   * a damaged file is recovered from that copy; with no usable copy it is set
///     aside as `licensing.json.damaged` (never deleted) and the store starts fresh;
///   * writes are forced to disk (see durable.rs), so a power cut can't leave an empty file.
pub struct FileStore {
    path: PathBuf,
}

enum Loaded {
    Data(Map<String, Value>),
    /// No file, or a file that holds nothing usable and has no usable backup.
    Fresh,
    /// The file is there but is not a JSON object, and the backup couldn't replace it.
    Damaged,
}

fn parse_store(text: &str) -> Option<Map<String, Value>> {
    serde_json::from_str::<Value>(text).ok().and_then(|v| v.as_object().cloned())
}

impl FileStore {
    pub fn new(path: PathBuf) -> Self {
        FileStore { path }
    }

    fn backup(&self) -> PathBuf {
        crate::durable::sibling(&self.path, ".bak")
    }

    fn from_backup(&self) -> Option<Map<String, Value>> {
        crate::durable::read_text(&self.backup()).ok().flatten().and_then(|t| parse_store(&t))
    }

    fn load(&self) -> Result<Loaded, String> {
        let unreadable = |e: std::io::Error| format!("The licence file on this computer can't be read right now ({e}). Close other programs using it and try again.");
        match crate::durable::read_text(&self.path) {
            Ok(Some(text)) => match parse_store(&text) {
                Some(map) => Ok(Loaded::Data(map)),
                None => Ok(self.from_backup().map(Loaded::Data).unwrap_or(Loaded::Damaged)),
            },
            // Not valid UTF-8: damaged content, not a busy file.
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => Ok(self.from_backup().map(Loaded::Data).unwrap_or(Loaded::Damaged)),
            Err(e) => Err(unreadable(e)),
            // The file itself is gone (a cleaner or antivirus removed it): the backup is the licence.
            Ok(None) => Ok(self.from_backup().map(Loaded::Data).unwrap_or(Loaded::Fresh)),
        }
    }

    /// `Err` only when the file exists but can't be read right now.
    pub fn try_get(&self, key: &str) -> Result<Option<Value>, String> {
        Ok(match self.load()? {
            Loaded::Data(m) => m.get(key).cloned(),
            Loaded::Fresh | Loaded::Damaged => None,
        })
    }

    pub fn get(&self, key: &str) -> Option<Value> {
        self.try_get(key).ok().flatten()
    }

    pub fn set(&self, key: &str, value: Value) -> Result<(), String> {
        let saving = |e: std::io::Error| format!("Couldn't save the licence on this computer: {e}");
        let mut data = match self.load()? {
            Loaded::Data(m) => m,
            Loaded::Fresh => Map::new(),
            Loaded::Damaged => {
                // Keep the damaged file for inspection instead of silently replacing it.
                let _ = std::fs::rename(&self.path, crate::durable::sibling(&self.path, ".damaged"));
                Map::new()
            }
        };
        data.insert(key.to_string(), value);
        let text = serde_json::to_string(&Value::Object(data)).map_err(|e| e.to_string())?;
        crate::durable::write_atomic(&self.path, text.as_bytes()).map_err(saving)?;
        // The backup always mirrors the latest real change. The clock marker moves about once a
        // minute, so it alone doesn't refresh it. The backup is written AFTER the main file, from
        // data this code just wrote, so a damaged main file is never copied over it.
        if key != "clock_high" {
            let _ = crate::durable::write_atomic(&self.backup(), text.as_bytes());
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- the model

pub struct Licensing {
    app_id: String,
    prefix: String,
    version: u32,
    public: Option<VerifyingKey>,
    store: FileStore,
    /// The activation code, once read or made. Held so a file that becomes unreadable
    /// for a moment can never make this copy invent a second code.
    id_cache: Mutex<Option<String>>,
    grace_days: i64,
    today: Box<dyn Fn() -> NaiveDate + Send + Sync>,
    clock: Box<dyn Fn() -> i64 + Send + Sync>,
}

impl Licensing {
    pub fn new(app_id: &str, prefix: &str, public_key_hex: &str, store: FileStore) -> Self {
        // A missing, malformed or off-curve key leaves `public` empty: the app
        // then refuses everything ("unconfigured"), it never falls back to open.
        let public = hex_decode(public_key_hex)
            .and_then(|b| <[u8; 32]>::try_from(b).ok())
            .and_then(|b| VerifyingKey::from_bytes(&b).ok());
        Licensing {
            app_id: app_id.to_string(),
            prefix: prefix.to_string(),
            version: VERSION,
            public,
            store,
            id_cache: Mutex::new(None),
            grace_days: 0,
            today: Box::new(|| Local::now().date_naive()),
            clock: Box::new(|| chrono::Utc::now().timestamp()),
        }
    }

    /// False when the build has no valid owner public key.
    pub fn configured(&self) -> bool {
        self.public.is_some()
    }

    fn tag(&self) -> String {
        format!("{}{}", self.prefix, self.version)
    }

    /// This copy's activation code, created once and kept in the data folder.
    /// `Err` when the licence file can't be read or written right now: nothing is
    /// invented in that case (a new code would invalidate the customer's licence).
    fn id_or_create(&self) -> Result<String, String> {
        let mut cached = self.id_cache.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(code) = cached.as_ref() {
            return Ok(code.clone());
        }
        if let Some(Value::String(code)) = self.store.try_get("install_id")? {
            if norm_code(&code).len() == 12 {
                *cached = Some(code.clone());
                return Ok(code);
            }
        }
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let raw: String = (0..12).map(|_| CODE_ALPHABET[rng.gen_range(0..CODE_ALPHABET.len())] as char).collect();
        let code = format!("{}-{}-{}", &raw[0..4], &raw[4..8], &raw[8..12]);
        self.store.set("install_id", json!(code))?;
        *cached = Some(code.clone());
        Ok(code)
    }

    /// Empty only when the licence file is unreadable (`state()` then says why).
    pub fn install_id(&self) -> String {
        self.id_or_create().unwrap_or_default()
    }

    /// Check signature, app, computer and expiry. Returns the licence data.
    pub fn read(&self, token: &str, for_install: Option<&str>) -> Result<Value, String> {
        let public = self.public.as_ref().ok_or_else(|| {
            "This copy of the app can't check licences yet (the owner key isn't set in the build).".to_string()
        })?;
        let not_a_licence = || "That doesn't look like a licence. Paste the whole text the owner sent you.".to_string();

        let t = strip_ws(token);
        let parts: Vec<&str> = t.split('.').collect();
        if parts.len() != 3 || parts[0] != self.tag() {
            return Err(not_a_licence());
        }
        let payload = unb64(parts[1]).ok_or_else(not_a_licence)?;
        let sig_bytes = unb64(parts[2]).ok_or_else(not_a_licence)?;
        let data: Value = serde_json::from_slice(&payload).map_err(|_| not_a_licence())?;
        if !data.is_object() {
            return Err(not_a_licence());
        }

        // The signature is checked over the exact bytes received, never over a re-serialisation.
        let sig = <[u8; 64]>::try_from(sig_bytes.as_slice()).map(|b| Signature::from_bytes(&b));
        if !matches!(sig, Ok(s) if public.verify(&payload, &s).is_ok()) {
            return Err("This licence wasn't signed by the owner, so it can't be used.".into());
        }
        if data.get("app").and_then(Value::as_str) != Some(self.app_id.as_str()) {
            return Err("This licence is for a different app.".into());
        }
        let here = match for_install {
            Some(c) => norm_code(c),
            None => norm_code(&self.id_or_create()?),
        };
        if data.get("for").and_then(Value::as_str) != Some(here.as_str()) {
            return Err("This licence was made for a different computer. Send the owner the activation code shown on this screen.".into());
        }
        let expires = data
            .get("expires")
            .and_then(Value::as_str)
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
            .ok_or_else(|| "That licence has no valid expiry date.".to_string())?;
        if expires + Days::days(self.grace_days) < (self.today)() {
            return Err("This licence has expired. Ask the owner for a new one.".into());
        }
        Ok(data)
    }

    /// Verify and keep a licence (first activation, or a renewal; pass
    /// `expect_user` to make a renewal belong to the same person).
    pub fn activate(&self, token: &str, expect_user: Option<&str>) -> Result<Value, String> {
        let data = self.read(token, None)?;
        if let Some(expected) = expect_user {
            let holder = data.get("user").and_then(Value::as_str).unwrap_or("");
            if holder.to_lowercase() != expected.trim().to_lowercase() {
                return Err("This licence is for a different person than the one who uses this computer.".into());
            }
        }
        self.store.set("license", json!(strip_ws(token)))?;
        Ok(data)
    }

    pub(crate) fn parse_owner_key(&self, text: &str) -> Result<[u8; 32], String> {
        let bad = || format!("That isn't an owner key. It starts with {}-OWNER- and is in the owner key file.", self.prefix);
        let t = strip_ws(text);
        let rest = t.strip_prefix(&format!("{}-OWNER-", self.prefix)).ok_or_else(bad)?;
        if rest.len() != 43 || !rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return Err(bad());
        }
        let raw = unb64(rest).ok_or_else(bad)?;
        <[u8; 32]>::try_from(raw).map_err(|_| bad())
    }

    pub(crate) fn owner_key_matches(&self, seed: &[u8; 32]) -> bool {
        match &self.public {
            Some(p) => SigningKey::from_bytes(seed).verifying_key().to_bytes() == p.to_bytes(),
            None => false,
        }
    }

    fn owner_msg(&self) -> Result<Vec<u8>, String> {
        Ok(format!("owner:{}:{}", self.app_id, norm_code(&self.id_or_create()?)).into_bytes())
    }

    /// The owner proves it is them, once, on their own computer. A signed proof
    /// bound to this install is kept; the key itself is never stored.
    pub fn claim_owner(&self, key_text: &str) -> Result<(), String> {
        let seed = self.parse_owner_key(key_text)?;
        if !self.owner_key_matches(&seed) {
            return Err("That owner key doesn't belong to this app, so it can't claim the owner copy.".into());
        }
        let sig = SigningKey::from_bytes(&seed).sign(&self.owner_msg()?);
        self.store.set("owner_proof", json!(B64.encode(sig.to_bytes())))
    }

    /// True on the owner's own computer (a valid owner proof is stored here).
    pub fn is_owner_copy(&self) -> bool {
        self.owner_ok()
    }

    pub fn today(&self) -> NaiveDate {
        (self.today)()
    }

    #[cfg(test)]
    pub(crate) fn with_today(mut self, today: impl Fn() -> NaiveDate + Send + Sync + 'static) -> Self {
        self.today = Box::new(today);
        self
    }

    /// Sign a licence for one computer. OWNER SIDE ONLY: `seed` comes from the
    /// owner's key, which the owner unlocks in memory (see licence_admin.rs) and
    /// which is never stored or shipped. Same bytes as `issue_licence.py`.
    pub fn issue(&self, seed: &[u8; 32], code: &str, user: &str, expires: NaiveDate, features: &[String]) -> Result<String, String> {
        if !self.owner_key_matches(seed) {
            return Err("That owner key doesn't belong to this app. Use the key made for this app's installer.".into());
        }
        let norm = norm_code(code);
        if norm.len() != 12 {
            return Err("The activation code is 12 letters and numbers, like ABCD-EFGH-JKLM.".into());
        }
        let user = user.split_whitespace().collect::<Vec<_>>().join(" ");
        if user.is_empty() || user.chars().count() > 60 {
            return Err("Give the licence holder's name (1 to 60 characters).".into());
        }
        let today = (self.today)();
        if expires <= today || expires > today + Days::days(365) {
            return Err("A licence lasts 1 to 365 days from today.".into());
        }
        let mut feats: Vec<String> = features.to_vec();
        feats.sort();
        feats.dedup();
        let feats_json = if feats.is_empty() {
            String::new()
        } else {
            let items: Vec<String> = feats.iter().map(|f| serde_json::to_string(f).unwrap_or_default()).collect();
            format!("\"features\":[{}],", items.join(","))
        };
        // Compact, keys sorted, exactly like the Python tool: the signature covers these bytes.
        let payload = format!(
            "{{\"app\":{},\"expires\":\"{}\",{}\"for\":\"{}\",\"issued\":\"{}\",\"role\":\"member\",\"user\":{},\"v\":{}}}",
            serde_json::to_string(&self.app_id).map_err(|e| e.to_string())?,
            expires,
            feats_json,
            norm,
            today,
            serde_json::to_string(&user).map_err(|e| e.to_string())?,
            self.version
        );
        let sig = SigningKey::from_bytes(seed).sign(payload.as_bytes());
        Ok(format!("{}.{}.{}", self.tag(), B64.encode(payload.as_bytes()), B64.encode(sig.to_bytes())))
    }

    fn owner_ok(&self) -> bool {
        let (Some(public), Ok(Some(Value::String(proof))), Ok(msg)) = (&self.public, self.store.try_get("owner_proof"), self.owner_msg()) else {
            return false;
        };
        unb64(&proof)
            .and_then(|b| <[u8; 64]>::try_from(b).ok())
            .map(|b| public.verify(&msg, &Signature::from_bytes(&b)).is_ok())
            .unwrap_or(false)
    }

    /// A computer whose clock was wound back to stretch a licence is refused.
    fn clock_ok(&self) -> bool {
        let high = self.store.get("clock_high").and_then(|v| v.as_i64()).unwrap_or(0);
        let t = (self.clock)();
        if t < high - ROLLBACK_SECONDS {
            return false;
        }
        if t > high + 60 {
            let _ = self.store.set("clock_high", json!(t));
        }
        true
    }

    /// The one question the app asks: may this copy be used right now?
    pub fn state(&self) -> Value {
        if !self.configured() {
            return json!({"ok": false, "kind": "unconfigured",
                "message": "This build has no owner key, so it can't be used. Set the owner public key and rebuild."});
        }
        if self.owner_ok() {
            return json!({"ok": true, "kind": "owner", "message": ""});
        }
        let unlicensed = |message: String| {
            json!({"ok": false, "kind": "unlicensed", "message": message, "install_id": self.install_id()})
        };
        let token = match self.store.try_get("license") {
            Ok(Some(Value::String(t))) => t,
            Ok(_) => return unlicensed("This computer isn't licensed yet.".into()),
            // The file is there but can't be read right now. Say so; never present it as "no licence".
            Err(message) => return json!({"ok": false, "kind": "storage", "message": message}),
        };
        let data = match self.read(&token, None) {
            Ok(d) => d,
            Err(e) => return unlicensed(e),
        };
        if !self.clock_ok() {
            return unlicensed("This computer's clock was set back. Fix the date and time to continue.".into());
        }
        let left = data
            .get("expires")
            .and_then(Value::as_str)
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
            .map(|d| (d - (self.today)()).num_days())
            .unwrap_or(0);
        json!({"ok": true, "kind": "licensed", "message": "", "expires": data["expires"], "days_left": left,
               "user": data.get("user"), "features": data.get("features").cloned().unwrap_or_else(|| json!([]))})
    }
}

// ---------------------------------------------------------------- this app's instance + gate

fn global() -> &'static Mutex<Licensing> {
    static LIC: OnceLock<Mutex<Licensing>> = OnceLock::new();
    LIC.get_or_init(|| {
        // Set by lib.rs's setup() before the API server starts.
        let dir = std::env::var("SME_APP_DATA_DIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."));
        Mutex::new(Licensing::new(
            APP_ID,
            PREFIX,
            crate::owner_key::OWNER_PUBLIC_KEY,
            FileStore::new(dir.join("licensing.json")),
        ))
    })
}

fn lic() -> std::sync::MutexGuard<'static, Licensing> {
    global().lock().unwrap_or_else(|p| p.into_inner())
}

/// Run `f` against this app's licensing instance (used by the owner-only manager).
pub(crate) fn with_lic<R>(f: impl FnOnce(&Licensing) -> R) -> R {
    f(&lic())
}

type Cache = Mutex<Option<(Instant, Value)>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

fn reset_cache() {
    *cache().lock().unwrap_or_else(|p| p.into_inner()) = None;
}

/// `state()` remembered for about a minute: it verifies a signature, and the
/// gate asks on every request.
fn cached_state() -> Value {
    let mut c = cache().lock().unwrap_or_else(|p| p.into_inner());
    if let Some((at, v)) = c.as_ref() {
        if at.elapsed() < Duration::from_secs(60) {
            return v.clone();
        }
    }
    let v = lic().state();
    *c = Some((Instant::now(), v.clone()));
    v
}

/// `None` = this copy may be used. `Some((402, body))` = refuse the request.
pub fn gate_block() -> Option<(u16, Value)> {
    let st = cached_state();
    if st["ok"].as_bool() == Some(true) {
        return None;
    }
    let message = st["message"].as_str().unwrap_or("This copy isn't licensed.").to_string();
    Some((402, json!({"error": message, "licence": st})))
}

/// The three routes the activation screen needs. Always reachable.
///   GET  /licence/state         -> state + this copy's activation code
///   POST /licence/activate      body {"licence": "..."}   (first activation or renewal)
///   POST /licence/claim-owner   body {"owner_key": "..."} (owner's own computer only)
pub fn handle_route(method: &str, path: &str, body: &str) -> (u16, Value) {
    let field = |name: &str| -> String {
        serde_json::from_str::<Value>(body).ok().and_then(|v| v.get(name).and_then(Value::as_str).map(str::to_string)).unwrap_or_default()
    };
    let with_id = |l: &Licensing| {
        let mut s = l.state();
        if l.configured() {
            s["install_id"] = json!(l.install_id());
        }
        s
    };
    match (method, path) {
        ("GET", "/licence/state") => (200, with_id(&lic())),
        ("POST", "/licence/activate") => {
            let l = lic();
            // A renewal must belong to the person already licensed on this computer.
            let current = l.state();
            let expect = if current["kind"] == "licensed" { current["user"].as_str().map(str::to_string) } else { None };
            let result = l.activate(&field("licence"), expect.as_deref());
            let out = match result {
                Ok(_) => (200, with_id(&l)),
                Err(e) => (400, json!({"error": e})),
            };
            drop(l);
            reset_cache();
            out
        }
        ("POST", "/licence/claim-owner") => {
            let l = lic();
            let out = match l.claim_owner(&field("owner_key")) {
                Ok(()) => (200, with_id(&l)),
                Err(e) => (400, json!({"error": e})),
            };
            drop(l);
            reset_cache();
            out
        }
        (m, p) if p.starts_with("/licence/admin/") => crate::licence_admin::handle(m, p, body),
        _ => (404, json!({"error": "not found"})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, AtomicI64, Ordering};
    use std::sync::Arc;

    // Throwaway conformance key from the kit's token-spec (public, not secret).
    const SEED: &str = "3cf49a98e3c3183b259755133b9edc39550b2083f6fde514d63ae67fac7ddf6b";
    const PUBLIC: &str = "7a25536be5bd7d9462fcbe457e004d298ecff116dfd4aafd4304364bb57c91cc";
    const CODE: &str = "ABCD-EFGH-JKLM";
    const VECTOR: &str = "CNF1.eyJhcHAiOiJDb25mb3JtYW5jZSBBcHAiLCJleHBpcmVzIjoiMjAyNi0wMS0zMSIsImZlYXR1cmVzIjpbInBybyJdLCJmb3IiOiJBQkNERUZHSEpLTE0iLCJpc3N1ZWQiOiIyMDI2LTAxLTAxIiwicm9sZSI6Im1lbWJlciIsInVzZXIiOiJUZXN0IFVzZXIiLCJ2IjoxfQ.1jo45EXZ9tV8w-iPDEgyt5syxF5kC25qKn_yvckRIUf_ZTflW3pR5tqojK-45g79LB3J3pWOF6A-Abk1p2YiAg";

    fn seed() -> [u8; 32] {
        <[u8; 32]>::try_from(hex_decode(SEED).unwrap()).unwrap()
    }

    fn date(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn tmp_store() -> FileStore {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!("smp-lic-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&dir);
        FileStore::new(dir.join("licensing.json"))
    }

    fn lic_for(app: &str, prefix: &str, public: &str, today: &str) -> Licensing {
        let mut l = Licensing::new(app, prefix, public, tmp_store());
        let d = date(today);
        l.today = Box::new(move || d);
        l
    }

    /// Mirrors issue_licence.py (compact JSON, sorted keys) for tests only.
    fn issue(l: &Licensing, code: &str, user: &str, days: i64, features: &[&str]) -> String {
        let today = (l.today)();
        let expires = today + Days::days(days);
        let feats = if features.is_empty() {
            String::new()
        } else {
            format!("\"features\":[{}],", features.iter().map(|f| format!("\"{f}\"")).collect::<Vec<_>>().join(","))
        };
        let payload = format!(
            "{{\"app\":\"{}\",\"expires\":\"{}\",{}\"for\":\"{}\",\"issued\":\"{}\",\"role\":\"member\",\"user\":\"{}\",\"v\":{}}}",
            l.app_id, expires, feats, norm_code(code), today, user, l.version
        );
        let sig = SigningKey::from_bytes(&seed()).sign(payload.as_bytes());
        format!("{}.{}.{}", l.tag(), B64.encode(payload.as_bytes()), B64.encode(sig.to_bytes()))
    }

    #[test]
    fn conformance_vector_matches_the_kit_byte_for_byte() {
        // (a) the public key derives from the seed
        assert_eq!(SigningKey::from_bytes(&seed()).verifying_key().to_bytes().to_vec(), hex_decode(PUBLIC).unwrap());
        let l = lic_for("Conformance App", "CNF", PUBLIC, "2026-01-01");
        // Ed25519 is deterministic: we reproduce the Python-made licence exactly
        assert_eq!(issue(&l, CODE, "Test User", 30, &["pro"]), VECTOR);
        // (b) accepted for the right code on the right day
        let data = l.read(VECTOR, Some(CODE)).unwrap();
        assert_eq!(data["user"], "Test User");
        assert_eq!(data["features"], json!(["pro"]));
        // (c) rejected for another code, after expiry, or with one changed character
        assert!(l.read(VECTOR, Some("ABCD-EFGH-JKLN")).unwrap_err().contains("different computer"));
        let late = lic_for("Conformance App", "CNF", PUBLIC, "2026-02-01");
        assert!(late.read(VECTOR, Some(CODE)).unwrap_err().contains("expired"));
        let last_day = lic_for("Conformance App", "CNF", PUBLIC, "2026-01-31");
        assert!(last_day.read(VECTOR, Some(CODE)).is_ok());
        let mut chars: Vec<char> = VECTOR.chars().collect();
        let i = VECTOR.len() - 5;
        chars[i] = if chars[i] == 'A' { 'B' } else { 'A' };
        let tampered: String = chars.into_iter().collect();
        assert!(l.read(&tampered, Some(CODE)).unwrap_err().contains("wasn't signed"));
    }

    #[test]
    fn whitespace_in_a_pasted_licence_is_tolerated() {
        let l = lic_for("Conformance App", "CNF", PUBLIC, "2026-01-01");
        let spaced = format!("  {}\n{}  \r\n", &VECTOR[..40], &VECTOR[40..]);
        assert!(l.read(&spaced, Some(CODE)).is_ok());
    }

    #[test]
    fn lifecycle_unlicensed_activate_licensed() {
        let l = lic_for("SME Pro", "SMP", PUBLIC, "2026-01-01");
        let st = l.state();
        assert_eq!(st["ok"], false);
        assert_eq!(st["kind"], "unlicensed");
        let id = l.install_id();
        assert_eq!(l.install_id(), id, "the activation code is stable");
        assert_eq!(norm_code(&id).len(), 12);
        let token = issue(&l, &id, "Jane Doe", 30, &[]);
        l.activate(&token, None).unwrap();
        let st = l.state();
        assert_eq!(st["ok"], true);
        assert_eq!(st["kind"], "licensed");
        assert_eq!(st["days_left"], 30);
        assert_eq!(st["user"], "Jane Doe");
        // renewal for someone else is refused, for the same person accepted
        let other = issue(&l, &id, "Someone Else", 30, &[]);
        assert!(l.activate(&other, Some("Jane Doe")).unwrap_err().contains("different person"));
        assert!(l.activate(&issue(&l, &id, "jane doe", 60, &[]), Some("Jane Doe")).is_ok());
    }

    #[test]
    fn wrong_computer_app_key_and_expiry_are_refused() {
        let l = lic_for("SME Pro", "SMP", PUBLIC, "2026-01-01");
        let id = l.install_id();
        assert!(l.read(&issue(&l, "ZZZZ-ZZZZ-ZZZZ", "A", 30, &[]), None).unwrap_err().contains("different computer"));
        let other_app = lic_for("Other App", "SMP", PUBLIC, "2026-01-01");
        assert!(l.read(&issue(&other_app, &id, "A", 30, &[]), None).unwrap_err().contains("different app"));
        let stranger = SigningKey::from_bytes(&[7u8; 32]);
        let payload = format!("{{\"app\":\"SME Pro\",\"expires\":\"2026-02-01\",\"for\":\"{}\",\"issued\":\"2026-01-01\",\"role\":\"member\",\"user\":\"A\",\"v\":1}}", norm_code(&id));
        let forged = format!("SMP1.{}.{}", B64.encode(payload.as_bytes()), B64.encode(stranger.sign(payload.as_bytes()).to_bytes()));
        assert!(l.read(&forged, None).unwrap_err().contains("wasn't signed"));
        assert!(l.read("not a licence", None).unwrap_err().contains("doesn't look like a licence"));
        assert!(l.read(&issue(&l, &id, "A", 30, &[]).replace("SMP1.", "XYZ1."), None).is_err());
        let t = issue(&l, &id, "A", 3, &[]);
        let later = lic_for("SME Pro", "SMP", PUBLIC, "2026-01-05");
        later.store.set("install_id", json!(id)).unwrap();
        assert!(later.read(&t, None).unwrap_err().contains("expired"));
    }

    #[test]
    fn fails_closed_without_a_valid_public_key() {
        for bad in ["", "   ", "zz", "abcd", &"0".repeat(63)] {
            let l = lic_for("SME Pro", "SMP", bad, "2026-01-01");
            assert!(!l.configured());
            let st = l.state();
            assert_eq!(st["ok"], false);
            assert_eq!(st["kind"], "unconfigured");
            assert!(l.read(VECTOR, Some(CODE)).is_err());
        }
    }

    #[test]
    fn the_shipped_owner_key_is_a_valid_key_and_an_empty_one_refuses_everything() {
        let shipped = crate::owner_key::OWNER_PUBLIC_KEY;
        let l = Licensing::new(APP_ID, PREFIX, shipped, tmp_store());
        if shipped.trim().is_empty() {
            assert_eq!(l.state()["kind"], "unconfigured");
        } else {
            // A typo in the committed key would make every licence fail on every computer.
            assert!(l.configured(), "OWNER_PUBLIC_KEY is not a valid Ed25519 public key");
        }
    }

    #[test]
    fn owner_proof_is_bound_to_this_install_and_key() {
        let l = lic_for("SME Pro", "SMP", PUBLIC, "2026-01-01");
        let key_text = format!("SMP-OWNER-{}", B64.encode(seed()));
        assert!(l.claim_owner("SMP-OWNER-short").unwrap_err().contains("isn't an owner key"));
        assert!(l.claim_owner(&format!("SMP-OWNER-{}", B64.encode([9u8; 32]))).unwrap_err().contains("doesn't belong"));
        assert_eq!(l.state()["ok"], false);
        l.claim_owner(&format!("  {key_text}\n")).unwrap();
        assert_eq!(l.state()["kind"], "owner");
        // the key itself is never stored
        let stored = std::fs::read_to_string(&l.store.path).unwrap();
        assert!(!stored.contains(&B64.encode(seed())));
        // copying the proof to another install does not carry ownership
        let proof = l.store.get("owner_proof").unwrap();
        let other = lic_for("SME Pro", "SMP", PUBLIC, "2026-01-01");
        other.store.set("owner_proof", proof).unwrap();
        assert_eq!(other.state()["kind"], "unlicensed");
    }

    #[test]
    fn a_clock_set_back_is_refused() {
        let now = Arc::new(AtomicI64::new(1_800_000_000));
        let mut l = lic_for("SME Pro", "SMP", PUBLIC, "2026-01-01");
        let n = now.clone();
        l.clock = Box::new(move || n.load(Ordering::SeqCst));
        let id = l.install_id();
        l.activate(&issue(&l, &id, "A", 90, &[]), None).unwrap();
        assert_eq!(l.state()["ok"], true);
        now.store(1_800_000_000 + 10 * 86_400, Ordering::SeqCst);
        assert_eq!(l.state()["ok"], true);
        now.store(1_800_000_000, Ordering::SeqCst); // 10 days back
        let st = l.state();
        assert_eq!(st["ok"], false);
        assert!(st["message"].as_str().unwrap().contains("clock was set back"));
    }

    #[test]
    fn issue_reproduces_the_kit_conformance_licence_exactly() {
        let l = lic_for("Conformance App", "CNF", PUBLIC, "2026-01-01");
        let token = l.issue(&seed(), CODE, "Test User", date("2026-01-31"), &["pro".to_string()]).unwrap();
        assert_eq!(token, VECTOR);
        assert!(l.read(&token, Some(CODE)).is_ok());
        // wrong key, bad code, bad name, out-of-range dates are all refused
        assert!(l.issue(&[9u8; 32], CODE, "A", date("2026-01-31"), &[]).unwrap_err().contains("doesn't belong"));
        assert!(l.issue(&seed(), "ABC", "A", date("2026-01-31"), &[]).is_err());
        assert!(l.issue(&seed(), CODE, "   ", date("2026-01-31"), &[]).is_err());
        assert!(l.issue(&seed(), CODE, "A", date("2026-01-01"), &[]).is_err());
        assert!(l.issue(&seed(), CODE, "A", date("2027-01-02"), &[]).is_err());
        assert!(l.issue(&seed(), CODE, "A", date("2027-01-01"), &[]).is_ok());
        // a name needing JSON escaping still produces a valid, verifiable licence
        let odd = l.issue(&seed(), CODE, "Jane \"JD\" O'Neil", date("2026-02-01"), &[]).unwrap();
        assert_eq!(l.read(&odd, Some(CODE)).unwrap()["user"], "Jane \"JD\" O'Neil");
    }

    #[test]
    fn features_come_through_and_default_to_none() {
        let l = lic_for("SME Pro", "SMP", PUBLIC, "2026-01-01");
        let id = l.install_id();
        l.activate(&issue(&l, &id, "A", 30, &[]), None).unwrap();
        assert_eq!(l.state()["features"], json!([]));
        l.activate(&issue(&l, &id, "A", 30, &["pro"]), None).unwrap();
        assert_eq!(l.state()["features"], json!(["pro"]));
    }

    // ---- the file store must never turn "can't read it right now" into "empty"

    #[test]
    fn a_file_that_cannot_be_read_is_never_overwritten_and_never_changes_the_activation_code() {
        let l = lic_for("SME Pro", "SMP", PUBLIC, "2026-01-01");
        let id = l.install_id();
        l.activate(&issue(&l, &id, "Jane", 30, &[]), None).unwrap();
        // A second copy of the same data folder that can't read the file (a folder stands in for a locked file).
        let path = l.store.path.clone();
        let held = path.with_extension("held");
        std::fs::rename(&path, &held).unwrap();
        std::fs::create_dir(&path).unwrap();
        let fresh = Licensing::new("SME Pro", "SMP", PUBLIC, FileStore::new(path.clone()));
        assert_eq!(fresh.install_id(), "", "no new code is invented while the file is unreadable");
        assert!(fresh.store.set("clock_high", json!(1)).is_err(), "nothing is written while the file is unreadable");
        assert_eq!(fresh.state()["kind"], "storage");
        // The running copy keeps its code in memory even though its file is now unreadable.
        assert_eq!(l.install_id(), id);
        std::fs::remove_dir(&path).unwrap();
        std::fs::rename(&held, &path).unwrap();
        let mut back = Licensing::new("SME Pro", "SMP", PUBLIC, FileStore::new(path));
        back.today = Box::new(|| date("2026-01-01"));
        assert_eq!(back.install_id(), id);
        assert_eq!(back.state()["kind"], "licensed", "the licence survived");
    }

    #[test]
    fn an_empty_or_damaged_file_is_recovered_from_the_backup() {
        let l = lic_for("SME Pro", "SMP", PUBLIC, "2026-01-01");
        let id = l.install_id();
        l.activate(&issue(&l, &id, "Jane", 30, &[]), None).unwrap();
        l.claim_owner(&format!("SMP-OWNER-{}", B64.encode(seed()))).unwrap();
        for ruined in ["", "{ half a fi"] {
            std::fs::write(&l.store.path, ruined).unwrap(); // what a power cut leaves behind
            let again = Licensing::new("SME Pro", "SMP", PUBLIC, FileStore::new(l.store.path.clone()));
            assert_eq!(again.install_id(), id, "same activation code");
            assert!(again.store.get("license").is_some(), "licence kept");
            assert!(again.is_owner_copy(), "owner proof kept");
        }
        std::fs::remove_file(&l.store.path).unwrap(); // removed by a cleaner
        let again = Licensing::new("SME Pro", "SMP", PUBLIC, FileStore::new(l.store.path.clone()));
        assert_eq!(again.install_id(), id);
    }

    #[test]
    fn damaged_with_no_backup_is_set_aside_not_deleted_and_the_app_can_start_over() {
        let store = tmp_store();
        std::fs::create_dir_all(store.path.parent().unwrap()).unwrap();
        std::fs::write(&store.path, "garbage").unwrap();
        let l = Licensing::new("SME Pro", "SMP", PUBLIC, store);
        let id = l.install_id();
        assert_eq!(norm_code(&id).len(), 12);
        assert_eq!(std::fs::read_to_string(crate::durable::sibling(&l.store.path, ".damaged")).unwrap(), "garbage");
        assert_eq!(l.state()["kind"], "unlicensed");
    }
}
