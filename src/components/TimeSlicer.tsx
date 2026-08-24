import { useState } from 'react';

export interface DateRange {
  start: string;
  end: string;
  label: string;
}

function pad(n: number) { return n.toString().padStart(2, '0'); }
function dateStr(d: Date) { return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`; }

// The backend stamps every record's created_at with SQLite's
// `datetime('now')`, which is always UTC — never local time (see any
// of the many `datetime('now')` calls across src-tauri/src/*.rs; none
// use the 'localtime' modifier). Reports then compare these range
// bounds against created_at as plain strings.
//
// The previous version of this file built those bounds from the
// browser's LOCAL calendar date/time and sent them as-is, with no
// timezone conversion — correct only for a browser sitting in UTC+0.
// This app runs in Kenya (UTC+3, EAT): a sale rung up at, say,
// 11:30pm local time is stored as "…20:30:00" UTC, already the NEXT
// day's date once it rolls past 9pm local. Asking for "Today" sent a
// literal local-looking string that the backend read as a UTC
// boundary, so the last few hours of every real business day —
// closing time, exactly when a shop does a lot of its selling — were
// silently missing from "Today", landing under "yesterday" instead,
// in every report, KPI, and the dashboard's own chart. Same
// mechanism, opposite direction, at the start of each day.
//
// The fix: a `Date` object's own getTime() already IS the correct UTC
// instant for whatever local moment it represents — the bug was only
// ever in which calendar fields got read back OUT of it. Reading the
// UTC fields instead of the local ones here converts "midnight local"
// into the true UTC timestamp string the backend actually needs, so a
// business day's boundary means the same real moment in time on both
// ends, not just the same-looking digits.
function toBackendTimestamp(d: Date): string {
  return `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())} ${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}:${pad(d.getUTCSeconds())}`;
}

// Both take a Date already carrying the intended LOCAL calendar day
// (from presetRange's `today`, or applyCustom's locally-constructed
// picks below) and pin it to that day's real local midnight /
// 23:59:59 before converting to the UTC string the backend compares
// against — so "today" genuinely means this calendar day in the
// business's own timezone, start to finish.
function endOfDay(d: Date) { return toBackendTimestamp(new Date(d.getFullYear(), d.getMonth(), d.getDate(), 23, 59, 59)); }
function startOfDay(d: Date) { return toBackendTimestamp(new Date(d.getFullYear(), d.getMonth(), d.getDate(), 0, 0, 0)); }

function presetRange(preset: string): DateRange {
  const now = new Date();
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());

  switch (preset) {
    case 'today':
      return { start: startOfDay(today), end: endOfDay(today), label: 'Today' };
    case 'week': {
      const start = new Date(today);
      start.setDate(start.getDate() - start.getDay()); // back to Sunday
      return { start: startOfDay(start), end: endOfDay(today), label: 'This week' };
    }
    case 'month': {
      const start = new Date(today.getFullYear(), today.getMonth(), 1);
      return { start: startOfDay(start), end: endOfDay(today), label: 'This month' };
    }
    case 'quarter': {
      const qStartMonth = Math.floor(today.getMonth() / 3) * 3;
      const start = new Date(today.getFullYear(), qStartMonth, 1);
      return { start: startOfDay(start), end: endOfDay(today), label: 'This quarter' };
    }
    case 'year': {
      const start = new Date(today.getFullYear(), 0, 1);
      return { start: startOfDay(start), end: endOfDay(today), label: 'This year' };
    }
    default:
      return { start: startOfDay(today), end: endOfDay(today), label: 'Today' };
  }
}

const PRESETS = [
  { id: 'today', label: 'Today' },
  { id: 'week', label: 'This week' },
  { id: 'month', label: 'This month' },
  { id: 'quarter', label: 'This quarter' },
  { id: 'year', label: 'This year' },
];

export default function TimeSlicer({ value, onChange }: { value: DateRange; onChange: (range: DateRange) => void }) {
  const [showCustom, setShowCustom] = useState(false);
  const [customStart, setCustomStart] = useState('');
  const [customEnd, setCustomEnd] = useState('');

  function applyCustom() {
    if (!customStart || !customEnd) return;
    // customStart/customEnd are bare "YYYY-MM-DD" strings straight off
    // an <input type="date">. `new Date("2026-08-23")` parses that as
    // UTC midnight, NOT local midnight — a second, independent place
    // the same local-vs-UTC mixup could creep back in even after
    // fixing startOfDay/endOfDay above. Parsing the digits by hand and
    // building the Date with the local-timezone constructor instead
    // means "2026-08-23" unambiguously means local calendar day Aug
    // 23rd, matching what someone picking that date on a calendar
    // actually means, regardless of the browser's own timezone offset.
    const [sy, sm, sd] = customStart.split('-').map(Number);
    const [ey, em, ed] = customEnd.split('-').map(Number);
    const start = new Date(sy, sm - 1, sd);
    const end = new Date(ey, em - 1, ed);
    onChange({
      start: startOfDay(start),
      end: endOfDay(end),
      label: start.getTime() === end.getTime() ? dateStr(start) : `${dateStr(start)} – ${dateStr(end)}`,
    });
    setShowCustom(false);
  }

  const activePreset = PRESETS.find((p) => presetRange(p.id).label === value.label);

  return (
    <div style={styles.wrap}>
      {PRESETS.map((p) => (
        <button
          key={p.id}
          className={activePreset?.id === p.id && !showCustom ? 'btn' : 'btn btn-outline'}
          style={styles.pill}
          onClick={() => { setShowCustom(false); onChange(presetRange(p.id)); }}
        >
          {p.label}
        </button>
      ))}
      <button
        className={showCustom || (!activePreset && value.label.includes('–')) || (!activePreset && !value.label.includes('–') && value.label !== 'Today') ? 'btn' : 'btn btn-outline'}
        style={styles.pill}
        onClick={() => setShowCustom((v) => !v)}
      >
        Custom{!showCustom && !activePreset ? `: ${value.label}` : ''}
      </button>

      {showCustom && (
        <div style={styles.customRow}>
          <input type="date" value={customStart} onChange={(e) => setCustomStart(e.target.value)} style={styles.dateInput} />
          <span style={{ color: 'var(--ink-faint)' }}>to</span>
          <input type="date" value={customEnd} onChange={(e) => setCustomEnd(e.target.value)} style={styles.dateInput} />
          <button className="btn btn-stamp" style={styles.pill} onClick={applyCustom} disabled={!customStart || !customEnd}>
            Apply
          </button>
        </div>
      )}
    </div>
  );
}

export function defaultRange(): DateRange {
  return presetRange('month');
}

const styles: Record<string, React.CSSProperties> = {
  wrap: { display: 'flex', flexWrap: 'wrap', gap: '0.5rem', alignItems: 'center' },
  pill: { fontSize: '0.78rem', padding: '0.35em 0.8em' },
  customRow: { display: 'flex', alignItems: 'center', gap: '0.5rem', width: '100%', marginTop: '0.3rem' },
  dateInput: { fontSize: '0.82rem', padding: '0.35em 0.6em' },
};
