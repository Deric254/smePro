import { useState } from 'react';
import type { HourOfDayPattern } from '../api';
import { formatMoney } from '../lib/money';

// Fixed display order for the four period buckets — not alphabetical,
// not the order the backend happens to emit rows in, but the order a
// business day actually runs in.
const PERIOD_ORDER = ['Morning', 'Afternoon', 'Evening', 'Night'];

export default function HourOfDayCard({ items, currency }: { items: HourOfDayPattern[]; currency: string }) {
  const [expanded, setExpanded] = useState<string | null>(null);

  const withData = items.filter((h) => h.avg_revenue_cents > 0 || h.avg_order_count > 0);
  if (withData.length === 0) {
    return (
      <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
        <div style={{ fontWeight: 600, marginBottom: '0.2rem' }}>Sales by time of day</div>
        <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>Not enough sales history yet.</div>
      </div>
    );
  }

  const byPeriod = PERIOD_ORDER.map((period) => {
    const hours = items.filter((h) => h.period === period).sort((a, b) => a.hour - b.hour);
    const revenue_cents = hours.reduce((sum, h) => sum + h.avg_revenue_cents, 0);
    const order_count = hours.reduce((sum, h) => sum + h.avg_order_count, 0);
    return { period, hours, revenue_cents, order_count };
  });
  const maxPeriodRevenue = Math.max(...byPeriod.map((p) => p.revenue_cents), 1);
  const maxHourRevenue = Math.max(...items.map((h) => h.avg_revenue_cents), 1);

  return (
    <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
      <div style={{ fontWeight: 600, marginBottom: '0.2rem' }}>Sales by time of day</div>
      <div style={{ fontSize: '0.76rem', color: 'var(--ink-soft)', marginBottom: '0.5rem' }}>
        Averages per day over the last {items[0]?.occurrences ?? 0} days, in your local time. Tap a period to see the specific hour.
      </div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: '0.35rem' }}>
        {byPeriod.map((p) => (
          <div key={p.period}>
            <button
              onClick={() => setExpanded(expanded === p.period ? null : p.period)}
              style={{
                display: 'flex', alignItems: 'center', gap: '0.6rem', fontSize: '0.84rem',
                width: '100%', background: 'none', border: 'none', padding: '0.15rem 0', cursor: 'pointer', textAlign: 'left',
              }}
            >
              <span style={{ width: 78, flexShrink: 0, color: 'var(--ink-soft)' }}>
                {expanded === p.period ? '▾' : '▸'} {p.period}
              </span>
              <div style={{ flex: 1, background: 'var(--paper-line)', borderRadius: 4, height: 8, overflow: 'hidden' }}>
                <div style={{ width: `${(p.revenue_cents / maxPeriodRevenue) * 100}%`, background: 'var(--stamp)', height: '100%' }} />
              </div>
              <span style={{ width: 74, flexShrink: 0, textAlign: 'right', fontWeight: 600 }}>
                {formatMoney(p.revenue_cents, currency)}
              </span>
            </button>
            {expanded === p.period && (
              <div style={{ display: 'flex', flexDirection: 'column', gap: '0.3rem', margin: '0.35rem 0 0.5rem 1.4rem' }}>
                {p.hours.map((h) => (
                  <div key={h.hour} style={{ display: 'flex', alignItems: 'center', gap: '0.6rem', fontSize: '0.78rem' }}>
                    <span style={{ width: 60, flexShrink: 0, color: 'var(--ink-soft)' }}>{h.hour_label}</span>
                    <div style={{ flex: 1, background: 'var(--paper-line)', borderRadius: 4, height: 6, overflow: 'hidden' }}>
                      <div style={{ width: `${(h.avg_revenue_cents / maxHourRevenue) * 100}%`, background: 'var(--ink-soft)', height: '100%' }} />
                    </div>
                    <span style={{ width: 68, flexShrink: 0, textAlign: 'right' }}>{formatMoney(h.avg_revenue_cents, currency)}</span>
                  </div>
                ))}
              </div>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}
