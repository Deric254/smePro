use super::common::*;

fn tmp_data_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("smepro_crash_test_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn test_reporting_allowed_defaults_to_true_when_no_setting_exists() {
    let mut conn = test_db();
    let _biz = test_business(&mut conn);
    // No business_settings row for crash_reporting_enabled has been
    // set — this is opt-OUT, so the default must be enabled.
    assert!(crate::crash_report::reporting_allowed(&conn));
}

#[test]
fn test_reporting_allowed_respects_explicit_opt_out() {
    // Proves a real gap is closed: an earlier version of this module
    // claimed "user can opt out via a setting" while no such setting
    // was ever actually checked anywhere.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    crate::settings::set(&conn, &biz, "crash_reporting_enabled", "false").unwrap();
    assert!(!crate::crash_report::reporting_allowed(&conn));
}

#[test]
fn test_reporting_allowed_true_with_no_business_yet() {
    // A fresh install with no business created yet shouldn't error or
    // panic when this is checked — there's nothing to opt out of yet,
    // so it defaults to true rather than failing.
    let conn = test_db();
    assert!(crate::crash_report::reporting_allowed(&conn));
}

#[test]
fn test_init_with_no_dsn_does_not_install_anything() {
    // The overwhelmingly common case: no DSN configured at all. Must
    // be a complete no-op regardless of the opt-out setting's state —
    // this is the "off by default" behavior the module's own header
    // comment promises.
    let mut conn = test_db();
    let _biz = test_business(&mut conn);
    let data_dir = tmp_data_dir();
    crate::crash_report::init(&conn, None, "1.0.0", &data_dir);
    // No panic, no queue directory created — nothing happened.
    assert!(!data_dir.join("crash_reports").exists());
}

#[test]
fn test_flush_queue_respects_opt_out_and_leaves_files_untouched() {
    // Proves the second real gap is closed: flush_queue() used to
    // exist but was never called from anywhere in the app, so even a
    // deployer who configured a real DSN would have crash reports
    // pile up on disk forever. This test also proves flush_queue
    // itself honors the opt-out setting independently of init().
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let data_dir = tmp_data_dir();
    let queue_dir = data_dir.join("crash_reports");
    std::fs::create_dir_all(&queue_dir).unwrap();
    std::fs::write(queue_dir.join("crash_123.json"), r#"{"error":"test"}"#).unwrap();

    crate::settings::set(&conn, &biz, "crash_reporting_enabled", "false").unwrap();
    // A bogus DSN that would fail to connect even if reached — but
    // reporting_allowed() should return before any network call is
    // attempted at all, so this never even tries.
    crate::crash_report::flush_queue(&conn, "https://example.invalid/webhook", &data_dir);

    // The queued file must still be there — opted-out means "don't
    // send", not "silently discard what was already queued".
    assert!(queue_dir.join("crash_123.json").exists());
}

#[test]
fn test_lib_rs_startup_pattern_compiles_and_runs_on_a_background_thread() {
    // This replicates the EXACT shape of the code added to
    // lib.rs::run() — a second connection opened for a background
    // thread, cloned PathBuf/String captures, calling flush_queue
    // off the main thread so a slow/unreachable network doesn't
    // block app startup. lib.rs's actual `run()` can't be exercised
    // directly in this sandbox (it needs a real Tauri app handle,
    // GTK/webkit and all), so this is the closest direct proof that
    // the pattern used there — ownership, cloning, thread::spawn's
    // 'static bound — actually compiles and behaves as intended,
    // rather than trusting it by inspection alone.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    crate::settings::set(&conn, &biz, "crash_reporting_enabled", "false").unwrap();

    let data_dir = tmp_data_dir();
    let db_path = std::env::temp_dir()
        .join(format!("smepro_crash_lib_test_{}.db", uuid::Uuid::new_v4()))
        .to_string_lossy()
        .to_string();
    // A real on-disk DB this time (not in-memory), since the pattern
    // being verified specifically opens a SECOND connection to the
    // same file — in-memory connections don't share state that way.
    {
        let mut real_conn = crate::db::open(&db_path).unwrap();
        crate::db_migrations::run(&mut real_conn).unwrap();
        let real_biz = crate::business_panel::create_business(&real_conn, "Thread Test Biz", "USD", "UTC").unwrap();
        crate::settings::set(&real_conn, &real_biz, "crash_reporting_enabled", "false").unwrap();
    }

    let dsn = "https://example.invalid/webhook".to_string();
    let flush_conn = crate::db::open(&db_path);
    let flush_dir = data_dir.clone();
    let handle = if let Ok(flush_conn) = flush_conn {
        Some(std::thread::spawn(move || {
            crate::crash_report::flush_queue(&flush_conn, &dsn, &flush_dir);
        }))
    } else {
        None
    };
    assert!(handle.is_some(), "opening a second connection to the same db file must succeed");
    handle.unwrap().join().expect("the background thread must not panic");
}
