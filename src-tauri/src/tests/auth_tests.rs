use super::common::*;

#[test]
fn test_login_success() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (_, token) = test_owner(&mut conn, &biz);
    assert!(!token.is_empty());
}

#[test]
fn test_login_wrong_password() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let _ = test_owner(&mut conn, &biz);
    let result = crate::auth::login(&conn, &biz, "owner", "wrongpass");
    assert!(result.is_err());
}

#[test]
fn test_password_hashing() {
    let hash = crate::auth::hash_secret("testpass").unwrap();
    assert!(crate::auth::verify_secret("testpass", &hash));
    assert!(!crate::auth::verify_secret("wrong", &hash));
}

#[test]
fn test_rate_limiting() {
    use std::time::Duration;
    let limiter = crate::rate_limit::RateLimiter::new(3, Duration::from_secs(60));
    let key = "test:user";
    assert!(limiter.check(key).is_ok());
    assert!(limiter.check(key).is_ok());
    assert!(limiter.check(key).is_ok());
    assert!(limiter.check(key).is_err());
    limiter.reset(key);
    assert!(limiter.check(key).is_ok());
}

#[test]
fn test_security_question_recovery_enforces_password_strength() {
    // Proves a real gap is actually closed: password strength was
    // enforced at account creation but NOT at recovery, meaning
    // passing the security questions could set the account to any
    // weak password despite the policy — same class of bug as the
    // update()/money validation gap found earlier in this session.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    crate::auth::set_security_questions(&conn, &uid, "Pet name?", "Rex", "City born?", "Nairobi").unwrap();

    let weak = crate::auth::recover_via_security_questions(&conn, &biz, "owner", "Rex", "Nairobi", "weak");
    assert!(weak.is_err(), "a weak password must be rejected even via recovery");

    // And the strong-password path must still actually work end to end.
    let strong = crate::auth::recover_via_security_questions(&conn, &biz, "owner", "Rex", "Nairobi", "NewStrongP@ss1");
    assert!(strong.is_ok());
    assert!(crate::auth::login(&conn, &biz, "owner", "NewStrongP@ss1").is_ok());
}

#[test]
fn test_admin_code_recovery_enforces_password_strength() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (_uid, _) = test_owner(&mut conn, &biz);
    let code_hash = crate::auth::hash_secret("AC-TEST-CODE").unwrap();
    crate::business_panel::set_admin_recovery_code(&conn, &biz, &code_hash).unwrap();

    let weak = crate::auth::recover_via_admin_code(&conn, &biz, "AC-TEST-CODE", "owner", "weak");
    assert!(weak.is_err(), "a weak password must be rejected even via the admin-code last-resort path");

    let strong = crate::auth::recover_via_admin_code(&conn, &biz, "AC-TEST-CODE", "owner", "NewStrongP@ss1");
    assert!(strong.is_ok());
    assert!(crate::auth::login(&conn, &biz, "owner", "NewStrongP@ss1").is_ok());
}
