# SME Pro

A business management app for small and medium businesses: point of sale,
inventory with batch and expiry tracking, purchasing, invoicing, debt and
credit, accounting, reports and an optional AI assistant. Built by DericBI.

One app, one codebase: a Tauri shell (desktop and Android) around a Rust
backend (`src-tauri/`) and a React + TypeScript frontend (`src/`). The
backend starts itself inside the app and serves a local HTTP API on
`127.0.0.1:8080`; the UI talks to it. All business data lives in an
encrypted (SQLCipher) SQLite database on the user's own device.

## Layout

```
src/                React frontend (Vite + TypeScript)
src-tauri/
  src/              Rust backend: one module per feature (see BACKEND.md)
  src/bin/          Dev-only tools (need --features dev-tools; never shipped)
  modules/*.json    Module schemas (inventory, sales, purchasing, ...)
  schema.sql        Base schema; db_migrations.rs applies every change after it
  tauri.conf.json   Tauri configuration
.github/workflows/  Release pipeline (see RELEASE.md)
scripts/            Setup and release helpers
```

## Build and run

Prerequisites: Node 22+, Rust 1.85+ (via rustup), and on Linux the Tauri
system libraries. `scripts/setup.sh` (macOS/Linux) or `scripts/setup.ps1`
(Windows) checks and installs them.

| Platform | Dev mode | Installer | Android |
|---|---|---|---|
| Windows | `run.bat` | `build-installer.bat` | use macOS/Linux |
| macOS / Linux | `./run.sh` | `./build-installer.sh` | `./mobile-android.sh --dev` / `--build` |

Or directly: `npm install`, then `npm run tauri dev` / `npm run dist`.

## Checks

```
npm run lint && npm run build          # frontend: oxlint, tsc, vite
cd src-tauri && cargo test --lib       # backend test suite
```

## Release and updates

Pushing to `main` runs `.github/workflows/release.yml`: it bumps the
version, builds Windows, macOS and Linux installers plus an Android APK,
and publishes one GitHub Release. Desktop installs check that release for
updates; Android has its own in-app updater. One-time signing-key setup
and the commit-message version controls are in `RELEASE.md`. Android
specifics are in `MOBILE.md`.

## Further reading

- `BACKEND.md` (in `src-tauri/`): backend modules, API and design rules
- `FRONTEND.md`: design system and frontend structure
- `RELEASE.md`, `MOBILE.md`: release pipeline and Android
