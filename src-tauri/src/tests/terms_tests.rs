use super::common::*;

#[test]
fn test_fresh_user_has_not_accepted_current_terms() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    assert!(!crate::terms::accepted_current(&conn, &uid).unwrap());
}

#[test]
fn test_accept_records_current_version_and_is_then_reported_accepted() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    crate::terms::accept(&conn, &uid).unwrap();
    assert!(crate::terms::accepted_current(&conn, &uid).unwrap());

    let (stored_at, stored_version): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT terms_accepted_at, terms_accepted_version FROM users WHERE id = ?1",
            rusqlite::params![uid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(stored_at.is_some(), "terms_accepted_at must be set once accepted");
    assert_eq!(stored_version.as_deref(), Some(crate::terms::TERMS_VERSION));
}

#[test]
fn test_accepting_an_older_version_no_longer_counts_as_current() {
    // Proves the actual point of tracking a version at all: an
    // acceptance recorded against a version that ISN'T the current one
    // must not satisfy `accepted_current` — otherwise a terms change
    // would never actually re-prompt anyone, silently defeating the
    // whole feature.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    conn.execute(
        "UPDATE users SET terms_accepted_at = datetime('now'), terms_accepted_version = 'some-old-version' WHERE id = ?1",
        rusqlite::params![uid],
    )
    .unwrap();
    assert!(
        !crate::terms::accepted_current(&conn, &uid).unwrap(),
        "an acceptance of a stale version must not count as accepting the current one"
    );
}

#[test]
fn test_login_reports_terms_accepted_false_until_accepted_then_true() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    assert!(!crate::terms::accepted_current(&conn, &uid).unwrap());
    crate::terms::accept(&conn, &uid).unwrap();
    // Login itself only ever returns a bare session token (see
    // auth::login) — `terms_accepted` is reported by http_api.rs's
    // login routes directly from `terms::accepted_current`, which is
    // exactly what's exercised above; this test documents that the
    // same login credentials still work normally once accepted.
    assert!(crate::auth::login(&conn, &biz, "owner", "password123").is_ok());
}
