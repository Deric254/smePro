// Suppresses the console/terminal window that would otherwise appear
// alongside the app on Windows. Rust binaries default to the "console"
// subsystem unless told otherwise — harmless for a CLI tool, but wrong
// for a desktop GUI app, where a customer seeing a black terminal
// window pop up next to the real window looks broken even though
// nothing actually is. No effect on macOS/Linux (Windows-only concept).
#![windows_subsystem = "windows"]

// Tauri's convention for a shared desktop+mobile codebase: the actual
// app logic lives in `lib.rs::run()`, which mobile targets call via the
// `#[tauri::mobile_entry_point]` attribute automatically. This file's
// only job is to be the desktop binary's entry point.
fn main() {
    core_engine::run();
}
