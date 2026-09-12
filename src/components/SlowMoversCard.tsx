import type { SlowMover } from '../api';
import { formatMoney } from '../lib/money';

export default function SlowMoversCard({ items, currency }: { items: SlowMover[]; currency: string }) {
  if (items.length === 0) {
    return (
      <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
        <div style={{ fontWeight: 600, marginBottom: '0.2rem' }}>Slow-moving stock</div>
        <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>Everything in stock has sold recently.</div>
      </div>
    );
  }

  // Total across this slow-movers list only (there's no whole-business
  // inventory value passed in here) — so the % reads as "share of the
  // at-risk stock shown", the same "of shown" honesty as TopCustomersCard.
  const totalAtRisk = items.reduce((sum, it) => sum + it.value_at_risk_cents, 0);

  return (
    <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
      <div style={{ fontWeight: 600, marginBottom: '0.2rem' }}>Slow-moving stock</div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: '0.4rem' }}>
        {items.map((it) => (
          <div
            key={it.item_name}
            style={{
              display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start',
              padding: '0.4rem 0', borderBottom: '1px solid var(--paper-line)', fontSize: '0.86rem', gap: '0.6rem',
            }}
          >
            <div style={{ minWidth: 0 }}>
              <div>{it.item_name}</div>
              <div style={{ fontSize: '0.76rem', color: 'var(--ink-soft)' }}>
                {it.quantity} in stock · {it.days_since_last_sale === null ? 'never sold' : `last sold ${it.days_since_last_sale}d ago`}
              </div>
            </div>
            <div style={{ textAlign: 'right', flexShrink: 0 }}>
              <div style={{ fontWeight: 600 }}>{formatMoney(it.value_at_risk_cents, currency)}</div>
              {totalAtRisk > 0 && (
                <div style={{ fontSize: '0.74rem', color: 'var(--ink-soft)' }}>
                  {((it.value_at_risk_cents / totalAtRisk) * 100).toFixed(1)}% of shown
                </div>
              )}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
