use super::common::*;
use base64::Engine as _;

fn tmp_data_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("smepro_branding_test_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const MINIMAL_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG magic bytes
    0, 0, 0, 0, // padding — enough bytes to pass the 8-byte minimum check
];

#[test]
fn test_valid_png_logo_and_slogan_both_saved() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let data_dir = tmp_data_dir();

    let b64 = base64::engine::general_purpose::STANDARD.encode(MINIMAL_PNG);
    let path = crate::business_branding::update_branding(&mut conn, &biz, Some(&b64), Some("Fresh bread daily"), &data_dir).unwrap();
    assert!(!path.is_empty());
    assert!(std::path::Path::new(&path).exists());

    let branding = crate::business_branding::get_branding(&conn, &biz).unwrap();
    assert_eq!(branding["slogan"].as_str().unwrap(), "Fresh bread daily");
    assert!(branding["logo_path"].as_str().unwrap().ends_with(".png"));
}

#[test]
fn test_too_long_slogan_never_writes_an_orphaned_logo_file() {
    // Proves a real bug is fixed: slogan validation used to run AFTER
    // the logo file was already written to disk, so a too-long slogan
    // left an orphaned file behind with no DB row ever pointing to it.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let data_dir = tmp_data_dir();

    let too_long = "x".repeat(500);
    let b64 = base64::engine::general_purpose::STANDARD.encode(MINIMAL_PNG);
    let result = crate::business_branding::update_branding(&mut conn, &biz, Some(&b64), Some(&too_long), &data_dir);
    assert!(result.is_err());

    // The whole point: no logo file should exist anywhere under the
    // uploads dir after a call that failed validation.
    let uploads_dir = data_dir.join("uploads");
    let leftover = std::fs::read_dir(&uploads_dir).map(|d| d.count()).unwrap_or(0);
    assert_eq!(leftover, 0, "a rejected update must not leave any file behind on disk");

    // And the DB must be completely untouched too.
    let branding = crate::business_branding::get_branding(&conn, &biz).unwrap();
    assert!(branding["slogan"].as_str().unwrap_or("").is_empty());
    assert!(branding["logo_path"].is_null());
}

#[test]
fn test_svg_with_embedded_script_is_rejected() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let data_dir = tmp_data_dir();

    let malicious = r#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(document.cookie)</script></svg>"#;
    let b64 = base64::engine::general_purpose::STANDARD.encode(malicious.as_bytes());
    let result = crate::business_branding::update_branding(&mut conn, &biz, Some(&b64), None, &data_dir);
    assert!(result.is_err(), "an SVG containing <script> must be rejected");
}

#[test]
fn test_svg_with_event_handler_attribute_is_rejected() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let data_dir = tmp_data_dir();

    let malicious = r#"<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)"><circle r="5"/></svg>"#;
    let b64 = base64::engine::general_purpose::STANDARD.encode(malicious.as_bytes());
    let result = crate::business_branding::update_branding(&mut conn, &biz, Some(&b64), None, &data_dir);
    assert!(result.is_err(), "an SVG with an onload= event handler must be rejected");
}

#[test]
fn test_legitimate_svg_with_xml_declaration_is_accepted() {
    // Proves the fix didn't overcorrect: a real, benign SVG exported
    // by any standard tool (which virtually always includes an <?xml
    // ...?> declaration before <svg>) must still be accepted — the
    // original, stricter "must start with exactly <svg" check would
    // have wrongly rejected this.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let data_dir = tmp_data_dir();

    let legit = r#"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><circle cx="50" cy="50" r="40"/></svg>"#;
    let b64 = base64::engine::general_purpose::STANDARD.encode(legit.as_bytes());
    let result = crate::business_branding::update_branding(&mut conn, &biz, Some(&b64), None, &data_dir);
    assert!(result.is_ok(), "a legitimate SVG with an XML declaration must be accepted, not rejected");
}

#[test]
fn test_serve_logo_rejects_path_traversal() {
    let data_dir = tmp_data_dir();
    std::fs::create_dir_all(data_dir.join("uploads")).unwrap();
    // A file that genuinely exists OUTSIDE the uploads directory —
    // the traversal attempt this test tries to reach.
    let secret_path = data_dir.join("outside_secret.txt");
    std::fs::write(&secret_path, b"do not serve this").unwrap();

    let traversal_attempt = data_dir.join("uploads").join("..").join("outside_secret.txt");
    let result = crate::business_branding::serve_logo(&traversal_attempt.to_string_lossy(), &data_dir);
    assert!(result.is_err(), "a path escaping the uploads directory must be rejected");
}
