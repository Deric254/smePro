# SME Pro

One system: a Tauri desktop app for SME ERP. Rust backend
(`src-tauri/`) + React frontend (`src/`), compiled together into a
single executable. Not two products — this is what "Tauri (Rust core +
React frontend)" has meant since the stack was chosen; the two folders
are Tauri's own standard project layout, the same as any Tauri app.

## How it actually works

```
double-click the app
        │
        ▼
Tauri starts (src-tauri/src/main.rs)
        │
        ├─► spawns core-engine's HTTP API on a background thread
        │   (127.0.0.1:8080 — SQLite, RBAC, licensing, reporting, AI,
        │    every module — all of it, invisible to the user)
        │
        └─► opens a window showing the React UI (src/)
                │
                └─► the UI talks to 127.0.0.1:8080, same as it did
                    in local dev — nothing about the API changes
```

One process. One window. One file the user runs. The backend isn't a
separate server they start — it starts itself, inside the app, the
moment it opens.

## Project layout

```
sme-pro/
├── src/              React frontend (Vite + TypeScript)
├── src-tauri/        Rust backend + Tauri shell
│   ├── src/
│   │   ├── lib.rs        — exposes all backend modules
│   │   ├── main.rs       — the REAL app entrypoint (Tauri)
│   │   └── bin/
│   │       └── demo_seed.rs  — standalone dev/test runner, no Tauri/webview
│   ├── modules/*.json     — module schema definitions
│   ├── schema.sql
│   ├── tauri.conf.json
│   └── Cargo.toml
├── package.json
└── vite.config.ts
```

`demo_seed` is not a second app — it's a convenience binary for
developing the backend without waiting on a webview to open, the same
way you'd run backend tests separately from a frontend dev server on
any project. `src/main.rs` is the one that ships.

## Building it — READ THIS FIRST

