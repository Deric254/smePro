use anyhow::{anyhow, Result};
use rand::Rng;
use rusqlite::{params, Connection};
use uuid::Uuid;

use crate::module::ModuleDef;
use crate::rbac;

/// True if a business has already been set up on this install. The
/// first-run setup endpoint uses this to refuse running a second time
/// without authentication — it's the one HTTP route in this whole app
/// that's deliberately public (there's no user to authenticate as yet
/// on a fresh install), so it must close itself off the moment it's
/// been used once.
pub fn any_business_exists(conn: &Connection) -> Result<bool> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM businesses", [], |r| r.get(0))?;
    Ok(count > 0)
}

/// Generates a real, random admin recovery code — not the hardcoded
/// placeholder used in the dev/test seed binary. Shown to the owner
/// exactly once at business creation; only its hash is ever stored.
pub fn generate_admin_code() -> String {
    const CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789"; // no 0/O/1/I ambiguity
    let mut rng = rand::thread_rng();
    let group = |rng: &mut rand::rngs::ThreadRng| -> String {
        (0..4).map(|_| CHARS[rng.gen_range(0..CHARS.len())] as char).collect()
    };
    format!("AC-{}-{}", group(&mut rng), group(&mut rng))
}

/// Creates a new business tenant. This is step one of onboarding —
/// everything else (modules, users, roles) hangs off this business_id.
pub fn create_business(
    conn: &Connection,
    name: &str,
    currency: &str,
    timezone: &str,
) -> Result<String> {
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO businesses (id, name, currency, timezone, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, datetime('now'), datetime('now'))",
        params![id, name, currency, timezone],
    )?;

    // Every business gets a built-in, undeletable Owner role. Without this,
    // the very first user created would have no role to attach to.
    let owner_role_id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO roles (id, business_id, name, is_system) VALUES (?1, ?2, 'Owner', 1)",
        params![owner_role_id, id],
    )?;

    // Seed a sensible starting set of units/currencies — plain, fully
    // editable/deletable rows (see reference_data.rs), not a hardcoded
    // list baked into the engine.
    crate::reference_data::seed_defaults(conn, &id)?;

    Ok(id)
}

/// Updates branding/config fields shown in the Business Panel UI.
/// Only non-null fields are changed — this is a partial update, not a
/// full overwrite, so the panel can save one field at a time.
pub fn update_branding(
    conn: &Connection,
    business_id: &str,
    logo_path: Option<&str>,
    currency: Option<&str>,
    tax_rate: Option<f64>,
) -> Result<()> {
    if let Some(logo) = logo_path {
        conn.execute(
            "UPDATE businesses SET logo_path = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![logo, business_id],
        )?;
    }
    if let Some(cur) = currency {
        conn.execute(
            "UPDATE businesses SET currency = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![cur, business_id],
        )?;
    }
    if let Some(rate) = tax_rate {
        conn.execute(
            "UPDATE businesses SET tax_rate = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![rate, business_id],
        )?;
    }
    Ok(())
}

/// Enables a module for a business: loads its JSON definition, materializes
/// its table if this is the first time, seeds default roles/permissions,
/// and marks it enabled in the registry. Calling this again on an already-
/// enabled module is safe (idempotent) — the panel can call it freely on
/// every toggle-on without needing to track prior state itself.
pub fn enable_module(conn: &mut Connection, business_id: &str, module_json_path: &str) -> Result<()> {
    let raw = std::fs::read_to_string(module_json_path)?;
    let module = ModuleDef::from_json_str(&raw)?;
    module.create_table(conn, business_id)?;
    rbac::seed_default_roles(conn, business_id, &module)?;
    conn.execute(
        "UPDATE modules SET enabled = 1 WHERE business_id = ?1 AND id = ?2",
        params![business_id, module.id],
    )?;
    Ok(())
}

/// Disables a module for a business. Deliberately does NOT drop the table
/// or its data — disabling hides it from the UI and blocks new writes via
/// RBAC/enabled checks, but the owner's data is never destroyed by a
/// toggle. Re-enabling brings it right back.
pub fn disable_module(conn: &Connection, business_id: &str, module_id: &str) -> Result<()> {
    let changed = conn.execute(
        "UPDATE modules SET enabled = 0 WHERE business_id = ?1 AND id = ?2",
        params![business_id, module_id],
    )?;
    if changed == 0 {
        return Err(anyhow!("module '{module_id}' not found for this business"));
    }
    Ok(())
}

pub struct ModuleStatus {
    pub id: String,
    pub display_name: String,
    pub enabled: bool,
}

/// Lists every module known to this business and whether it's currently
/// on — exactly what the Business Panel's module toggle screen needs.
pub fn list_modules(conn: &Connection, business_id: &str) -> Result<Vec<ModuleStatus>> {
    let mut stmt = conn.prepare(
        "SELECT id, display_name, enabled FROM modules WHERE business_id = ?1 ORDER BY display_name",
    )?;
    let rows = stmt.query_map(params![business_id], |r| {
        Ok(ModuleStatus {
            id: r.get(0)?,
            display_name: r.get(1)?,
            enabled: r.get::<_, i64>(2)? == 1,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
}

/// Creates a new staff user under a role. Password/security-answer
/// hashing is the caller's responsibility (pass already-hashed strings) —
/// this function never touches plaintext secrets, so it can't be the
/// place a hashing step gets accidentally skipped.
pub fn add_user(
    conn: &Connection,
    business_id: &str,
    username: &str,
    password_hash: &str,
    role_name: &str,
) -> Result<String> {
    let role_id: String = conn
        .query_row(
            "SELECT id FROM roles WHERE business_id = ?1 AND name = ?2",
            params![business_id, role_name],
            |r| r.get(0),
        )
        .map_err(|_| anyhow!("role '{role_name}' does not exist for this business"))?;

    let user_id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO users (id, business_id, username, password_hash, role_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
        params![user_id, business_id, username, password_hash, role_id],
    )?;
    Ok(user_id)
}

/// Sets up (or rotates) the admin master recovery code — the fallback
/// after security questions fail. Caller passes an already-hashed code;
/// this only ever stores the hash, never the raw code.
pub fn set_admin_recovery_code(conn: &Connection, business_id: &str, code_hash: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO admin_recovery (business_id, admin_code_hash, generated_at)
         VALUES (?1, ?2, datetime('now'))
         ON CONFLICT(business_id) DO UPDATE SET
            admin_code_hash = excluded.admin_code_hash,
            generated_at = excluded.generated_at",
        params![business_id, code_hash],
    )?;
    Ok(())
}
