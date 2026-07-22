# core-engine — Phase 1 scaffold

This is a real, compiling, running Rust project — not pseudocode. It was
built and tested in a sandbox (compiled with `cargo build`, executed with
`cargo run`, output verified) before being handed to you.

## What's here
- `schema.sql` — the core tables every business/tenant needs regardless of
  which modules are enabled: businesses, modules, users, roles, permissions,
  audit_log, licenses, admin_recovery.
- `modules/inventory.json` — an example module definition. This is the
  **only** thing you write to add a new module (Sales, HR, POS, ...): a JSON
  file describing fields, actions, and default role permissions.
- `src/module.rs` — reads a module JSON file and generates a real SQL table
  for it at runtime (`CREATE TABLE module_<id> (...)`), validates records
  against the schema before they're written. This is the "no code changes to
  add a module" mechanism.
- `src/db.rs` — opens the local SQLite file and applies the core schema.
- `src/rbac.rs` — the single choke point for permission checks
  (`rbac::require(conn, user_id, module_id, action)`), plus a seeder that
  turns a module's `default_roles` into real role/permission rows.
- `src/audit.rs` — append-only audit logging, called after every write.
- `src/main.rs` — a working end-to-end demo: creates a business, loads the
  inventory module from JSON, seeds roles, creates an Owner and a Staff user,
  does an RBAC-gated insert with an audit entry, then proves a Staff user is
  correctly blocked from deleting.

## Verified working (already run in sandbox)
```
Created business: 081e94c1-...
Materialized table: module_inventory
Created user 'nia' with Owner role
Inserted inventory record e9cfdcef-..., audit entry logged
RBAC correctly blocked staff: permission denied: user ... cannot 'delete' on module 'inventory'
```

## To run it yourself
```
cargo build
cargo run
```
This creates `erp.db` in the project folder — delete it to start fresh.

## Phase 2 — Business Panel (added)
`src/business_panel.rs` is the admin-facing operations layer the Business
Panel UI will call into:
- `create_business()` — onboarding, auto-creates the built-in Owner role
- `update_branding()` — logo, currency, tax rate (partial updates)
- `enable_module()` / `disable_module()` — idempotent toggles; **disabling
  never drops data**, it only hides the module and can be reversed
- `list_modules()` — what the panel's module-toggle screen reads
- `add_user()` — creates a user under an existing role (caller must pass
  an already-hashed password — this function never sees plaintext)
- `set_admin_recovery_code()` — stores/rotates the admin master recovery
  code (hash only)

Verified in this run: business created → branding set → module enabled →
users created under Owner/Staff roles → RBAC allows Owner, blocks Staff →
module disabled (table confirmed still present, data intact) → re-enabled
→ RBAC still correctly enforced afterward.

## Phase 3 — Generic CRUD + Local REST API (added)
`src/crud.rs` — generic create/list/update/delete that works against
**any** enabled module by reading its schema back out of the `modules`
registry table at request time (not from a hardcoded struct). This is the
actual proof that "no code changes to add a module" holds: inventory,
sales, HR, whatever — same four functions handle all of them.
- `create()` — validates, applies field defaults, inserts, audits
- `list()` — pagination + free-text search across all text fields,
  generated dynamically from the module's field list
- `update()` — partial update of any subset of fields, rejects unknown
  field names, audits an old→new diff
- `delete()` — **soft delete only** (`deleted_at` timestamp) — an owner's
  data is never actually destroyed by a DELETE call, only hidden, and the
  audit trail records exactly what disappeared and when

`src/http_api.rs` — a local REST API (`tiny_http`) exposing this over
`http://127.0.0.1:8080/modules/{module_id}/records`. This is what the
Tauri/React desktop shell and Android app will both call — no UI touches
SQLite directly, which is what keeps every module consistent.
**Auth is a placeholder** (`X-User-Id` / `X-Business-Id` headers) — real
session-token auth lands in Phase 4.

### Verified with real HTTP requests (curl), not just unit logic
- `POST .../inventory/records` → created two real rows
- `GET .../inventory/records?search=Rice` → returned exactly the matching
  record (a genuine bug — all search clauses collided on the same SQL
  parameter index — was caught here and fixed)
- `PUT .../inventory/records/{id}` → partial update applied
- `DELETE .../inventory/records/{id}` → soft-deleted, disappeared from
  subsequent list, but the module table itself was untouched
- `POST` as a Staff user (read-only role) → correctly `403 permission denied`

## Phase 4 — Auth, Password Recovery, Licensing Lock (added)
`src/auth.rs` — real Argon2id password hashing (no more plaintext
placeholders), session tokens (12h expiry), and the two-step forgot-
password ladder:
1. Security questions (both must match, answers are normalized so
   "Rex" / " rex " / "REX" all match, and are themselves hashed — never
   stored in plaintext)
2. Admin master recovery code (last resort, hash-only storage)

Both recovery paths reset the password **and revoke every existing
session** for that user — a stolen device's session dies the moment the
real owner recovers their account.

`src/license.rs` — the monetization engine:
- `activate()` — one-time activation fee starts the first 30-day cycle
- `record_payment()` — extends from `max(today, current_due_date)`, so
  early payment doesn't shorten a cycle and late payment doesn't stack
  penalty days
- `check_status()` — Active / Grace (5-day warning window) / Locked
  (grace elapsed) / Inactive (never activated)
- `require_export_allowed()` — the **only** thing gated by payment status.
  Core business operations (create/read/update/delete) keep working
  through Grace and Locked — an SME never gets locked out of running
  their shop, only out of exporting data, matching what was agreed on
  earlier in the process.

