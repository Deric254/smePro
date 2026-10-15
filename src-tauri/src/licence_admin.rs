//! Owner-only licence manager: type a customer's activation code, get their
//! licence; keep a list of customers; tick the months they have paid; export.
//!
//! HOW THE OWNER KEY IS HANDLED (the important part):
//!   * The private owner key is NEVER stored by the app and never shipped. If
//!     the app held it, any customer copy could mint licences.
//!   * The owner "unlocks" the manager by giving the key (choose the key file
//!     or paste it). It is held in memory only, wiped on Lock, after 30 idle
//!     minutes, or when the app closes.
//!   * Every route here needs BOTH: this computer is the claimed owner copy
//!     (licensing::Licensing::is_owner_copy) AND the manager is unlocked.
//!     Customers' copies fail the first check, whatever they call.
//!
//! The customer list lives in its own small file (`licence_customers.json` in
//! the app data folder): it touches neither the database nor sign-in, so it is
//! not part of business backups. Use Export regularly.
//!
//! Licence rule for a ticked month: valid until the end of the latest paid
//! month plus a short grace period, capped at 365 days ahead. The licence
//! still has to be sent to the customer (the app is offline); ticking only
//! makes it for you. Un-ticking a month does not revoke anything (licences
//! can't be revoked); it just fixes the record.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{Datelike, Duration as Days, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use zeroize::Zeroizing;

use crate::licensing::{norm_code, with_lic, Licensing};

/// Days after the end of the last paid month that the licence still works.
const GRACE_DAYS: i64 = 5;
const DEFAULT_DAYS: i64 = 31;
const MAX_DAYS: i64 = 365;
const EXPIRING_DAYS: i64 = 7;
const UNLOCK_IDLE: Duration = Duration::from_secs(30 * 60);

type Seed = Zeroizing<[u8; 32]>;

// ---------------------------------------------------------------- data

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Customer {
    pub id: String,
    pub name: String,
    /// Normalised activation code (12 letters/digits, no dashes).
    pub code: String,
    #[serde(default)]
    pub note: String,
    pub created: String,
    #[serde(default)]
    pub expires: Option<String>,
    #[serde(default)]
    pub last_issued: Option<String>,
    /// The latest licence text (not secret: it only works on that computer), so it can be re-copied.
    #[serde(default)]
    pub licence: Option<String>,
    /// Paid months, "YYYY-MM", sorted.
    #[serde(default)]
    pub paid: Vec<String>,
}

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default)]
    pub customers: Vec<Customer>,
}

fn pretty_code(code: &str) -> String {
    code.as_bytes().chunks(4).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join("-")
}

fn month_key(d: NaiveDate) -> String {
    format!("{:04}-{:02}", d.year(), d.month())
}

fn first_of_month(m: &str) -> Result<NaiveDate, String> {
    let ok_shape = m.len() == 7 && m.as_bytes()[4] == b'-';
    if ok_shape {
        if let Ok(d) = NaiveDate::parse_from_str(&format!("{m}-01"), "%Y-%m-%d") {
            return Ok(d);
        }
    }
    Err("That month isn't valid. Use the form 2026-10.".into())
}

fn end_of_month(first: NaiveDate) -> NaiveDate {
    let (y, m) = if first.month() == 12 { (first.year() + 1, 1) } else { (first.year(), first.month() + 1) };
    NaiveDate::from_ymd_opt(y, m, 1).unwrap_or(first) - Days::days(1)
}

fn parse_date(s: &Option<String>) -> Option<NaiveDate> {
    s.as_deref().and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
}

// ---------------------------------------------------------------- file storage

fn dir() -> PathBuf {
    std::env::var("SME_APP_DATA_DIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."))
}

fn ledger_path() -> PathBuf {
    dir().join("licence_customers.json")
}

/// A missing file is an empty list. An unreadable one is an ERROR and is never
/// overwritten, so a damaged file can't silently wipe the customer list.
fn load(path: &Path) -> Result<Ledger, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str::<Ledger>(&text)
            .map_err(|_| format!("The customer list file is damaged and was left untouched: {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Ledger::default()),
        Err(e) => Err(format!("Couldn't read the customer list: {e}")),
    }
}

