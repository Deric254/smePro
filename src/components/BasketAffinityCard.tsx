import type { BasketPair } from '../api';
import { formatMoney } from '../lib/money';

// Plain read-only list — kept out of AnalyticsSection.tsx so it
// doesn't force recharts to load where it isn't needed.
export default function BasketAffinityCard({ pairs, currency }: { pairs: BasketPair[]; currency: string }) {
  if (pairs.length === 0) {
    return (
      <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
        <div style={{ fontWeight: 600, marginBottom: '0.2rem' }}>Frequently bought together</div>
        <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>
          Not enough shared-order history yet.
        </div>
      </div>
    );
  }

  return (
    <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
      <div style={{ fontWeight: 600, marginBottom: '0.2rem' }}>Frequently bought together</div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: '0.4rem' }}>
        {pairs.map((p) => (
          <div
            key={`${p.item_a}::${p.item_b}`}
            style={{
              display: 'flex', justifyContent: 'space-between', alignItems: 'center',
              padding: '0.4rem 0', borderBottom: '1px solid var(--paper-line)', fontSize: '0.86rem', gap: '0.6rem',
            }}
          >
            <span>{p.item_a} <span style={{ color: 'var(--ink-soft)' }}>+</span> {p.item_b}</span>
            <span style={{ textAlign: 'right', flexShrink: 0, color: 'var(--ink-soft)' }}>
              {p.order_count} order{p.order_count === 1 ? '' : 's'}
              <span style={{ marginLeft: '0.5rem', color: 'var(--ink)', fontWeight: 600 }}>
                {formatMoney(p.combined_revenue_cents, currency)}
              </span>
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}
