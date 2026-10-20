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
//! One licence is one computer (one activation code). The same customer name on a
//! second code is never added silently: the owner must say whether it is a
//! Migration (the old code is marked retired and is never renewed) or an Extra
//! computer (a second seat, listed and charged). The list shows how many live
//! computers each customer has.
//!
//! Licence rule for a ticked month: valid until the end of the latest paid
//! month plus a short grace period, capped at 365 days ahead. The licence
//! still has to be sent to the customer (the app is offline); ticking only
//! makes it for you. Un-ticking a month does not revoke anything (licences
//! can't be revoked); it just fixes the record.

use std::collections::HashMap;
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
    /// Replaced by a newer computer in a migration: kept in the list, never renewed.
    #[serde(default)]
    pub retired: bool,
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

/// Live (not retired) computers per customer name, case-insensitive.
fn live_counts(ledger: &Ledger) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for c in ledger.customers.iter().filter(|c| !c.retired) {
        *counts.entry(c.name.to_lowercase()).or_insert(0) += 1;
    }
    counts
}

fn sorted(ledger: &Ledger) -> Vec<&Customer> {
    let mut customers: Vec<&Customer> = ledger.customers.iter().collect();
    customers.sort_by_key(|c| (c.name.to_lowercase(), c.retired));
    customers
}

// ---------------------------------------------------------------- file storage

fn dir() -> PathBuf {
    std::env::var("SME_APP_DATA_DIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."))
}

fn ledger_path() -> PathBuf {
    dir().join("licence_customers.json")
}

/// A missing file is an empty list. An unreadable one is an ERROR and is never
/// overwritten, so a damaged file can't silently wipe the customer list. A file
/// that is damaged (not just busy) is read from its `.bak` copy when that is good.
fn load(path: &Path) -> Result<Ledger, String> {
    let backup = crate::durable::sibling(path, ".bak");
    let from_backup = || crate::durable::read_text(&backup).ok().flatten().and_then(|t| serde_json::from_str::<Ledger>(&t).ok());
    let damaged = || format!("The customer list file is damaged and was left untouched: {}", path.display());
    match crate::durable::read_text(path) {
        Ok(Some(text)) => match serde_json::from_str::<Ledger>(&text) {
            Ok(l) => Ok(l),
            Err(_) => from_backup().ok_or_else(damaged),
        },
        Ok(None) => Ok(from_backup().unwrap_or_default()),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => from_backup().ok_or_else(damaged),
        Err(e) => Err(format!("Couldn't read the customer list: {e}")),
    }
}

