fn main() {
    // Without this, cargo has no tracked reason to rerun the Windows
    // resource-embedding step when only an icon asset changes (no .rs
    // or Cargo.toml touched) — combined with swatinem/rust-cache
    // restoring a previous target/ dir in CI, that can let a stale,
    // already-compiled icon resource silently ship in a release build
    // even after icon.ico/icon.png are updated in the repo.
    println!("cargo:rerun-if-changed=icons");

    tauri_build::build()
}
