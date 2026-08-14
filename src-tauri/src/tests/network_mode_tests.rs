use crate::network_mode::{load, save, NetworkModeConfig};
use std::path::PathBuf;
use uuid::Uuid;

/// A fresh, unique temp directory per test — this module has no
/// `tempfile` dev-dependency (nothing else in this codebase needed
/// real filesystem fixtures before; everything else uses
/// `Connection::open_in_memory()`), so this uses the OS temp dir with
/// a UUID subfolder instead, cleaned up at the end of each test via
/// RAII (see the Drop impl below) rather than relying on every test
/// remembering to clean up manually.
struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("smepro_network_mode_test_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        Self(dir)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn test_load_defaults_to_standalone_when_no_file_exists() {
    let dir = TempDir::new();
    let config = load(&dir.0);
    assert_eq!(config.mode, "standalone");
    assert_eq!(config.host_address, None);
}

#[test]
fn test_save_then_load_roundtrip_host_mode() {
    let dir = TempDir::new();
    let config = NetworkModeConfig { mode: "host".to_string(), host_address: None };
    save(&dir.0, &config).unwrap();

    let loaded = load(&dir.0);
    assert_eq!(loaded.mode, "host");
    assert_eq!(loaded.host_address, None);
}

#[test]
fn test_save_then_load_roundtrip_client_mode_with_host_address() {
    let dir = TempDir::new();
    let config = NetworkModeConfig { mode: "client".to_string(), host_address: Some("192.168.1.42:8080".to_string()) };
    save(&dir.0, &config).unwrap();

    let loaded = load(&dir.0);
    assert_eq!(loaded.mode, "client");
    assert_eq!(loaded.host_address, Some("192.168.1.42:8080".to_string()));
}

#[test]
fn test_switching_modes_actually_overwrites_the_previous_value() {
    // Regression-shaped: proves this isn't accidentally append-only or
    // stuck on the first value ever written — a business switching
    // from host back to standalone (a real Admin → Network flow) must
    // actually see standalone on the next load, not host.
    let dir = TempDir::new();
    save(&dir.0, &NetworkModeConfig { mode: "host".to_string(), host_address: None }).unwrap();
    assert_eq!(load(&dir.0).mode, "host");

    save(&dir.0, &NetworkModeConfig { mode: "standalone".to_string(), host_address: None }).unwrap();
    let loaded = load(&dir.0);
    assert_eq!(loaded.mode, "standalone");
    assert_eq!(loaded.host_address, None);
}

#[test]
fn test_load_defaults_to_standalone_on_corrupt_json() {
    // A device stuck with a half-written or corrupted config file must
    // still start up as a normal, fully-functional standalone install
    // — never as an unreachable "client" pointed at garbage, and never
    // a hard crash on launch. See load()'s own doc comment on why
    // "fail toward standalone" is the deliberate choice here.
    let dir = TempDir::new();
    std::fs::write(dir.0.join("network_mode.json"), b"{not valid json at all}").unwrap();

    let config = load(&dir.0);
    assert_eq!(config.mode, "standalone");
    assert_eq!(config.host_address, None);
}

#[test]
fn test_local_lan_ip_does_not_panic() {
    // Can't assert a specific address (depends entirely on the
    // sandbox/CI network environment), but this proves the function
    // returns cleanly either way rather than panicking when there's no
    // usable network interface.
    let _ = crate::network_mode::local_lan_ip();
}
