//! Customer capture and lifetime value.
//!
//! Deliberately optional at every step: a cashier types a name and/or
//! phone at checkout only if the customer offers one, or doesn't. Most
//! sales stay fully anonymous, same as before this existed — this only
//! activates for the ones where real customer details were actually
//! given.
//!
//! Phone is the PREFERRED identity key when given — reliably unique
//! per person in practice. Name alone is also accepted (see
//! `find_or_create` below) for cashiers/businesses that don't ask for
//! a phone number — genuinely weaker matching, since a name isn't
//! reliably unique ("John" shows up a lot), and this is a deliberate,
//! disclosed trade-off, not a claim that name-only tracking is just as
//! reliable as phone-based tracking. See schema.sql's own comment on
//! the customers table for the same point.
//!
//! Lifetime value is NOT a stored, cached number — it's computed fresh
//! on every read, by summing the real sales records matching this
//! customer. A cached total would need updating every time a sale
//! happens, gets refunded, or gets corrected — and any missed update
//! point means a wrong number silently sitting there. Computing it
//! live means it can never disagree with the actual transaction
//! history, by construction, not by discipline.

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use uuid::Uuid;

/// Strips common formatting a cashier might type — spaces, dashes,
/// dots, parentheses — while keeping digits and a leading `+`, so
/// "0712 345 678" and "0712-345-678" resolve to the SAME customer
/// instead of silently creating a second record and splitting their
/// real lifetime value across two entries. This is deliberately NOT
/// full E.164/country-code canonicalization (turning a local number
/// into its international form needs a real phone-number library and
/// country context this app doesn't have) — it only removes the
/// formatting noise that varies between two typings of the exact same
/// number, nothing that requires guessing intent.
pub fn normalize_phone(phone: &str) -> String {
    phone
        .trim()
        .chars()
        .enumerate()
        .filter(|(i, c)| c.is_ascii_digit() || (*i == 0 && *c == '+'))
        .map(|(_, c)| c)
        .collect()
}

/// Finds an existing customer by phone (if given) or by name alone
/// (if not), or creates one. Called from POS checkout and service
/// sales — never called directly by an HTTP route, since a customer
/// record should only ever originate from an actual sale. At least
/// one of `name`/`phone` must be non-empty; both being absent is a
/// caller error, not a silent no-op.
///
/// Phone match takes priority whenever a phone is given, and updates
/// the stored name if a different one was given this time (people's
/// names get typed slightly differently visit to visit; phone is the
/// stable identity there). When no phone is given, matching falls
/// back to name alone, case-insensitively — "Asha" and "asha" resolve
/// to the same phone-less customer, but "Asha" and "Aisha" do not
/// (this is intentionally NOT fuzzy matching; only case is folded).
pub fn find_or_create(conn: &Connection, business_id: &str, name: Option<&str>, phone: Option<&str>) -> Result<String> {
    let phone = phone.map(normalize_phone).filter(|p| !p.is_empty());
    let name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());

    if phone.is_none() && name.is_none() {
        return Err(anyhow!("a name or phone number is required to identify a customer"));
    }

    if let Some(phone) = &phone {
        let existing: Option<String> = conn
            .query_row(
                "SELECT id FROM customers WHERE business_id = ?1 AND phone = ?2",
                params![business_id, phone],
                |r| r.get(0),
            )
            .optional()?;

        if let Some(id) = existing {
            if let Some(n) = &name {
                conn.execute(
                    "UPDATE customers SET name = ?1, updated_at = datetime('now') WHERE id = ?2",
                    params![n, id],
                )?;
            }
            return Ok(id);
        }

        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO customers (id, business_id, name, phone, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'), datetime('now'))",
            params![id, business_id, name, phone],
        )?;
        return Ok(id);
    }

    // Name-only path — matched case-insensitively against other
    // phone-less customers only (see idx_customers_name_only, a
    // partial index scoped to `WHERE phone IS NULL`), so this can
    // never accidentally collide with or shadow a phone-tracked
    // customer who happens to share a name.
    let name = name.expect("checked above: at least one of name/phone is present");
    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM customers WHERE business_id = ?1 AND phone IS NULL AND LOWER(name) = LOWER(?2)",
            params![business_id, name],
            |r| r.get(0),
        )
        .optional()?;

    if let Some(id) = existing {
        return Ok(id);
    }

    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO customers (id, business_id, name, phone, created_at, updated_at)
         VALUES (?1, ?2, ?3, NULL, datetime('now'), datetime('now'))",
        params![id, business_id, name],
    )?;
    Ok(id)
}

/// The JOIN condition shared by `list` and `detail` below: a
/// phone-tracked customer matches sales by phone; a phone-less
/// customer matches by name instead (case-insensitively, same rule as
/// `find_or_create`), restricted to sales that ALSO have no phone
/// recorded — a sale that did include a phone belongs to whichever
/// phone-tracked customer owns that phone, never to a same-named
/// phone-less record.
const SALE_MATCH_CONDITION: &str = "
    (
        (c.phone IS NOT NULL AND s.customer_phone = c.phone)
        OR
        (c.phone IS NULL AND (s.customer_phone IS NULL OR s.customer_phone = '') AND LOWER(s.customer) = LOWER(c.name))
    )
";

