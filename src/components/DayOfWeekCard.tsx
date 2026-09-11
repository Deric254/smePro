import type { DayOfWeekPattern } from '../api';
import { formatMoney } from '../lib/money';

export default function DayOfWeekCard({ items, currency }: { items: DayOfWeekPattern[]; currency: string }) {
  const withData = items.filter((d) => d.occurrences > 0);
  if (withData.length === 0) {
    return (
      <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
        <div style={{ fontWeight: 600, marginBottom: '0.2rem' }}>Sales by day of week</div>
        <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>Not enough sales history yet.</div>
      </div>
    );
  }

  const maxRevenue = Math.max(...items.map((d) => d.avg_revenue_cents), 1);
  const minOccurrences = Math.min(...withData.map((d) => d.occurrences));

  return (
    <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
      <div style={{ fontWeight: 600, marginBottom: '0.2rem' }}>Sales by day of week</div>
      {minOccurrences < 4 && (
        <div style={{ fontSize: '0.76rem', color: 'var(--ink-soft)', marginBottom: '0.5rem' }}>
          Based on as few as {minOccurrences} occurrence{minOccurrences === 1 ? '' : 's'} of some days — will settle in with more history.
        </div>
      )}
      <div style={{ display: 'flex', flexDirection: 'column', gap: '0.35rem' }}>
        {items.map((d) => (
          <div key={d.day_name} style={{ display: 'flex', alignItems: 'center', gap: '0.6rem', fontSize: '0.84rem' }}>
            <span style={{ width: 78, flexShrink: 0, color: 'var(--ink-soft)' }}>{d.day_name}</span>
            <div style={{ flex: 1, background: 'var(--paper-line)', borderRadius: 4, height: 8, overflow: 'hidden' }}>
              <div style={{ width: `${(d.avg_revenue_cents / maxRevenue) * 100}%`, background: 'var(--stamp)', height: '100%' }} />
            </div>
            <span style={{ width: 74, flexShrink: 0, textAlign: 'right', fontWeight: 600 }}>{formatMoney(d.avg_revenue_cents, currency)}</span>
          </div>
        ))}
      </div>
    </div>
  );
}
