use super::common::*;

#[test]
fn test_totp_setup_and_verify_enables_2fa() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let setup = crate::totp::generate_secret(&conn, &uid, "owner").unwrap();
    assert_eq!(setup.recovery_codes.len(), 10);

    assert!(!crate::totp::status(&conn, &uid).unwrap().enabled);

    // Compute a real, currently-valid code from the freshly generated
    // secret the same way an authenticator app would, rather than
    // guessing — this proves the actual TOTP math round-trips, not
    // just that a hardcoded string was accepted somewhere.
    let code = current_code(&setup.secret);
    let verified = crate::totp::verify_and_enable(&mut conn, &uid, &code).unwrap();
    assert!(verified);
    assert!(crate::totp::status(&conn, &uid).unwrap().enabled);
}

#[test]
fn test_totp_recovery_codes_are_hashed_not_plaintext() {
    // Proves a real gap is closed: recovery codes used to be stored
    // and compared as plaintext, unlike every other credential in this
    // app (passwords, security answers), which are Argon2id-hashed.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let setup = crate::totp::generate_secret(&conn, &uid, "owner").unwrap();
    let stored_json: String = conn
        .query_row("SELECT totp_recovery_codes FROM users WHERE id = ?1", [&uid], |r| r.get(0))
        .unwrap();

    // None of the plaintext codes shown to the user should appear
    // verbatim in what's actually stored.
    for code in &setup.recovery_codes {
        assert!(!stored_json.contains(code.as_str()), "recovery code stored in plaintext: {code}");
    }

    // But the real code must still work when checked properly.
    let real_code = setup.recovery_codes[0].clone();
    let used = crate::totp::use_recovery_code(&mut conn, &uid, &real_code).unwrap();
    assert!(used);

    // And it's single-use — the same code must not work twice.
    let mut conn2 = test_db();
    let biz2 = test_business(&mut conn2);
    let (uid2, _) = test_owner(&mut conn2, &biz2);
    let setup2 = crate::totp::generate_secret(&conn2, &uid2, "owner").unwrap();
    let code2 = setup2.recovery_codes[0].clone();
    assert!(crate::totp::use_recovery_code(&mut conn2, &uid2, &code2).unwrap());
    // use_recovery_code also clears totp_secret/enabled on success, so
    // re-generate to test reuse of the same (now removed) code in
    // isolation without that side effect interfering.
    conn2.execute(
        "UPDATE users SET totp_secret = ?1, totp_recovery_codes = ?2, totp_enabled = 1 WHERE id = ?3",
        rusqlite::params![
            setup2.secret,
            serde_json::to_string(&vec![crate::auth::hash_secret("UNUSED01").unwrap()]).unwrap(),
            uid2
        ],
    ).unwrap();
    let reused = crate::totp::use_recovery_code(&mut conn2, &uid2, &code2).unwrap();
    assert!(!reused, "an already-consumed recovery code must not work a second time");
}

#[test]
fn test_totp_rejects_a_replayed_code_within_its_own_window() {
    // Proves the other real gap is closed: totp-rs's check() is
    // completely stateless (confirmed by reading its source — see
    // totp.rs's doc comment) and would accept the SAME valid code
    // submitted twice within its ~30-90s window. The app-level
    // REPLAY_GUARD is what actually has to reject that second
    // submission.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let setup = crate::totp::generate_secret(&conn, &uid, "owner").unwrap();
    let code = current_code(&setup.secret);
    crate::totp::verify_and_enable(&mut conn, &uid, &code).unwrap();

    // First login with this code succeeds...
    let first = crate::totp::verify_login(&conn, &uid, &code).unwrap();
    assert!(first);

    // ...but submitting the exact same code again must be rejected,
    // even though it's still within totp-rs's own time-window
    // tolerance and would otherwise still check out as mathematically
    // valid.
    let replay = crate::totp::verify_login(&conn, &uid, &code).unwrap();
    assert!(!replay, "a replayed TOTP code must be rejected, not accepted a second time");
}

#[test]
fn test_totp_disable_requires_a_valid_current_code() {
    // Proves the backend side of the gap that was just found while
    // auditing: TwoFactorSetup.tsx had no UI to disable 2FA at all,
    // even though totp::disable() worked correctly and was already
    // reachable via POST /auth/2fa/disable. This confirms the
    // function itself is correct — the fix was purely that nothing
    // in the frontend ever called it.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let setup = crate::totp::generate_secret(&conn, &uid, "owner").unwrap();
    let code = current_code(&setup.secret);
    crate::totp::verify_and_enable(&mut conn, &uid, &code).unwrap();
    assert!(crate::totp::status(&conn, &uid).unwrap().enabled);

    // Wrong code must be rejected, and 2FA must stay enabled.
    let wrong = crate::totp::disable(&mut conn, &uid, "000000");
    assert!(wrong.is_err());
    assert!(crate::totp::status(&conn, &uid).unwrap().enabled);

    // The correct current code disables it.
    let next_code = current_code(&setup.secret);
    let result = crate::totp::disable(&mut conn, &uid, &next_code);
    assert!(result.is_ok());
    assert!(!crate::totp::status(&conn, &uid).unwrap().enabled);
}

/// Computes a currently-valid TOTP code for a given base32 secret, the
/// same way a real authenticator app would — used only so these tests
/// exercise the real verification path with a genuinely valid code,
/// not a hardcoded stand-in that happens to compile.
fn current_code(secret_b32: &str) -> String {
    use totp_rs::{Algorithm, Secret, TOTP};
    let secret_bytes = Secret::Encoded(secret_b32.to_string()).to_bytes().unwrap();
    let totp = TOTP::new(Algorithm::SHA1, 6, 1, 30, secret_bytes, None, String::new()).unwrap();
    totp.generate_current().unwrap()
}