/// Lists every customer with a real purchase, sorted by lifetime value
/// (highest first) — the naturally useful default: a business owner
/// glancing at this list sees their best customers first, not an
/// arbitrary alphabetical or chronological order.
pub fn list(conn: &Connection, business_id: &str) -> Result<Value> {
    let sales_table = crate::crud::load_module(conn, business_id, "sales")
        .map(|m| m.table_name())
        .unwrap_or_else(|_| "sales_records".to_string());

    let sql = format!(
        "SELECT c.id, c.name, c.phone, c.created_at,
                COALESCE(SUM(s.revenue), 0) as lifetime_value,
                COUNT(s.id) as order_count,
                MAX(s.created_at) as last_purchase_at
         FROM customers c
         LEFT JOIN {sales_table} s ON s.business_id = c.business_id AND s.deleted_at IS NULL AND {SALE_MATCH_CONDITION}
         WHERE c.business_id = ?1
         GROUP BY c.id
         ORDER BY lifetime_value DESC"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![business_id], |r| {
        Ok(json!({
            "id": r.get::<_, String>(0)?,
            "name": r.get::<_, Option<String>>(1)?,
            "phone": r.get::<_, Option<String>>(2)?,
            "customer_since": r.get::<_, String>(3)?,
            "lifetime_value": r.get::<_, i64>(4)?,
            "order_count": r.get::<_, i64>(5)?,
            "last_purchase_at": r.get::<_, Option<String>>(6)?,
        }))
    })?;

    let customers: Vec<Value> = rows.filter_map(|r| r.ok()).collect();
    Ok(json!({ "customers": customers }))
}

/// Full detail for one customer — their info plus every purchase,
/// most recent first, so clicking into a customer answers "what did
/// they buy, and when" directly, not just "how much have they spent."
pub fn detail(conn: &Connection, business_id: &str, customer_id: &str) -> Result<Value> {
    let (name, phone, since): (Option<String>, Option<String>, String) = conn
        .query_row(
            "SELECT name, phone, created_at FROM customers WHERE id = ?1 AND business_id = ?2",
            params![customer_id, business_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|_| anyhow!("customer not found"))?;

    let sales_table = crate::crud::load_module(conn, business_id, "sales")
        .map(|m| m.table_name())
        .unwrap_or_else(|_| "sales_records".to_string());

    let sql = format!(
        "SELECT item_name, quantity, revenue, order_id, created_at
         FROM {sales_table} s
         WHERE s.business_id = ?1 AND s.deleted_at IS NULL AND (
             (?2 IS NOT NULL AND s.customer_phone = ?2)
             OR
             (?2 IS NULL AND (s.customer_phone IS NULL OR s.customer_phone = '') AND LOWER(s.customer) = LOWER(?3))
         )
         ORDER BY created_at DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![business_id, phone, name], |r| {
        Ok(json!({
            "item_name": r.get::<_, String>(0)?,
            "quantity": r.get::<_, i64>(1)?,
            "revenue": r.get::<_, i64>(2)?,
            "order_id": r.get::<_, Option<String>>(3)?,
            "date": r.get::<_, String>(4)?,
        }))
    })?;
    let purchases: Vec<Value> = rows.filter_map(|r| r.ok()).collect();
    let lifetime_value: i64 = purchases.iter().filter_map(|p| p.get("revenue").and_then(|r| r.as_i64())).sum();

    Ok(json!({
        "id": customer_id,
        "name": name,
        "phone": phone,
        "customer_since": since,
        "lifetime_value": lifetime_value,
        "order_count": purchases.len(),
        "purchases": purchases,
    }))
}

/// Searches existing customers by partial name or phone match, for
/// the POS/service-sale "does this person already exist?" lookup —
/// this is the actual, structural fix for duplication: rather than
/// relying on normalization to reconcile two different typings of the
/// same customer after the fact, the cashier can see and click the
/// existing record WHILE typing, before a near-duplicate ever gets
/// created. Deliberately returns id + name + phone only, not lifetime
/// value or purchase history — this runs on every keystroke behind a
/// debounce, so it stays cheap, and a cashier picking a customer mid-
/// sale doesn't need to see their spending history to do it.
///
/// Matches phone against the SAME normalized form `find_or_create`
/// stores (see normalize_phone) — searching "0712-345" for a customer
/// stored as "0712345678" needs to normalize the search term the same
/// way the stored value was normalized, or the LIKE would silently
/// never match despite the number clearly being right.
pub fn search(conn: &Connection, business_id: &str, query: &str) -> Result<Vec<Value>> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let name_pattern = format!("%{query}%");
    let normalized_phone = normalize_phone(query);

    // The phone clause is only included when there's actually a digit
    // in the search term — normalize_phone("Asha") strips every
    // character and returns "", and `phone LIKE '%%'` would then match
    // EVERY customer who has any phone at all, regardless of whether
    // their name matches. A name-only search must stay name-only.
    let (sql, phone_pattern) = if normalized_phone.is_empty() {
        ("SELECT id, name, phone FROM customers
          WHERE business_id = ?1 AND name LIKE ?2
          ORDER BY updated_at DESC LIMIT 8", None)
    } else {
        ("SELECT id, name, phone FROM customers
          WHERE business_id = ?1 AND (name LIKE ?2 OR phone LIKE ?3)
          ORDER BY updated_at DESC LIMIT 8", Some(format!("%{normalized_phone}%")))
    };

    let mut stmt = conn.prepare(sql)?;
    let rows = if let Some(phone_pattern) = &phone_pattern {
        stmt.query_map(params![business_id, name_pattern, phone_pattern], |r| {
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "name": r.get::<_, Option<String>>(1)?,
                "phone": r.get::<_, Option<String>>(2)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
    } else {
        stmt.query_map(params![business_id, name_pattern], |r| {
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "name": r.get::<_, Option<String>>(1)?,
                "phone": r.get::<_, Option<String>>(2)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
    };
    rows.map_err(Into::into)
}