/// Atomic write, keeping the previous version as `.bak`.
fn save(path: &Path, ledger: &Ledger) -> Result<(), String> {
    let text = serde_json::to_string_pretty(ledger).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if path.exists() {
        let _ = std::fs::copy(path, path.with_extension("json.bak"));
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("Couldn't save the customer list: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("Couldn't save the customer list: {e}"))
}

// ---------------------------------------------------------------- operations (pure: testable)

fn clean_name(name: &str) -> Result<String, String> {
    let n = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if n.is_empty() || n.chars().count() > 60 {
        return Err("Give the customer's name (1 to 60 characters).".into());
    }
    Ok(n)
}

fn issue_for(lic: &Licensing, seed: &[u8; 32], c: &Customer, expires: NaiveDate) -> Result<String, String> {
    lic.issue(seed, &c.code, &c.name, expires, &[])
}

fn add_customer(lic: &Licensing, seed: &[u8; 32], ledger: &mut Ledger, code: &str, name: &str, note: &str, days: i64) -> Result<(Customer, String), String> {
    let norm = norm_code(code);
    if norm.len() != 12 {
        return Err("The activation code is 12 letters and numbers, like ABCD-EFGH-JKLM.".into());
    }
    let name = clean_name(name)?;
    let note: String = note.trim().chars().take(200).collect();
    if !(1..=MAX_DAYS).contains(&days) {
        return Err(format!("A licence lasts 1 to {MAX_DAYS} days."));
    }
    if let Some(existing) = ledger.customers.iter().find(|c| c.code == norm) {
        return Err(format!("{} is already listed for {}. Use Renew on that row.", pretty_code(&norm), existing.name));
    }
    let today = lic.today();
    let mut c = Customer {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        code: norm,
        note,
        created: today.to_string(),
        expires: None,
        last_issued: None,
        licence: None,
        paid: vec![],
    };
    let expires = today + Days::days(days);
    let token = issue_for(lic, seed, &c, expires)?;
    c.expires = Some(expires.to_string());
    c.last_issued = Some(today.to_string());
    c.licence = Some(token.clone());
    ledger.customers.push(c.clone());
    Ok((c, token))
}

fn find_mut<'a>(ledger: &'a mut Ledger, id: &str) -> Result<&'a mut Customer, String> {
    ledger.customers.iter_mut().find(|c| c.id == id).ok_or_else(|| "That customer isn't in the list any more.".to_string())
}

fn renew(lic: &Licensing, seed: &[u8; 32], ledger: &mut Ledger, id: &str, days: i64) -> Result<(Customer, String), String> {
    if !(1..=MAX_DAYS).contains(&days) {
        return Err(format!("A licence lasts 1 to {MAX_DAYS} days."));
    }
    let c = find_mut(ledger, id)?;
    let today = lic.today();
    let expires = today + Days::days(days);
    let token = issue_for(lic, seed, c, expires)?;
    c.expires = Some(expires.to_string());
    c.last_issued = Some(today.to_string());
    c.licence = Some(token.clone());
    Ok((c.clone(), token))
}

/// Tick or un-tick a paid month. Ticking issues a licence valid to the end of
/// the latest paid month plus the grace period.
fn set_paid(lic: &Licensing, seed: &[u8; 32], ledger: &mut Ledger, id: &str, month: &str, paid: bool) -> Result<(Customer, Option<String>), String> {
    let first = first_of_month(month)?;
    let key = month_key(first);
    let c = find_mut(ledger, id)?;
    if !paid {
        c.paid.retain(|m| *m != key);
        return Ok((c.clone(), None));
    }
    let mut months = c.paid.clone();
    if !months.contains(&key) {
        months.push(key);
    }
    months.sort();
    let latest = first_of_month(months.last().map(String::as_str).unwrap_or(month))?;
    let expires = end_of_month(latest) + Days::days(GRACE_DAYS);
    let today = lic.today();
    if expires <= today {
        return Err("That month (plus the grace days) has already ended, so a licence for it would be expired. Tick the current month as well.".into());
    }
    if expires > today + Days::days(MAX_DAYS) {
        return Err("A licence can run at most 365 days ahead. Tick only the months up to about a year from now.".into());
    }
    let token = issue_for(lic, seed, c, expires)?;
    c.paid = months;
    c.expires = Some(expires.to_string());
    c.last_issued = Some(today.to_string());
    c.licence = Some(token.clone());
    Ok((c.clone(), Some(token)))
}

