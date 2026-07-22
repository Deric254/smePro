-- =========================================================
-- CORE ENGINE SCHEMA
-- These tables exist once, regardless of what modules are
-- enabled. Every module's own tables are generated at
-- runtime from its JSON definition (see module.rs).
-- =========================================================

PRAGMA foreign_keys = ON;

-- One row per SME tenant. In this local-first version there is
-- normally exactly one active business per install, but the schema
-- supports multiple in case of shared/kiosk installs.
CREATE TABLE IF NOT EXISTS businesses (
    id              TEXT PRIMARY KEY,          -- uuid
    name            TEXT NOT NULL,
    logo_path       TEXT,
    currency        TEXT NOT NULL DEFAULT 'USD',
    tax_rate        REAL NOT NULL DEFAULT 0.0,
    timezone        TEXT NOT NULL DEFAULT 'UTC',
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

-- Registry of module definitions installed on this system.
-- `schema_json` is the full field/workflow definition (see modules/*.json).
-- `enabled` is what the Business Panel toggles.
CREATE TABLE IF NOT EXISTS modules (
    id              TEXT NOT NULL,              -- e.g. "inventory", "sales" — unique PER BUSINESS, not globally
    business_id     TEXT NOT NULL REFERENCES businesses(id) ON DELETE CASCADE,
    display_name    TEXT NOT NULL,
    schema_json     TEXT NOT NULL,             -- raw module definition
    enabled         INTEGER NOT NULL DEFAULT 1,-- 0/1
    table_created   INTEGER NOT NULL DEFAULT 0,-- has its data table been generated?
    created_at      TEXT NOT NULL,
    PRIMARY KEY (business_id, id)
);

CREATE TABLE IF NOT EXISTS users (
    id              TEXT PRIMARY KEY,
    business_id     TEXT NOT NULL REFERENCES businesses(id) ON DELETE CASCADE,
    username        TEXT NOT NULL,
    password_hash   TEXT NOT NULL,
    security_q1     TEXT,
    security_a1_hash TEXT,
    security_q2     TEXT,
    security_a2_hash TEXT,
    role_id         TEXT NOT NULL,
    active          INTEGER NOT NULL DEFAULT 1,
    created_at      TEXT NOT NULL,
    UNIQUE(business_id, username)
);

CREATE TABLE IF NOT EXISTS roles (
    id              TEXT PRIMARY KEY,
    business_id     TEXT NOT NULL REFERENCES businesses(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,             -- e.g. "Owner", "Cashier", "Accountant" — fully user-defined, nothing beyond "Owner" itself is a fixed name anywhere in the engine
    is_system       INTEGER NOT NULL DEFAULT 0,-- system roles can't be deleted (e.g. Owner)
    can_administer  INTEGER NOT NULL DEFAULT 0,-- grants the "admin tier" (payments history, notifications, settings, reference data) WITHOUT being Owner — a capability flag an Owner toggles per role, not a hardcoded role name like "Manager"
    UNIQUE(business_id, name)
);

-- One row per (role, module, action) permission grant.
-- action is one of: create, read, update, delete, export, approve
CREATE TABLE IF NOT EXISTS permissions (
    id              TEXT PRIMARY KEY,
    role_id         TEXT NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    module_id       TEXT NOT NULL,
    action          TEXT NOT NULL,
    UNIQUE(role_id, module_id, action)
);

-- Append-only. Never UPDATE or DELETE rows here.
CREATE TABLE IF NOT EXISTS audit_log (
    id              TEXT PRIMARY KEY,
    business_id     TEXT NOT NULL,
    user_id         TEXT,
    module_id       TEXT NOT NULL,
    action          TEXT NOT NULL,             -- create/update/delete/export/login/...
    record_id       TEXT,
    details_json    TEXT,                      -- before/after snapshot or extra context
    timestamp       TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS licenses (
    business_id     TEXT PRIMARY KEY REFERENCES businesses(id) ON DELETE CASCADE,
    activated       INTEGER NOT NULL DEFAULT 0,
    activation_date TEXT,
    last_paid_date  TEXT,
    next_due_date   TEXT,
    status          TEXT NOT NULL DEFAULT 'inactive', -- inactive/active/grace/locked
    license_token   TEXT
);

CREATE TABLE IF NOT EXISTS admin_recovery (
    business_id     TEXT PRIMARY KEY REFERENCES businesses(id) ON DELETE CASCADE,
    admin_code_hash TEXT NOT NULL,
    generated_at    TEXT NOT NULL
);

-- Real login sessions. A token here is a bearer credential — treat rows
-- in this table as secrets. expires_at is checked on every request.
CREATE TABLE IF NOT EXISTS sessions (
    token           TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    business_id     TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    expires_at      TEXT NOT NULL
);

-- Outbound WhatsApp/SMS queue. Every notification is recorded here
-- regardless of whether a real provider is configured — this is what
-- lets the system work (and be testable) with zero external accounts,
-- and gives an owner a visible log of what was sent to whom.
CREATE TABLE IF NOT EXISTS notifications (
    id              TEXT PRIMARY KEY,
    business_id     TEXT NOT NULL,
    channel         TEXT NOT NULL,       -- 'whatsapp' | 'sms'
    recipient       TEXT NOT NULL,       -- phone number
    message         TEXT NOT NULL,
    status          TEXT NOT NULL,       -- 'queued' | 'sent' | 'failed'
    provider_response TEXT,
    created_at      TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_audit_business_time ON audit_log(business_id, timestamp);
CREATE INDEX IF NOT EXISTS idx_permissions_role ON permissions(role_id);

-- Tracks every payment attempt initiated through Stripe or M-Pesa, so
-- when their webhook/callback arrives — often with nothing more
-- identifying than a session ID or CheckoutRequestID — we can look up
-- which business and which purpose (activation vs. monthly renewal) it
-- belongs to.
CREATE TABLE IF NOT EXISTS payment_intents (
    id                  TEXT PRIMARY KEY,
    business_id         TEXT NOT NULL,
    provider            TEXT NOT NULL,       -- 'stripe' | 'mpesa'
    provider_reference  TEXT NOT NULL,       -- Stripe session id / M-Pesa CheckoutRequestID
    purpose             TEXT NOT NULL,       -- 'activation' | 'subscription'
    amount              REAL NOT NULL,
    currency            TEXT NOT NULL,
    status              TEXT NOT NULL DEFAULT 'pending', -- pending/completed/failed
    created_at          TEXT NOT NULL,
    completed_at        TEXT
);
CREATE INDEX IF NOT EXISTS idx_payment_intents_reference ON payment_intents(provider_reference);

-- Vendor-issued license key redemption (see vendor_license.rs and the
-- separate vendor-authority/ service). This is a distinct mechanism from
-- the recurring payment license above — it's the classic "one key, one
-- device" model, where the vendor issues a key out-of-band and this
-- install redeems it exactly once. Both tables are single-row-per-device
-- (id = 1) by design: a device_id is generated once per install and
-- never changes, and a license key is bound to that device for the life
-- of the install. Actually created defensively at runtime by
-- vendor_license.rs (CREATE TABLE IF NOT EXISTS) — listed here purely so
-- the full schema is visible in one place, matching every other table.
CREATE TABLE IF NOT EXISTS vendor_device (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    device_id   TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS vendor_license (
    id            INTEGER PRIMARY KEY CHECK (id = 1),
    key_id        TEXT NOT NULL,
    device_id     TEXT NOT NULL,
    activated_at  TEXT NOT NULL
);

-- User-addable master data — nothing here is a hardcoded enum in Rust.
-- A module field can be given type "unit" or "currency" (see module.rs /
-- reference_data.rs) and its values are then validated against exactly
-- these business-scoped, fully editable rows, seeded with sensible
-- defaults at business creation but never locked to them.
CREATE TABLE IF NOT EXISTS units (
    id            TEXT PRIMARY KEY,
    business_id   TEXT NOT NULL REFERENCES businesses(id) ON DELETE CASCADE,
    name          TEXT NOT NULL,      -- e.g. "Kilogram", "Dozen", "Crate of 24"
    abbreviation  TEXT,               -- e.g. "kg", "dz"
    created_at    TEXT NOT NULL,
    UNIQUE(business_id, name)
);
CREATE TABLE IF NOT EXISTS currencies (
    id            TEXT PRIMARY KEY,
    business_id   TEXT NOT NULL REFERENCES businesses(id) ON DELETE CASCADE,
    code          TEXT NOT NULL,      -- e.g. "KES", "USD" — or an informal local token, nothing enforces ISO 4217
    symbol        TEXT,
    name          TEXT,
    created_at    TEXT NOT NULL,
    UNIQUE(business_id, code)
);

-- Generic per-business key/value settings — theme, locale, date format,
-- and anything else the frontend wants to make configurable later
-- without a schema migration every time. Values are opaque strings
-- (usually JSON) from the engine's point of view.
CREATE TABLE IF NOT EXISTS business_settings (
    business_id   TEXT NOT NULL REFERENCES businesses(id) ON DELETE CASCADE,
    key           TEXT NOT NULL,
    value         TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    PRIMARY KEY (business_id, key)
);
