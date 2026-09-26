import { useState } from 'react';

export interface DateRange {
  start: string;
  end: string;
  label: string;
}

function pad(n: number) { return n.toString().padStart(2, '0'); }
function dateStr(d: Date) { return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`; }

// Backend timestamps (created_at) are always UTC. These convert a
// LOCAL calendar date/time to the correct UTC timestamp string so
// "today" means this calendar day in the business's own timezone,
// not UTC+0 — reading Date's UTC fields, not local ones.
function toBackendTimestamp(d: Date): string {
  return `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())} ${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}:${pad(d.getUTCSeconds())}`;
}

// Pins a Date's intended local calendar day to that day's real local
// midnight / 23:59:59 before converting to the backend's UTC string.
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
