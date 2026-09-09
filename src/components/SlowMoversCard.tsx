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
            <div style={{ fontWeight: 600, flexShrink: 0 }}>
              {formatMoney(it.value_at_risk_cents, currency)}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
