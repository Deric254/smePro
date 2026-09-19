//! Terms & Conditions text, version, and per-user acceptance tracking.
//!
//! DericBI is the licensor of record here, not "smePro" — smePro is a
//! product of DericBI (the same relationship as Meta owning Facebook,
//! Instagram, WhatsApp: one legal entity behind several products), so
//! the text below names DericBI throughout.
//!
//! See http_api.rs's `GET /terms` (serves TEXT/VERSION, no auth — a
//! user must be able to read this before they've logged in) and
//! `POST /terms/accept` (records acceptance, auth required) routes,
//! and the two `/auth/*login` routes, which report acceptance status
//! the moment a session is created (see `accepted_current` below) so
//! the frontend can block on it right after login.

use anyhow::Result;
use rusqlite::{params, Connection};

/// Bump this, and ONLY this, whenever TERMS_TEXT below changes.
/// db_migrations.rs's v37 added `users.terms_accepted_version` for
/// exactly this reason: bumping this string is what makes every
/// user's prior acceptance stale — including ones who already
/// accepted an older version — and routes them through the acceptance
/// screen again on their very next login. See `accepted_current`.
pub const TERMS_VERSION: &str = "2026-09-19";

/// PLACEHOLDER LEGAL TEXT. Not reviewed by a lawyer — do not treat as
/// final or production-ready. Every `[PLACEHOLDER: ...]` marker below
/// is a real fact this document is missing (DericBI's exact registered
/// legal name/jurisdiction, a real support contact) that must be
/// filled in, and the whole document needs a Kenyan lawyer's review,
/// before this is relied on for real users. See this feature's own
/// engineering brief for the full list.
pub const TERMS_TEXT: &str = r#"smePro Terms & Conditions

Version: 2026-09-19

These Terms & Conditions ("Terms") are an agreement between you and
DericBI [PLACEHOLDER: exact registered legal name and jurisdiction of
incorporation, if not Kenya] ("DericBI", "we", "us", "our"), the
company that develops and licenses smePro. smePro is one of DericBI's
products, not a separate legal entity. By accepting these Terms you
agree to them on behalf of yourself and, if you were given access by
your employer, on behalf of your use of smePro for that business.

1. What smePro is
   smePro is a business-management application (point of sale,
   inventory with batch and expiry tracking, purchasing, and basic
   accounting) intended for small and medium businesses, schools,
   SACCOs, NGOs, and healthcare organizations.

2. Your account
   Each individual user of smePro has their own account within their
   business. You are responsible for keeping your login credentials
   confidential and for activity that happens under your account.

3. Your data
   The business data you enter into smePro (inventory, sales,
   customers, and similar records) belongs to your business, not to
   DericBI. It is stored locally, encrypted at rest, on the device
   smePro is installed on.

4. The AI assistant feature
   If you use smePro's AI assistant, the questions you type and
   relevant business figures needed to answer them are sent to
   Google's Gemini API to generate a response. On Gemini's free tier,
   Google's own terms permit Google to use that data to improve their
   services. Do not use the AI assistant to ask about information you
   do not want processed under those terms. This is disclosed here
   because it is a real transfer of data outside smePro and outside
   DericBI's own systems, not a hypothetical one.

5. Data protection
   Where the Kenyan Data Protection Act, 2019 applies to your use of
   smePro, DericBI will handle personal data processed by the app
   consistently with it. [PLACEHOLDER: confirm any additional
   data-protection commitments, and whether a separate Privacy Policy
   document is also required.]

6. Acceptable use
   You agree not to use smePro to store or process data you do not
   have the right to hold, and not to attempt to circumvent its
   security, licensing, or access controls.

7. Warranty disclaimer
   smePro is provided "as is". [PLACEHOLDER: DericBI's actual warranty
   position — generic "as is" language is a placeholder only, not
   confirmed legal advice for this business.]

8. Limitation of liability
   [PLACEHOLDER: DericBI's actual liability position. Software that
   touches accounting and inventory data for real businesses typically
   needs specific, deliberate limitation-of-liability language, not a
   generic clause — get a lawyer's input before relying on this
   section.]

9. Changes to these Terms
   DericBI may update these Terms from time to time. If the terms
   change, every user will be asked to accept the new version the next
   time they log in before continuing to use smePro.

10. Governing law
    These Terms are governed by the laws of Kenya [PLACEHOLDER:
    confirm this is correct for DericBI's actual registration].

11. Contact
    [PLACEHOLDER: real support/contact email or address for DericBI.]

By selecting "I accept" below, you confirm that you have read and
agree to these Terms.
"#;

/// Whether the given user has accepted the CURRENT version of the
/// terms — not just "accepted at some point in the past". A version
/// bump (see TERMS_VERSION) makes every prior acceptance stale on
/// purpose: someone who accepted an older version has not agreed to
/// whatever changed since.
pub fn accepted_current(conn: &Connection, user_id: &str) -> Result<bool> {
    let accepted_version: Option<String> = conn.query_row(
        "SELECT terms_accepted_version FROM users WHERE id = ?1",
        params![user_id],
        |r| r.get(0),
    )?;
    Ok(accepted_version.as_deref() == Some(TERMS_VERSION))
}

/// Records that the given user has accepted the CURRENT version, now.
pub fn accept(conn: &Connection, user_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE users SET terms_accepted_at = datetime('now'), terms_accepted_version = ?1 WHERE id = ?2",
        params![TERMS_VERSION, user_id],
    )?;
    Ok(())
}
