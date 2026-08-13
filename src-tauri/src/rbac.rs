use anyhow::Result;
use rusqlite::Connection;

/// The exact prefix `rbac::require`'s error message starts with. Defined
/// once, here, and used both to construct that message below AND by
/// `http_api::crud_error` to classify it as a 403 rather than a generic
/// 400 — centralizing this is what stops the two from silently drifting
/// apart if either one is edited later without knowing the other exists.
pub const PERMISSION_DENIED_PREFIX: &str = "permission denied";

/// Fetches the role name (not just role_id) for a user — the module
/// permission system (`is_allowed`/`require` below) is keyed by
/// module+action, but several endpoints in this app aren't about a
/// specific module at all (activating the license, initiating a real
/// payment, reconfiguring which modules exist, sending a paid SMS) —
/// those need a coarser "is this person the Owner" check instead.
fn role_name(conn: &Connection, user_id: &str) -> Result<String> {
    conn.query_row(
        "SELECT r.name FROM users u JOIN roles r ON r.id = u.role_id WHERE u.id = ?1 AND u.active = 1",
        [user_id],
        |row| row.get(0),
    )
    .map_err(|_| anyhow::anyhow!("user not found or inactive"))
}

/// Requires the user's role to be exactly "Owner" — every business gets
/// this built-in, undeletable system role automatically
/// (`business_panel::create_business`). Used for actions that commit the
/// business financially or structurally: activating/paying the license,
/// initiating a real payment charge, and reconfiguring which modules are
/// enabled. A Staff or Manager account being able to do any of these
/// was a real gap — RBAC existed for module data, but not for these.
pub fn require_owner(conn: &Connection, user_id: &str) -> Result<()> {
    let role = role_name(conn, user_id)?;
    if role == "Owner" {
        Ok(())
    } else {
        anyhow::bail!("this action is restricted to the business Owner (your role: {role})")
    }
}

/// Requires the user's role to be one of `allowed` — for actions that
/// shouldn't be Owner-only but also shouldn't be open to Staff (sending
/// paid notifications, viewing payment history).
pub fn require_role(conn: &Connection, user_id: &str, allowed: &[&str]) -> Result<()> {
    let role = role_name(conn, user_id)?;
    if allowed.contains(&role.as_str()) {
        Ok(())
    } else {
        anyhow::bail!("this action requires one of these roles: {} (your role: {role})", allowed.join(", "))
    }
}

/// Requires the user to be Owner OR hold a role with the `can_administer`
/// capability flag set. This is the "admin tier" gate for actions like
/// viewing payment history, sending paid notifications, and managing
/// reference data (units/currencies) or settings — deliberately NOT a
/// hardcoded role-name check (it used to check for the literal string
/// "Manager", which meant a business whose second-in-command role was
/// named anything else, in any language, silently couldn't do any of
/// this). An Owner grants the flag to whichever role should have it via
/// `roles::set_admin_flag` — the role can be called anything.
pub fn require_admin_tier(conn: &Connection, user_id: &str) -> Result<()> {
    let (role, is_system, can_administer): (String, i64, i64) = conn
        .query_row(
            "SELECT r.name, r.is_system, r.can_administer
             FROM users u JOIN roles r ON r.id = u.role_id
             WHERE u.id = ?1 AND u.active = 1",
            [user_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|_| anyhow::anyhow!("user not found or inactive"))?;
    if is_system == 1 || can_administer == 1 {
        Ok(())
    } else {
        anyhow::bail!("this action requires admin-tier access, which your role ('{role}') doesn't have")
    }
}

/// Checks whether the given user is allowed to perform `action` on `module_id`.
/// This is the single choke point every module screen and API call must go
/// through — no module should ever query the DB directly without this check
/// first, or "role based access" becomes a suggestion instead of a rule.
pub fn is_allowed(conn: &Connection, user_id: &str, module_id: &str, action: &str) -> Result<bool> {
    let role_id: String = conn.query_row(
        "SELECT role_id FROM users WHERE id = ?1 AND active = 1",
        [user_id],
        |r| r.get(0),
    )?;

    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM permissions WHERE role_id = ?1 AND module_id = ?2 AND action = ?3",
        rusqlite::params![role_id, module_id, action],
        |r| r.get(0),
    )?;

    Ok(count > 0)
}

/// Convenience wrapper used at the very top of every module handler.
/// Returns an error (not a silent false) so a missing permission check
/// can never be accidentally swallowed by the caller.
pub fn require(conn: &Connection, user_id: &str, module_id: &str, action: &str) -> Result<()> {
    if is_allowed(conn, user_id, module_id, action)? {
        Ok(())
    } else {
        anyhow::bail!(
            "{PERMISSION_DENIED_PREFIX}: user {} cannot '{}' on module '{}'",
            user_id,
            action,
            module_id
        )
    }
}

/// Seeds the default roles + permissions for a module, using the
/// `default_roles` map baked into its JSON definition. Called once when a
/// module is first enabled for a business.
pub fn seed_default_roles(
    conn: &mut Connection,
    business_id: &str,
    module: &crate::module::ModuleDef,
) -> Result<()> {
    let tx = conn.transaction()?;
    for (role_name, actions) in &module.default_roles {
        // Ensure the role exists for this business (idempotent). A role
        // literally named "Manager" gets the admin-tier flag by default
        // here — purely a sensible starting point matching this app's
        // prior behavior, NOT an authorization rule. Nothing downstream
        // checks the name "Manager" anymore (see rbac::require_admin_tier);
        // an Owner can revoke this flag, grant it to a differently-named
        // role instead, or rename this role entirely, and every check
        // still works correctly either way.
        tx.execute(
            "INSERT INTO roles (id, business_id, name, is_system, can_administer)
             VALUES (lower(hex(randomblob(16))), ?1, ?2, 0, ?3)
             ON CONFLICT(business_id, name) DO NOTHING",
            rusqlite::params![business_id, role_name, (role_name == "Manager") as i64],
        )?;
        let role_id: String = tx.query_row(
            "SELECT id FROM roles WHERE business_id = ?1 AND name = ?2",
            rusqlite::params![business_id, role_name],
            |r| r.get(0),
        )?;

        for action in actions {
            tx.execute(
                "INSERT INTO permissions (id, role_id, module_id, action)
                 VALUES (lower(hex(randomblob(16))), ?1, ?2, ?3)
                 ON CONFLICT(role_id, module_id, action) DO NOTHING",
                rusqlite::params![role_id, module.id, action],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}
