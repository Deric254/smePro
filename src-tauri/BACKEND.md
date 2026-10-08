# Backend

Rust library crate `core_engine` plus the `sme-pro` Tauri binary. The
library is the whole application; `main.rs` only starts Tauri and the
local HTTP server (`http_api.rs`, `127.0.0.1:8080`, bearer-token sessions).

## Design rules

- **Money is exact integer minor units** (`money.rs`). Release builds keep
  `overflow-checks = true`, so an overflow fails loudly instead of wrapping.
- **Stock starts at zero and only changes through defined workflows**:
  receiving, checkout, refund, repack, stock take, expired write-off. Every
  change is written to the stock movement ledger.
- **Batches are consumed FEFO** (first expired, first out). Expired stock is
  never sellable. Each batch carries its own cost and selling price; there
  is no default price.
- **Bookkeeping reconciles to the cent**: sales, purchases, refunds and
  rounding differences post to the ledger automatically.
- **Every authenticated action is permission-checked** (`rbac.rs`); a few
  structural actions are Owner-only.
- **Data is encrypted at rest** (SQLCipher) and stays on the device. The
  only outbound traffic is the AI assistant (user-configured provider),
  optional crash reports, exchange-rate refresh, and update checks.
- **Schema changes are migrations** (`db_migrations.rs`), idempotent and
  transactional. Removed features get a migration that drops their tables.

## Modules (`src/`)

| File | Purpose |
|---|---|
| `ai_assistant.rs` | AI assistant provider layer (provider chosen in Admin > AI Settings) |
| `ai_chat.rs` | Persisted AI chat sessions |
| `ai_context.rs` | Business snapshot given to the AI assistant |
| `android_service.rs` | Android background execution |
| `audit.rs` | Audit log |
| `auth.rs` | Login, sessions, password recovery |
| `backup.rs` | Backup and restore |
| `basket_analysis.rs` | "Frequently bought together" analysis |
| `batches.rs` | Batch-costed inventory with FEFO consumption |
| `business_branding.rs` | Logo and slogan |
| `business_panel.rs` | Business creation and module enablement |
| `business_pulse.rs` | Computed performance readout |
| `crash_report.rs` | Optional crash reporting (off unless configured; opt-out setting) |
| `crud.rs` | Generic schema-driven record create/read/update/delete |
| `currency.rs` | Exchange rates and conversion |
| `customers.rs` | Customer capture and lifetime value |
| `db.rs` | Database open (SQLCipher) and connection setup |
| `db_migrations.rs` | Versioned, transactional schema migrations |
| `debt_settlement.rs` | Settling debts and credits |
| `excel_import.rs` | Excel bulk import with templates |
| `forecast.rs` | Module forecasting |
| `http_api.rs` | The local HTTP API and routing |
| `installer.rs` | Android APK handoff |
| `invoice.rs` | Invoices (auto-created on every sale) |
| `module.rs` | Module schema loading and validation |
| `money.rs` | Money as integer minor units |
| `onboarding.rs` | Business-type presets |
| `pos.rs` | Point of sale and service-sale checkout |
| `profit.rs` | Gross profit |
| `rate_limit.rs` | Request rate limiting |
| `rbac.rs` | Role-based access control |
| `receipt.rs` | Receipts (original as-sold amounts plus refund disclosure) |
| `receiving.rs` | Receiving stock into batches |
| `reference_data.rs` | Units and currencies master data |
| `refund.rs` | Refunds |
| `refund_analysis.rs` | Refund rate by item |
| `repack.rs` | Repacking bulk stock |
| `report.rs` | Reporting and slicing engine |
| `report_highlights.rs` | Dashboard teasers |
| `roles.rs` | Role and permission management |
| `rollback.rs` | Roll back to a previous release |
| `sales_patterns.rs` | Day-of-week and hour-of-day sales patterns |
| `security.rs` | Request size limits, hardening helpers |
| `settings.rs` | Per-business settings |
| `stock_health.rs` | Slow movers, expiring batches, stock runway |
| `stock_movement.rs` | Append-only stock movement ledger |
| `stock_take.rs` | Guided physical counts and expired write-off |
| `terms.rs` | Terms & Conditions text, version and per-user acceptance |
| `users.rs` | User lifecycle |
| `xlsx_export.rs` | Excel export |

Module schemas (fields, validation, permissions) live in `modules/*.json`:
inventory, sales, purchasing, refunds, accounting, debt_credit, invoice.

## API

Routes are matched in `http_api.rs`. Groups: `/setup/*` (first run),
`/auth/*`, `/terms`, `/modules/*` (schema-driven CRUD, import, export,
report), `/pos/*`, `/inventory/*` (batches, movements, repack, stock take),
`/sales/*` (profit and pattern reports, refunds), `/purchasing/*`,
`/debt_credit/*`, `/customers/*`, `/invoices/*`, `/ai/*`, `/currency/*`,
`/roles`, `/users`, `/settings`, `/business/*`, `/admin/*`, `/audit-log`.

## Tests and tools

```
cargo test --lib                              # full suite
cargo run --bin demo_seed --features dev-tools    # seed demo data
```

Dev binaries (`demo_seed`, `empty_server`, `check_query_plan`) require the
`dev-tools` feature, which is never enabled in release builds.