/// Crash-safe write (forced to disk), keeping the previous GOOD version as `.bak`.
/// A file that doesn't parse is never copied over the backup.
fn save(path: &Path, ledger: &Ledger) -> Result<(), String> {
    let text = serde_json::to_string_pretty(ledger).map_err(|e| e.to_string())?;
    if let Ok(Some(old)) = crate::durable::read_text(path) {
        if serde_json::from_str::<Ledger>(&old).is_ok() {
            let _ = crate::durable::write_atomic(&crate::durable::sibling(path, ".bak"), old.as_bytes());
        }
    }
    crate::durable::write_atomic(path, text.as_bytes()).map_err(|e| format!("Couldn't save the customer list: {e}"))
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

/// What the owner decided when the name already has a live computer.
#[derive(Debug, PartialEq)]
enum Seat {
    Unasked,
    /// The new code replaces an old one (the id, or None when only one is live).
    Migration(Option<String>),
    Extra,
}

#[derive(Debug)]
enum AddError {
    Refused(String),
    /// The name already has live computers (these ones): ask Migration or Extra.
    NeedsChoice(Vec<Customer>),
}

impl From<String> for AddError {
    fn from(m: String) -> Self {
        AddError::Refused(m)
    }
}

impl From<&str> for AddError {
    fn from(m: &str) -> Self {
        AddError::Refused(m.to_string())
    }
}

#[allow(clippy::too_many_arguments)]
fn add_customer(lic: &Licensing, seed: &[u8; 32], ledger: &mut Ledger, code: &str, name: &str, note: &str, days: i64, seat: &Seat) -> Result<(Customer, String), AddError> {
    let norm = norm_code(code);
    if norm.len() != 12 {
        return Err("The activation code is 12 letters and numbers, like ABCD-EFGH-JKLM.".into());
    }
    let name = clean_name(name)?;
    let note: String = note.trim().chars().take(200).collect();
    if !(1..=MAX_DAYS).contains(&days) {
        return Err(format!("A licence lasts 1 to {MAX_DAYS} days.").into());
    }
    if let Some(existing) = ledger.customers.iter().find(|c| c.code == norm) {
        return Err(format!("{} is already listed for {}. Use Renew on that row.", pretty_code(&norm), existing.name).into());
    }
    let key = name.to_lowercase();
    let live: Vec<usize> = ledger.customers.iter().enumerate().filter(|(_, c)| !c.retired && c.name.to_lowercase() == key).map(|(i, _)| i).collect();
    let mut retire = None;
    if !live.is_empty() {
        retire = match seat {
            Seat::Unasked => return Err(AddError::NeedsChoice(live.iter().map(|&i| ledger.customers[i].clone()).collect())),
            Seat::Extra => None,
            Seat::Migration(Some(id)) => match live.iter().find(|&&i| ledger.customers[i].id == *id) {
                Some(&i) => Some(i),
                None => return Err("The computer being replaced isn't in the list any more.".into()),
            },
            Seat::Migration(None) if live.len() == 1 => Some(live[0]),
            Seat::Migration(None) => return Err("Say which computer is being replaced.".into()),
        };
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
        retired: false,
    };
    let expires = today + Days::days(days);
    let token = issue_for(lic, seed, &c, expires)?;
    c.expires = Some(expires.to_string());
    c.last_issued = Some(today.to_string());
    c.licence = Some(token.clone());
    if let Some(i) = retire {
        ledger.customers[i].retired = true;
    }
    ledger.customers.push(c.clone());
    Ok((c, token))
}

/// Warning shown when a name already has live computers. The ledger is untouched.
fn seat_prompt(seats: &[Customer]) -> Value {
    let name = seats.first().map(|c| c.name.as_str()).unwrap_or_default();
    let latest = seats.iter().filter_map(|c| c.expires.as_deref()).max();
    let message = match (seats.len(), latest) {
        (1, Some(e)) => format!("{name} already has a computer, ending {e}. Is this a migration or an extra computer?"),
        (1, None) => format!("{name} already has a computer. Is this a migration or an extra computer?"),
        (n, Some(e)) => format!("{name} already has {n} computers, the latest ending {e}. Is this a migration or an extra computer?"),
        (n, None) => format!("{name} already has {n} computers. Is this a migration or an extra computer?"),
    };
    json!({
        "error": message,
        "needs_choice": true,
        "seats": seats.iter().map(|c| json!({"id": c.id, "code": pretty_code(&c.code), "expires": c.expires})).collect::<Vec<_>>(),
    })
}

fn find_mut<'a>(ledger: &'a mut Ledger, id: &str) -> Result<&'a mut Customer, String> {
    ledger.customers.iter_mut().find(|c| c.id == id).ok_or_else(|| "That customer isn't in the list any more.".to_string())
}

/// A retired computer was replaced in a migration and is never renewed.
fn refuse_retired(c: &Customer) -> Result<(), String> {
    if c.retired {
        return Err(format!("{} ({}) was retired in a migration and is never renewed.", c.name, pretty_code(&c.code)));
    }
    Ok(())
}

