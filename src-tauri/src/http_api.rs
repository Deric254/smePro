use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tiny_http::{Header, Method, Response, Server};

use crate::rate_limit::RateLimiter;
use crate::report::Dimension;
use crate::{ai_assistant, audit, auth, backup, crud, excel_import, forecast, notifications, ocr_import, onboarding, pos, rbac, receiving, reference_data, refund, report, repack, roles, settings, users, xlsx_export};
use std::time::Duration;

enum ApiResponse {
    Json(u16, Value),
    Xlsx(u16, Vec<u8>, String), // status, bytes, filename
    Image(u16, Vec<u8>, String), // status, bytes, mime type
}

pub fn serve(conn: Connection, addr: &str) {
    let server = Server::http(addr).expect("failed to bind local API server");
    let conn = Arc::new(Mutex::new(conn));
    // 5 attempts per 15-minute rolling window — generous enough that a
    // real user fumbling their password isn't locked out, tight enough
    // to make brute-forcing a password or the admin recovery code
    // impractical.
    let auth_limiter = Arc::new(RateLimiter::new(5, Duration::from_secs(15 * 60)));
    println!("[api] listening on http://{addr}");

    for mut request in server.incoming_requests() {
        let method = request.method().clone();
        let url = request.url().to_string();

        // CORS preflight — the frontend runs on a different port (Vite
        // dev server) than this API, so browsers send an OPTIONS request
        // before PUT/DELETE calls and calls with custom headers.
        if method == Method::Options {
            let headers = cors_headers();
            let response = Response::from_string("").with_status_code(204);
            let response = headers.into_iter().fold(response, |r, h| r.with_header(h));
            let _ = request.respond(response);
            continue;
        }

        let mut body_str = String::new();
        let _ = request.as_reader().read_to_string(&mut body_str);

        if let Err(e) = crate::security::check_body_size(body_str.as_bytes()) {
            let response = Response::from_string(json!({"error": e.to_string()}).to_string()).with_status_code(413);
            let response = cors_headers().into_iter().fold(response, |r, h| r.with_header(h));
            let _ = request.respond(response);
            continue;
        }

        let bearer = header_value(request.headers(), "Authorization")
            .and_then(|v| v.strip_prefix("Bearer ").map(|s| s.to_string()));
        let business_id_header = header_value(request.headers(), "X-Business-Id");

        let mut conn_guard = conn.lock().unwrap();
        let response = route(&mut conn_guard, &method, &url, &body_str, bearer.as_deref(), business_id_header.as_deref(), &auth_limiter);
        drop(conn_guard);

        let http_response = match response {
            ApiResponse::Json(status, payload) => {
                let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
                Response::from_string(payload.to_string()).with_status_code(status).with_header(header)
            }
            ApiResponse::Xlsx(status, bytes, filename) => {
                let ctype = Header::from_bytes(
                    &b"Content-Type"[..],
                    &b"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"[..],
                ).unwrap();
                let disposition = Header::from_bytes(
                    &b"Content-Disposition"[..],
                    format!("attachment; filename=\"{filename}\"").as_bytes(),
                ).unwrap();
                Response::from_data(bytes).with_status_code(status).with_header(ctype).with_header(disposition)
            }
            ApiResponse::Image(status, bytes, mime) => {
                let ctype = Header::from_bytes(&b"Content-Type"[..], mime.as_bytes()).unwrap();
                Response::from_data(bytes).with_status_code(status).with_header(ctype)
            }
        };
        let http_response = cors_headers().into_iter().fold(http_response, |r, h| r.with_header(h));
        let http_response = crate::security::security_headers().into_iter().fold(http_response, |r, (name, value)| {
            r.with_header(Header::from_bytes(name.as_bytes(), value.as_bytes()).unwrap())
        });
        let _ = request.respond(http_response);
    }
}

/// CORS is wide-open (`*`) because this API only ever binds to
/// 127.0.0.1 — it's not reachable from outside the device, so the usual
/// cross-origin risk model doesn't apply the way it would for a public
/// API. This is what lets the Tauri/React frontend (served from its own
/// dev-server port, or from `tauri://localhost` in production) call it.
fn cors_headers() -> Vec<Header> {
    vec![
        Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap(),
        Header::from_bytes(&b"Access-Control-Allow-Methods"[..], &b"GET, POST, PUT, DELETE, OPTIONS"[..]).unwrap(),
        Header::from_bytes(&b"Access-Control-Allow-Headers"[..], &b"Content-Type, Authorization, X-Business-Id"[..]).unwrap(),
    ]
}

fn header_value(headers: &[Header], name: &str) -> Option<String> {
    headers.iter().find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name)).map(|h| h.value.as_str().to_string())
}

fn json_body(body: &str) -> Option<serde_json::Map<String, Value>> {
    match serde_json::from_str(body) {
        Ok(Value::Object(m)) => Some(m),
        _ => None,
    }
}

fn query_params(url: &str) -> HashMap<String, String> {
    url.split_once('?')
        .map(|(_, q)| {
            q.split('&')
                .filter_map(|kv| kv.split_once('='))
                .map(|(k, v)| (k.to_string(), urlish_decode(v)))
                .collect()
        })
        .unwrap_or_default()
}

/// Minimal `%XX` + `+` decoder — good enough for the query strings this
/// local API generates and receives; not meant as a general URL library.
fn urlish_decode(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '+' => out.push(' '),
            '%' => {
                let hex: String = chars.by_ref().take(2).collect();
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    out.push(byte as char);
                }
            }
            other => out.push(other),
        }
    }
    out
}

