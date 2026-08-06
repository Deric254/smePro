# Integration changelog

This merges the "SME Pro Complete Integration" bundle (receipts, invoices,
business branding, 2FA, tax engine, currency exchange, session security,
crash reporting, Android foreground service) into your existing app.

Verification performed in this environment: `cargo check` **and**
`cargo test` both pass clean (0 errors, 0 warnings, 10/10 tests green) on
every non-Tauri backend file, using a real Rust toolchain against a real
in-memory SQLite/SQLCipher database. `npx tsc -b`, `npx vite build`, and
`npx oxlint` all pass clean on the full frontend. The one thing I could
**not** run here is a full `cargo build` of the Tauri desktop binary
itself (GTK/webview stack) — this sandbox has no display and an old
system Rust that can't resolve today's crates.io graph for that part.
Your existing CI (`.github/workflows`) already does full desktop builds,
so that step is covered there.

## Real bugs found in the integration bundle and fixed

These are not style preferences — each one would have broken the build
or broken behavior at runtime if pasted in as-shipped:

1. **`db_migrations.rs` — invalid SQL, would break every app startup.**
   `DEFAULT datetime('now')` is not valid SQLite; function-call defaults
   must be parenthesized: `DEFAULT (datetime('now'))`. This appeared in
   the `_schema_version` table (the very first thing migrations create)
   and in the `sessions.last_activity` column migration. Confirmed by
   reproducing the exact syntax error against a real SQLite connection.
   Since `db::open()` calls migrations on every startup, this would have
   made the app fail to open its database at all. Fixed both spots.

2. **`totp.rs` — three separate API mismatches with totp-rs 5.7.2:**
   - `Secret::generate_secret()` requires the `gen_secret` cargo feature;
     the bundle's Cargo.toml only requested `qr` (which doesn't imply it).
   - `TOTP::get_uri()` doesn't exist; the real method is `get_url()`.
   - `TOTP::new()` with 7 arguments (issuer + account name) requires the
     `otpauth` feature to even be visible in scope; without it only the
     5-arg version compiles, giving a "wrong number of arguments" error.
   - Fixed by changing the `totp-rs` feature flags to `["gen_secret",
     "otpauth"]` (dropping the heavier, unnecessary `qr`/qrcodegen-image
     dependency — nothing here renders a QR image, the frontend just
     shows the `otpauth://` URL text) and correcting the two call sites.

3. **`totp.rs` — secret round-tripping bug (functional, not a compile
   error).** The stored secret is base32-*encoded* text (via
   `.to_encoded()`), but `build_totp()` reconstructed it with
   `Secret::Raw(secret.as_bytes().to_vec())`, which treats the encoded
   text's own ASCII bytes as the secret instead of decoding it first.
   That would generate codes that never match what a real authenticator
   app produces after scanning the same secret. Fixed to
   `Secret::Encoded(secret.to_string())`.

4. **`totp.rs` — wrong type on a nullable-column check.** `existing:
   Option<String>` was declared for a `query_row(...).ok()` over a
   nullable column, then `.flatten()` was called on it — `.flatten()`
   needs `Option<Option<_>>`. Fixed the type to `Option<Option<String>>`.

