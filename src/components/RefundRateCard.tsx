import type { RefundRate } from '../api';
import { formatMoney } from '../lib/money';

export default function RefundRateCard({ items, currency }: { items: RefundRate[]; currency: string }) {
  if (items.length === 0) {
    return (
      <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
        <div style={{ fontWeight: 600, marginBottom: '0.2rem' }}>Refund rate by item</div>
        <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>No refunds recorded yet.</div>
      </div>
    );
  }

  return (
    <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
      <div style={{ fontWeight: 600, marginBottom: '0.2rem' }}>Refund rate by item</div>
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
                {it.refunded_quantity} of {it.sold_quantity} sold refunded
              </div>
            </div>
            <div style={{ textAlign: 'right', flexShrink: 0 }}>
              <div style={{ fontWeight: 600, color: 'var(--stamp)' }}>
                {it.refund_rate_pct === null ? 'n/a' : `${it.refund_rate_pct.toFixed(1)}%`}
              </div>
              <div style={{ fontSize: '0.76rem', color: 'var(--ink-soft)' }}>
                {formatMoney(it.refunded_amount_cents, currency)}
              </div>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
