//! Who may talk to the local API.
//!
//! The server listens on 127.0.0.1 only, but a web page open in the owner's
//! browser can still send requests there, and with `Access-Control-Allow-Origin: *`
//! it could also read the answers (including the owner licence routes while the
//! Licence Manager is unlocked). Two checks close that:
//!   * `origin_allowed`: a browser always names the page a request comes from in
//!     the `Origin` header and a page cannot change it. Only this app's own
//!     windows are accepted. A request with no Origin is not from a web page.
//!   * `host_allowed`: stops DNS rebinding (a hostile site whose name is pointed
//!     at 127.0.0.1 would otherwise look "same origin" and send no Origin header).

/// The windows this app itself serves its screens from: Tauri on Windows/Android
/// (`http(s)://tauri.localhost`), macOS/Linux/iOS (`tauri://localhost`) and the Vite dev server.
pub fn origin_allowed(origin: &str) -> bool {
    let o = origin.trim().trim_end_matches('/');
    if matches!(o, "tauri://localhost" | "http://tauri.localhost" | "https://tauri.localhost") {
        return true;
    }
    cfg!(debug_assertions) && matches!(o, "http://localhost:5173" | "http://127.0.0.1:5173")
}

/// The API is only ever addressed as the loopback address.
pub fn host_allowed(host: &str) -> bool {
    let h = host.trim().to_ascii_lowercase();
    let name = if let Some(rest) = h.strip_prefix('[') {
        // [::1]:8080
        rest.split(']').next().map(|a| format!("[{a}]")).unwrap_or_default()
    } else {
        h.split(':').next().unwrap_or("").to_string()
    };
    matches!(name.as_str(), "127.0.0.1" | "localhost" | "[::1]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_this_apps_own_windows_are_trusted() {
        for ok in ["tauri://localhost", "http://tauri.localhost", "https://tauri.localhost", "http://tauri.localhost/"] {
            assert!(origin_allowed(ok), "{ok}");
        }
        for bad in ["https://evil.example", "http://localhost:8080", "http://127.0.0.1:8080", "null", "", "http://tauri.localhost.evil.com", "tauri://localhost.evil.com"] {
            assert!(!origin_allowed(bad), "{bad}");
        }
    }

    #[test]
    fn only_the_loopback_host_name_is_accepted() {
        for ok in ["127.0.0.1:8080", "localhost:49321", "LOCALHOST", "[::1]:8080", "127.0.0.1"] {
            assert!(host_allowed(ok), "{ok}");
        }
        for bad in ["evil.example:8080", "127.0.0.1.evil.example", "192.168.1.5:8080", "", "localhost.evil.com"] {
            assert!(!host_allowed(bad), "{bad}");
        }
    }
}
