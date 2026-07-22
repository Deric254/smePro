# erp-frontend

A real, running React frontend for the core-engine backend — not mockups.
Verified end-to-end with a headless browser (Playwright) driving the
actual Vite dev server against the actual Rust API, not just `npm run build`.

## Design identity: "SME Pro"

Grounded in the subject rather than a generic SaaS-dashboard look: aged
ledger-paper background with faint horizontal rules (like ruled
accounting paper), a rubber ink-stamp motif for module icons and status
marks (the signature element — a slightly rotated double-ring circle,
used consistently for module badges, the AI button, and nowhere else),
Newsreader (serif, ledger-appropriate) for headings, IBM Plex Sans for
body text, IBM Plex Mono for tabular numbers so amounts/SKUs/quantities
align like a real ledger column.

- `--paper` / `--paper-card` / `--paper-line` — the ledger paper and its
  ruling
- `--ink` / `--ink-soft` / `--ink-faint` — text, in decreasing emphasis
- `--stamp` — the one accent color, used only for primary actions and
  the module/AI badges, so it stays meaningful rather than decorative
- `--ok` / `--warn` — license status states (active / grace-locked)

## What's actually built

- **Login** — business ID + username/password against the real
  `/auth/login` endpoint
- **Sidebar** — renders whatever modules are enabled for the business,
  entirely from `GET /modules`. No hardcoded module list.
- **Module view** — table + dynamic create form + search + delete +
  Excel export, all generated from `GET /modules/{id}/schema` at
  runtime. The exact same component renders Inventory, Sales, HR,
  Accounting, Purchasing, and Debt/Credit correctly — verified live by
  switching between Inventory and Sales in the same test run and seeing
  completely different fields render correctly.
- **Report tab** — dimension/measure slicer with a simple custom bar
  visualization (no chart library — kept the bundle small and the look
  consistent with the ledger identity rather than reaching for
  recharts' default look).
- **License banner** — shows nothing when active (no need to nag), and
  a clear, non-alarming explanation + one-click action for
  inactive/grace/locked states. Confirmed disappearing correctly after
  activation in the live test.
- **AI floating button** — chat panel calling `/ai/ask`, styled with
  the same stamp badge motif as the module icons.

## Verified with a real headless browser (Playwright + Chromium), not just `npm run build`

1. Backend + Vite dev server started together, a real Chromium instance
   navigated to the app
2. Logged in against the real API → landed on the dashboard
3. Activated the license via the UI button → banner correctly
   disappeared
4. Opened the dynamic create form for Inventory, filled it, saved →
   record appeared correctly in the table with all 8 schema-driven
   columns
5. Switched to the Report tab → slicer ran automatically and showed the
   correct total
6. Switched modules via the sidebar to Sales → the same component
   correctly rendered Sales' completely different field set
   (item_name, customer, branch, quantity, revenue, sale_date)
7. Opened the AI panel and asked a question → got the correct "not
   configured, here's how to get a free key" message end-to-end through
   the UI (no API key was set in this test environment)

## Running it

```
# terminal 1 — backend
cd ../core-engine && cargo build && cargo run

# terminal 2 — frontend
cd erp-frontend && npm install && npm run dev
```

Then open the printed Vite URL, and use the `business_id` /
`owner_username` / `owner_password` values printed by the backend (also
written to `core-engine/seed_ids.json`) to log in.

## Known gaps
- This is a **web frontend calling a local HTTP API** — it is not yet
  wrapped in Tauri as an actual desktop `.exe`/`.app`. That's the
  remaining step to match the original "Tauri + React" architecture
  decision; the React code itself doesn't need to change, only the
  shell it's packaged in.
- No mobile (Android) shell yet — Phase 10 in the roadmap.
- Fonts are loaded from Google Fonts via CDN — fine for normal internet
  access, but add local font files if you need this fully offline from
  first paint (it already falls back gracefully to Georgia/system-ui
  when the CDN is unreachable, as seen in this sandbox's restricted
  network).
