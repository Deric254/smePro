use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex};
use tiny_http::{Header, Method, Response, Server};

use crate::rate_limit::RateLimiter;
use crate::report::Dimension;
use crate::{ai_assistant, audit, auth, crud, forecast, license, notifications, ocr_import, onboarding, payment, rbac, reference_data, report, roles, settings, users, xlsx_export};
use std::time::Duration;

enum ApiResponse {
    Json(u16, Value),
    Xlsx(u16, Vec<u8>, String), // status, bytes, filename
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

        let bearer = header_value(request.headers(), "Authorization")
            .and_then(|v| v.strip_prefix("Bearer ").map(|s| s.to_string()));
        let business_id_header = header_value(request.headers(), "X-Business-Id");
        let stripe_sig_header = header_value(request.headers(), "Stripe-Signature");

        let mut conn_guard = conn.lock().unwrap();
        let response = route(&mut conn_guard, &method, &url, &body_str, bearer.as_deref(), business_id_header.as_deref(), stripe_sig_header.as_deref(), &auth_limiter);
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
        };
        let http_response = cors_headers().into_iter().fold(http_response, |r, h| r.with_header(h));
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
    stripe_sig_header: Option<&str>,
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
        if owner_password.len() < 8 {
            return json_err(400, "owner_password must be at least 8 characters");
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

    if parts.as_slice() == ["auth", "login"] && *method == Method::Post {
        let biz = match business_id_header { Some(b) => b, None => return json_err(400, "X-Business-Id header required") };
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "body must be JSON with username/password") };
        let username = obj.get("username").and_then(Value::as_str).unwrap_or("");
        let password = obj.get("password").and_then(Value::as_str).unwrap_or("");

        let limiter_key = format!("login:{biz}:{username}");
        if let Err(retry_after) = auth_limiter.check(&limiter_key) {
            return json_err(429, &format!("too many login attempts, try again in {retry_after} seconds"));
        }
        return match auth::login(conn, biz, username, password) {
            Ok(token) => {
                auth_limiter.reset(&limiter_key);
                // Best-effort: look up the user_id this token belongs to
                // so the audit entry names who logged in, not just that
                // "someone" did.
                if let Ok((logged_in_user_id, _)) = auth::current_user(conn, &token) {
                    let _ = audit::log(conn, biz, Some(&logged_in_user_id), "_auth", "login_success", None, None);
                }
                ApiResponse::Json(200, json!({"token": token}))
            }
            Err(e) => {
                let _ = audit::log(conn, biz, None, "_auth", "login_failed", None, Some(&json!({"username": username})));
                json_err(401, &e.to_string())
            }
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

    // ---- Payment webhooks: called directly by Stripe/Safaricom, never
    // by our own frontend, so these are deliberately public (no bearer
    // token). Stripe's request is authenticated instead by its own
    // signature header; M-Pesa's is trusted based on the CheckoutRequestID
    // matching a pending intent we ourselves created.
    if parts.as_slice() == ["payments", "webhook", "stripe"] && *method == Method::Post {
        let secret = match std::env::var("STRIPE_WEBHOOK_SECRET") {
            Ok(s) => s,
            Err(_) => return json_err(501, "Stripe webhook secret not configured"),
        };
        let sig_header = match stripe_sig_header {
            Some(h) => h,
            None => return json_err(400, "missing Stripe-Signature header"),
        };
        if let Err(e) = payment::stripe::verify_webhook_signature(body, sig_header, &secret) {
            return json_err(400, &e.to_string());
        }
        let event: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(_) => return json_err(400, "invalid JSON body"),
        };
        return match payment::stripe::handle_webhook_event(conn, &event) {
            Ok(()) => ApiResponse::Json(200, json!({"received": true})),
            Err(e) => json_err(400, &e.to_string()),
        };
    }
    if parts.as_slice() == ["payments", "webhook", "mpesa"] && *method == Method::Post {
        let parsed: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(_) => return json_err(400, "invalid JSON body"),
        };
        return match payment::mpesa::handle_callback(conn, &parsed) {
            Ok(()) => ApiResponse::Json(200, json!({"ResultCode": 0, "ResultDesc": "Accepted"})),
            Err(e) => json_err(400, &e.to_string()),
        };
    }

    // ---- Protected routes ----
    let token = match bearer { Some(t) => t, None => return json_err(401, "missing Authorization: Bearer <token>") };
    let (user_id, business_id) = match auth::current_user(conn, token) {
        Ok(pair) => pair,
        Err(e) => return json_err(401, &e.to_string()),
    };

    if parts.as_slice() == ["auth", "logout"] && *method == Method::Post {
        return match auth::logout(conn, token) { Ok(()) => ApiResponse::Json(200, json!({"logged_out": true})), Err(e) => json_err(400, &e.to_string()) };
    }
    if parts.as_slice() == ["license", "activate"] && *method == Method::Post {
        if let Err(e) = rbac::require_owner(conn, &user_id) { return json_err(403, &e.to_string()); }
        return match license::activate(conn, &business_id) {
            Ok(()) => {
                let _ = audit::log(conn, &business_id, Some(&user_id), "_license", "activate", None, None);
                ApiResponse::Json(200, json!({"activated": true}))
            }
            Err(e) => json_err(400, &e.to_string()),
        };
    }
    if parts.as_slice() == ["license", "pay"] && *method == Method::Post {
        if let Err(e) = rbac::require_owner(conn, &user_id) { return json_err(403, &e.to_string()); }
        return match license::record_payment(conn, &business_id) {
            Ok(()) => {
                let _ = audit::log(conn, &business_id, Some(&user_id), "_license", "manual_payment", None, None);
                ApiResponse::Json(200, json!({"paid": true}))
            }
            Err(e) => json_err(400, &e.to_string()),
        };
    }
    // POST /license/vendor/redeem {"key": "LKC-...."} — one-time
    // activation of a vendor-issued license key, locked to this device by
    // the vendor's own authority server (VENDOR_LICENSE_URL). Owner-only,
    // same tier as every other license/billing action.
    if parts.as_slice() == ["license", "vendor", "redeem"] && *method == Method::Post {
        if let Err(e) = rbac::require_owner(conn, &user_id) { return json_err(403, &e.to_string()); }
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "invalid body") };
        let key = obj.get("key").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if key.is_empty() { return json_err(400, "key is required"); }
        if let Err(e) = crate::vendor_license::validate_key_format(&key) {
            return json_err(400, &format!("invalid key: {e}"));
        }
        let vendor_url = match std::env::var("VENDOR_LICENSE_URL") {
            Ok(u) if !u.is_empty() => u,
            _ => return json_err(500, "VENDOR_LICENSE_URL is not configured on this install"),
        };
        return match crate::vendor_license::redeem(conn, &vendor_url, &key) {
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

    if parts.as_slice() == ["license", "status"] && *method == Method::Get {
        return match license::check_status(conn, &business_id) { Ok(s) => ApiResponse::Json(200, license_status_json(s)), Err(e) => json_err(400, &e.to_string()) };
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

    // ---- Payments: initiate a real checkout (Stripe) or STK push (M-Pesa) ----
    if parts.as_slice() == ["payments", "checkout"] && *method == Method::Post {
        if let Err(e) = rbac::require_owner(conn, &user_id) { return json_err(403, &e.to_string()); }
        let obj = match json_body(body) { Some(o) => o, None => return json_err(400, "invalid body") };
        let purpose = match obj.get("purpose").and_then(Value::as_str).map(payment::Purpose::parse) {
            Some(Ok(p)) => p,
            Some(Err(e)) => return json_err(400, &e.to_string()),
            None => return json_err(400, "'purpose' is required: 'activation' or 'subscription'"),
        };
        let amount = match obj.get("amount").and_then(Value::as_f64) {
            Some(a) if a > 0.0 => a,
            _ => return json_err(400, "'amount' is required and must be positive"),
        };
        let provider = obj.get("provider").and_then(Value::as_str)
            .map(|s| s.to_string())
            .unwrap_or_else(|| std::env::var("PAYMENT_PROVIDER").unwrap_or_else(|_| "stripe".to_string()));

        return match provider.as_str() {
            "stripe" => {
                let currency = obj.get("currency").and_then(Value::as_str).unwrap_or("usd");
                let success_url = obj.get("success_url").and_then(Value::as_str).unwrap_or("https://example.com/payment-success");
                let cancel_url = obj.get("cancel_url").and_then(Value::as_str).unwrap_or("https://example.com/payment-cancelled");
                match payment::stripe::create_checkout_session(conn, &business_id, purpose, amount, currency, success_url, cancel_url) {
                    Ok(checkout_url) => {
                        let _ = audit::log(conn, &business_id, Some(&user_id), "_payments", "checkout_initiated",
                            None, Some(&json!({"provider": "stripe", "purpose": purpose.as_str(), "amount": amount, "currency": currency})));
                        ApiResponse::Json(200, json!({"provider": "stripe", "checkout_url": checkout_url}))
                    }
                    Err(e) => json_err(502, &e.to_string()),
                }
            }
            "mpesa" => {
                let phone = match obj.get("phone").and_then(Value::as_str) {
                    Some(p) => p,
                    None => return json_err(400, "'phone' is required for M-Pesa (format: 2547XXXXXXXX)"),
                };
                let callback_url = std::env::var("MPESA_CALLBACK_URL")
                    .unwrap_or_else(|_| "http://127.0.0.1:8080/payments/webhook/mpesa".to_string());
                match payment::mpesa::initiate_stk_push(conn, &business_id, purpose, amount, phone, &callback_url) {
                    Ok(checkout_request_id) => {
                        let _ = audit::log(conn, &business_id, Some(&user_id), "_payments", "checkout_initiated",
                            None, Some(&json!({"provider": "mpesa", "purpose": purpose.as_str(), "amount": amount, "phone": phone})));
                        ApiResponse::Json(200, json!({
                            "provider": "mpesa",
                            "checkout_request_id": checkout_request_id,
                            "message": "Check your phone to complete the M-Pesa payment"
                        }))
                    }
                    Err(e) => json_err(502, &e.to_string()),
                }
            }
            other => json_err(400, &format!("unknown provider '{other}', expected 'stripe' or 'mpesa'")),
        };
    }

    if parts.as_slice() == ["payments", "history"] && *method == Method::Get {
        if let Err(e) = rbac::require_admin_tier(conn, &user_id) { return json_err(403, &e.to_string()); }
        let mut stmt = match conn.prepare(
            "SELECT provider, provider_reference, purpose, amount, currency, status, created_at, completed_at
             FROM payment_intents WHERE business_id = ?1 ORDER BY created_at DESC LIMIT 50",
        ) {
            Ok(s) => s,
            Err(e) => return json_err(500, &e.to_string()),
        };
        let rows = stmt.query_map(rusqlite::params![business_id], |r| {
            Ok(json!({
                "provider": r.get::<_, String>(0)?,
                "reference": r.get::<_, String>(1)?,
                "purpose": r.get::<_, String>(2)?,
                "amount": r.get::<_, f64>(3)?,
                "currency": r.get::<_, String>(4)?,
                "status": r.get::<_, String>(5)?,
                "created_at": r.get::<_, String>(6)?,
                "completed_at": r.get::<_, Option<String>>(7)?,
            }))
        });
        return match rows.and_then(|r| r.collect::<rusqlite::Result<Vec<_>>>()) {
            Ok(list) => ApiResponse::Json(200, json!({"payments": list})),
            Err(e) => json_err(500, &e.to_string()),
        };
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

    // ---- Raw data export: /modules/{id}/export — real .xlsx, license-gated ----
    if parts.len() == 3 && parts[0] == "modules" && parts[2] == "export" && *method == Method::Get {
        let module_id = parts[1];
        if let Err(e) = license::require_export_allowed(conn, &business_id) {
            return json_err(402, &e.to_string());
        }
        return match crud::list(conn, &business_id, &user_id, module_id, None, 100000, 0) {
            Ok(records) => match xlsx_export::records_to_xlsx(&records, module_id) {
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

    // ---- Report export: same params, but returns .xlsx, license-gated ----
    if parts.len() == 4 && parts[0] == "modules" && parts[2] == "report" && parts[3] == "export" && *method == Method::Get {
        let module_id = parts[1];
        if let Err(e) = license::require_export_allowed(conn, &business_id) {
            return json_err(402, &e.to_string());
        }
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

fn license_status_json(status: license::LicenseStatus) -> Value {
    use license::LicenseStatus::*;
    match status {
        Active => json!({"status": "active"}),
        Inactive => json!({"status": "inactive"}),
        Grace { days_left } => json!({"status": "grace", "days_left": days_left}),
        Locked { days_overdue } => json!({"status": "locked", "days_overdue": days_overdue}),
    }
}
