// Tauri's convention for a shared desktop+mobile codebase: the actual
// app logic lives in `lib.rs::run()`, which mobile targets call via the
// `#[tauri::mobile_entry_point]` attribute automatically. This file's
// only job is to be the desktop binary's entry point.
fn main() {
    core_engine::run();
}
