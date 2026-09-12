import type { PeriodTrendPoint } from '../api';
import { formatMoney } from '../lib/money';

// Shared by the weekly and monthly trend sections on the Reports
// page — same data shape (sales_patterns::PeriodTrendPoint), same
// "is this bucket even finished yet" honesty concern either way, so
// one component renders both rather than two near-identical copies.
export default function PeriodTrendCard({
  title,
  items,
  currency,
  formatLabel,
}: {
  title: string;
  items: PeriodTrendPoint[];
  currency: string;
  formatLabel: (label: string) => string;
}) {
  const hasAnyData = items.some((p) => p.revenue_cents > 0);
  if (!hasAnyData) {
    return (
      <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
        <div style={{ fontWeight: 600, marginBottom: '0.2rem' }}>{title}</div>
        <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>Not enough sales history yet.</div>
      </div>
    );
  }

  const maxRevenue = Math.max(...items.map((p) => p.revenue_cents), 1);

  return (
    <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
      <div style={{ fontWeight: 600, marginBottom: '0.2rem' }}>{title}</div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: '0.35rem' }}>
        {items.map((p) => (
          <div key={p.label} style={{ display: 'flex', alignItems: 'center', gap: '0.6rem', fontSize: '0.84rem' }}>
            <span style={{ width: 86, flexShrink: 0, color: 'var(--ink-soft)' }}>
              {formatLabel(p.label)}
              {/* Not a comparable full period yet — see PeriodTrendPoint's
                  own doc comment. Flagged inline rather than left out of
                  the chart entirely, since "in progress" is itself real,
                  useful information for the most recent bar specifically. */}
              {!p.is_complete && <span title="Still in progress — not a full period yet">*</span>}
            </span>
            <div style={{ flex: 1, background: 'var(--paper-line)', borderRadius: 4, height: 8, overflow: 'hidden' }}>
              <div
                style={{
                  width: `${(p.revenue_cents / maxRevenue) * 100}%`,
                  background: 'var(--stamp)',
                  height: '100%',
                  opacity: p.is_complete ? 1 : 0.55,
                }}
              />
            </div>
            <span style={{ width: 74, flexShrink: 0, textAlign: 'right', fontWeight: 600 }}>{formatMoney(p.revenue_cents, currency)}</span>
          </div>
        ))}
      </div>
      {items.some((p) => !p.is_complete) && (
        <div style={{ fontSize: '0.76rem', color: 'var(--ink-soft)', marginTop: '0.5rem' }}>
          * still in progress — not a full period yet, not a fair comparison against the full ones next to it.
        </div>
      )}
    </div>
  );
}