fn status_of(c: &Customer, today: NaiveDate) -> (&'static str, Option<i64>) {
    match parse_date(&c.expires) {
        None => ("none", None),
        Some(e) => {
            let left = (e - today).num_days();
            let s = if left < 0 {
                "expired"
            } else if left <= EXPIRING_DAYS {
                "expiring"
            } else {
                "active"
            };
            (s, Some(left))
        }
    }
}

fn customer_view(c: &Customer, month: &str, today: NaiveDate) -> Value {
    let (status, left) = status_of(c, today);
    json!({
        "id": c.id, "name": c.name, "code": pretty_code(&c.code), "note": c.note, "created": c.created,
        "expires": c.expires, "status": status, "days_left": left, "last_issued": c.last_issued,
        "licence": c.licence, "paid": c.paid, "paid_for_month": c.paid.iter().any(|m| m == month),
    })
}

fn list_view(ledger: &Ledger, month: &str, today: NaiveDate) -> Value {
    let mut customers: Vec<&Customer> = ledger.customers.iter().collect();
    customers.sort_by_key(|c| c.name.to_lowercase());
    let count = |f: &dyn Fn(&Customer) -> bool| ledger.customers.iter().filter(|c| f(c)).count();
    let paid = count(&|c| c.paid.iter().any(|m| m == month));
    let status = |s: &'static str| count(&|c| status_of(c, today).0 == s);
    json!({
        "month": month,
        "summary": {
            "total": ledger.customers.len(),
            "active": status("active") + status("expiring"),
            "expiring": status("expiring"),
            "expired": status("expired"),
            "paid_for_month": paid,
            "unpaid_for_month": ledger.customers.len() - paid,
        },
        "customers": customers.iter().map(|c| customer_view(c, month, today)).collect::<Vec<_>>(),
    })
}

/// Quoted CSV cell; a leading = + - @ is neutralised so Excel never runs it as a formula.
fn csv_cell(s: &str) -> String {
    let guarded = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{s}") } else { s.to_string() };
    format!("\"{}\"", guarded.replace('"', "\"\""))
}

fn export_csv(ledger: &Ledger, month: &str, today: NaiveDate) -> String {
    let mut customers: Vec<&Customer> = ledger.customers.iter().collect();
    customers.sort_by_key(|c| c.name.to_lowercase());
    let head = ["Name", "Activation code", "Note", "Added", "Licence expires", "Status", "Days left", "All paid months", &format!("Paid for {month}"), "Last licence issued"];
    let mut out = String::from("\u{feff}");
    out.push_str(&head.iter().map(|h| csv_cell(h)).collect::<Vec<_>>().join(","));
    out.push_str("\r\n");
    for c in customers {
        let (status, left) = status_of(c, today);
        let row = [
            c.name.clone(),
            pretty_code(&c.code),
            c.note.clone(),
            c.created.clone(),
            c.expires.clone().unwrap_or_default(),
            status.to_string(),
            left.map(|n| n.to_string()).unwrap_or_default(),
            c.paid.join(" "),
            if c.paid.iter().any(|m| m == month) { "Yes".into() } else { "No".to_string() },
            c.last_issued.clone().unwrap_or_default(),
        ];
        out.push_str(&row.iter().map(|v| csv_cell(v)).collect::<Vec<_>>().join(","));
        out.push_str("\r\n");
    }
    out
}

// ---------------------------------------------------------------- the unlocked key (memory only)

type Session = Mutex<Option<(Seed, Instant)>>;

fn session() -> &'static Session {
    static S: OnceLock<Session> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

