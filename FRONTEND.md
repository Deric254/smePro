# Frontend

React 19 + TypeScript, built with Vite, charts with Recharts. The UI talks
to the local backend API (`127.0.0.1:8080`) through `src/api.ts`, the only
place requests are made.

## Structure

- `src/pages/`: Login, FirstRunSetup, Dashboard, ModuleView (generic
  table, create form, import and export for every module, generated from
  `GET /modules/{id}/schema`), PointOfSale, ServiceSale, Customers,
  StockTake, Reports, AdminPanel.
- `src/components/`: Sidebar, dashboard and report cards, invoice and
  receipt views, AI assistant button, update checkers, shared dialogs.
- `src/lib/`: money formatting and parsing (integer minor units, mirrored
  by `src-tauri/src/money.rs`), dates, retry, markdown rendering.
- `src/styles/`: mobile layout and print styles for receipts and invoices.

The sidebar lists only the modules enabled for the business. Nothing about
a module's fields is hardcoded in the UI.

## Design identity

Ledger-paper background with faint ruling, a rubber-stamp motif for module
badges and the AI button, Newsreader for headings, IBM Plex Sans for body
text and IBM Plex Mono for tabular figures. Colors are CSS variables in
`src/index.css`: `--paper*`, `--ink*`, and `--stamp` (the one accent,
used for primary actions only).

Fonts load from Google Fonts without blocking first paint and fall back to
Georgia / system-ui when offline.

## Commands

```
npm install
npm run dev        # Vite dev server (backend must be running)
npm run lint       # oxlint
npm run build      # tsc -b && vite build
```
