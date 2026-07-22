//! Role and permission management.
//!
//! The data model here was already dynamic (roles live in a per-business
//! `roles` table, permissions in a `role_id`/`module_id`/`action` table) —
//! what was missing was any way for an Owner to actually USE that: create
//! a new role, delete one, or change what it can do. This module is that
//! missing layer.
//!
//! "Owner" itself remains the one fixed name in the system — every
//! business needs exactly one non-negotiable top authority, the same way
//! a filesystem needs a root user. Everything else, including the
//! "admin tier" previously hardcoded as the literal string "Manager" in
//! `http_api.rs`, is now a capability flag (`roles.can_administer`) an
//! Owner can grant to any role under any name.

use crate::module::ModuleDef;
use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use uuid::Uuid;

pub fn list_roles(conn: &Connection, business_id: &str) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, is_system, can_administer FROM roles WHERE business_id = ?1 ORDER BY is_system DESC, name",
    )?;
    let rows = stmt.query_map(params![business_id], |r| {
        Ok(json!({
            "id": r.get::<_, String>(0)?,
            "name": r.get::<_, String>(1)?,
            "is_system": r.get::<_, i64>(2)? == 1,
            "can_administer": r.get::<_, i64>(3)? == 1,
        }))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn create_role(conn: &Connection, business_id: &str, name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(anyhow!("role name cannot be empty"));
    }
    if name.len() > 64 {
        return Err(anyhow!("role name is too long (max 64 characters)"));
    }
    if name.eq_ignore_ascii_case("owner") {
        return Err(anyhow!("'Owner' is a reserved system role name"));
    }
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO roles (id, business_id, name, is_system, can_administer) VALUES (?1, ?2, ?3, 0, 0)",
        params![id, business_id, name],
    )
    .map_err(|e| {
        if e.to_string().to_uppercase().contains("UNIQUE") {
            anyhow!("a role named '{name}' already exists")
        } else {
            anyhow!(e)
        }
    })?;
    Ok(id)
}

/// Refuses to delete the protected Owner role, and refuses to delete any
/// role that still has active users assigned to it (checked directly,
/// not assumed) — deleting it out from under them would leave those
/// users' sessions pointing at a role_id that no longer exists.
pub fn delete_role(conn: &Connection, business_id: &str, role_id: &str) -> Result<()> {
    let (name, is_system): (String, i64) = conn
        .query_row(
            "SELECT name, is_system FROM roles WHERE id = ?1 AND business_id = ?2",
            params![role_id, business_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| anyhow!("role not found"))?;
    if is_system == 1 {
        return Err(anyhow!("'{name}' is a protected system role and cannot be deleted"));
    }
    let user_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM users WHERE role_id = ?1 AND active = 1",
        params![role_id],
        |r| r.get(0),
    )?;
    if user_count > 0 {
        return Err(anyhow!(
            "cannot delete role '{name}': {user_count} active user(s) are still assigned to it — reassign them first"
        ));
    }
    conn.execute("DELETE FROM permissions WHERE role_id = ?1", params![role_id])?;
    let deleted = conn.execute(
        "DELETE FROM roles WHERE id = ?1 AND business_id = ?2 AND is_system = 0",
        params![role_id, business_id],
    )?;
    if deleted == 0 {
        return Err(anyhow!("role not found"));
    }
    Ok(())
}

/// Grants or revokes the admin-tier capability on a role. Cannot be
/// changed on the Owner role — Owner is always admin-tier and then some,
/// toggling the flag on it would be meaningless and could be mistaken
/// for a way to demote it (it isn't; Owner-only checks are separate and
/// unaffected by this flag either way).
pub fn set_admin_flag(conn: &Connection, business_id: &str, role_id: &str, can_administer: bool) -> Result<()> {
    let is_system: i64 = conn
        .query_row(
            "SELECT is_system FROM roles WHERE id = ?1 AND business_id = ?2",
            params![role_id, business_id],
            |r| r.get(0),
        )
        .map_err(|_| anyhow!("role not found"))?;
    if is_system == 1 {
        return Err(anyhow!("the Owner role's privileges can't be changed — it always has full access"));
    }
    conn.execute(
        "UPDATE roles SET can_administer = ?1 WHERE id = ?2 AND business_id = ?3",
        params![can_administer as i64, role_id, business_id],
    )?;
    Ok(())
}

pub fn get_permissions(conn: &Connection, business_id: &str, role_id: &str) -> Result<Value> {
    let _: String = conn
        .query_row(
            "SELECT id FROM roles WHERE id = ?1 AND business_id = ?2",
            params![role_id, business_id],
            |r| r.get(0),
        )
        .map_err(|_| anyhow!("role not found"))?;
    let mut stmt = conn.prepare("SELECT module_id, action FROM permissions WHERE role_id = ?1 ORDER BY module_id, action")?;
    let rows: Vec<(String, String)> = stmt
        .query_map(params![role_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();
    let mut by_module: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for (m, a) in rows {
        by_module.entry(m).or_default().push(a);
    }
    Ok(json!(by_module))
}

/// Replaces the full set of actions a role has on one module — sending
/// `{"module_id": "inventory", "actions": ["read"]}` means "this role can
/// now ONLY read inventory," including revoking anything not listed, not
/// just adding what's listed. Simplest semantics that can't drift out of
/// sync with what the caller actually intended to grant.
pub fn set_permissions(conn: &mut Connection, business_id: &str, role_id: &str, module_id: &str, actions: &[String]) -> Result<()> {
    let is_system: i64 = conn
        .query_row(
            "SELECT is_system FROM roles WHERE id = ?1 AND business_id = ?2",
            params![role_id, business_id],
            |r| r.get(0),
        )
        .map_err(|_| anyhow!("role not found"))?;
    if is_system == 1 {
        return Err(anyhow!("the Owner role's permissions can't be edited — it always has full access to every module"));
    }

    let schema_json: String = conn
        .query_row(
            "SELECT schema_json FROM modules WHERE id = ?1 AND business_id = ?2",
            params![module_id, business_id],
            |r| r.get(0),
        )
        .map_err(|_| anyhow!("module '{module_id}' not found for this business"))?;
    let module = ModuleDef::from_json_str(&schema_json)?;

    let mut seen = std::collections::HashSet::new();
    for a in actions {
        if !module.actions.contains(a) {
            return Err(anyhow!(
                "module '{module_id}' has no action '{a}' — valid actions: {}",
                module.actions.join(", ")
            ));
        }
        if !seen.insert(a.as_str()) {
            return Err(anyhow!("duplicate action '{a}' in request"));
        }
    }

    let tx = conn.transaction()?;
    tx.execute("DELETE FROM permissions WHERE role_id = ?1 AND module_id = ?2", params![role_id, module_id])?;
    for a in actions {
        tx.execute(
            "INSERT INTO permissions (id, role_id, module_id, action) VALUES (?1, ?2, ?3, ?4)",
            params![Uuid::new_v4().to_string(), role_id, module_id, a],
        )?;
    }
    tx.commit()?;
    Ok(())
}
