import type { SeasonalMonthPattern } from '../api';
import { formatMoney } from '../lib/money';

// THE HONESTY THIS CARD IS BUILT AROUND: a business with one good
// December and eleven other months would otherwise show "December
// runs 340% above average" — true of the one December on record, and
// meaningless as a claim about December itself. years_seen is shown
// right next to every bar for exactly that reason: a bar with
// years_seen: 1 is one data point wearing an average's clothing, and
// this card says so instead of hiding it behind a confident-looking
// chart. See sales_patterns::seasonal_month_pattern's own doc comment.
export default function SeasonalCard({ items, currency }: { items: SeasonalMonthPattern[]; currency: string }) {
  const withData = items.filter((m) => m.years_seen > 0);
  if (withData.length === 0) {
    return (
      <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
        <div style={{ fontWeight: 600, marginBottom: '0.2rem' }}>Seasonal pattern (by month)</div>
        <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>Not enough sales history yet.</div>
      </div>
    );
  }

  const maxYearsSeen = Math.max(...withData.map((m) => m.years_seen));
  const maxRevenue = Math.max(...items.map((m) => m.avg_revenue_cents), 1);

  return (
    <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
      <div style={{ fontWeight: 600, marginBottom: '0.2rem' }}>Seasonal pattern (by month)</div>
      {maxYearsSeen < 2 && (
        <div style={{ fontSize: '0.76rem', color: 'var(--ink-soft)', marginBottom: '0.5rem' }}>
          Every month here only has one year of history behind it so far — real seasonality (does this month
          reliably run hot or cold every year) needs at least a second year to compare against. Treat these as
          rough shape, not a confirmed pattern yet.
        </div>
      )}
      <div style={{ display: 'flex', flexDirection: 'column', gap: '0.35rem' }}>
        {items.map((m) => (
          <div key={m.month_name} style={{ display: 'flex', alignItems: 'center', gap: '0.6rem', fontSize: '0.84rem' }}>
            <span style={{ width: 86, flexShrink: 0, color: 'var(--ink-soft)' }}>{m.month_name}</span>
            <div style={{ flex: 1, background: 'var(--paper-line)', borderRadius: 4, height: 8, overflow: 'hidden' }}>
              <div style={{ width: `${(m.avg_revenue_cents / maxRevenue) * 100}%`, background: 'var(--stamp)', height: '100%' }} />
            </div>
            <span style={{ width: 74, flexShrink: 0, textAlign: 'right', fontWeight: 600 }}>
              {m.years_seen > 0 ? formatMoney(m.avg_revenue_cents, currency) : '—'}
            </span>
            <span style={{ width: 58, flexShrink: 0, textAlign: 'right', color: 'var(--ink-soft)', fontSize: '0.76rem' }}>
              {m.years_seen} yr{m.years_seen === 1 ? '' : 's'}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}