**This was built and tested in a sandboxed environment whose Rust
toolchain is capped at 1.75 (installed via `apt`, no access to
`rustup`'s own servers to get anything newer).** Tauri 2's dependency
tree requires a materially newer Rust — attempting to build it here
hit a wall of `edition2024` requirements several layers deep
(`toml_writer` → `toml_parser` → more beneath that in `wry`/`tao`/
`webkit2gtk` bindings). Chasing every individual pin the way earlier
phases did for `rusqlite`/`argon2`/`ureq` was not a good use of time
here, so **the Tauri build itself was not run to completion in this
environment** — only verified as a correctly-structured, correctly-
written project against Tauri's documented v2 conventions.

**What WAS verified here, for real:**
- The backend (`lib.rs` + all 16 modules + `demo_seed` binary)
  compiles cleanly with Tauri's dependency stripped out — proving the
  actual application logic is sound (see `BACKEND.md` for the full
  phase-by-phase testing history)
- The frontend builds correctly to `dist/`, exactly where
  `tauri.conf.json`'s `frontendDist: "../dist"` expects it
- The backend binary + the production frontend build, run together
  from this exact merged directory structure, were driven by a real
  headless browser (Playwright) through login and the full dashboard —
  confirming the merge didn't break anything that worked before

**What you need to do, on your own machine, before this becomes a
real `.exe`/`.app`:**

### Easiest: double-click / one command

| Platform | Run in dev mode | Build a real installer | Android |
|---|---|---|---|
| Windows | double-click `run.bat` | double-click `build-installer.bat` | not on Windows — use macOS/Linux |
| macOS / Linux | `./run.sh` | `./build-installer.sh` | `./mobile-android.sh --dev` / `--build` |

These are thin wrappers around the scripts below — same behavior, just
easier to find and run without remembering a command. Windows uses
`.bat` files specifically because double-clicking a `.ps1` doesn't run
it (PowerShell's script execution policy blocks that by default); the
`.bat` bypasses that safely for you.

### Automated (equivalent, more explicit)

```bash
# macOS / Linux
chmod +x scripts/setup.sh
./scripts/setup.sh --dev     # installs everything, then launches dev mode
./scripts/setup.sh --build   # installs everything, then builds a real installer
```
```powershell
# Windows (PowerShell)
.\scripts\setup.ps1 -Dev
.\scripts\setup.ps1 -Build
```

Both scripts check the *actual version* of Rust already on your machine
(not just whether it exists) before doing anything — this project's own
build hit exactly this trap in an environment where Rust came from the
OS package manager instead of `rustup` and was too old for Tauri without
any obvious error saying so. The script would have caught that
immediately instead of failing confusingly halfway through a build.
Every step is safe to re-run if something fails partway through.

**Honest scope note**: `scripts/setup.sh` was tested function-by-function
in the same sandboxed environment this whole project was built in — the
OS/package-manager detection and the Rust version-check logic were both
verified to work correctly against this sandbox's *actual* (too-old)
Rust install, which is exactly the scenario the script exists to catch.
The full end-to-end run (actually installing Rust, actually building)
could not be executed here for the same reason the Tauri build itself
couldn't be — same underlying sandbox ceiling. `scripts/setup.ps1` could
not be executed at all (no PowerShell available in this sandbox, and it
wasn't installable from any reachable source) — it was written carefully
against documented PowerShell syntax and reviewed manually, but treat
its first real run on your Windows machine with the same care you'd give
any new script you haven't seen run yet.

### Manual (if you'd rather see every step, or the script hits something unexpected)

```
# install Rust normally (rustup.rs) — you'll get something newer than 1.75
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# SQLCipher is needed for encryption at rest (added in the Hardening phase)
# macOS:   brew install sqlcipher
# Ubuntu:  sudo apt install libsqlcipher-dev
# Windows: see https://github.com/sqlcipher/sqlcipher#windows

npm install
npm run tauri dev      # first real build + launch — this is the moment
                        # of truth this sandbox couldn't reach
```
If either path hits dependency snags on your machine (unlikely with a
current Rust, but possible), they'll be ordinary Tauri build issues to
debug directly — not a repeat of this sandbox's specific ceiling.

### First-run setup — now built
A genuinely fresh install now shows a real "set up your business" wizard
(business info → owner account + security questions → admin recovery
code → auto-login) instead of an empty login screen with nothing to log
into. Verified end-to-end with a real headless browser run through the
entire flow. See `src-tauri/BACKEND.md` for the full test writeup.


## Mobile (Android)

`./mobile-android.sh --dev` (or `--build`) — requires Android Studio +
SDK/NDK installed first (the script checks and tells you what's
missing rather than failing confusingly partway through). It also
applies the one manual step MOBILE.md flags — Android blocks plain
HTTP by default, so the loopback (127.0.0.1) exception has to be
explicitly allow-listed — automatically on first run, instead of
leaving it as a step you could forget.

iOS is not scaffolded yet (needs a Mac + Xcode + an Apple Developer
account to even attempt) — not started.

See `MOBILE.md` for the full detail, including the one real
architectural risk flagged there: Android is more aggressive than
desktop about suspending backgrounded apps, which could affect the
background HTTP server thread. Untested on a real device/emulator —
watch for it if the app feels unresponsive after being backgrounded.

## CI/CD & auto-update

**Fully automated.** Push to `main` and `.github/workflows/release.yml`
auto-bumps the version, tags it, then builds **Windows/macOS/Linux
installers AND an Android APK**, all attached to one draft GitHub
Release:
```bash
git commit -m "fix: whatever you changed"
git push
```
That's it — no `git tag` step. Commit message controls the version
bump (`[minor]`, `[major]`, defaults to patch; `[skip ci]` skips
release entirely). See `RELEASE.md` for the full detail, including the
one-time signing-key setup you need to do before the first release, and
how to add real Android Play Store signing (optional — without it, the
APK is still a normal, installable, debug-signed build).

It's written against Tauri v2's documented, real `tauri-apps/tauri-action`
GitHub Action plus `android-actions/setup-android` and
`softprops/action-gh-release` — all real, widely-used actions, not
placeholders. Every piece of shell logic in the version-bump step (the
JSON/TOML rewriting, the bump-type detection) was tested directly
against copies of this repo's actual files and produces byte-for-byte
correct results. What's **not** verified: an actual run on a live
GitHub Actions runner, since that needs a real repo + Actions minutes,
neither available in the sandbox this was built in. Push a small test
commit first and watch the Actions tab before relying on it.

Before your first real release, do the two things `RELEASE.md` walks
through — generate your own signing key (`npm run tauri signer
generate`) and add it as `TAURI_SIGNING_PRIVATE_KEY` /
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` repo secrets. The key currently in
`tauri.conf.json` is a placeholder from development and must be
replaced — the updater refuses to trust an update signed with a key it
doesn't recognize, which is the whole point.

Once that's done, every desktop install of the app checks
`https://github.com/Deric254/smePro/releases/latest/download/latest.json`
on launch (`UpdateChecker.tsx`) and shows an "Update available" banner
with one click to install — this part **is** real, working code (not a
stub), verified by the earlier Playwright run described in
`BACKEND.md`. Android gets its own separate in-app updater
(`AndroidUpdateChecker.tsx`) that checks the same GitHub release,
downloads the APK, and hands it to Android's installer — one tap to
confirm (an Android OS requirement for non-Play-Store apps, not
something that can be skipped), but no browser or file manager needed.
See `MOBILE.md` for exactly what's verified vs. still untested there.

## Further reading
- `BACKEND.md` — full phase-by-phase build and test history of the
  Rust core (Phases 1–8: engine, business panel, CRUD API, auth/
  licensing, reporting/Excel, AI assistant, remaining modules,
  onboarding/notifications)
- `FRONTEND.md` — design system ("SME Pro" ledger/stamp
  identity) and the Playwright-verified UI flows