/// The unlocked key, if it is still fresh. Each use keeps it fresh; idle for
/// 30 minutes locks it (and wipes it from memory).
fn fresh_seed(now: Instant, slot: &mut Option<(Seed, Instant)>) -> Option<Seed> {
    match slot {
        Some((seed, last)) if now.duration_since(*last) <= UNLOCK_IDLE => {
            *last = now;
            Some(seed.clone())
        }
        _ => {
            *slot = None; // dropping a Zeroizing value wipes it
            None
        }
    }
}

fn current_seed() -> Option<Seed> {
    let mut slot = session().lock().unwrap_or_else(|p| p.into_inner());
    fresh_seed(Instant::now(), &mut slot)
}

// ---------------------------------------------------------------- routes

fn field(body: &str, name: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get(name).and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default()
}

fn number(body: &str, name: &str) -> Option<i64> {
    let v = serde_json::from_str::<Value>(body).ok()?;
    let n = v.get(name)?;
    n.as_i64().or_else(|| n.as_str().and_then(|s| s.trim().parse().ok()))
}

fn flag(body: &str, name: &str) -> bool {
    serde_json::from_str::<Value>(body).ok().and_then(|v| v.get(name).and_then(Value::as_bool)).unwrap_or(false)
}

fn err(status: u16, message: impl Into<String>) -> (u16, Value) {
    (status, json!({"error": message.into()}))
}

fn ledger_lock() -> std::sync::MutexGuard<'static, ()> {
    static L: Mutex<()> = Mutex::new(());
    L.lock().unwrap_or_else(|p| p.into_inner())
}

/// Routes under /licence/admin/ (see licensing::handle_route).
pub fn handle(method: &str, path: &str, body: &str) -> (u16, Value) {
    let sub = path.trim_start_matches("/licence/admin/");
    let owner = with_lic(|l| l.is_owner_copy());

    if (method, sub) == ("GET", "status") {
        return (200, json!({"owner": owner, "unlocked": owner && current_seed().is_some()}));
    }
    if !owner {
        return err(403, "This is only available on the owner's computer. Claim it with your owner key first.");
    }

    if (method, sub) == ("POST", "unlock") {
        let result = with_lic(|l| {
            let seed = l.parse_owner_key(&field(body, "owner_key"))?;
            if !l.owner_key_matches(&seed) {
                return Err("That owner key doesn't belong to this app.".to_string());
            }
            Ok(Zeroizing::new(seed))
        });
        return match result {
            Ok(seed) => {
                *session().lock().unwrap_or_else(|p| p.into_inner()) = Some((seed, Instant::now()));
                (200, json!({"owner": true, "unlocked": true}))
            }
            Err(e) => err(400, e),
        };
    }
    if (method, sub) == ("POST", "lock") {
        *session().lock().unwrap_or_else(|p| p.into_inner()) = None;
        return (200, json!({"owner": true, "unlocked": false}));
    }

    let Some(seed) = current_seed() else {
        return (401, json!({"error": "Locked. Unlock with your owner key first.", "locked": true}));
    };
    let path = ledger_path();
    let _guard = ledger_lock();
    let mut ledger = match load(&path) {
        Ok(l) => l,
        Err(e) => return err(500, e),
    };
    let today = with_lic(|l| l.today());
    let month = {
        let m = field(body, "month");
        if m.is_empty() { month_key(today) } else { m }
    };
    if let Err(e) = first_of_month(&month) {
        return err(400, e);
    }

    match (method, sub) {
        ("POST", "list") => (200, list_view(&ledger, &month, today)),
        ("POST", "export") => {
            let file = format!("smepro-customers-{}.csv", today);
            (200, json!({"filename": file, "csv": export_csv(&ledger, &month, today)}))
        }
        ("POST", "add") => {
            let days = number(body, "days").unwrap_or(DEFAULT_DAYS);
            let r = with_lic(|l| add_customer(l, &seed, &mut ledger, &field(body, "code"), &field(body, "name"), &field(body, "note"), days));
            finish(r.map(|(c, t)| (c, Some(t))), &path, &ledger, &month, today)
        }
        ("POST", "renew") => {
            let days = number(body, "days").unwrap_or(DEFAULT_DAYS);
            let r = with_lic(|l| renew(l, &seed, &mut ledger, &field(body, "id"), days));
            finish(r.map(|(c, t)| (c, Some(t))), &path, &ledger, &month, today)
        }
        ("POST", "paid") => {
            let r = with_lic(|l| set_paid(l, &seed, &mut ledger, &field(body, "id"), &month, flag(body, "paid")));
            finish(r, &path, &ledger, &month, today)
        }
        ("POST", "remove") => {
            let id = field(body, "id");
            let before = ledger.customers.len();
            ledger.customers.retain(|c| c.id != id);
            if ledger.customers.len() == before {
                return err(404, "That customer isn't in the list any more.");
            }
            match save(&path, &ledger) {
                Ok(()) => (200, list_view(&ledger, &month, today)),
                Err(e) => err(500, e),
            }
        }
        _ => err(404, "not found"),
    }
}

