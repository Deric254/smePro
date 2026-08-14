//! Device-level network mode — standalone, LAN host, or LAN client.
//!
//! This is deliberately NOT stored in the encrypted business database:
//! a device in "client" mode has no local database at all (see lib.rs's
//! setup() — client mode skips db::open entirely), so this can't live
//! anywhere that assumes a database already exists. It's a small plain
//! JSON file next to the database instead, read once at app startup —
//! same "app data dir" location as everything else per-device (see
//! lib.rs's own comment on why app_data_dir(), not a relative path).
//!
//! Three modes:
//! - "standalone" (default, and the ONLY behavior every existing
//!   install has ever had): this device runs its own local server on
//!   127.0.0.1, reachable only from itself. Nothing about this file
//!   existing changes standalone behavior in any way — an install that
//!   never touches Admin → Network never even creates this file, and a
//!   missing file reads back as standalone.
//! - "host": same local server, but bound to every network interface
//!   instead of just localhost, so other devices on the same WiFi can
//!   reach it. Still has its own local encrypted database — the host
//!   IS the single source of truth other devices point at.
//! - "client": this device runs NO local server and opens NO local
//!   database at all. Every request the frontend makes goes straight
//!   to `host_address` instead (see api.ts's setApiBase, called during
//!   startup — see main.tsx).

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkModeConfig {
    pub mode: String, // "standalone" | "host" | "client"
    pub host_address: Option<String>, // e.g. "192.168.1.42:8080" — only meaningful for "client"
}

impl Default for NetworkModeConfig {
    fn default() -> Self {
        Self { mode: "standalone".to_string(), host_address: None }
    }
}

fn config_path(app_data_dir: &Path) -> std::path::PathBuf {
    app_data_dir.join("network_mode.json")
}

/// Loads the current config, defaulting to standalone on ANY problem
/// (file doesn't exist yet, corrupt JSON, unreadable) — a device
/// falling back to fully-local, fully-working standalone behavior is
/// always the safe failure mode here; falling back to an unreachable
/// "client" pointed at nothing, or a "host" that silently isn't
/// actually listening where the user thinks it is, would not be.
pub fn load(app_data_dir: &Path) -> NetworkModeConfig {
    std::fs::read_to_string(config_path(app_data_dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(app_data_dir: &Path, config: &NetworkModeConfig) -> Result<()> {
    let json = serde_json::to_string_pretty(config)?;
    std::fs::write(config_path(app_data_dir), json)?;
    Ok(())
}

/// This device's own LAN-reachable IP address, for display when
/// setting up host mode ("other devices should connect to
/// 192.168.1.42:8080"). Uses the standard no-dependency trick: opening
/// a UDP socket and "connecting" it to a public address never actually
/// sends a packet (UDP is connectionless) but makes the OS resolve
/// which local interface/IP WOULD be used to route there, which is
/// exactly the LAN-facing address other devices on the same network
/// can reach this one at. Returns None if there's no network interface
/// at all (e.g. airplane mode) — genuinely nothing useful to show in
/// that case, not an error to hide.
pub fn local_lan_ip() -> Option<String> {
    use std::net::UdpSocket;
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    socket.local_addr().ok().map(|a| a.ip().to_string())
}