5. **`totp_api.txt`'s proposed 2FA login flow didn't compile at all** —
   it called `auth::create_session(conn, temp_token, &biz)` (that
   function didn't exist) and `totp::verify_login(conn, temp_token,
   code)` (wrong signature — real one takes a user id, not a token).
   Rather than patch around it, I refactored properly: `auth::login`
   split into `verify_password` (password check only) + `create_session`
   (issues a token for an already-verified user), plus a new short-lived
   in-memory pending-token store in `totp.rs`. A real session is never
   created until both factors pass. No schema changes needed.

6. **`crash_report.rs` — wrong return type.** `ureq::AgentBuilder::build()`
   returns `Agent` directly in ureq 2.10.1, not `Result<Agent, _>`; the
   bundle wrapped it in a `match Ok/Err`, which doesn't type-check.
   Fixed in both `send_to_sentry` and `send_to_webhook`.

7. **`business_branding.rs` — missing trait import.** Called
   `base64::engine::general_purpose::STANDARD.decode(...)` without
   `use base64::Engine;` in scope — `.decode()` is a trait method, not
   inherent. Added the import.

8. **`android_service.rs` — lifetime error.** `start_server_platform`
   moved its `addr: &str` parameter into `std::thread::spawn`, which
   requires `'static`. Changed to `&'static str` (matches how it's
   actually called, with a string literal).

9. **`currency.rs`'s `RateRecord` and `invoice.rs`'s `InvoiceItem`** only
   derived `Deserialize`, but both get serialized back out (rates
   returned to the frontend; invoice items re-serialized into storage).
   Added `Serialize` to both.

10. **`tests/mod.rs` never declared `mod common;`**, so every test file's
    `use super::common::*;` failed to resolve, and none of the shipped
    tests (auth, CRUD, POS checkout, receipts) could even compile, let
    alone run. Added the missing module declaration. All 10 tests now
    pass, including checkout stock deduction, oversell blocking, and
    receipt generation against real order data.

11. **Frontend: `ReceiptView.tsx`, `InvoiceView.tsx`, `BusinessBranding.tsx`,
    `TwoFactorSetup.tsx`** called an undefined `token()` function (or,
    in two files, a locally-defined one reading the wrong localStorage
    key — the app stores the session under `erp_token`, not `token`).
    Exported a `getToken()` accessor from `api.ts` and fixed all four
    call sites — this would have been a silent, always-401 auth failure.

## Everything else that had to be wired, not just copied in

- `db.rs`: runs `db_migrations::run()` right after the base schema on
  every open.
- `lib.rs`: registers all 10 new modules; switches from the plain
  `std::thread::spawn` HTTP server startup to
  `android_service::start_server_platform` (keeps the API alive when
  backgrounded on Android; unchanged behavior on desktop); initializes
  crash reporting (off by default — no DSN configured, by design).
- `http_api.rs`: session-inactivity expiry check, request body size
  limit, security response headers, and all new routes (receipts,
  invoices, branding + logo upload/serving, 2FA setup/verify/status/
  disable, tax rates + compute, currency rates/convert/refresh, and a
  new `POST /modules/:id/enable` route — needed because "invoice" isn't
  part of any business-type preset, so there was previously no way to
  turn it on after initial setup).
- `users.rs` / setup flow: swapped the old `password.len() < 8` checks
  for the shared `security::validate_password` policy.
- Frontend: mobile CSS wired globally; print CSS scoped into Receipt/
  Invoice views; `AdminPanel.tsx` gained a **Business** tab (branding +
  "Enable Invoices" toggle) and a **Security** tab (2FA setup); POS
  checkout screen gained a **Print receipt** button; `Login.tsx` gained
  the second-step 2FA code-entry screen so turning on 2FA doesn't lock
  anyone out at their next login.

## Known gaps / things to decide before shipping

- **Crash reporting has no DSN configured** (intentionally — see
  `lib.rs`). It's fully wired and inert until you set one.
  `crash_report::flush_queue()` also isn't called anywhere yet; wire it
  in once you have a real endpoint to send queued reports to.
- **TOTP secrets are stored in plaintext** in the `users` table
  (`totp.rs`'s own comments claim AES-256-GCM encryption, but the code
  doesn't actually encrypt — this is a pre-existing gap in the bundle,
  not something I introduced or was asked to fix). Worth encrypting at
  rest before relying on this in production.
- No tests shipped for the new tax/currency/invoice/totp modules
  specifically (only auth/CRUD/POS/receipt were covered). The logic was
  reviewed by hand and cross-checked against real schema/column names,
  but isn't exercised by an automated test the way the older modules are.
- `ModuleView` renders "invoice" generically once enabled (same as any
  other module) — there's no dedicated "click a row to open the full
  printable InvoiceView" wiring yet; that component is built and ready,
  just not hooked into a click handler.