fn finish(r: Result<(Customer, Option<String>), String>, path: &Path, ledger: &Ledger, month: &str, today: NaiveDate) -> (u16, Value) {
    match r {
        Err(e) => err(400, e),
        Ok((customer, licence)) => match save(path, ledger) {
            Ok(()) => {
                let mut v = list_view(ledger, month, today);
                v["licence"] = json!(licence);
                v["customer"] = json!(customer.name);
                (200, v)
            }
            Err(e) => err(500, e),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use std::sync::atomic::{AtomicU32, Ordering};

    // Throwaway key from the licensing kit's published conformance vector.
    const SEED: [u8; 32] = [
        0x3c, 0xf4, 0x9a, 0x98, 0xe3, 0xc3, 0x18, 0x3b, 0x25, 0x97, 0x55, 0x13, 0x3b, 0x9e, 0xdc, 0x39, 0x55, 0x0b, 0x20, 0x83, 0xf6, 0xfd, 0xe5,
        0x14, 0xd6, 0x3a, 0xe6, 0x7f, 0xac, 0x7d, 0xdf, 0x6b,
    ];

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn lic_at(today: &str) -> Licensing {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!("smp-admin-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let public: String = SigningKey::from_bytes(&SEED).verifying_key().to_bytes().iter().map(|b| format!("{b:02x}")).collect();
        let day = d(today);
        Licensing::new("SME Pro", "SMP", &public, crate::licensing::FileStore::new(dir.join("licensing.json"))).with_today(move || day)
    }

    fn tmp_file(name: &str) -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!("smp-ledger-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join(name)
    }

    #[test]
    fn adding_a_customer_issues_a_licence_that_verifies_on_their_code() {
        let l = lic_at("2026-10-15");
        let mut led = Ledger::default();
        let (c, token) = add_customer(&l, &SEED, &mut led, "abcd efgh jklm", "  Jane   Doe ", "Nairobi shop", 31).unwrap();
        assert_eq!(c.code, "ABCDEFGHJKLM");
        assert_eq!(c.name, "Jane Doe");
        assert_eq!(c.expires.as_deref(), Some("2026-11-15"));
        let data = l.read(&token, Some("ABCD-EFGH-JKLM")).unwrap();
        assert_eq!(data["user"], "Jane Doe");
        assert!(l.read(&token, Some("ZZZZ-ZZZZ-ZZZZ")).is_err());
        // same code again is refused and names who has it
        let dup = add_customer(&l, &SEED, &mut led, "ABCD-EFGH-JKLM", "Other", "", 31).unwrap_err();
        assert!(dup.contains("already listed for Jane Doe"));
        assert_eq!(led.customers.len(), 1);
    }

    #[test]
    fn bad_input_is_refused_without_changing_the_list() {
        let l = lic_at("2026-10-15");
        let mut led = Ledger::default();
        assert!(add_customer(&l, &SEED, &mut led, "ABC", "A", "", 31).is_err());
        assert!(add_customer(&l, &SEED, &mut led, "ABCD-EFGH-JKLM", "   ", "", 31).is_err());
        assert!(add_customer(&l, &SEED, &mut led, "ABCD-EFGH-JKLM", "A", "", 0).is_err());
        assert!(add_customer(&l, &SEED, &mut led, "ABCD-EFGH-JKLM", "A", "", 366).is_err());
        assert!(add_customer(&l, &[1u8; 32], &mut led, "ABCD-EFGH-JKLM", "A", "", 31).unwrap_err().contains("doesn't belong"));
        assert!(led.customers.is_empty());
    }

    #[test]
    fn ticking_a_month_issues_a_licence_to_month_end_plus_grace() {
        let l = lic_at("2026-10-15");
        let mut led = Ledger::default();
        let (c, _) = add_customer(&l, &SEED, &mut led, "ABCD-EFGH-JKLM", "Jane", "", 10).unwrap();
        let (c, token) = set_paid(&l, &SEED, &mut led, &c.id, "2026-10", true).unwrap();
        let token = token.unwrap();
        assert_eq!(c.paid, vec!["2026-10"]);
        assert_eq!(c.expires.as_deref(), Some("2026-11-05")); // Oct 31 + 5 days
        assert!(l.read(&token, Some("ABCDEFGHJKLM")).is_ok());
        // a later month extends it; ticking the same month twice does not duplicate
        let (c, _) = set_paid(&l, &SEED, &mut led, &c.id, "2026-12", true).unwrap();
        let (c, _) = set_paid(&l, &SEED, &mut led, &c.id, "2026-12", true).unwrap();
        assert_eq!(c.paid, vec!["2026-10", "2026-12"]);
        assert_eq!(c.expires.as_deref(), Some("2027-01-05"));
        // un-ticking fixes the record only: no new licence, the old one stays
        let (c2, none) = set_paid(&l, &SEED, &mut led, &c.id, "2026-12", false).unwrap();
        assert!(none.is_none());
        assert_eq!(c2.paid, vec!["2026-10"]);
        assert_eq!(c2.expires, c.expires);
    }

    #[test]
    fn tick_edge_cases_are_refused_cleanly() {
        let l = lic_at("2026-10-15");
        let mut led = Ledger::default();
        let (c, _) = add_customer(&l, &SEED, &mut led, "ABCD-EFGH-JKLM", "Jane", "", 10).unwrap();
        let before = led.customers[0].expires.clone();
        assert!(set_paid(&l, &SEED, &mut led, &c.id, "2026-08", true).unwrap_err().contains("already ended"));
        assert!(set_paid(&l, &SEED, &mut led, &c.id, "2027-12", true).unwrap_err().contains("365 days"));
        assert!(set_paid(&l, &SEED, &mut led, &c.id, "2026-13", true).is_err());
        assert!(set_paid(&l, &SEED, &mut led, &c.id, "garbage", true).is_err());
        assert!(set_paid(&l, &SEED, &mut led, "nope", "2026-10", true).is_err());
        assert!(led.customers[0].paid.is_empty(), "a refused tick records nothing");
        assert_eq!(led.customers[0].expires, before);
        // December rolls the year correctly
        let l2 = lic_at("2026-12-01");
        let (c, _) = add_customer(&l2, &SEED, &mut led, "WXYZ-2345-6789", "Dec", "", 5).unwrap();
        let (c, _) = set_paid(&l2, &SEED, &mut led, &c.id, "2026-12", true).unwrap();
        assert_eq!(c.expires.as_deref(), Some("2027-01-05"));
    }

    #[test]
    fn renew_and_remove() {
        let l = lic_at("2026-10-15");
        let mut led = Ledger::default();
        let (c, _) = add_customer(&l, &SEED, &mut led, "ABCD-EFGH-JKLM", "Jane", "", 10).unwrap();
        let (c, _) = renew(&l, &SEED, &mut led, &c.id, 90).unwrap();
        assert_eq!(c.expires.as_deref(), Some("2027-01-13"));
        assert!(renew(&l, &SEED, &mut led, &c.id, 0).is_err());
        assert!(renew(&l, &SEED, &mut led, "nope", 30).is_err());
    }

    #[test]
    fn summary_counts_and_statuses() {
        let l = lic_at("2026-10-15");
        let mut led = Ledger::default();
        let mut mk = |code: &str, name: &str, exp: &str, paid: &[&str]| {
            let (c, _) = add_customer(&l, &SEED, &mut led, code, name, "", 30).unwrap();
            let c = led.customers.iter_mut().find(|x| x.id == c.id).unwrap();
            c.expires = Some(exp.to_string());
            c.paid = paid.iter().map(|s| s.to_string()).collect();
        };
        mk("AAAA-AAAA-AAAA", "Active", "2026-12-01", &["2026-10"]);
        mk("BBBB-BBBB-BBBB", "Soon", "2026-10-20", &[]);
        mk("CCCC-CCCC-CCCC", "Gone", "2026-10-01", &["2026-09"]);
        let v = list_view(&led, "2026-10", d("2026-10-15"));
        let s = &v["summary"];
        assert_eq!(s["total"], 3);
        assert_eq!(s["active"], 2);
        assert_eq!(s["expiring"], 1);
        assert_eq!(s["expired"], 1);
        assert_eq!(s["paid_for_month"], 1);
        assert_eq!(s["unpaid_for_month"], 2);
        assert_eq!(v["customers"][0]["name"], "Active"); // sorted by name
        assert_eq!(v["customers"][1]["name"], "Gone");
        assert_eq!(v["customers"][1]["status"], "expired");
        assert_eq!(v["customers"][2]["status"], "expiring");
    }

    #[test]
    fn csv_export_is_excel_safe() {
        let l = lic_at("2026-10-15");
        let mut led = Ledger::default();
        add_customer(&l, &SEED, &mut led, "ABCD-EFGH-JKLM", "=HYPERLINK(\"x\")", "say \"hi\", ok", 30).unwrap();
        let csv = export_csv(&led, "2026-10", d("2026-10-15"));
        assert!(csv.starts_with('\u{feff}'));
        assert!(csv.contains("\"Paid for 2026-10\""));
        assert!(csv.contains("\"'=HYPERLINK(\"\"x\"\")\""), "formula is neutralised and quotes doubled: {csv}");
        assert!(csv.contains("\"say \"\"hi\"\", ok\""));
        assert!(csv.contains("\"ABCD-EFGH-JKLM\""));
        assert_eq!(csv.matches("\r\n").count(), 2);
    }

    #[test]
    fn ledger_file_round_trips_and_a_damaged_file_is_never_overwritten() {
        let path = tmp_file("licence_customers.json");
        assert!(load(&path).unwrap().customers.is_empty(), "missing file = empty list");
        let l = lic_at("2026-10-15");
        let mut led = Ledger::default();
        add_customer(&l, &SEED, &mut led, "ABCD-EFGH-JKLM", "Jane", "", 30).unwrap();
        save(&path, &led).unwrap();
        save(&path, &led).unwrap(); // second save leaves a .bak
        assert!(path.with_extension("json.bak").exists());
        assert_eq!(load(&path).unwrap().customers[0].name, "Jane");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(load(&path).unwrap_err().contains("damaged"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json", "damaged file left as it was");
    }

    #[test]
    fn unlocked_key_expires_after_idle_and_is_wiped() {
        let t0 = Instant::now();
        let mut slot: Option<(Seed, Instant)> = Some((Zeroizing::new(SEED), t0));
        assert!(fresh_seed(t0 + Duration::from_secs(29 * 60), &mut slot).is_some());
        // use refreshed the clock, so another 29 minutes later it is still open
        assert!(fresh_seed(t0 + Duration::from_secs(58 * 60), &mut slot).is_some());
        assert!(fresh_seed(t0 + Duration::from_secs(90 * 60), &mut slot).is_none());
        assert!(slot.is_none(), "an expired key is dropped");
    }

    #[test]
    fn admin_routes_refuse_a_copy_that_is_not_the_claimed_owner() {
        // This test process has no owner proof, so every route but `status` refuses.
        for (m, p) in [("POST", "list"), ("POST", "add"), ("POST", "unlock"), ("POST", "export"), ("POST", "paid"), ("POST", "remove")] {
            let (status, v) = handle(m, &format!("/licence/admin/{p}"), "{}");
            assert_eq!(status, 403, "{p}: {v}");
        }
        let (status, v) = handle("GET", "/licence/admin/status", "");
        assert_eq!(status, 200);
        assert_eq!(v["owner"], false);
        assert_eq!(v["unlocked"], false);
    }
}