fn route(
    conn: &mut Connection,
    method: &Method,
    url: &str,
    body: &str,
    bearer: Option<&str>,
    business_id_header: Option<&str>,
    auth_limiter: &RateLimiter,
) -> ApiResponse {
    let path = url.split('?').next().unwrap_or("");
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').filter(|s| !s.is_empty()).collect();

    // ---- Public routes ----

    // First-run setup: GET /setup/status tells the frontend whether to
    // show the "create your business" screen or the normal login screen.
    if parts.as_slice() == ["setup", "status"] && *method == Method::Get {
        return match crate::business_panel::any_business_exists(conn) {
            Ok(exists) => ApiResponse::Json(200, json!({"has_business": exists})),
            Err(e) => json_err(500, &e.to_string()),
        };
    }

    // GET /setup/business-id — resolves the business ID automatically
    // for the normal case (one install, one business), so the login
    // screen never has to ask someone to know or paste a raw UUID just
    // to sign in. Public/no-auth on purpose: a business ID is a routing
    // key, not a secret — the password is what actually protects the
    // account. Returns null (not an error) when there's more than one
    // business in this database, which the frontend treats as "ask for
    // it explicitly" rather than silently guessing.
    if parts.as_slice() == ["setup", "business-id"] && *method == Method::Get {
        return match crate::business_panel::resolve_single_business_id(conn) {
            Ok(id) => ApiResponse::Json(200, json!({"business_id": id})),
            Err(e) => json_err(500, &e.to_string()),
        };
    }

    // GET /setup/branding — public and pre-auth on purpose: the login
    // screen needs to show a business's own logo and name BEFORE
    // anyone has signed in (that's the entire point — a returning
    // owner should see their own shop's identity looking back at them
    // on the sign-in screen, not a generic placeholder). Not a
    // security concern to expose without a session — a business's own
    // logo/name isn't sensitive, same reasoning already applied to
    // resolving the business ID itself above.
    if parts.as_slice() == ["setup", "branding"] && *method == Method::Get {
        let business_id = match crate::business_panel::resolve_single_business_id(conn) {
            Ok(Some(id)) => id,
            Ok(None) => return ApiResponse::Json(200, json!({"name": null, "logo_url": null, "slogan": null})),
            Err(e) => return json_err(500, &e.to_string()),
        };
        return match crate::business_branding::get_branding(conn, &business_id) {
            Ok(v) => {
                // get_branding() returns the full stored filesystem
                // path in logo_path — not directly usable by a
                // frontend <img> tag. The /uploads/{filename} route
                // only wants the filename portion, joining it with the
                // real app data dir itself — so that's what this
                // derives and exposes as logo_url instead of leaking
                // the raw filesystem path to the client.
                let logo_url = v.get("logo_path").and_then(|p| p.as_str()).and_then(|p| {
                    std::path::Path::new(p).file_name().map(|f| format!("/uploads/{}", f.to_string_lossy()))
                });
                ApiResponse::Json(200, json!({
                    "name": v.get("name"),
                    "slogan": v.get("slogan"),
                    "logo_url": logo_url,
                }))
            }
            Err(_) => ApiResponse::Json(200, json!({"name": null, "logo_url": null, "slogan": null})),
        };
    }

    // GET /uploads/{filename} — serves the actual logo image bytes.
    // Public for the same reason /setup/branding above is: the login
    // screen's <img> tag has no way to attach an Authorization header,
    // and a business's own logo file isn't sensitive data. Path
    // traversal is guarded inside serve_logo() itself, not here.
    if parts.len() == 2 && parts[0] == "uploads" && *method == Method::Get {
        let app_data_dir = std::path::PathBuf::from(
            std::env::var("SME_APP_DATA_DIR").unwrap_or_else(|_| "./".to_string())
        );
        let file_path = app_data_dir.join("uploads").join(parts[1]);
        return match crate::business_branding::serve_logo(&file_path.to_string_lossy(), &app_data_dir) {
            Ok((mime, bytes)) => ApiResponse::Image(200, bytes, mime),
            Err(e) => json_err(404, &e.to_string()),
        };
    }

    // POST /setup/create-business — the ONE public write endpoint in
    // this whole API, because on a genuinely fresh install there is no
    // user yet to authenticate as. Guarded by refusing to run a second
    // time the moment any business exists, so it can't be replayed to
    // create a rogue additional business without authentication.
    if parts.as_slice() == ["setup", "create-business"] && *method == Method::Post {
        match crate::business_panel::any_business_exists(conn) {
            Ok(true) => return json_err(409, "setup has already been completed on this install"),
            Ok(false) => {}
            Err(e) => return json_err(500, &e.to_string()),
        }
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "invalid body") };
        let g = |k: &str| obj.get(k).and_then(Value::as_str).unwrap_or("");

        let business_name = g("business_name");
        let currency = if g("currency").is_empty() { "USD" } else { g("currency") };
        let timezone = if g("timezone").is_empty() { "UTC" } else { g("timezone") };
        let business_type = g("business_type");
        let owner_username = g("owner_username");
        let owner_password = g("owner_password");
        let (sq1, sa1, sq2, sa2) = (g("security_q1"), g("security_a1"), g("security_q2"), g("security_a2"));

        if business_name.is_empty() || owner_username.is_empty() || owner_password.is_empty() {
            return json_err(400, "business_name, owner_username, and owner_password are all required");
        }
        if let Err(e) = crate::security::validate_password(owner_password) {
            return json_err(400, &format!("owner_password {e}"));
        }
        if sq1.is_empty() || sa1.is_empty() || sq2.is_empty() || sa2.is_empty() {
            return json_err(400, "both security questions and answers are required — this is the account's forgot-password path");
        }

        let business_id = match crate::business_panel::create_business(conn, business_name, currency, timezone) {
            Ok(id) => id,
            Err(e) => return json_err(500, &e.to_string()),
        };

        if !business_type.is_empty() {
            if let Err(e) = crate::onboarding::apply_business_type(conn, &business_id, business_type) {
                return json_err(400, &e.to_string());
            }
        }

        let password_hash = match auth::hash_secret(owner_password) { Ok(h) => h, Err(e) => return json_err(500, &e.to_string()) };
        let owner_id = match crate::business_panel::add_user(conn, &business_id, owner_username, &password_hash, "Owner") {
            Ok(id) => id,
            Err(e) => return json_err(500, &e.to_string()),
        };
        if let Err(e) = auth::set_security_questions(conn, &owner_id, sq1, sa1, sq2, sa2) {
            return json_err(500, &e.to_string());
        }

        let admin_code = crate::business_panel::generate_admin_code();
        let admin_code_hash = match auth::hash_secret(&admin_code) { Ok(h) => h, Err(e) => return json_err(500, &e.to_string()) };
        if let Err(e) = crate::business_panel::set_admin_recovery_code(conn, &business_id, &admin_code_hash) {
            return json_err(500, &e.to_string());
        }

        return ApiResponse::Json(201, json!({
            "business_id": business_id,
            "owner_id": owner_id,
            "admin_recovery_code": admin_code,
            "warning": "Save this admin recovery code now — it is shown exactly once and cannot be retrieved later."
        }));
    }

    // POST /setup/restore — the actual disaster-recovery path, not just
    // the "I'm already logged in and want to roll back" one that
    // /admin/restore below covers. A REAL disaster (dead hard drive,
    // wiped machine, fresh reinstall) leaves nothing to log into —
    // there's no business, no user, no session to authenticate with.
    // Without this, the backup/restore system this app already has
    // would be completely unreachable at the exact moment it's needed
    // most. Guarded the same way create-business is: only reachable
    // before any business exists on this install, closed off the
    // instant one does (same 409 pattern) so it can't be replayed
    // against a live, already-set-up install without authentication.
    // What actually gates this from being a free-for-all: the restore
    // payload itself must contain a backup that decrypts correctly and
    // is shaped like a real SME Pro database (validated inside
    // backup::stage_restore) — only someone who actually has your
    // backup file could produce that.
    if parts.as_slice() == ["setup", "restore"] && *method == Method::Post {
        match crate::business_panel::any_business_exists(conn) {
            Ok(true) => return json_err(409, "this install already has a business set up — use Admin \u{2192} Backup to restore from within the app instead"),
            Ok(false) => {}
            Err(e) => return json_err(500, &e.to_string()),
        }
        let input: backup::RestoreInput = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(_) => return json_err(400, "invalid restore payload"),
        };
        return match backup::stage_restore(conn, input) {
            Ok(()) => ApiResponse::Json(200, json!({"staged": true, "message": "Restore staged. Restart the app to complete it."})),
            Err(e) => json_err(400, &e.to_string()),
        };
    }

    if parts.as_slice() == ["auth", "login"] && *method == Method::Post {
        let biz = match business_id_header { Some(b) => b, None => return json_err(400, "X-Business-Id header required") };
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "body must be JSON with username/password") };
        let username = obj.get("username").and_then(Value::as_str).unwrap_or("");
        let password = obj.get("password").and_then(Value::as_str).unwrap_or("");

        let limiter_key = format!("login:{biz}:{username}");
        if let Err(retry_after) = auth_limiter.check(&limiter_key) {
            return json_err(429, &format!("too many login attempts, try again in {retry_after} seconds"));
        }
        let logged_in_user_id = match auth::verify_password(conn, biz, username, password) {
            Ok(id) => id,
            Err(e) => {
                let _ = audit::log(conn, biz, None, "_auth", "login_failed", None, Some(&json!({"username": username})));
                return json_err(401, &e.to_string());
            }
        };
        auth_limiter.reset(&limiter_key);

        // If this user has 2FA enabled, don't issue a real session yet —
        // hand back a short-lived pending token instead. The frontend
        // then calls /auth/2fa/login with that token + a TOTP code to
        // actually get a session.
        if crate::totp::status(conn, &logged_in_user_id).map(|s| s.enabled).unwrap_or(false) {
            let temp_token = crate::totp::issue_pending_token(&logged_in_user_id, biz);
            return ApiResponse::Json(202, json!({"requires_2fa": true, "temp_token": temp_token}));
        }

        return match auth::create_session(conn, &logged_in_user_id, biz) {
            Ok(token) => {
                let _ = audit::log(conn, biz, Some(&logged_in_user_id), "_auth", "login_success", None, None);
                ApiResponse::Json(200, json!({"token": token}))
            }
            Err(e) => json_err(500, &e.to_string()),
        };
    }
    if parts.as_slice() == ["auth", "2fa", "login"] && *method == Method::Post {
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "invalid body") };
        let temp_token = obj.get("temp_token").and_then(Value::as_str).unwrap_or("");
        let code = obj.get("code").and_then(Value::as_str).unwrap_or("");

        let (pending_user_id, pending_business_id) = match crate::totp::resolve_pending_token(temp_token) {
            Some(pair) => pair,
            None => return json_err(401, "2FA login expired or already used — please log in again"),
        };

        let valid = match crate::totp::verify_login(conn, &pending_user_id, code) {
            Ok(v) => v,
            Err(e) => return json_err(400, &e.to_string()),
        };
        if !valid {
            let _ = audit::log(conn, &pending_business_id, Some(&pending_user_id), "_auth", "2fa_login_failed", None, None);
            return json_err(401, "invalid TOTP code");
        }

        return match auth::create_session(conn, &pending_user_id, &pending_business_id) {
            Ok(token) => {
                let _ = audit::log(conn, &pending_business_id, Some(&pending_user_id), "_auth", "login_success", None, None);
                ApiResponse::Json(200, json!({"token": token}))
            }
            Err(e) => json_err(500, &e.to_string()),
        };
    }
    if parts.as_slice() == ["auth", "recover", "security-questions"] && *method == Method::Post {
        let biz = match business_id_header { Some(b) => b, None => return json_err(400, "X-Business-Id header required") };
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "invalid body") };
        let g = |k: &str| obj.get(k).and_then(Value::as_str).unwrap_or("");

        let limiter_key = format!("recover-sq:{biz}:{}", g("username"));
        if let Err(retry_after) = auth_limiter.check(&limiter_key) {
            return json_err(429, &format!("too many recovery attempts, try again in {retry_after} seconds"));
        }
        return match auth::recover_via_security_questions(conn, biz, g("username"), g("answer1"), g("answer2"), g("new_password")) {
            Ok(()) => { auth_limiter.reset(&limiter_key); ApiResponse::Json(200, json!({"reset": true})) }
            Err(e) => json_err(400, &e.to_string()),
        };
    }
    if parts.as_slice() == ["auth", "recover", "admin-code"] && *method == Method::Post {
        let biz = match business_id_header { Some(b) => b, None => return json_err(400, "X-Business-Id header required") };
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "invalid body") };
        let g = |k: &str| obj.get(k).and_then(Value::as_str).unwrap_or("");

        let limiter_key = format!("recover-admin:{biz}");
        if let Err(retry_after) = auth_limiter.check(&limiter_key) {
            return json_err(429, &format!("too many recovery attempts, try again in {retry_after} seconds"));
        }
        return match auth::recover_via_admin_code(conn, biz, g("admin_code"), g("username"), g("new_password")) {
            Ok(()) => { auth_limiter.reset(&limiter_key); ApiResponse::Json(200, json!({"reset": true})) }
            Err(e) => json_err(400, &e.to_string()),
        };
    }

    // ---- Protected routes ----
    let token = match bearer { Some(t) => t, None => return json_err(401, "missing Authorization: Bearer <token>") };
    match crate::security::check_session_expired(conn, token) {
        Ok(true) => return json_err(401, "session expired due to inactivity, please log in again"),
        Ok(false) => {}
        Err(e) => return json_err(500, &e.to_string()),
    }
    let (user_id, business_id) = match auth::current_user(conn, token) {
        Ok(pair) => pair,
        Err(e) => return json_err(401, &e.to_string()),
    };

    if parts.as_slice() == ["auth", "logout"] && *method == Method::Post {
        return match auth::logout(conn, token) { Ok(()) => ApiResponse::Json(200, json!({"logged_out": true})), Err(e) => json_err(400, &e.to_string()) };
    }
    // POST /license/vendor/redeem {"key": "SPK-...."} — one-time,
    // fully offline activation of a vendor-issued license key. See
    // vendor_license.rs for the full design and its honest trade-off.
    // Owner-only.
    if parts.as_slice() == ["license", "vendor", "redeem"] && *method == Method::Post {
        if let Err(e) = rbac::require_owner(conn, &user_id) { return json_err(403, &e.to_string()); }
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "invalid body") };
        let key = obj.get("key").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if key.is_empty() { return json_err(400, "key is required"); }
        if let Err(e) = crate::vendor_license::validate_key_format(&key) {
            return json_err(400, &format!("invalid key: {e}"));
        }
        // Fully offline now — no server URL, no network call. See
        // vendor_license.rs for why (and its honest trade-off).
        return match crate::vendor_license::redeem(conn, &key) {
            Ok(result) => {
                let _ = audit::log(conn, &business_id, Some(&user_id), "_license", "vendor_key_redeem", None, Some(&result));
                ApiResponse::Json(200, result)
            }
            Err(e) => json_err(400, &e.to_string()),
        };
    }
    // GET /license/vendor/status — local-only, no network call.
    if parts.as_slice() == ["license", "vendor", "status"] && *method == Method::Get {
        return match crate::vendor_license::status(conn) {
            Ok(v) => ApiResponse::Json(200, v),
            Err(e) => json_err(500, &e.to_string()),
        };
    }

    // ---- Backup & restore — Owner-only, real disaster recovery. ----
    if parts.as_slice() == ["admin", "backup"] && *method == Method::Post {
        if let Err(e) = rbac::require_owner(conn, &user_id) { return json_err(403, &e.to_string()); }
        let passphrase = match json_body(body).and_then(|o| o.get("passphrase").and_then(|v| v.as_str()).map(String::from)) {
            Some(p) => p,
            None => return json_err(400, "a passphrase is required to create a backup"),
        };
        return match backup::create_backup(conn, &passphrase) {
            Ok(data) => {
                let _ = audit::log(conn, &business_id, Some(&user_id), "_backup", "create", None, None);
                match serde_json::to_value(&data) {
                    Ok(v) => ApiResponse::Json(200, v),
                    Err(e) => json_err(500, &e.to_string()),
                }
            }
            Err(e) => json_err(500, &e.to_string()),
        };
    }
    if parts.as_slice() == ["admin", "restore"] && *method == Method::Post {
        if let Err(e) = rbac::require_owner(conn, &user_id) { return json_err(403, &e.to_string()); }
        let input: backup::RestoreInput = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(_) => return json_err(400, "invalid restore payload"),
        };
        return match backup::stage_restore(conn, input) {
            Ok(()) => {
                let _ = audit::log(conn, &business_id, Some(&user_id), "_backup", "restore_staged", None, None);
                ApiResponse::Json(200, json!({"staged": true, "message": "Restore staged. Restart the app to complete it."}))
            }
            Err(e) => json_err(400, &e.to_string()),
        };
    }

    if parts.as_slice() == ["business"] && *method == Method::Get {
        let result: rusqlite::Result<(String, String, Option<String>)> = conn.query_row(
            "SELECT name, currency, logo_path FROM businesses WHERE id = ?1",
            rusqlite::params![business_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        );
        return match result {
            Ok((name, currency, logo_path)) => ApiResponse::Json(200, json!({"name": name, "currency": currency, "logo_path": logo_path})),
            Err(e) => json_err(500, &e.to_string()),
        };
    }

    // ---- Users — Owner-only. Missing entirely before this: the only
    // user ever created was the first-run Owner. ----
    if parts.as_slice() == ["users"] && *method == Method::Get {
        if let Err(e) = rbac::require_owner(conn, &user_id) { return json_err(403, &e.to_string()); }
        return match users::list_users(conn, &business_id) { Ok(v) => ApiResponse::Json(200, json!({"users": v})), Err(e) => json_err(500, &e.to_string()) };
    }
    if parts.as_slice() == ["users"] && *method == Method::Post {
        if let Err(e) = rbac::require_owner(conn, &user_id) { return json_err(403, &e.to_string()); }
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "invalid body") };
        let g = |k: &str| obj.get(k).and_then(Value::as_str).unwrap_or("");
        return match users::create_user(conn, &business_id, g("username"), g("password"), g("role_id"), g("security_q1"), g("security_a1"), g("security_q2"), g("security_a2")) {
            Ok(id) => {
                let _ = audit::log(conn, &business_id, Some(&user_id), "_users", "create_user", Some(&id), Some(&json!({"username": g("username")})));
                ApiResponse::Json(201, json!({"id": id}))
            }
            Err(e) => json_err(400, &e.to_string()),
        };
    }
    if parts.len() == 3 && parts[0] == "users" && parts[2] == "role" && *method == Method::Put {
        if let Err(e) = rbac::require_owner(conn, &user_id) { return json_err(403, &e.to_string()); }
        let target_user_id = parts[1];
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "invalid body") };
        let role_id = obj.get("role_id").and_then(Value::as_str).unwrap_or("");
        return match users::set_role(conn, &business_id, target_user_id, role_id) {
            Ok(()) => {
                let _ = audit::log(conn, &business_id, Some(&user_id), "_users", "set_role", Some(target_user_id), Some(&json!({"role_id": role_id})));
                ApiResponse::Json(200, json!({"ok": true}))
            }
            Err(e) => json_err(400, &e.to_string()),
        };
    }
    if parts.len() == 2 && parts[0] == "users" && *method == Method::Delete {
        if let Err(e) = rbac::require_owner(conn, &user_id) { return json_err(403, &e.to_string()); }
        let target_user_id = parts[1];
        return match users::deactivate_user(conn, &business_id, target_user_id) {
            Ok(()) => {
                let _ = audit::log(conn, &business_id, Some(&user_id), "_users", "deactivate_user", Some(target_user_id), None);
                ApiResponse::Json(200, json!({"deactivated": true}))
            }
            Err(e) => json_err(400, &e.to_string()),
        };
    }

    // ---- Roles & permissions — fully user-manageable, "Owner" is the
    // only fixed name anywhere in this system. Reading the role list is
    // safe for any authenticated user (it's what a "who can do what"
    // screen needs), structural changes are Owner-only. ----
    if parts.as_slice() == ["roles"] && *method == Method::Get {
        return match roles::list_roles(conn, &business_id) { Ok(v) => ApiResponse::Json(200, json!({"roles": v})), Err(e) => json_err(500, &e.to_string()) };
    }
    if parts.as_slice() == ["roles"] && *method == Method::Post {
        if let Err(e) = rbac::require_owner(conn, &user_id) { return json_err(403, &e.to_string()); }
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "invalid body") };
        let name = obj.get("name").and_then(Value::as_str).unwrap_or("");
        return match roles::create_role(conn, &business_id, name) {
            Ok(id) => {
                let _ = audit::log(conn, &business_id, Some(&user_id), "_roles", "create_role", Some(&id), Some(&json!({"name": name})));
                ApiResponse::Json(201, json!({"id": id}))
            }
            Err(e) => json_err(400, &e.to_string()),
        };
    }
    if parts.len() == 2 && parts[0] == "roles" && *method == Method::Delete {
        if let Err(e) = rbac::require_owner(conn, &user_id) { return json_err(403, &e.to_string()); }
        let role_id = parts[1];
        return match roles::delete_role(conn, &business_id, role_id) {
            Ok(()) => {
                let _ = audit::log(conn, &business_id, Some(&user_id), "_roles", "delete_role", Some(role_id), None);
                ApiResponse::Json(200, json!({"deleted": true}))
            }
            Err(e) => json_err(400, &e.to_string()),
        };
    }
    if parts.len() == 3 && parts[0] == "roles" && parts[2] == "admin-flag" && *method == Method::Put {
        if let Err(e) = rbac::require_owner(conn, &user_id) { return json_err(403, &e.to_string()); }
        let role_id = parts[1];
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "invalid body") };
        let can_administer = obj.get("can_administer").and_then(Value::as_bool).unwrap_or(false);
        return match roles::set_admin_flag(conn, &business_id, role_id, can_administer) {
            Ok(()) => {
                let _ = audit::log(conn, &business_id, Some(&user_id), "_roles", "set_admin_flag", Some(role_id), Some(&json!({"can_administer": can_administer})));
                ApiResponse::Json(200, json!({"ok": true}))
            }
            Err(e) => json_err(400, &e.to_string()),
        };
    }
    if parts.len() == 3 && parts[0] == "roles" && parts[2] == "permissions" && *method == Method::Get {
        if let Err(e) = rbac::require_owner(conn, &user_id) { return json_err(403, &e.to_string()); }
        let role_id = parts[1];
        return match roles::get_permissions(conn, &business_id, role_id) { Ok(v) => ApiResponse::Json(200, v), Err(e) => json_err(400, &e.to_string()) };
    }
    if parts.len() == 3 && parts[0] == "roles" && parts[2] == "permissions" && *method == Method::Put {
        if let Err(e) = rbac::require_owner(conn, &user_id) { return json_err(403, &e.to_string()); }
        let role_id = parts[1].to_string();
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "invalid body") };
        let module_id = obj.get("module_id").and_then(Value::as_str).unwrap_or("").to_string();
        let actions: Vec<String> = obj
            .get("actions")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        if module_id.is_empty() { return json_err(400, "module_id is required"); }
        return match roles::set_permissions(conn, &business_id, &role_id, &module_id, &actions) {
            Ok(()) => {
                let _ = audit::log(conn, &business_id, Some(&user_id), "_roles", "set_permissions", Some(&role_id), Some(&json!({"module_id": module_id, "actions": actions})));
                ApiResponse::Json(200, json!({"ok": true}))
            }
            Err(e) => json_err(400, &e.to_string()),
        };
    }

    // ---- Units of measure — user-addable master data, referenced by
    // any module field with `"type": "unit"`. Reading is open to any
    // authenticated user (needed to populate a dropdown on the create
    // form); managing the list requires admin tier. ----
    if parts.as_slice() == ["units"] && *method == Method::Get {
        return match reference_data::list_units(conn, &business_id) { Ok(v) => ApiResponse::Json(200, json!({"units": v})), Err(e) => json_err(500, &e.to_string()) };
    }
    if parts.as_slice() == ["units"] && *method == Method::Post {
        if let Err(e) = rbac::require_admin_tier(conn, &user_id) { return json_err(403, &e.to_string()); }
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "invalid body") };
        let name = obj.get("name").and_then(Value::as_str).unwrap_or("");
        let abbr = obj.get("abbreviation").and_then(Value::as_str);
        return match reference_data::create_unit(conn, &business_id, name, abbr) {
            Ok(id) => {
                let _ = audit::log(conn, &business_id, Some(&user_id), "_units", "create", Some(&id), Some(&json!({"name": name})));
                ApiResponse::Json(201, json!({"id": id}))
            }
            Err(e) => json_err(400, &e.to_string()),
        };
    }
    if parts.len() == 2 && parts[0] == "units" && *method == Method::Delete {
        if let Err(e) = rbac::require_admin_tier(conn, &user_id) { return json_err(403, &e.to_string()); }
        let unit_id = parts[1];
        return match reference_data::delete_unit(conn, &business_id, unit_id) {
            Ok(()) => {
                let _ = audit::log(conn, &business_id, Some(&user_id), "_units", "delete", Some(unit_id), None);
                ApiResponse::Json(200, json!({"deleted": true}))
            }
            Err(e) => json_err(400, &e.to_string()),
        };
    }

    // ---- Currencies — same pattern as units. ----
    if parts.as_slice() == ["currencies"] && *method == Method::Get {
        return match reference_data::list_currencies(conn, &business_id) { Ok(v) => ApiResponse::Json(200, json!({"currencies": v})), Err(e) => json_err(500, &e.to_string()) };
    }
    if parts.as_slice() == ["currencies"] && *method == Method::Post {
        if let Err(e) = rbac::require_admin_tier(conn, &user_id) { return json_err(403, &e.to_string()); }
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "invalid body") };
        let code = obj.get("code").and_then(Value::as_str).unwrap_or("");
        let symbol = obj.get("symbol").and_then(Value::as_str);
        let name = obj.get("name").and_then(Value::as_str);
        return match reference_data::create_currency(conn, &business_id, code, symbol, name) {
            Ok(id) => {
                let _ = audit::log(conn, &business_id, Some(&user_id), "_currencies", "create", Some(&id), Some(&json!({"code": code})));
                ApiResponse::Json(201, json!({"id": id}))
            }
            Err(e) => json_err(400, &e.to_string()),
        };
    }
    if parts.len() == 2 && parts[0] == "currencies" && *method == Method::Delete {
        if let Err(e) = rbac::require_admin_tier(conn, &user_id) { return json_err(403, &e.to_string()); }
        let currency_id = parts[1];
        return match reference_data::delete_currency(conn, &business_id, currency_id) {
            Ok(()) => {
                let _ = audit::log(conn, &business_id, Some(&user_id), "_currencies", "delete", Some(currency_id), None);
                ApiResponse::Json(200, json!({"deleted": true}))
            }
            Err(e) => json_err(400, &e.to_string()),
        };
    }

    // ---- Settings — generic key/value (theme, locale, etc). ----
    if parts.as_slice() == ["settings"] && *method == Method::Get {
        return match settings::get_all(conn, &business_id) { Ok(v) => ApiResponse::Json(200, v), Err(e) => json_err(500, &e.to_string()) };
    }
    if parts.as_slice() == ["settings"] && *method == Method::Put {
        if let Err(e) = rbac::require_admin_tier(conn, &user_id) { return json_err(403, &e.to_string()); }
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "invalid body") };
        let key = obj.get("key").and_then(Value::as_str).unwrap_or("");
        let value = obj.get("value").and_then(Value::as_str).unwrap_or("");
        return match settings::set(conn, &business_id, key, value) {
            Ok(()) => {
                let _ = audit::log(conn, &business_id, Some(&user_id), "_settings", "set", None, Some(&json!({"key": key, "value": value})));
                ApiResponse::Json(200, json!({"ok": true}))
            }
            Err(e) => json_err(400, &e.to_string()),
        };
    }

    // GET /ai/settings — admin-only (unlike GET /settings above, which
    // deliberately excludes API keys and stays open to every role).
    // Returns which providers have a key configured, never the key
    // itself — same "never show a raw secret back" discipline as any
    // real settings screen. Saving still goes through the existing
    // PUT /settings, already admin-gated, using keys like
    // "ai_nvidia_api_key" — no separate write endpoint needed.
    if parts.as_slice() == ["ai", "settings"] && *method == Method::Get {
        if let Err(e) = rbac::require_admin_tier(conn, &user_id) { return json_err(403, &e.to_string()); }
        let all = match settings::get_all_including_keys(conn, &business_id) {
            Ok(v) => v,
            Err(e) => return json_err(500, &e.to_string()),
        };
        let has = |k: &str| all.get(k).and_then(Value::as_str).map(|s| !s.trim().is_empty()).unwrap_or(false);
        return ApiResponse::Json(200, json!({
            "provider": all.get("ai_provider").and_then(Value::as_str).unwrap_or("nvidia"),
            "nvidia_key_set": has("ai_nvidia_api_key"),
            "gemini_key_set": has("ai_gemini_api_key"),
            "openai_key_set": has("ai_openai_api_key"),
            "claude_key_set": has("ai_claude_api_key"),
        }));
    }

    // ---- OCR import: photograph a paper ledger, extract text, propose candidate records ----
    if parts.as_slice() == ["import", "ocr", "extract"] && *method == Method::Post {
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "body must include 'image_base64'") };
        let b64 = match obj.get("image_base64").and_then(Value::as_str) {
            Some(s) => s,
            None => return json_err(400, "'image_base64' is required"),
        };
        use base64::Engine;
        let bytes = match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(b) => b,
            Err(_) => return json_err(400, "'image_base64' is not valid base64"),
        };
        return match ocr_import::extract_text(&bytes) {
            Ok(text) => ApiResponse::Json(200, json!({"raw_text": text})),
            Err(e) => json_err(502, &e.to_string()),
        };
    }

    if parts.as_slice() == ["import", "ocr", "parse"] && *method == Method::Post {
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "invalid body") };
        let module_id = match obj.get("module_id").and_then(Value::as_str) {
            Some(m) => m,
            None => return json_err(400, "'module_id' is required"),
        };
        if let Err(e) = rbac::require(conn, &user_id, module_id, "create") { return json_err(403, &e.to_string()); }
        let raw_text = match obj.get("raw_text").and_then(Value::as_str) {
            Some(t) => t,
            None => return json_err(400, "'raw_text' is required"),
        };
        let schema_json: Result<String, _> = conn.query_row(
            "SELECT schema_json FROM modules WHERE business_id = ?1 AND id = ?2 AND enabled = 1",
            rusqlite::params![business_id, module_id],
            |r| r.get(0),
        );
        let module = match schema_json {
            Ok(raw) => match crate::module::ModuleDef::from_json_str(&raw) {
                Ok(m) => m,
                Err(e) => return json_err(500, &e.to_string()),
            },
            Err(_) => return json_err(404, &format!("module '{module_id}' is not enabled for this business")),
        };
        let candidates = ocr_import::parse_into_candidates(&module, raw_text);
        return ApiResponse::Json(200, json!({"candidates": candidates}));
    }

    // ---- Bulk create: used by the OCR import "confirm and import" step,
    // also generally useful for CSV-style imports ----
    if parts.len() == 4 && parts[0] == "modules" && parts[2] == "records" && parts[3] == "bulk" && *method == Method::Post {
        let module_id = parts[1];
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "invalid body") };
        let records = match obj.get("records").and_then(Value::as_array) {
            Some(r) => r,
            None => return json_err(400, "'records' must be an array"),
        };
        let mut created = 0;
        let mut errors = Vec::new();
        for (i, rec) in records.iter().enumerate() {
            let rec_obj = match rec.as_object() {
                Some(o) => o,
                None => { errors.push(json!({"index": i, "error": "not a JSON object"})); continue; }
            };
            match crud::create(conn, &business_id, &user_id, module_id, rec_obj) {
                Ok(_) => created += 1,
                Err(e) => errors.push(json!({"index": i, "error": e.to_string()})),
            }
        }
        return ApiResponse::Json(200, json!({"created": created, "errors": errors}));
    }

    // GET /modules/{id}/import-template — a downloadable .xlsx with
    // this module's real field names as headers, so what comes back
    // via /import-excel below is unambiguous.
    if parts.len() == 3 && parts[0] == "modules" && parts[2] == "import-template" && *method == Method::Get {
        let module_id = parts[1];
        let module = match crud::load_module(conn, &business_id, module_id) {
            Ok(m) => m,
            Err(e) => return json_err(404, &e.to_string()),
        };
        return match excel_import::generate_template(&module) {
            Ok(bytes) => ApiResponse::Xlsx(200, bytes, format!("{module_id}_import_template.xlsx")),
            Err(e) => json_err(500, &e.to_string()),
        };
    }

    // POST /modules/{id}/import-excel {"file_base64": "...", "key_field": "sku"}
    // See excel_import.rs — creates new records or updates matching
    // ones by key_field, same validation as a hand-typed record.
    if parts.len() == 3 && parts[0] == "modules" && parts[2] == "import-excel" && *method == Method::Post {
        let module_id = parts[1];
        let module = match crud::load_module(conn, &business_id, module_id) {
            Ok(m) => m,
            Err(e) => return json_err(404, &e.to_string()),
        };
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "invalid body") };
        let file_b64 = match obj.get("file_base64").and_then(Value::as_str) {
            Some(s) => s,
            None => return json_err(400, "'file_base64' is required"),
        };
        let key_field = obj.get("key_field").and_then(Value::as_str).unwrap_or_else(|| {
            module.fields.first().map(|f| f.name.as_str()).unwrap_or("id")
        });
        use base64::Engine;
        let bytes = match base64::engine::general_purpose::STANDARD.decode(file_b64) {
            Ok(b) => b,
            Err(e) => return json_err(400, &format!("invalid base64 file data: {e}")),
        };
        return match excel_import::import(conn, &business_id, &user_id, &module, bytes, key_field) {
            Ok(result) => {
                let _ = audit::log(conn, &business_id, Some(&user_id), module_id, "excel_import",
                    None, Some(&json!({"created": result.created, "updated": result.updated, "error_count": result.errors.len()})));
                ApiResponse::Json(200, json!({"created": result.created, "updated": result.updated, "errors": result.errors}))
            }
            Err(e) => json_err(400, &e.to_string()),
        };
    }

    // ---- Point of sale: the real link between Sales and Inventory —
    // see pos.rs for why this is its own module rather than going
    // through the generic create/update endpoints directly. ----
    if parts.as_slice() == ["pos", "checkout"] && *method == Method::Post {
        let req: pos::CheckoutRequest = match serde_json::from_str(body) {
            Ok(r) => r,
            Err(e) => return json_err(400, &format!("invalid checkout request: {e}")),
        };
        return match pos::checkout(conn, &business_id, &user_id, req) {
            Ok(summary) => ApiResponse::Json(200, summary),
            Err(e) => crud_error(&e),
        };
    }
    if parts.len() == 3 && parts[0] == "pos" && parts[1] == "orders" && *method == Method::Get {
        let order_id = parts[2];
        return match pos::get_order(conn, &business_id, order_id) {
            Ok(order) => ApiResponse::Json(200, order),
            Err(e) => json_err(404, &e.to_string()),
        };
    }

    // ---- Refunds: the counterpart to POS checkout — see refund.rs
    // for why this is its own module rather than the generic create
    // endpoint. ----
    if parts.as_slice() == ["sales", "refund"] && *method == Method::Post {
        let req: refund::RefundRequest = match serde_json::from_str(body) {
            Ok(r) => r,
            Err(e) => return json_err(400, &format!("invalid refund request: {e}")),
        };
        return match refund::process_refund(conn, &business_id, &user_id, req) {
            Ok(summary) => ApiResponse::Json(200, summary),
            Err(e) => crud_error(&e),
        };
    }

    // ---- Receiving stock: the buying-side counterpart to POS — see
    // receiving.rs for why this is its own module rather than the
    // generic update endpoint. ----
    if parts.as_slice() == ["purchasing", "receive"] && *method == Method::Post {
        let req: receiving::ReceiveRequest = match serde_json::from_str(body) {
            Ok(r) => r,
            Err(e) => return json_err(400, &format!("invalid receive request: {e}")),
        };
        return match receiving::receive(conn, &business_id, &user_id, req) {
            Ok(summary) => ApiResponse::Json(200, summary),
            Err(e) => crud_error(&e),
        };
    }

    // ---- Repacking / breaking bulk: see repack.rs. ----
    if parts.as_slice() == ["inventory", "repack"] && *method == Method::Post {
        let req: repack::RepackRequest = match serde_json::from_str(body) {
            Ok(r) => r,
            Err(e) => return json_err(400, &format!("invalid repack request: {e}")),
        };
        return match repack::repack(conn, &business_id, &user_id, req) {
            Ok(summary) => ApiResponse::Json(200, summary),
            Err(e) => crud_error(&e),
        };
    }

    // ---- Audit log: the actual point of recording all of this is being
    // able to look at it. Owner-only — this is oversight data about
    // what every user in the business has done, not something a Staff
    // account should be able to read about themselves or others.
    if parts.as_slice() == ["audit-log"] && *method == Method::Get {
        if let Err(e) = rbac::require_owner(conn, &user_id) { return json_err(403, &e.to_string()); }
        let q = query_params(url);
        let limit: i64 = q.get("limit").and_then(|s| s.parse().ok()).unwrap_or(100).min(1000);
        let module_filter = q.get("module_id").cloned();

        let (sql, use_filter) = if module_filter.is_some() {
            ("SELECT id, user_id, module_id, action, record_id, details_json, timestamp
              FROM audit_log WHERE business_id = ?1 AND module_id = ?2 ORDER BY timestamp DESC LIMIT ?3", true)
        } else {
            ("SELECT id, user_id, module_id, action, record_id, details_json, timestamp
              FROM audit_log WHERE business_id = ?1 ORDER BY timestamp DESC LIMIT ?2", false)
        };

        let mut stmt = match conn.prepare(sql) { Ok(s) => s, Err(e) => return json_err(500, &e.to_string()) };
        let map_row = |r: &rusqlite::Row| -> rusqlite::Result<Value> {
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "user_id": r.get::<_, Option<String>>(1)?,
                "module_id": r.get::<_, String>(2)?,
                "action": r.get::<_, String>(3)?,
                "record_id": r.get::<_, Option<String>>(4)?,
                "details": r.get::<_, Option<String>>(5)?.and_then(|s| serde_json::from_str::<Value>(&s).ok()),
                "timestamp": r.get::<_, String>(6)?,
            }))
        };
        let rows = if use_filter {
            stmt.query_map(rusqlite::params![business_id, module_filter.unwrap(), limit], map_row)
        } else {
            stmt.query_map(rusqlite::params![business_id, limit], map_row)
        };
        return match rows.and_then(|r| r.collect::<rusqlite::Result<Vec<_>>>()) {
            Ok(list) => ApiResponse::Json(200, json!({"entries": list})),
            Err(e) => json_err(500, &e.to_string()),
        };
    }

    // ---- Onboarding wizard: POST /onboarding/setup {"business_type": "retail"} ----
    if parts.as_slice() == ["onboarding", "setup"] && *method == Method::Post {
        if let Err(e) = rbac::require_owner(conn, &user_id) { return json_err(403, &e.to_string()); }
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "body must be JSON with 'business_type'") };
        let business_type = match obj.get("business_type").and_then(Value::as_str) {
            Some(t) => t,
            None => return json_err(400, "'business_type' is required (retail, food, services, manufacturing)"),
        };
        return match onboarding::apply_business_type(conn, &business_id, business_type) {
            Ok(enabled) => {
                let _ = audit::log(conn, &business_id, Some(&user_id), "_onboarding", "apply_business_type",
                    None, Some(&json!({"business_type": business_type, "enabled_modules": enabled})));
                ApiResponse::Json(200, json!({"enabled_modules": enabled}))
            }
            Err(e) => json_err(400, &e.to_string()),
        };
    }

    // ---- Notifications: WhatsApp/SMS ----
    if parts.as_slice() == ["notifications", "send"] && *method == Method::Post {
        if let Err(e) = rbac::require_admin_tier(conn, &user_id) { return json_err(403, &e.to_string()); }
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "invalid body") };
        let g = |k: &str| obj.get(k).and_then(Value::as_str).unwrap_or("");
        if g("channel").is_empty() || g("recipient").is_empty() || g("message").is_empty() {
            return json_err(400, "'channel', 'recipient', and 'message' are all required");
        }
        return match notifications::send(conn, &business_id, g("channel"), g("recipient"), g("message")) {
            Ok(rec) => ApiResponse::Json(200, json!(rec)),
            Err(e) => json_err(400, &e.to_string()),
        };
    }
    if parts.as_slice() == ["notifications", "low-stock-alert"] && *method == Method::Post {
        if let Err(e) = rbac::require_admin_tier(conn, &user_id) { return json_err(403, &e.to_string()); }
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "invalid body") };
        let g = |k: &str| obj.get(k).and_then(Value::as_str).unwrap_or("");
        if g("channel").is_empty() || g("recipient").is_empty() {
            return json_err(400, "'channel' and 'recipient' are required");
        }
        return match notifications::send_low_stock_alert(conn, &business_id, &user_id, g("channel"), g("recipient")) {
            Ok(rec) => ApiResponse::Json(200, json!(rec)),
            Err(e) => json_err(400, &e.to_string()),
        };
    }
    if parts.as_slice() == ["notifications"] && *method == Method::Get {
        return match notifications::list_recent(conn, &business_id, 50) {
            Ok(list) => ApiResponse::Json(200, json!({"notifications": list})),
            Err(e) => json_err(400, &e.to_string()),
        };
    }

    // ---- Module registry: what a generic frontend needs to render itself ----
    if parts.as_slice() == ["modules"] && *method == Method::Get {
        return match crate::business_panel::list_modules(conn, &business_id) {
            Ok(list) => ApiResponse::Json(200, json!({"modules": list.into_iter().map(|m| json!({
                "id": m.id, "display_name": m.display_name, "enabled": m.enabled
            })).collect::<Vec<_>>()})),
            Err(e) => json_err(400, &e.to_string()),
        };
    }
    if parts.len() == 3 && parts[0] == "modules" && parts[2] == "schema" && *method == Method::Get {
        let module_id = parts[1];
        let schema_json: Result<String, _> = conn.query_row(
            "SELECT schema_json FROM modules WHERE business_id = ?1 AND id = ?2 AND enabled = 1",
            rusqlite::params![business_id, module_id],
            |r| r.get(0),
        );
        return match schema_json {
            Ok(raw) => match serde_json::from_str::<Value>(&raw) {
                Ok(mut v) => {
                    // `actions` in the raw schema is the module's theoretical
                    // capability list — the same for every business regardless
                    // of who's asking. It is NOT what the current user is
                    // actually permitted to do. Compute that here, per-user,
                    // so the frontend can hide (not just disable-on-click)
                    // actions this specific person doesn't have — a Staff
                    // account was previously shown "+ New"/"Delete" buttons
                    // that would 403 on click, which is a real UX gap: the UI
                    // should never offer an action it already knows will fail.
                    let all_actions = v["actions"].as_array().cloned().unwrap_or_default();
                    let my_permissions: Vec<Value> = all_actions.into_iter()
                        .filter(|a| a.as_str().map(|s| rbac::is_allowed(conn, &user_id, module_id, s).unwrap_or(false)).unwrap_or(false))
                        .collect();
                    v["my_permissions"] = json!(my_permissions);
                    ApiResponse::Json(200, v)
                }
                Err(e) => json_err(500, &e.to_string()),
            },
            Err(_) => json_err(404, &format!("module '{module_id}' is not enabled for this business")),
        };
    }

    // ---- Module CRUD ----
    if parts.len() >= 3 && parts[0] == "modules" && parts[2] == "records" {
        let module_id = parts[1];
        let record_id = parts.get(3).copied();
        return match (method, record_id) {
            (Method::Get, None) => {
                let q = query_params(url);
                match crud::list(conn, &business_id, &user_id, module_id, q.get("search").map(|s| s.as_str()), 50, 0) {
                    Ok(records) => ApiResponse::Json(200, json!({"records": records})),
                    Err(e) => crud_error(&e),
                }
            }
            (Method::Post, None) => match json_body(body) {
                Some(obj) => match crud::create(conn, &business_id, &user_id, module_id, &obj) {
                    Ok(id) => ApiResponse::Json(201, json!({"id": id})),
                    Err(e) => crud_error(&e),
                },
                None => json_err(400, "body must be a JSON object"),
            },
            (Method::Put, Some(id)) => match json_body(body) {
                Some(obj) => match crud::update(conn, &business_id, &user_id, module_id, id, &obj) {
                    Ok(()) => ApiResponse::Json(200, json!({"updated": true})),
                    Err(e) => crud_error(&e),
                },
                None => json_err(400, "body must be a JSON object"),
            },
            (Method::Delete, Some(id)) => match crud::delete(conn, &business_id, &user_id, module_id, id) {
                Ok(()) => ApiResponse::Json(200, json!({"deleted": true})),
                Err(e) => crud_error(&e),
            },
            _ => json_err(404, "not found"),
        };
    }

    // ---- Raw data export: /modules/{id}/export — real .xlsx ----
    if parts.len() == 3 && parts[0] == "modules" && parts[2] == "export" && *method == Method::Get {
        let module_id = parts[1];
        // The module's own field definitions are what tell the
        // exporter which columns are money (integer cents) so it can
        // write a proper decimal-formatted currency value instead of
        // a raw, misleading cents integer.
        let module_def = match crud::load_module(conn, &business_id, module_id) {
            Ok(m) => m,
            Err(e) => return json_err(400, &e.to_string()),
        };
        return match crud::list(conn, &business_id, &user_id, module_id, None, 100000, 0) {
            Ok(records) => match xlsx_export::records_to_xlsx(&records, module_id, &module_def) {
                Ok(bytes) => {
                    let _ = audit::log(conn, &business_id, Some(&user_id), module_id, "export",
                        None, Some(&json!({"record_count": records.len()})));
                    ApiResponse::Xlsx(200, bytes, format!("{module_id}_export.xlsx"))
                }
                Err(e) => json_err(500, &e.to_string()),
            },
            Err(e) => crud_error(&e),
        };
    }

    // ---- Report (view): /modules/{id}/report?measure=&agg=&dimension=time|category&field=&bucket=&start=&end= ----
    if parts.len() == 3 && parts[0] == "modules" && parts[2] == "report" && *method == Method::Get {
        let module_id = parts[1];
        let q = query_params(url);
        return match build_report(conn, &business_id, &user_id, module_id, &q) {
            Ok(points) => ApiResponse::Json(200, json!({"report": points})),
            Err(e) => json_err(400, &e.to_string()),
        };
    }

    // ---- Report export: same params, but returns .xlsx ----
    if parts.len() == 4 && parts[0] == "modules" && parts[2] == "report" && parts[3] == "export" && *method == Method::Get {
        let module_id = parts[1];
        let q = query_params(url);
        return match build_report(conn, &business_id, &user_id, module_id, &q) {
            Ok(points) => {
                let measure_label = q.get("measure").cloned().unwrap_or_else(|| "count".to_string());
                match xlsx_export::report_to_xlsx(&points, &measure_label) {
                    Ok(bytes) => {
                        let _ = audit::log(conn, &business_id, Some(&user_id), module_id, "export_report",
                            None, Some(&json!({"measure": measure_label, "point_count": points.len()})));
                        ApiResponse::Xlsx(200, bytes, format!("{module_id}_report.xlsx"))
                    }
                    Err(e) => json_err(500, &e.to_string()),
                }
            }
            Err(e) => json_err(400, &e.to_string()),
        };
    }

    // ---- AI context transparency: GET /ai/context — shows exactly what
    // data the assistant would be grounded in, without calling the API.
    // Doubles as a trust feature ("what does the AI see about my business?")
    // and as a way to verify the context builder independent of network
    // access to the Claude API.
    if parts.as_slice() == ["ai", "context"] && *method == Method::Get {
        return match crate::ai_context::build_snapshot(conn, &business_id, &user_id) {
            Ok(snapshot) => ApiResponse::Json(200, snapshot),
            Err(e) => json_err(400, &e.to_string()),
        };
    }

    // ---- AI floating assistant: POST /ai/ask {question} ----
    if parts.as_slice() == ["ai", "ask"] && *method == Method::Post {
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "body must be JSON with a 'question' field") };
        let question = match obj.get("question").and_then(Value::as_str) {
            Some(q) if !q.trim().is_empty() => q,
            _ => return json_err(400, "'question' is required"),
        };
        return match ai_assistant::ask(conn, &business_id, &user_id, question) {
            Ok(answer) => ApiResponse::Json(200, json!({"answer": answer})),
            Err(e) => json_err(502, &e.to_string()),
        };
    }

    // ---- Forecast: /modules/{id}/forecast?measure=&bucket=&method=&window=&alpha= ----
    if parts.len() == 3 && parts[0] == "modules" && parts[2] == "forecast" && *method == Method::Get {
        let module_id = parts[1];
        let q = query_params(url);
        let measure = match q.get("measure") { Some(m) => m, None => return json_err(400, "'measure' query param is required") };
        let bucket = q.get("bucket").cloned().unwrap_or_else(|| "month".to_string());
        let result = match q.get("method").map(|s| s.as_str()).unwrap_or("moving_average") {
            "exponential_smoothing" => {
                let alpha: f64 = q.get("alpha").and_then(|s| s.parse().ok()).unwrap_or(0.5);
                forecast::exponential_smoothing_forecast(conn, &business_id, &user_id, module_id, measure, &bucket, alpha)
            }
            _ => {
                let window: usize = q.get("window").and_then(|s| s.parse().ok()).unwrap_or(3);
                forecast::moving_average_forecast(conn, &business_id, &user_id, module_id, measure, &bucket, window)
            }
        };
        return match result {
            Ok(r) => ApiResponse::Json(200, json!(r)),
            Err(e) => json_err(400, &e.to_string()),
        };
    }

    // ---- Receipts ----
    if parts.len() == 3 && parts[0] == "pos" && parts[1] == "receipt" && *method == Method::Get {
        let order_id = parts[2];
        return match crate::receipt::generate(conn, &business_id, &user_id, order_id) {
            Ok(receipt) => ApiResponse::Json(200, json!(receipt)),
            Err(e) => json_err(404, &e.to_string()),
        };
    }

    // ---- Customers & lifetime value — see customers.rs. Read-only
    // from the HTTP layer on purpose: a customer record is only ever
    // created as a side effect of a real POS checkout, never directly. ----
    if parts.as_slice() == ["customers"] && *method == Method::Get {
        if let Err(e) = rbac::require(conn, &user_id, "sales", "read") { return json_err(403, &e.to_string()); }
        return match crate::customers::list(conn, &business_id) {
            Ok(v) => ApiResponse::Json(200, v),
            Err(e) => json_err(500, &e.to_string()),
        };
    }
    if parts.len() == 2 && parts[0] == "customers" && *method == Method::Get {
        if let Err(e) = rbac::require(conn, &user_id, "sales", "read") { return json_err(403, &e.to_string()); }
        return match crate::customers::detail(conn, &business_id, parts[1]) {
            Ok(v) => ApiResponse::Json(200, v),
            Err(e) => json_err(404, &e.to_string()),
        };
    }

    // ---- Invoices ----
    if parts.as_slice() == ["invoices"] && *method == Method::Post {
        let req: crate::invoice::CreateInvoiceRequest = match serde_json::from_str(body) {
            Ok(r) => r,
            Err(e) => return json_err(400, &format!("invalid invoice request: {e}")),
        };
        return match crate::invoice::create_invoice(conn, &business_id, &user_id, req) {
            Ok(v) => ApiResponse::Json(201, v),
            Err(e) => json_err(400, &e.to_string()),
        };
    }
    if parts.len() == 3 && parts[0] == "invoices" && parts[2] == "send" && *method == Method::Post {
        return match crate::invoice::mark_sent(conn, &business_id, &user_id, parts[1]) {
            Ok(()) => ApiResponse::Json(200, json!({"status": "sent"})),
            Err(e) => json_err(400, &e.to_string()),
        };
    }
    if parts.len() == 3 && parts[0] == "invoices" && parts[2] == "pay" && *method == Method::Post {
        return match crate::invoice::mark_paid(conn, &business_id, &user_id, parts[1]) {
            Ok(()) => ApiResponse::Json(200, json!({"status": "paid"})),
            Err(e) => json_err(400, &e.to_string()),
        };
    }
    if parts.len() == 3 && parts[0] == "invoices" && parts[2] == "cancel" && *method == Method::Post {
        return match crate::invoice::mark_cancelled(conn, &business_id, &user_id, parts[1]) {
            Ok(()) => ApiResponse::Json(200, json!({"status": "cancelled"})),
            Err(e) => json_err(400, &e.to_string()),
        };
    }

    // ---- Business branding (logo + slogan) ----
    if parts.as_slice() == ["business", "branding"] && *method == Method::Get {
        return match crate::business_branding::get_branding(conn, &business_id) {
            Ok(v) => ApiResponse::Json(200, v),
            Err(e) => json_err(500, &e.to_string()),
        };
    }
    if parts.as_slice() == ["business", "branding"] && *method == Method::Put {
        if let Err(e) = rbac::require_admin_tier(conn, &user_id) { return json_err(403, &e.to_string()); }
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "invalid body") };
        let logo_b64 = obj.get("logo_base64").and_then(|v| v.as_str());
        let slogan = obj.get("slogan").and_then(|v| v.as_str());
        let app_data_dir = std::path::PathBuf::from(
            std::env::var("SME_APP_DATA_DIR").unwrap_or_else(|_| "./".to_string())
        );
        return match crate::business_branding::update_branding(conn, &business_id, logo_b64, slogan, &app_data_dir) {
            Ok(path) => {
                let _ = audit::log(conn, &business_id, Some(&user_id), "_business", "branding_update", None, None);
                ApiResponse::Json(200, json!({"updated": true, "logo_path": path}))
            }
            Err(e) => json_err(400, &e.to_string()),
        };
    }
    // ---- 2FA management (setup/verify/status/disable — the login-time
    // check is up in the public routes section above, alongside
    // /auth/login and /auth/2fa/login) ----
    if parts.as_slice() == ["auth", "2fa", "setup"] && *method == Method::Post {
        let username: String = conn.query_row(
            "SELECT username FROM users WHERE id = ?1", rusqlite::params![user_id], |r| r.get(0)
        ).unwrap_or_default();
        return match crate::totp::generate_secret(conn, &user_id, &username) {
            Ok(setup) => ApiResponse::Json(200, json!(setup)),
            Err(e) => json_err(400, &e.to_string()),
        };
    }
    if parts.as_slice() == ["auth", "2fa", "verify"] && *method == Method::Post {
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "invalid body") };
        let code = obj.get("code").and_then(Value::as_str).unwrap_or("");
        return match crate::totp::verify_and_enable(conn, &user_id, code) {
            Ok(true) => ApiResponse::Json(200, json!({"enabled": true})),
            Ok(false) => json_err(400, "invalid TOTP code — 2FA not enabled"),
            Err(e) => json_err(400, &e.to_string()),
        };
    }
    if parts.as_slice() == ["auth", "2fa", "status"] && *method == Method::Get {
        return match crate::totp::status(conn, &user_id) {
            Ok(s) => ApiResponse::Json(200, json!(s)),
            Err(e) => json_err(500, &e.to_string()),
        };
    }
    if parts.as_slice() == ["auth", "2fa", "disable"] && *method == Method::Post {
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "invalid body") };
        let code = obj.get("code").and_then(Value::as_str).unwrap_or("");
        return match crate::totp::disable(conn, &user_id, code) {
            Ok(()) => ApiResponse::Json(200, json!({"disabled": true})),
            Err(e) => json_err(400, &e.to_string()),
        };
    }

    // ---- Tax engine ----
    if parts.as_slice() == ["tax", "rates"] && *method == Method::Get {
        return match crate::tax::list_rates(conn, &business_id) {
            Ok(rates) => ApiResponse::Json(200, json!({"rates": rates})),
            Err(e) => json_err(500, &e.to_string()),
        };
    }
    if parts.as_slice() == ["tax", "rates"] && *method == Method::Post {
        if let Err(e) = rbac::require_owner(conn, &user_id) { return json_err(403, &e.to_string()); }
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "invalid body") };
        let category = obj.get("category").and_then(Value::as_str).unwrap_or("");
        let rate = obj.get("rate").and_then(Value::as_f64).unwrap_or(0.0);
        return match crate::tax::set_category_rate(conn, &business_id, &user_id, category, rate) {
            Ok(()) => ApiResponse::Json(200, json!({"ok": true})),
            Err(e) => json_err(400, &e.to_string()),
        };
    }
    if parts.as_slice() == ["tax", "compute"] && *method == Method::Post {
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "invalid body") };
        let empty = Vec::new();
        let items = obj.get("items").and_then(Value::as_array).unwrap_or(&empty);
        let tax_inclusive = obj.get("tax_inclusive").and_then(Value::as_bool).unwrap_or(false);
        let parsed: Vec<(String, i64, i64)> = items.iter().filter_map(|v| {
            let cat = v.get("category")?.as_str()?.to_string();
            // Integer minor units (cents) on the wire — see money.rs.
            // A caller sending a fractional dollar value here is a bug
            // upstream, not something to coerce; as_i64() correctly
            // returns None for it rather than silently truncating.
            let price = v.get("unit_price")?.as_i64()?;
            let qty = v.get("quantity")?.as_i64()?;
            Some((cat, price, qty))
        }).collect();
        return match crate::tax::compute(conn, &business_id, &parsed, tax_inclusive) {
            Ok(summary) => ApiResponse::Json(200, json!(summary)),
            Err(e) => json_err(400, &e.to_string()),
        };
    }

    // ---- Currency exchange ----
    if parts.as_slice() == ["currency", "rates"] && *method == Method::Get {
        let q = query_params(url);
        let base = q.get("base").map(|s| s.as_str()).unwrap_or("USD");
        return match crate::currency::list_rates(conn, base) {
            Ok(rates) => ApiResponse::Json(200, json!({"rates": rates, "stale": crate::currency::rates_stale(conn, base).unwrap_or(true)})),
            Err(e) => json_err(500, &e.to_string()),
        };
    }
    if parts.as_slice() == ["currency", "convert"] && *method == Method::Post {
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "invalid body") };
        let from = obj.get("from").and_then(Value::as_str).unwrap_or("USD");
        let to = obj.get("to").and_then(Value::as_str).unwrap_or("USD");
        // Integer minor units (cents) — see money.rs.
        let amount = obj.get("amount").and_then(Value::as_i64).unwrap_or(0);
        return match crate::currency::convert(conn, from, to, amount) {
            Ok(result) => ApiResponse::Json(200, json!({"from": from, "to": to, "amount": amount, "result": result})),
            Err(e) => json_err(400, &e.to_string()),
        };
    }
    if parts.as_slice() == ["currency", "refresh"] && *method == Method::Post {
        if let Err(e) = rbac::require_owner(conn, &user_id) { return json_err(403, &e.to_string()); }
        let q = query_params(url);
        let base = q.get("base").map(|s| s.as_str()).unwrap_or("USD").to_string();
        return match crate::currency::refresh_rates(conn, &base) {
            Ok(()) => ApiResponse::Json(200, json!({"refreshed": true})),
            Err(e) => json_err(500, &e.to_string()),
        };
    }

    // ---- Module registry: enable an additional module beyond the
    // business-type preset (e.g. turning on Invoices after setup) ----
    if parts.len() == 3 && parts[0] == "modules" && parts[2] == "enable" && *method == Method::Post {
        if let Err(e) = rbac::require_owner(conn, &user_id) { return json_err(403, &e.to_string()); }
        let module_id = parts[1];
        let path = crate::modules_dir().join(format!("{module_id}.json"));
        if !path.exists() {
            return json_err(404, &format!("unknown module '{module_id}'"));
        }
        return match crate::business_panel::enable_module(conn, &business_id, &path.to_string_lossy()) {
            Ok(()) => {
                let _ = audit::log(conn, &business_id, Some(&user_id), "_modules", "enable", None, Some(&json!({"module_id": module_id})));
                ApiResponse::Json(200, json!({"enabled": module_id}))
            }
            Err(e) => json_err(400, &e.to_string()),
        };
    }

    json_err(404, "not found")
}

