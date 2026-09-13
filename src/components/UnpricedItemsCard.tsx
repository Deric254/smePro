import type { UnpricedItem } from '../api';
import { formatMoney } from '../lib/money';

export default function UnpricedItemsCard({ items, currency }: { items: UnpricedItem[]; currency: string }) {
  if (items.length === 0) {
    return (
      <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
        <div style={{ fontWeight: 600, marginBottom: '0.2rem' }}>Unpriced items</div>
        <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>Every item in stock has a price set.</div>
      </div>
    );
  }

  return (
    <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
      <div style={{ fontWeight: 600, marginBottom: '0.2rem' }}>Unpriced items</div>
      <div style={{ color: 'var(--ink-soft)', fontSize: '0.8rem', marginBottom: '0.4rem' }}>
        In stock, but would sell for {formatMoney(0, currency)} right now — check these weren't just missed at setup.
      </div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: '0.4rem' }}>
        {items.map((it) => (
          <div
            key={it.item_name}
            style={{
              display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start',
              padding: '0.4rem 0', borderBottom: '1px solid var(--paper-line)', fontSize: '0.86rem', gap: '0.6rem',
            }}
          >
            <span>{it.item_name}</span>
            <span style={{ textAlign: 'right', flexShrink: 0, color: 'var(--ink-soft)' }}>
              {it.quantity} in stock
              {it.unit_cost_cents > 0 && <> · cost {formatMoney(it.unit_cost_cents, currency)}</>}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}
