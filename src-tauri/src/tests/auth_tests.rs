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