fn build_report(
    conn: &Connection,
    business_id: &str,
    user_id: &str,
    module_id: &str,
    q: &HashMap<String, String>,
) -> anyhow::Result<Vec<report::ReportPoint>> {
    let agg = q.get("agg").map(|s| s.as_str()).unwrap_or("sum");
    let measure = q.get("measure").map(|s| s.as_str());

    let dimension = match q.get("dimension").map(|s| s.as_str()) {
        Some("time") => {
            let field = q.get("field").map(|s| s.as_str()).unwrap_or("created_at");
            let bucket = report::parse_time_bucket(q.get("bucket").map(|s| s.as_str()).unwrap_or("month"))?;
            Dimension::Time { field, bucket }
        }
        Some("category") => {
            let field = q.get("field").ok_or_else(|| anyhow::anyhow!("'field' is required for dimension=category"))?;
            Dimension::Category { field }
        }
        _ => Dimension::None,
    };

    report::run(
        conn,
        business_id,
        user_id,
        module_id,
        measure,
        agg,
        dimension,
        q.get("start").map(|s| s.as_str()),
        q.get("end").map(|s| s.as_str()),
    )
}

fn json_err(status: u16, msg: &str) -> ApiResponse {
    ApiResponse::Json(status, json!({"error": msg}))
}

fn crud_error(e: &anyhow::Error) -> ApiResponse {
    let msg = e.to_string();
    let status = if msg.starts_with(rbac::PERMISSION_DENIED_PREFIX) { 403 } else { 400 };
    ApiResponse::Json(status, json!({"error": msg}))
}
