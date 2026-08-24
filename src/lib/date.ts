// Every full timestamp this backend sends (created_at, updated_at,
// closed_at, timestamp, etc.) comes from SQLite's `datetime('now')`,
// which is always UTC — never local time (see any of the many
// `datetime('now')` calls across src-tauri/src/*.rs; none use the
// 'localtime' modifier) — formatted as a plain "YYYY-MM-DD HH:MM:SS"
// string with no timezone marker at all: no "Z", no "T" separator,
// nothing.
//
// Handed straight to `new Date()`, that shape gets parsed as LOCAL
// time, not UTC (verified against actual engine behavior, not just
// spec-guessing — see the git history / test notes for this file). So
// the exact UTC clock digits were being echoed straight back out by
// .toLocaleString() as if they were already local — every timestamp
// shown anywhere in the app (audit log, receipts, stock take history,
// customer purchase history, backups, notifications) displayed a time
// shifted by the viewer's own UTC offset instead of the real local
// time the thing actually happened. In Kenya (UTC+3) that meant every
// one of those was showing a time three hours EARLIER than reality.
//
// The fix: mark the string as UTC before parsing, by turning the
// space into a 'T' and appending 'Z' — the exact format `new Date()`
// already parses correctly (same as any ISO string that already ends
// in 'Z') — so a later .toLocaleString() / .toLocaleDateString() call
// converts to the viewer's REAL local time instead of just
// re-displaying the raw UTC one under a wrong label.
export function parseBackendTimestamp(value: string): Date {
  const trimmed = value.trim();
  // A bare calendar date with no time component ("2026-08-23") is
  // already parsed as UTC midnight by every engine per spec, and
  // that's exactly right for something like a due date — it names a
  // day, not a specific instant, so there's nothing to convert.
  if (/^\d{4}-\d{2}-\d{2}$/.test(trimmed)) return new Date(trimmed);
  // Already has an explicit timezone (a 'Z', or a +HH:MM/-HHMM
  // offset) — already unambiguous, pass through untouched.
  if (trimmed.endsWith('Z') || /[+-]\d{2}:?\d{2}$/.test(trimmed)) return new Date(trimmed);
  return new Date(`${trimmed.replace(' ', 'T')}Z`);
}

// Convenience wrappers for the two call shapes used throughout the
// app — same UTC-correct parsing either way, just returning the
// formatted string directly instead of making every call site repeat
// `parseBackendTimestamp(x).toLocaleString()`.
export function formatBackendDateTime(value: string): string {
  return parseBackendTimestamp(value).toLocaleString();
}
export function formatBackendDate(value: string): string {
  return parseBackendTimestamp(value).toLocaleDateString();
}
