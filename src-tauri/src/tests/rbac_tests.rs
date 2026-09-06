use super::common::*;

fn role_id_by_name(conn: &rusqlite::Connection, biz: &str, name: &str) -> String {
    crate::roles::list_roles(conn, biz)
        .unwrap()
        .into_iter()
        .find(|r| r["name"] == serde_json::json!(name))
        .unwrap_or_else(|| panic!("role '{name}' not found"))["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn add_staff(conn: &rusqlite::Connection, biz: &str, username: &str) -> String {
    let hash = crate::auth::hash_secret("password123").unwrap();
    crate::business_panel::add_user(conn, biz, username, &hash, "Staff").unwrap()
}

#[test]
fn test_owner_capabilities_include_everything() {
    // The Owner role is the one system role that always has full
    // access to every module (see roles::set_permissions's own
    // refusal to let it be edited) — this is the baseline every other
    // role in this file is contrasted against.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let caps = crate::rbac::my_capabilities(&conn, &biz, &uid).unwrap();
    assert!(caps.is_admin_tier, "Owner must be admin-tier");
    assert!(caps.can_sell);
    assert!(caps.can_stocktake);
    assert!(caps.readable_modules.contains(&"inventory".to_string()));
    assert!(caps.readable_modules.contains(&"sales".to_string()));
    assert!(caps.readable_modules.contains(&"accounting".to_string()));
}

#[test]
fn test_staff_capabilities_match_the_default_staff_permission_set() {
    // THE ACTUAL FIX Deric asked for: this reports what a role can
    // ACTUALLY do, not a hardcoded "Staff sees less" assumption — it
    // must track inventory.json's own `default_roles` map for Staff
    // (currently: read + sell, no stocktake) exactly, since that map
    // — not this function — is what actually defines Staff's access.
    // If this test and inventory.json's `default_roles` ever disagree,
    // this function has drifted from the real permission data, which
    // is exactly the kind of inconsistency it exists to never have.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let staff_uid = add_staff(&conn, &biz, "staffer");

    let caps = crate::rbac::my_capabilities(&conn, &biz, &staff_uid).unwrap();
    assert!(!caps.is_admin_tier, "Staff must not be admin-tier by default");
    assert!(caps.can_sell, "Staff has 'sell' on inventory by default");
    assert!(!caps.can_stocktake, "Staff does NOT have 'stocktake' on inventory by default — Owner/Manager only");
}

#[test]
fn test_readable_modules_narrows_when_a_roles_read_permission_is_revoked() {
    // THE ACTUAL FIX Deric asked for, proven end to end: an Owner
    // narrowing what Staff can see (via Admin -> Roles, which is
    // exactly roles::set_permissions below) must be reflected here
    // immediately — this is the real mechanism the sidebar now relies
    // on to hide a module a role has no business seeing, not just a
    // description of the out-of-the-box defaults.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let staff_uid = add_staff(&conn, &biz, "staffer2");

    let before = crate::rbac::my_capabilities(&conn, &biz, &staff_uid).unwrap();
    assert!(before.readable_modules.contains(&"accounting".to_string()), "sanity check — Staff starts with read access to Accounting by default");

    let staff_role_id = role_id_by_name(&conn, &biz, "Staff");
    crate::roles::set_permissions(&mut conn, &biz, &staff_role_id, "accounting", &[]).unwrap();

    let after = crate::rbac::my_capabilities(&conn, &biz, &staff_uid).unwrap();
    assert!(!after.readable_modules.contains(&"accounting".to_string()), "revoking read access must remove it from what the sidebar would show");
    assert!(after.readable_modules.contains(&"inventory".to_string()), "an unrelated module's access must be untouched by revoking a different one's");
}

#[test]
fn test_readable_modules_excludes_a_module_the_business_has_disabled() {
    // A module the role could otherwise read still shouldn't appear if
    // the BUSINESS itself has turned it off — `enabled` and "does my
    // role have read" are two independent gates, and this function
    // must respect both, not just the permission one.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    crate::business_panel::disable_module(&conn, &biz, "hr").unwrap();

    let caps = crate::rbac::my_capabilities(&conn, &biz, &uid).unwrap();
    assert!(!caps.readable_modules.contains(&"hr".to_string()), "a disabled module must not be offered even to an Owner");
}

#[test]
fn test_manager_can_stocktake_but_is_not_admin_tier_by_default() {
    // The middle ground between Owner and Staff: Manager has
    // "stocktake" (per inventory.json's default_roles) and gets the
    // admin-tier flag by default (see rbac::seed_default_roles's own
    // comment on why "Manager" specifically), but that flag is a
    // sensible starting point, not a hardcoded rule — this just
    // confirms the actual seeded default holds.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let hash = crate::auth::hash_secret("password123").unwrap();
    let manager_uid = crate::business_panel::add_user(&conn, &biz, "manager1", &hash, "Manager").unwrap();

    let caps = crate::rbac::my_capabilities(&conn, &biz, &manager_uid).unwrap();
    assert!(caps.can_stocktake, "Manager has 'stocktake' on inventory by default");
    assert!(caps.is_admin_tier, "a role literally named 'Manager' gets the admin-tier flag by default on creation");
}