`src/http_api.rs` — replaced the placeholder header-based "auth" with
real bearer tokens (`Authorization: Bearer <token>`), added
`/auth/login`, `/auth/logout`, `/auth/recover/security-questions`,
`/auth/recover/admin-code`, `/license/activate`, `/license/pay`,
`/license/status`, and a license-gated `/modules/{id}/export`.

### Two real bugs found and fixed while testing this phase
1. **Multi-tenant schema bug**: `modules.id` was a global `PRIMARY KEY`
   instead of scoped per business, so a second business could never also
   have an "inventory" module — surfaced the moment two businesses
   existed in the same database. Fixed to `PRIMARY KEY (business_id, id)`.
2. Confirmed (not a bug, but verified): password reset correctly revokes
   the old session — tested by logging in, resetting the password, and
   proving the pre-reset token then returns 401.

### Verified with real HTTP requests
- Login success / wrong-password rejection (same generic error either way,
  so login can't be used to enumerate usernames)
- Export blocked (402) before activation
- Activation → export succeeds
- Simulated 2-days-overdue (via direct DB update, restarting the server to
  mimic a real elapsed-time scenario) → status correctly reports
  `grace, days_left: 3`; export blocked with a clear message; list/create
  continue to work normally
- Payment → status returns to `active`, export unblocked again
- Security-question recovery: wrong answers rejected, correct answers
  (with whitespace/case variance) succeed, old session dies, new password
  logs in, old password no longer works
- Admin-code recovery: wrong code rejected, correct code succeeds

## Phase 5 — Reporting & Slicer Engine (added)
`src/report.rs` — the generic pivot layer: any enabled module can be
sliced by **dimension × measure** without module-specific code.
- **Measures**: `sum` / `count` / `avg` over any numeric field
- **Dimensions**:
  - `None` — one grand total
  - `Time` — buckets by day / week / month / quarter / year over any
    date/datetime field (quarter is derived manually since SQLite has no
    native quarter function)
  - `Category` — groups by any field (branch, customer, product, etc.),
    sorted by value descending
- Optional date-range filtering when using a time dimension
- Validates every field name and type against the module's own schema
  before building SQL — an invalid field or a `sum` on a text field
  fails with a clear error instead of a confusing SQL error

`src/xlsx_export.rs` — real `.xlsx` file generation (via
`rust_xlsxwriter`), not CSV-with-a-different-extension: styled header
row, auto-sized columns, proper cell typing (numbers stay numbers).
Used for both raw module data export and report export.

New endpoints in `src/http_api.rs`:
- `GET /modules/{id}/report?agg=&measure=&dimension=&field=&bucket=&start=&end=`
  — **free to view, no license required**. Reports are how an owner
  makes decisions day to day; gating that behind payment would undercut
  the whole point of the system.
- `GET /modules/{id}/report/export?...` — same params, returns a real
  `.xlsx` file — **license-gated**, same rule as raw data export.
- `GET /modules/{id}/export` — now returns a real `.xlsx` file (was JSON
  in Phase 4).

A second module, `modules/sales.json`, was added specifically to prove
the reporting engine is genuinely generic — it was written once against
`inventory` and worked immediately against `sales` with zero changes.

### Verified with real HTTP requests
- Seeded 4 sales records across 2 branches, 3 customers
- Grand total revenue: **96.0** (48+15+24+9 — checks out)
- Revenue by branch: Town 63.0, Estate 33.0 (checks out)
- Revenue by customer: John 57.0, Peter 24.0, Mary 15.0 (checks out)
- Count of sales by day: correctly bucketed to today's date
- `sum` on a non-numeric field → clean rejection, not a SQL crash
- Unknown field name → clean rejection
- Downloaded both `/export` and `/report/export` as actual files,
  confirmed with `file` (reports "Microsoft Excel 2007+") and by opening
  the ZIP container and checking for real `xl/` internal structure — not
  faked
- Report viewing works without an active license; report/data export
  correctly returns 402 without one — confirms viewing and exporting are
  independently gated, matching the "always let them see their own data,
  only export costs money" design

## Phase 6 — AI Floating Assistant (added)
`src/forecast.rs` — local statistical forecasting, **not** an LLM asked
to "do math": moving average and exponential smoothing over any module's
time-bucketed measure (reuses the Phase 5 reporting engine for the
history series). The AI's job is to explain these numbers, never to
invent them.

`src/ai_context.rs` — builds a bounded, structured snapshot of the
business's current state across every enabled module: record counts,
sums of every numeric field, and a low-stock alert list (for any module
that happens to define both `quantity` and `reorder_level` fields — the
one place this builder leans on a naming convention, because that
pattern is so common in SME inventory data). Deliberately summarizes
rather than dumping raw rows — cheaper, faster, and the model can't lose
track of what matters in a pile of individual records.

`src/ai_assistant.rs` — calls a real AI provider, grounding the system
prompt in the context snapshot above so answers are based on the
business's actual data, not general knowledge. **Supports three
providers, chosen by the `AI_PROVIDER` env var:**
- **`nvidia_nim` (default)** — genuinely free, no credit card, ~40
  requests/min, OpenAI-compatible API. Serves DeepSeek, Llama, and 80+
  other models. Get a key at https://build.nvidia.com. This is the
  sensible default for a system built to be free until you're making
  money.
- **`gemini`** — Google's free tier (Flash / Flash-Lite models), no
  credit card. Note: on the free tier, Google's terms allow using your
  prompts to improve their models — worth flagging to a business owner
  if the data is sensitive. Get a key at https://aistudio.google.com.
- **`claude`** — paid, no free tier, included since it's Anthropic's own
  model and may be worth it once the business is generating revenue.

Without any key set, the default (`nvidia_nim`) returns a clear
"not configured, here's how to get a free key" message — every other
feature in the app keeps working regardless. See `.env.example` for the
full list of variables.

New endpoints:
- `GET /ai/context` — the raw snapshot, with no LLM call. Doubles as a
  transparency feature ("what does the AI see about my business?") and
  as a way to verify the context builder independent of network access.
- `POST /ai/ask {"question": "..."}` — the floating-button endpoint.
- `GET /modules/{id}/forecast?measure=&bucket=&method=&window=&alpha=`

The floating button's icon/branding was already solved in Phase 2 — it
reads `businesses.logo_path`, same as every other branded UI element.

### Two real problems found and fixed while testing this phase
1. `native-tls`'s ureq feature flag alone wasn't enough — had to build an
   explicit `ureq::Agent` wired to a `native_tls::TlsConnector`, or every
   HTTPS call failed with "no TLS backend configured". Found by actually
   trying to reach the API, not by reading the ureq docs and assuming.
2. (Environment-specific, not app code) `rustls`'s dependency chain
   required a newer Rust than this sandbox's apt-installed 1.75, so this
   build uses `native-tls` (via system OpenSSL) instead — noted here so
   it isn't a surprise on a machine with a newer Rust where rustls would
   "just work" and might seem preferable.

### Verified with real requests — including a genuine live API call
- Forecast (moving average and exponential smoothing) computed correctly
  from seeded sales data
- Forecast on a non-existent field rejected cleanly
- `/ai/ask` with no provider configured → clear 502 with actionable
  message pointing to the free NVIDIA NIM signup, not a crash
- `/ai/ask` against **Claude with a deliberately fake key** → the
  request reached the real `api.anthropic.com`, got a genuine `401
  authentication_error` response back, correctly parsed and surfaced.
  This confirms the whole pipeline (TLS, request shape, headers, JSON
  body, response parsing) against a real Anthropic server.
- **NVIDIA NIM and Gemini paths compile and are structurally complete
  but could not be live-tested from this particular sandbox** — its
  network egress is locked to a fixed domain allowlist (for crate
  registries, GitHub, and `api.anthropic.com` specifically) and returned
  `403` when reaching `integrate.api.nvidia.com` /
  `generativelanguage.googleapis.com` directly. On your own machine,
  with no such restriction, these should work — test them there before
  relying on them, the same way you would test any new integration.
- `/ai/context` snapshot manually checked: correctly flagged cooking oil
  (qty 3 ≤ reorder level 10) as low stock while leaving rice (qty 40)
  off the list; totals summed correctly across both seeded modules

## Phase 7 — Remaining Modules (added)
Four new modules, added as **pure JSON, zero engine code changed**:
`modules/hr.json`, `modules/accounting.json`, `modules/purchasing.json`,
`modules/debt_credit.json`. The debt/credit ledger in particular was
called out early in the design as something most "professional" ERPs
miss — informal credit is central to how SME trade actually works.

This phase is the strongest proof yet of the module-as-schema promise:
every one of CRUD, RBAC, audit logging, reporting/slicers, real `.xlsx`
export, and the AI context snapshot worked on all four new modules
**the first time they were run**, with literally nothing touched in
`crud.rs`, `report.rs`, `rbac.rs`, `xlsx_export.rs`, or `ai_context.rs`.

### Verified with real HTTP requests
- HR: created a staff record, listed it back correctly
- Accounting: logged one income (96.0) and one expense (22.5) entry,
  then sliced by `entry_type` in one report call — correct split with
  zero accounting-specific reporting code
- Purchasing: created a purchase order record
- Debt/Credit: logged both directions (owed to us / we owe), sliced by
  `direction` — correct totals (we_owe: 90.0, owed_to_us: 30.0)
- `/ai/context` snapshot now spans all six enabled modules automatically
- Downloaded a real `.xlsx` export of the accounting module, confirmed
  as a genuine Excel file

### Not built as separate modules (deliberately)
- **POS** — the `sales` module from Phase 5 already covers point-of-sale
  recording; a dedicated POS module would mostly duplicate it. A real
  POS *screen* (fast checkout UI) is a frontend concern, not a new
  backend module.
- **Expiry tracking** — `inventory.json` already has an `expiry_date`
  field; the AI context builder's low-stock logic could be extended to
  flag near-expiry items the same way it flags low stock, as a small
  follow-up rather than a new module.

## Phase 8 — Usability Layer (partial — see honest scope note below)

`src/onboarding.rs` — the onboarding wizard. A business type (`retail`,
`food`, `services`, `manufacturing`) maps to a preset module list;
applying it just calls the existing (idempotent) `enable_module()` for
each one. No separate wizard state machine needed — the whole "wizard"
is a lookup table plus a loop.

`src/notifications.rs` — WhatsApp/SMS engine with a swappable provider,
same pattern as the AI assistant:
- **`log` (default)** — records every notification to the `notifications`
  table and marks it "sent (logged only)". Needs **zero external
  accounts**, which is what makes the whole notification system usable
  and testable immediately, and gives an owner a visible history either way.
- **`twilio`** — real WhatsApp/SMS delivery via Twilio. Requires
  `TWILIO_ACCOUNT_SID`, `TWILIO_AUTH_TOKEN`, `TWILIO_FROM_NUMBER` (see
  `.env.example`). For WhatsApp specifically, the from-number must be
  WhatsApp-enabled in the Twilio console — an account setup step outside
  what code can do.

`send_low_stock_alert()` reuses the exact same context builder as the AI
assistant (`ai_context::build_snapshot`) — one source of truth for
"what's low," used by both features, so they can never disagree with
each other.

New endpoints:
- `POST /onboarding/setup {"business_type": "..."}`
- `POST /notifications/send {"channel","recipient","message"}`
- `POST /notifications/low-stock-alert {"channel","recipient"}`
- `GET /notifications` — transparency log of everything sent

### Verified with real requests
- Onboarding: `food` preset correctly enabled exactly
  `[inventory, sales, purchasing, debt_credit, accounting]`
- Onboarding: invalid business type rejected with a clear list of valid
  options, not a crash
- Notification: sent and logged correctly with the default (no-account)
  provider
- Notification: invalid channel rejected cleanly
- Low-stock alert: composed from real seeded data — correctly reported
  "Cooking oil 1L: 2.0 left (reorder at 10.0)"
- `/notifications` list correctly shows both sent messages with full
  history

### Honest scope note — what's NOT done in this phase
The original Phase 8 list was WhatsApp/SMS, voice input, local language
support, onboarding wizard, OCR import, and multi-device sync. Built:
**onboarding wizard** and **WhatsApp/SMS** (structurally complete,
Twilio path untestable from this sandbox for the same domain-allowlist
reason as NVIDIA/Gemini in Phase 6). Deliberately **not** built here,
because they're a different kind of work than what's been done so far:
- **Voice input** — mostly a frontend concern (Web Speech API or a
  device's native speech-to-text) feeding transcribed text into the
  same `POST /modules/{id}/records` endpoint that already exists. No new
  backend needed once there's a UI.
- **Local language support** — a UI string-table/i18n concern, not a
  backend one; the API already returns raw data with no hardcoded
  display text.
- **OCR import** — the one genuinely new backend capability left. Needs
  an OCR engine (e.g. Tesseract) which is a nontrivial system dependency
  to wire up properly, and is more honestly scoped as its own follow-up
  than squeezed in alongside everything above.
- **Multi-device local sync** — the biggest of the remaining pieces
  architecturally (conflict resolution, not just data transfer) and
  deserves dedicated design time rather than a bolt-on here.

## OCR Import (added)

`src/ocr_import.rs` — the "import from existing chaos" feature from the
original usability brainstorm: photograph a paper ledger, extract text
via Tesseract, propose candidate records for review before anything is
actually created. Shells out to the `tesseract` CLI rather than a Rust
binding crate — simpler, more robust, and avoids yet another large
native-dependency tree for one feature.

Deliberately conservative by design: `parse_into_candidates()` **never**
auto-creates anything. It tags each candidate with `confident_fields`
(numbers matched to numeric fields) versus fields it filled in
best-effort, and — critically — `missing_required_fields`: whatever the
module requires that the source document simply didn't contain (a SKU
code almost never appears on a handwritten ledger). This is what lets a
review screen ask for exactly the right thing instead of the import
silently failing later.

New endpoints: `POST /import/ocr/extract` (image → raw text),
`POST /import/ocr/parse` (raw text + module → candidate records),
`POST /modules/{id}/records/bulk` (the "confirm and import" step —
also generally useful for CSV-style imports, not just OCR).

### Two real bugs found and fixed while testing this
Generated an actual test image (`Rice 20kg    40    24.00` etc.) and ran
genuine Tesseract OCR on it — which, like any real OCR, wasn't perfect
(`24.00` → `2400`, `Sugar2kg` lost its space, `3.50` → `350`) — and
pushed the output through the full pipeline to actual record creation:

1. **First pass**: free text landed in `sku` (the first text field in
   inventory's schema order) instead of `name` — technically not wrong,
   practically unhelpful. Fixed by preferring a field literally named
   `name`/`description`/`item_name`/`full_name` when the module has one.
2. **Second pass**: `quantity` (typed `integer`) was rejected by the
   existing field validator with "expected type integer but got
   Number(40.0)" — the parser always emitted JSON floats, even for
   whole numbers. Fixed by checking each numeric field's declared type
   and emitting a JSON integer vs. float to match.

Both were caught because the test ran the **entire** pipeline to actual
database rows, not just the OCR extraction step in isolation — a
narrower test would have looked like it worked and failed later, in
front of a real user.

### Verified end-to-end, for real
- Real Tesseract OCR on a real generated image, not a canned string
- Parse correctly flagged `sku` as `missing_required_fields` on every
  candidate (since it genuinely wasn't in the source document) rather
  than silently failing at import time
- Simulated the review step a human would do (typing in the missing
  SKUs), then bulk-created — `{"created": 3, "errors": []}`
- Confirmed all 3 records actually present in `inventory` afterward,
  with correct types (integer `quantity`, real `unit_cost`)



## Payment Gateway Integration (added)

`src/payment.rs` — real Stripe and M-Pesa (Safaricom Daraja) integration,
converging on the same `license::activate()` / `license::record_payment()`
functions used everywhere else, so activation vs. renewal logic can
never drift between providers or diverge from the manual `/license/pay`
path used in earlier testing.

**Stripe**: creates a real Checkout Session via the Stripe API, returns
the checkout URL for the frontend to redirect to. Webhook handler
verifies Stripe's HMAC-SHA256 signature (`Stripe-Signature` header)
using a manual constant-time comparison (not just `==`, which can leak
timing information about how many bytes matched) before trusting
anything in the payload.

**M-Pesa**: real Daraja OAuth + STK Push (the "enter your PIN" prompt
that appears directly on a customer's phone) — genuinely the right
payment method for this market, not Stripe-by-default box-ticking.
Safaricom's callback doesn't echo back which business a payment
belongs to — only the `CheckoutRequestID` we generated at push time —
which is exactly why `payment_intents` exists: a durable table mapping
provider references back to (business, purpose), checked and consumed
(never a second time — replay protection) by both webhook handlers.

New endpoints: `POST /payments/checkout` (authenticated, initiates
either flow), `POST /payments/webhook/stripe` and
`POST /payments/webhook/mpesa` (deliberately public — these are called
directly by Stripe/Safaricom, authenticated by their own
signature/reference scheme instead of our bearer tokens), and
`GET /payments/history` (transparency).

### Verified
- Both `api.stripe.com` and Safaricom's Daraja sandbox confirmed
  directly (`curl`) to be blocked by this sandbox's network allowlist —
  checked, not assumed, the same way NVIDIA/Gemini were in the AI phase
- Every validation path tested clean: missing credentials, missing
  required fields (M-Pesa phone number), unknown provider — all return
  clear errors, never a crash
- With a deliberately fake Stripe key, confirmed the code correctly
  attempts the real API call and correctly surfaces the failure — this
  time the sandbox's own egress proxy blocked it before reaching
  Stripe's servers (unlike the Claude API test in Phase 6, where
  `api.anthropic.com` actually was reachable) — an important
  distinction, documented here rather than glossed over
- **Full webhook lifecycle genuinely tested end-to-end**: seeded a
  pending payment intent (via a small dev-only `seed_payment_intent`
  binary, since the database is now SQLCipher-encrypted and can't be
  poked at directly), constructed a **correctly HMAC-signed** Stripe
  webhook payload in Python using the same construction Stripe
  documents, sent it to the real running server, and watched the
  business's license status flip from `inactive` to `active` — a
  genuine proof of the whole activation path, not a mocked shortcut
- Forged signature → rejected. Correctly-signed-with-the-wrong-secret →
  rejected. Correct signature → accepted. **Replaying the same
  correctly-signed webhook a second time → rejected**, because the
  intent is no longer `pending` — confirms real replay protection, not
  just "it worked once"
- M-Pesa callback handler tested with both a successful
  (`ResultCode: 0`) and a cancelled/failed (`ResultCode: 1032`)
  Safaricom-shaped payload — both recorded with the correct outcome in
  payment history

## First-Run Setup (added)

Closes the gap flagged right after the frontend was first built: a
genuinely fresh install had nothing to log into. `business_panel.rs`
gained `any_business_exists()` and a real `generate_admin_code()`
(previously hardcoded in the dev seed binary) — random, ambiguity-free
characters (no `0`/`O`/`1`/`I`), shown to the owner exactly once.

`POST /setup/create-business` is the one deliberately public write
endpoint in the entire API — there's no user yet to authenticate as on
a fresh install — guarded by refusing to run a second time the moment
any business exists (`409`), so it can't be replayed to create a rogue
second business without authentication. Validates password length and
requires both security questions up front, rather than letting an
owner skip account recovery setup and regret it later.

`GET /setup/status` is what the frontend checks on launch to decide
between the first-run wizard and the normal login screen. Also added
`GET /business` — while building this, noticed the sidebar had a
**hardcoded** "Mama Nia General Store" left over from development,
which would have silently undermined the entire first-run flow for any
other business. Fixed to fetch the real name.

Frontend: `FirstRunSetup.tsx` — business info → owner account + security
questions → recovery code (with a mandatory "I've saved this" checkbox
before continuing, not just a dismissible dialog) → auto-login into the
freshly created business.

### Verified — including a full real-browser run through the entire wizard
- Fresh install: `/setup/status` correctly reports `has_business: false`
- Login before setup fails cleanly, not a crash
- Weak password (< 8 chars) rejected; missing security questions rejected
- Valid creation: returns business_id + a **real randomly-generated**
  admin code (confirmed different on each run, e.g. `AC-ER9R-9WND` and
  `AC-QFUL-MQZA` across two separate tests — not the old hardcoded
  placeholder)
- `retail` business type correctly enabled exactly its module preset
- **Attempting setup a second time correctly refused with 409** —
  the abuse guard works
- The freshly generated admin recovery code was then used to actually
  reset the owner's password — proving the full loop (generate → show
  once → hash → later redeem) works, not just the generation step
- **Full headless-browser run through the actual wizard UI**: welcome
  screen → business info → owner account + both security questions →
  recovery code screen (real generated code displayed, confirmed via
  the same assertion mechanism that would fail the test if it weren't
  genuinely present) → mandatory confirmation checkbox → auto-login →
  landed on the dashboard showing the **real** business name typed
  during setup ("Kanini Wholesalers"), not a leftover hardcoded one —
  zero API errors across the entire flow

## Integrity & Access-Control Audit (added)

A deliberate audit pass — not new features, going back through what
already existed and checking it actually holds up under "does RBAC
really gate everything, does the audit trail actually cover what
matters, does the UI actually reflect what a user can do." Found four
real gaps, all fixed and verified, not just reviewed.

### 1. Business-critical actions had zero role differentiation
Grepping for every place `rbac::require`/`is_allowed` was actually
called showed it was **only** wired into module CRUD (`crud.rs`) and
reporting (`report.rs`). Every other authenticated endpoint — meaning
any valid session, including the lowest-privilege Staff account —
could freely:
- Activate the license or record a payment (`/license/activate`, `/license/pay`)
- **Initiate a real Stripe/M-Pesa charge** (`/payments/checkout`) — the
  most serious of these
- Reconfigure which modules are enabled (`/onboarding/setup`)
- Send arbitrary WhatsApp/SMS to any number, at cost (`/notifications/send`)
- View payment history (`/payments/history`)

Fixed with two new `rbac.rs` primitives: `require_owner()` (exact-role
check against the built-in, undeletable "Owner" role every business
gets) and `require_role(&["Owner","Manager"])` (allow-list, for actions
that shouldn't be Owner-only but also shouldn't be Staff-accessible).
Applied to every endpoint above, plus `/import/ocr/parse` (now
correctly requires `create` permission on the *target* module — a
Staff account with read-only Inventory access can no longer get OCR to
propose Inventory records either).

**Verified with real Owner/Manager/Staff accounts** (added a Manager
test account specifically to check the Owner-or-Manager tier, not just
assume the allow-list logic worked because the Owner-only checks did):
Staff blocked from all six actions with a clear "restricted to Owner"
or "requires one of: Owner, Manager" message naming their actual role;
Manager correctly allowed into the Owner-or-Manager tier but still
blocked from the Owner-only actions; Owner allowed everywhere. OCR
parse tested against two modules with different Staff permissions
(allowed on `sales`, correctly `403` on `inventory`) — confirming the
gate is genuinely per-module, not a blanket check.

### 2. The audit log was written but never readable
`audit::log` calls existed since Phase 1 for module CRUD — but grepping
for any endpoint that reads `audit_log` back turned up **nothing**.
Data was being recorded with no way to ever look at it, which defeats
the entire point of an audit trail. Added `GET /audit-log`
(Owner-only, with `module_id` filtering and a capped `limit`), and
**extended audit coverage** to cover what the RBAC fix above revealed
was ungated: license activation/payment (both the manual endpoint *and*
the actual webhook-triggered path in `payment.rs`, so a real Stripe/
M-Pesa payment landing gets logged the same as a manual admin action),
payment checkout initiation, onboarding/module reconfiguration, data
exports (which module, how many records — export was previously
completely invisible to the audit trail despite being license-gated
and data-sensitive), and login success/failure (previously the only
record of a failed login attempt was the in-memory rate limiter, which
resets on every restart).

**Verified**: generated one of each event type against a real running
server, then confirmed every single one — including the webhook-style
distinction between `activate` (manual) vs `webhook_activate`, and a
`login_failed` entry correctly showing the attempted username with
`user_id: null` since no valid user was ever established — appeared
correctly in `GET /audit-log`, in the right order, filterable by
`module_id`, and correctly `403`'d for a Staff account trying to view it.

### 3. A real SQL bug caught before it ever ran
While building the audit-log endpoint's optional module filter, the
non-filtered query branch referenced a `LIMIT ?3` placeholder while
only ever passing 2 parameters — this is exactly the class of bug this
project has caught repeatedly by testing rather than assuming (the
Phase 3 search-parameter collision, the Phase 6 OCR type-coercion bugs).
Caught and fixed by inspection *before* the test run this time, rather
than by the test failing — a sign the pattern-matching for this kind
of mistake has gotten better over the course of this build, though the
discipline of testing everything regardless remains the actual safety net.

### 4. The frontend showed buttons that would fail
`ModuleView.tsx` derived which action buttons to show (`+ New`,
`Delete`, `Export to Excel`) from the module schema's `actions` field —
which is the module's theoretical capability list, identical for every
business regardless of who's looking at it. It was never checking what
*this specific logged-in user's role* actually permits. A Staff account
with read-only Inventory access would see a fully clickable "+ New"
button that would 403 the moment they used it — offering an action the
system already knew would fail.

Fixed by computing real per-user permissions server-side: `GET
/modules/{id}/schema` now also returns `my_permissions` — the subset of
`actions` this specific user's role actually grants, computed via the
same `rbac::is_allowed` used to enforce every CRUD call — and the
frontend now derives button visibility from that instead.

**Verified with a real headless-browser run, not just the API
response**: logged in as Staff, navigated to Inventory (read-only for
Staff) — confirmed both `+ New` and `Export to Excel` are genuinely
absent from the rendered page, not just disabled. Navigated to Sales
(Staff has `create` there) — confirmed `+ New` **is** present. The UI
now only ever offers an action it has already confirmed will succeed.

## Accuracy, Consistency & Speed Audit (added)

A second audit pass, on the remaining dimensions explicitly asked
about: accuracy of calculations, consistency of behavior, and speed.
Found three more real issues.

### 1. Every module table had zero indexes (speed)
`module.rs`'s `create_table()` generated the table but never an index —
meaning `crud::list`, `report::run`, `ai_context`'s totals, xlsx export,
and `forecast`'s history series were **all doing full table scans**,
every time, on the exact same `WHERE business_id = ? AND deleted_at IS
NULL` filter. Harmless at the record counts used in every test so far;
would have quietly gotten worse as a real business accumulated years of
data, and been genuinely annoying to diagnose later ("the app used to
be fast"). Added `CREATE INDEX ON module_<id>(business_id, deleted_at)`
alongside table creation.

**Verified, not assumed**: wrote a small dev tool
(`check_query_plan.rs`) that runs `EXPLAIN QUERY PLAN` against a real
module table and confirmed the output literally says `SEARCH
module_inventory USING INDEX idx_module_inventory_business` — not
`SCAN`. The index is genuinely used by SQLite's query planner, not just
present and ignored.

### 2. Aggregation accuracy — verified against an adversarial case, not just round numbers
Every report test so far used clean, round test data. Specifically
tested the classic floating-point trap (`0.1 + 0.2 != 0.3` in naive
floating point) combined with a negative refund: seeded `19.99, 0.1,
0.2, -5.50` and confirmed `SUM` returned exactly `14.79`, `AVG` exactly
`3.6975`, `COUNT` exactly `4` — SQLite's aggregate functions handle
this correctly, but it's the kind of thing worth actually checking
with adversarial numbers rather than assuming because round numbers
worked in every earlier test.

### 3. Error classification relied on a fragile, duplicated string match
`http_api::crud_error` decided between returning `403` and `400` by
checking whether the error message **started with the literal string
`"permission denied"`** — a magic string with no compile-time link to
where `rbac::require` actually constructs that message. Editing either
string independently later would have silently broken the
classification with no compiler warning. Fixed by centralizing it:
`rbac::PERMISSION_DENIED_PREFIX` is now a `pub const`, used both to
build the message in `rbac.rs` and to detect it in `http_api.rs` — the
two can no longer drift apart. Re-verified after the change that a
genuine permission denial still returns `403` and an ordinary
validation error still returns `400`, not the other way around.

### 4. A field-naming inconsistency that led to a real, more serious gap
Grepped every JSON field name returned by the API for stray camelCase
in an otherwise all-`snake_case` API — found `loggedOut` (fixed to
`logged_out`) and, separately, confirmed `ResultCode`/`ResultDesc` are
**correctly** camelCase because they're M-Pesa's own webhook-
acknowledgment contract, not this API's design, so those were correctly
left alone. Chasing the naming inconsistency down to its call site
surfaced something more important: **the frontend's "Sign out" button
never actually called `/auth/logout` at all** — it only cleared the
token from `localStorage`. The session stayed genuinely valid
server-side, usable by anyone with the old token, until it naturally
expired up to 12 hours later.

Fixed by wiring the frontend to actually call the backend logout
endpoint before clearing local state.

**Verified with a real browser, capturing the actual token and using it
directly against the API**: logged in, captured the session token from
`localStorage`, confirmed it worked (`200`), clicked the real "Sign
out" button in the UI, then made a direct API call with that *exact
same, unmodified* token — `401`. The session was genuinely invalidated
server-side by the click, not just hidden from the UI.

## Licensing Edge Cases (added)

A third, narrower pass specifically on `license.rs`'s date arithmetic,
done under an explicit commitment to enhance without breaking anything
already working — every change here was regression-tested against the
primary paths before being considered done, not just tested in isolation.

### Real bug: re-activating gave away free time
Unlike `record_payment()` (which deliberately extends from
`max(current_due, today)` specifically so paying early never shortens a
cycle), `activate()` had no equivalent protection — calling it a second
time unconditionally reset `next_due_date` to `today + 30`, regardless
of how much time remained on the current cycle.

**Confirmed with a direct, unambiguous test** (not inferred from status
labels, which show "active" either way and can't distinguish the two
cases): activated a business, used a dev-only tool to set its due date
to 5 days out (simulating being mid-cycle), re-activated, then **read
the actual stored due date back** — it had silently jumped to 30 days
out, a 25-day giveaway. Real-world exposure is low (the frontend's
"Activate" button only ever appears when status is exactly `inactive`,
so a normal user can't trigger this through the UI — only reachable via
direct API use) but the API itself should be correct regardless of what
today's frontend happens to expose.

**Fixed** by making `activate()` refuse to run if the license is
already activated, matching the real-world meaning of "activation" as
a one-time event rather than something safely repeatable.

**Caught and fixed a second-order risk before it shipped**: `activate()`
is also called from `payment.rs`'s webhook convergence point
(`apply_payment`). Naively applying the same guard there would mean a
duplicate "activation" payment webhook (e.g., a customer's checkout
started twice by accident) would make the webhook handler return an
error — and Stripe/Safaricom both retry failed webhooks indefinitely,
so that would create a permanent retry loop, not just a rejected
request. Fixed by having the webhook path gracefully fall back to
treating a duplicate activation payment as a renewal instead of erroring.

### Full regression suite run against this change
Four scenarios, all verified by directly reading the stored due date
(not just checking status labels), not just written and assumed:
1. **First-time activation via the manual endpoint** — unchanged,
   still sets due date to `today + 30`
2. **Re-activation via the manual endpoint** — now cleanly refused,
   due date correctly left untouched at its original value
3. **Duplicate activation payment via webhook** — acknowledged with
   `200` (not an error), due date correctly extended as a renewal from
   the existing due date rather than reset or ignored
4. **First-time activation via webhook, and a normal subscription
   renewal via webhook** — both untouched code paths, both confirmed
   still working exactly as before

### What was deliberately NOT changed
`today()` uses `Utc::now().date_naive()` — UTC calendar date, not the
business's own configured timezone (`businesses.timezone`, e.g.
"Africa/Nairobi"). This means the exact hour a license flips from
Active to Grace can be off by the business's UTC offset from what an
owner might expect as their local midnight. This is a real, if minor,
inconsistency — and it was deliberately **not touched**, because the
entire grace-period test suite from Phase 4 onward was built and
verified against UTC-based date arithmetic, and changing the timezone
basis now would risk subtly shifting boundary behavior across all of
that already-passing coverage for a low-severity, cosmetic-at-worst
issue. Flagging it here as a known, considered simplification rather
than silently fixing it in a way that could introduce a regression
no test in this suite would catch until it was too late.

## Module Schema Validation — Real SQL Injection Found and Closed (added)

The most significant finding across every audit pass so far. Module
field names and the module id are interpolated directly into raw SQL
strings throughout `module.rs` (`CREATE TABLE`), `crud.rs` (`SELECT`,
`INSERT`, `UPDATE`), and `report.rs` (`GROUP BY`) — none of it
parameterized, because SQL doesn't support parameterizing identifiers
the way it does values. Nothing had ever validated that a field name
from a module JSON file was actually safe to use that way.

### Proven exploitable, not just theoretical
Crafted a malicious module definition with a field name of
`x TEXT); DROP TABLE users; --` and loaded it directly through the real
`enable_module()` path (bypassing only the currently-nonexistent HTTP
endpoint for arbitrary module loading — the underlying engine
mechanism was tested, not a hypothetical). **The attack partially
succeeded**: it created a genuinely malformed, truncated `module_evil`
table missing several expected columns. It was only "caught" as an
accidental side effect of an unrelated piece of code (the index-
creation fix from the speed audit tripped over the table's missing
`deleted_at` column) — not by any actual protection. A differently-
crafted payload that didn't happen to omit a column the index needed
would have gone through completely undetected.

**Also found**: `create_table()` wasn't transactional — the malformed
table was left behind in the database even though the overall
operation reported failure to its caller, because `CREATE TABLE` had
already committed before the later step failed.

### Fixed at the root, not patched at each call site
Rather than trying to make every individual SQL-building function
"remember" to sanitize its inputs, added `validate_identifier()` in
`module.rs`, called once inside `ModuleDef::from_json_str()` — the
single choke point every module definition passes through before any
SQL is ever built from it. Rejects anything that isn't a plain
`[a-zA-Z_][a-zA-Z0-9_]*` identifier (no quotes, semicolons, spaces, SQL
keywords as special characters — nothing but letters, digits,
underscore), enforces a length cap, checks for duplicate field names,
and refuses any field trying to reuse a name the engine already uses
internally (`id`, `business_id`, `created_at`, `updated_at`,
`deleted_at`). Also made `create_table()` properly transactional
(`CREATE TABLE` + the index + the registry insert all-or-nothing), so
even an unrelated future failure partway through can't leave a
malformed table behind the way this one did.

### Full verification, both that the fix works and that nothing broke
- **The exact same attack, re-run against the fix**: cleanly rejected
  at parse time with a clear error, and — checked directly, not
  assumed — `module_evil` **does not exist in the database at all**
  this time (confirmed via `no such table: module_evil`, not the
  previous run's `no such column: deleted_at`, proving zero partial
  state, not just a different failure mode)
- Two additional adversarial cases: a field name with a space, and a
  field name colliding with a reserved column (`created_at`) — both
  cleanly rejected with specific, accurate error messages
- **Regression**: all six real, already-shipped modules (inventory,
  sales, hr, accounting, purchasing, debt_credit) still load
  correctly — none of their legitimate field names trip the new
  validation, confirmed by listing enabled modules after a normal seed
- **Regression**: full CRUD (create + list), reporting, and export all
  re-verified working end-to-end against a real module after the fix —
  not just "it compiles," the actual functional paths every earlier
  phase depended on

## Known gaps to close before this touches real business data
1. ~~Encryption at rest~~ — done in the Hardening phase (SQLCipher, see below).
2. ~~Password hashing~~ — done in Phase 4 (Argon2id).
3. ~~Admin recovery code~~ — done in Phase 4.
4. ~~License engine~~ — done in Phase 4.
5. **Real payment gateway integration**: `/license/pay` currently just
   records that a payment happened — it doesn't talk to Stripe/Razorpay/
   mobile money yet. That's a webhook handler calling `record_payment()`
   on a confirmed payment event.
6. ~~CSV/xlsx export~~ — done in Phase 5 (real .xlsx via rust_xlsxwriter).
7. ~~Rate limiting on login/recovery endpoints~~ — done in the
   Hardening phase (see below).
8. **To actually use the AI assistant**: copy `.env.example` to `.env`
   (or export the variables directly), get a **free** key from
   https://build.nvidia.com (default provider, no credit card), and run
   the server. Gemini and Claude are available as alternatives — see
   `.env.example`.
9. **CORS is wide open (`*`)** on the local API — acceptable since it
   only ever binds to `127.0.0.1` (not reachable from outside the
   device), but worth knowing if the binding address ever changes.

## Hardening Phase — Encryption at Rest + Rate Limiting (added)

Both items had sat in "Known gaps" since Phase 4. Closed properly, not
just noted again:

**Encryption at rest** (`src/db.rs`) — switched from bundled plain
SQLite to system-linked SQLCipher (`libsqlcipher-dev` via apt). A
32-byte random key is generated once per install into a `.key` file
next to the database (0600 permissions on Unix), and used with
SQLCipher's raw-key syntax — no password stretching needed since the
key is already high-entropy, not user-memorable. A deliberate
`PRAGMA key` + forced read-check on open means a wrong/missing key
fails immediately with a clear message, not a confusing "file is not a
database" error surfacing later from an unrelated query.

**Rate limiting** (`src/rate_limit.rs`) — a simple in-memory rolling-
window limiter (5 attempts / 15 minutes), applied to `/auth/login` and
both recovery endpoints, keyed so different users don't interfere with
each other and a successful login resets the count. Checked *before*
the expensive password-hashing work, so a lockout also saves real CPU.

### Verified
- `file` on the database now reports `data`, not `SQLite format 3` —
  confirmed genuinely encrypted, not just configured to be
- Full functional test (seed → login → create → list) passes identically
  with encryption active — zero regressions
- Corrupted/wrong key on an existing database produces the intended
  clear error, not a crash or generic failure
- 6 wrong-password attempts: first 5 return `401`, 6th returns `429`
  with a retry time; a *different* user logging in at the same time is
  completely unaffected; a correct login resets the counter so a
  legitimate user who fumbled their password isn't left near the limit
- Same pattern verified independently on admin-code recovery

### Honest scope note
This is a process-local, in-memory rate limiter — correct for a single-
tenant local desktop app, but it resets if the app restarts and doesn't
share state across multiple installs. That's the right scope for what
this is; a distributed rate limiter would be solving a problem this
architecture doesn't have.


## Why versions are pinned in Cargo.toml
The sandbox this was built in only had Rust 1.75 available (via `apt`,
since `rustup`'s own domain isn't reachable from here). A couple of
dependencies' newest releases require `edition2024`, which needs a newer
Rust. Versions are pinned to ones that build cleanly on 1.75. On your own
machine, install Rust normally via rustup (you'll get something newer) and
feel free to loosen these pins — just re-test `cargo build` after.
