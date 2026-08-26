import { useEffect, useState } from 'react';
import { searchCustomers, ApiError } from '../api';
import type { CustomerMatch } from '../api';

/**
 * Two plain inputs (name, phone) that ALSO show a debounced dropdown
 * of existing customers matching whatever's been typed so far — click
 * one to fill both fields from it. This is the actual fix for
 * accidental duplicate customers: normalizing phone numbers (see
 * customers.rs) reconciles near-duplicates AFTER the fact, but a
 * cashier who can see "Asha · 0712345678" already exists while typing
 * "Asha" never creates the near-duplicate in the first place.
 *
 * Selecting a suggestion doesn't lock the fields — the cashier can
 * still edit them afterward (e.g. correcting a name), which just goes
 * through the normal find-or-create matching on submit like any other
 * typed entry.
 */
export default function CustomerPicker({
  name,
  phone,
  onChangeName,
  onChangePhone,
}: {
  name: string;
  phone: string;
  onChangeName: (v: string) => void;
  onChangePhone: (v: string) => void;
}) {
  const [matches, setMatches] = useState<CustomerMatch[]>([]);
  const [showDropdown, setShowDropdown] = useState(false);

  const query = name.trim() || phone.trim();

  // Same debounce + stale-response guard as ModuleView.tsx's live
  // search — typing shouldn't fire a request per keystroke, and a
  // slow response from an earlier, shorter query shouldn't overwrite
  // the results of what's been typed since.
  useEffect(() => {
    if (!query || query.length < 2) {
      setMatches([]);
      return;
    }
    let cancelled = false;
    const timer = setTimeout(async () => {
      try {
        const r = await searchCustomers(query);
        if (!cancelled) {
          setMatches(r.customers);
          setShowDropdown(r.customers.length > 0);
        }
      } catch (err) {
        // A failed lookup shouldn't block typing or show an error
        // banner for what's often just a mid-typing hiccup — the
        // cashier can still just finish typing and submit normally.
        if (!cancelled && !(err instanceof ApiError)) setMatches([]);
      }
    }, 300);
    return () => { cancelled = true; clearTimeout(timer); };
  }, [query]);

  function selectMatch(m: CustomerMatch) {
    onChangeName(m.name ?? '');
    onChangePhone(m.phone ?? '');
    setShowDropdown(false);
  }

  return (
    <div style={{ display: 'flex', gap: '0.6rem', position: 'relative' }}>
      {/* minWidth: 0 overrides the browser's intrinsic min-width on
          text inputs (~170-200px), which otherwise refuses to shrink
          below that even inside a flex:1 parent — in the 320px-wide
          POS cart panel that pushed these two fields into an overflow
          or a forced wrap instead of the intended single left/right
          row. */}
      <div style={{ flex: 1, minWidth: 0 }}>
        {/* labelBox reserves 2 lines of height regardless of how many
            this particular label actually wraps to. Without it,
            "Customer name (optional)" wraps to 2 lines at this width
            while "Phone (optional)" fits on 1 — so the two inputs
            below them landed at different heights, visibly not "one
            line" even though the columns themselves sit side by side.
            Reserving the same space for both labels is what actually
            keeps the two inputs level. */}
        <label style={styles.labelBox}>Customer name (optional)</label>
        <input
          value={name}
          onChange={(e) => { onChangeName(e.target.value); setShowDropdown(true); }}
          onFocus={() => setShowDropdown(matches.length > 0)}
          style={{ width: '100%' }}
        />
      </div>
      <div style={{ flex: 1, minWidth: 0 }}>
        <label style={styles.labelBox}>Phone (optional)</label>
        <input
          value={phone}
          onChange={(e) => { onChangePhone(e.target.value); setShowDropdown(true); }}
          onFocus={() => setShowDropdown(matches.length > 0)}
          style={{ width: '100%' }}
          placeholder="e.g. 0712345678"
        />
      </div>

      {showDropdown && matches.length > 0 && (
        <div style={styles.dropdown} onMouseDown={(e) => e.preventDefault() /* keep the input focused so a click registers before blur closes the list */}>
          <div style={styles.dropdownHint}>Already a customer — click to use them instead:</div>
          {matches.map((m) => (
            <button key={m.id} type="button" onClick={() => selectMatch(m)} style={styles.matchRow}>
              <strong>{m.name || '(no name)'}</strong>
              {m.phone && <span style={{ color: 'var(--ink-soft)' }}> · {m.phone}</span>}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  // 2 lines at this label's line-height, so "Customer name (optional)"
  // wrapping to 2 lines and "Phone (optional)" fitting on 1 both
  // reserve identical space — see the comment above where this is
  // used for why that's what actually keeps the two inputs level.
  labelBox: { minHeight: '2.4em', lineHeight: '1.2em' },
  dropdown: {
    position: 'absolute', top: '100%', left: 0, right: 0, marginTop: '0.2rem', zIndex: 20,
    background: 'var(--paper-card)', border: '1px solid var(--paper-line)', borderRadius: 3,
    boxShadow: '0 6px 18px rgba(32,20,15,0.15)', maxHeight: 220, overflowY: 'auto',
  },
  dropdownHint: { padding: '0.4rem 0.7rem', fontSize: '0.72rem', color: 'var(--ink-faint)', textTransform: 'uppercase', letterSpacing: '0.03em' },
  matchRow: {
    display: 'block', width: '100%', textAlign: 'left', padding: '0.5rem 0.7rem',
    background: 'none', border: 'none', borderTop: '1px solid var(--paper-line)', fontSize: '0.86rem', cursor: 'pointer',
  },
};