fn renew(lic: &Licensing, seed: &[u8; 32], ledger: &mut Ledger, id: &str, days: i64) -> Result<(Customer, String), String> {
    if !(1..=MAX_DAYS).contains(&days) {
        return Err(format!("A licence lasts 1 to {MAX_DAYS} days."));
    }
    let c = find_mut(ledger, id)?;
    refuse_retired(c)?;
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
    refuse_retired(c)?;
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
    if c.retired {
        return ("retired", None);
    }
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

fn customer_view(c: &Customer, month: &str, today: NaiveDate, computers: usize) -> Value {
    let (status, left) = status_of(c, today);
    json!({
        "id": c.id, "name": c.name, "code": pretty_code(&c.code), "note": c.note, "created": c.created,
        "expires": c.expires, "status": status, "days_left": left, "last_issued": c.last_issued,
        "licence": c.licence, "paid": c.paid, "paid_for_month": c.paid.iter().any(|m| m == month),
        "retired": c.retired, "computers": computers,
    })
}

fn list_view(ledger: &Ledger, month: &str, today: NaiveDate) -> Value {
    let counts = live_counts(ledger);
    // Totals and payment counts are per live computer; retired ones are history.
    let live: Vec<&Customer> = ledger.customers.iter().filter(|c| !c.retired).collect();
    let count = |f: &dyn Fn(&Customer) -> bool| live.iter().filter(|c| f(c)).count();
    let paid = count(&|c| c.paid.iter().any(|m| m == month));
    let status = |s: &'static str| count(&|c| status_of(c, today).0 == s);
    json!({
        "month": month,
        "summary": {
            "total": live.len(),
            "active": status("active") + status("expiring"),
            "expiring": status("expiring"),
            "expired": status("expired"),
            "paid_for_month": paid,
            "unpaid_for_month": live.len() - paid,
        },
        "customers": sorted(ledger).iter().map(|c| customer_view(c, month, today, counts.get(&c.name.to_lowercase()).copied().unwrap_or(0))).collect::<Vec<_>>(),
    })
}

/// Quoted CSV cell; a leading = + - @ is neutralised so Excel never runs it as a formula.
fn csv_cell(s: &str) -> String {
    let guarded = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{s}") } else { s.to_string() };
    format!("\"{}\"", guarded.replace('"', "\"\""))
}

fn export_csv(ledger: &Ledger, month: &str, today: NaiveDate) -> String {
    let counts = live_counts(ledger);
    let head = ["Name", "Activation code", "Note", "Added", "Licence expires", "Status", "Days left", "All paid months", &format!("Paid for {month}"), "Last licence issued", "Computers"];
    let mut out = String::from("\u{feff}");
    out.push_str(&head.iter().map(|h| csv_cell(h)).collect::<Vec<_>>().join(","));
    out.push_str("\r\n");
    for c in sorted(ledger) {
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
            if c.retired { String::new() } else { counts.get(&c.name.to_lowercase()).copied().unwrap_or(0).to_string() },
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
            let seat = match field(body, "seat").as_str() {
                "migration" => Seat::Migration(Some(field(body, "replaces")).filter(|id| !id.is_empty())),
                "extra" => Seat::Extra,
                _ => Seat::Unasked,
            };
            match with_lic(|l| add_customer(l, &seed, &mut ledger, &field(body, "code"), &field(body, "name"), &field(body, "note"), days, &seat)) {
                Ok((c, t)) => finish(Ok((c, Some(t))), &path, &ledger, &month, today),
                Err(AddError::Refused(e)) => err(400, e),
                Err(AddError::NeedsChoice(seats)) => (409, seat_prompt(&seats)),
            }
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
        let (c, token) = add_customer(&l, &SEED, &mut led, "abcd efgh jklm", "  Jane   Doe ", "Nairobi shop", 31, &Seat::Unasked).unwrap();
        assert_eq!(c.code, "ABCDEFGHJKLM");
        assert_eq!(c.name, "Jane Doe");
        assert_eq!(c.expires.as_deref(), Some("2026-11-15"));
        let data = l.read(&token, Some("ABCD-EFGH-JKLM")).unwrap();
        assert_eq!(data["user"], "Jane Doe");
        assert!(l.read(&token, Some("ZZZZ-ZZZZ-ZZZZ")).is_err());
        // same code again is refused and names who has it
        let dup = add_customer(&l, &SEED, &mut led, "ABCD-EFGH-JKLM", "Other", "", 31, &Seat::Unasked).unwrap_err();
        assert!(matches!(&dup, AddError::Refused(m) if m.contains("already listed for Jane Doe")), "{dup:?}");
        assert_eq!(led.customers.len(), 1);
    }

    #[test]
    fn bad_input_is_refused_without_changing_the_list() {
        let l = lic_at("2026-10-15");
        let mut led = Ledger::default();
        assert!(add_customer(&l, &SEED, &mut led, "ABC", "A", "", 31, &Seat::Unasked).is_err());
        assert!(add_customer(&l, &SEED, &mut led, "ABCD-EFGH-JKLM", "   ", "", 31, &Seat::Unasked).is_err());
        assert!(add_customer(&l, &SEED, &mut led, "ABCD-EFGH-JKLM", "A", "", 0, &Seat::Unasked).is_err());
        assert!(add_customer(&l, &SEED, &mut led, "ABCD-EFGH-JKLM", "A", "", 366, &Seat::Unasked).is_err());
        assert!(matches!(add_customer(&l, &[1u8; 32], &mut led, "ABCD-EFGH-JKLM", "A", "", 31, &Seat::Unasked), Err(AddError::Refused(m)) if m.contains("doesn't belong")));
        assert!(led.customers.is_empty());
    }

    #[test]
    fn ticking_a_month_issues_a_licence_to_month_end_plus_grace() {
        let l = lic_at("2026-10-15");
        let mut led = Ledger::default();
        let (c, _) = add_customer(&l, &SEED, &mut led, "ABCD-EFGH-JKLM", "Jane", "", 10, &Seat::Unasked).unwrap();
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
        let (c, _) = add_customer(&l, &SEED, &mut led, "ABCD-EFGH-JKLM", "Jane", "", 10, &Seat::Unasked).unwrap();
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
        let (c, _) = add_customer(&l2, &SEED, &mut led, "WXYZ-2345-6789", "Dec", "", 5, &Seat::Unasked).unwrap();
        let (c, _) = set_paid(&l2, &SEED, &mut led, &c.id, "2026-12", true).unwrap();
        assert_eq!(c.expires.as_deref(), Some("2027-01-05"));
    }

    #[test]
    fn renew_and_remove() {
        let l = lic_at("2026-10-15");
        let mut led = Ledger::default();
        let (c, _) = add_customer(&l, &SEED, &mut led, "ABCD-EFGH-JKLM", "Jane", "", 10, &Seat::Unasked).unwrap();
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
            let (c, _) = add_customer(&l, &SEED, &mut led, code, name, "", 30, &Seat::Unasked).unwrap();
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
        add_customer(&l, &SEED, &mut led, "ABCD-EFGH-JKLM", "=HYPERLINK(\"x\")", "say \"hi\", ok", 30, &Seat::Unasked).unwrap();
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
        add_customer(&l, &SEED, &mut led, "ABCD-EFGH-JKLM", "Jane", "", 30, &Seat::Unasked).unwrap();
        save(&path, &led).unwrap();
        save(&path, &led).unwrap(); // second save leaves a .bak
        assert!(path.with_extension("json.bak").exists());
        assert_eq!(load(&path).unwrap().customers[0].name, "Jane");
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(load(&path).unwrap().customers[0].name, "Jane", "a damaged file is read from the good .bak copy");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json", "reading never rewrites the damaged file");
        save(&path, &led).unwrap();
        assert!(std::fs::read_to_string(path.with_extension("json.bak")).unwrap().contains("Jane"), "a damaged file is never copied over the good backup");
        std::fs::write(&path, "{ not json").unwrap();
        std::fs::remove_file(path.with_extension("json.bak")).unwrap();
        assert!(load(&path).unwrap_err().contains("damaged"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json", "damaged file left as it was");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap(); // unreadable (a busy file behaves the same way): an error, not an empty list
        assert!(load(&path).unwrap_err().contains("Couldn't read"));
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

    fn add(l: &Licensing, led: &mut Ledger, code: &str, name: &str, seat: &Seat) -> Result<Customer, AddError> {
        add_customer(l, &SEED, led, code, name, "", 30, seat).map(|(c, _)| c)
    }

    #[test]
    fn same_name_on_another_code_asks_before_adding() {
        let l = lic_at("2026-10-15");
        let mut led = Ledger::default();
        add_customer(&l, &SEED, &mut led, "AAAA-AAAA-AAAA", "sme x", "", 26, &Seat::Unasked).unwrap(); // ends 2026-11-10
        // case and spacing do not hide the same name
        let Err(AddError::NeedsChoice(seats)) = add(&l, &mut led, "BBBB-BBBB-BBBB", "  SME   X ", &Seat::Unasked) else { panic!("expected a question") };
        assert_eq!(led.customers.len(), 1, "nothing is added until the owner answers");
        let v = seat_prompt(&seats);
        assert_eq!(v["error"], "sme x already has a computer, ending 2026-11-10. Is this a migration or an extra computer?");
        assert_eq!(v["seats"][0]["code"], "AAAA-AAAA-AAAA");
        // a refused code or days is reported before the question
        assert!(matches!(add(&l, &mut led, "BBB", "sme x", &Seat::Unasked), Err(AddError::Refused(_))));
        // a different name never asks
        assert!(add(&l, &mut led, "CCCC-CCCC-CCCC", "other", &Seat::Unasked).is_ok());
    }

    #[test]
    fn migration_retires_the_old_code_and_it_is_never_renewed() {
        let l = lic_at("2026-10-15");
        let mut led = Ledger::default();
        let old = add(&l, &mut led, "AAAA-AAAA-AAAA", "Jane", &Seat::Unasked).unwrap();
        set_paid(&l, &SEED, &mut led, &old.id, "2026-10", true).unwrap();
        let new = add(&l, &mut led, "BBBB-BBBB-BBBB", "Jane", &Seat::Migration(None)).unwrap();
        assert!(!new.retired);
        assert!(led.customers[0].retired && !led.customers[1].retired);
        assert!(renew(&l, &SEED, &mut led, &old.id, 30).unwrap_err().contains("never renewed"));
        assert!(set_paid(&l, &SEED, &mut led, &old.id, "2026-11", true).unwrap_err().contains("never renewed"));
        assert_eq!(led.customers[0].paid, vec!["2026-10"], "a refused tick records nothing");
        // un-ticking only corrects the record and stays possible
        assert!(set_paid(&l, &SEED, &mut led, &old.id, "2026-10", false).is_ok());
        // the new code renews normally
        assert!(renew(&l, &SEED, &mut led, &new.id, 30).is_ok());
        // the list: retired shown as such, counts and totals are live computers only
        let v = list_view(&led, "2026-10", d("2026-10-15"));
        assert_eq!(v["summary"]["total"], 1);
        assert_eq!(v["summary"]["unpaid_for_month"], 1);
        assert_eq!(v["customers"][0]["status"], "active", "a customer's live computers list before its retired ones");
        assert_eq!(v["customers"][1]["status"], "retired");
        assert_eq!(v["customers"][0]["computers"], 1);
        assert_eq!(v["customers"][1]["computers"], 1);
        // a name with only retired computers can be added again without a question
        led.customers[1].retired = true;
        assert!(add(&l, &mut led, "CCCC-CCCC-CCCC", "Jane", &Seat::Unasked).is_ok());
    }

    #[test]
    fn extra_computer_is_a_second_listed_seat() {
        let l = lic_at("2026-10-15");
        let mut led = Ledger::default();
        add(&l, &mut led, "AAAA-AAAA-AAAA", "Jane", &Seat::Unasked).unwrap();
        add(&l, &mut led, "BBBB-BBBB-BBBB", "jane", &Seat::Extra).unwrap();
        assert!(led.customers.iter().all(|c| !c.retired));
        let v = list_view(&led, "2026-10", d("2026-10-15"));
        assert_eq!(v["summary"]["total"], 2);
        assert_eq!(v["customers"][0]["computers"], 2);
        assert_eq!(v["customers"][1]["computers"], 2);
        let csv = export_csv(&led, "2026-10", d("2026-10-15"));
        assert!(csv.contains("\"Computers\"\r\n"));
        assert!(csv.matches(",\"2\"\r\n").count() == 2, "{csv}");
        // a third computer still has to be answered
        assert!(matches!(add(&l, &mut led, "CCCC-CCCC-CCCC", "Jane", &Seat::Unasked), Err(AddError::NeedsChoice(s)) if s.len() == 2));
    }

    #[test]
    fn migration_with_several_live_computers_must_name_the_one_replaced() {
        let l = lic_at("2026-10-15");
        let mut led = Ledger::default();
        let a = add(&l, &mut led, "AAAA-AAAA-AAAA", "Jane", &Seat::Unasked).unwrap();
        let b = add(&l, &mut led, "BBBB-BBBB-BBBB", "Jane", &Seat::Extra).unwrap();
        assert!(matches!(add(&l, &mut led, "CCCC-CCCC-CCCC", "Jane", &Seat::Migration(None)), Err(AddError::Refused(_))));
        assert!(matches!(add(&l, &mut led, "CCCC-CCCC-CCCC", "Jane", &Seat::Migration(Some("nope".into()))), Err(AddError::Refused(_))));
        assert_eq!(led.customers.len(), 2);
        assert!(led.customers.iter().all(|c| !c.retired), "a refused migration retires nothing");
        add(&l, &mut led, "CCCC-CCCC-CCCC", "Jane", &Seat::Migration(Some(b.id.clone()))).unwrap();
        let retired: Vec<&str> = led.customers.iter().filter(|c| c.retired).map(|c| c.id.as_str()).collect();
        assert_eq!(retired, vec![b.id.as_str()]);
        assert!(!led.customers.iter().find(|c| c.id == a.id).unwrap().retired);
    }

    #[test]
    fn a_list_saved_before_seats_existed_still_loads() {
        let old = r#"{"customers":[{"id":"1","name":"Jane","code":"ABCDEFGHJKLM","created":"2026-10-01","expires":"2026-11-01"}]}"#;
        let led: Ledger = serde_json::from_str(old).unwrap();
        assert!(!led.customers[0].retired);
        assert_eq!(list_view(&led, "2026-10", d("2026-10-15"))["customers"][0]["computers"], 1);
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
