import { useState } from 'react';

export interface DateRange {
  start: string;
  end: string;
  label: string;
}

function pad(n: number) { return n.toString().padStart(2, '0'); }
function dateStr(d: Date) { return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`; }

// The backend compares these as plain strings against created_at
// timestamps ("2026-08-08 14:30:00" style) — a bare date like
// "2026-08-08" as an END bound would exclude everything from that day
// except exactly midnight, since "2026-08-08 14:30:00" sorts AFTER
// "2026-08-08" as a string. Appending 23:59:59 makes an end date
// genuinely mean "through the end of that day," which is what anyone
// picking a range actually means.
function endOfDay(d: Date) { return `${dateStr(d)} 23:59:59`; }
function startOfDay(d: Date) { return `${dateStr(d)} 00:00:00`; }

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
    const start = new Date(customStart);
    const end = new Date(customEnd);
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
