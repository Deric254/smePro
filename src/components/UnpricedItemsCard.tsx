import type { UnpricedItem, ZeroCostPurchase } from '../api';
import { formatMoney } from '../lib/money';

export default function UnpricedItemsCard({
  items, zeroCostPurchases, currency,
}: {
  items: UnpricedItem[];
  zeroCostPurchases: ZeroCostPurchase[];
  currency: string;
}) {
  if (items.length === 0 && zeroCostPurchases.length === 0) {
    return (
      <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
        <div style={{ fontWeight: 600, marginBottom: '0.2rem' }}>Unpriced items</div>
        <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>
          Every Inventory item has a cost and price, and no Purchasing order was recorded at $0 cost.
        </div>
      </div>
    );
  }

  const label = (it: UnpricedItem) => {
    if (it.missing === 'both') return 'no cost or price set';
    if (it.missing === 'cost') return 'no cost set';
    return 'no selling price set';
  };

  return (
    <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
      <div style={{ fontWeight: 600, marginBottom: '0.2rem' }}>Unpriced items</div>
      <div style={{ color: 'var(--ink-soft)', fontSize: '0.82rem', marginBottom: '0.4rem' }}>
        A $0 cost or price won't stop a sale — it'll just ring one up for free.
      </div>
      {items.length > 0 && (
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
                  {it.quantity} in stock · {label(it)}
                </div>
              </div>
              <div style={{ textAlign: 'right', flexShrink: 0, fontSize: '0.8rem', color: 'var(--ink-soft)' }}>
                <div>cost {formatMoney(it.unit_cost_cents, currency)}</div>
                <div>price {formatMoney(it.unit_price_cents, currency)}</div>
              </div>
            </div>
          ))}
        </div>
      )}
      {zeroCostPurchases.length > 0 && (
        <div style={{ marginTop: items.length > 0 ? '0.7rem' : 0 }}>
          <div style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', textTransform: 'uppercase', letterSpacing: '0.04em', marginBottom: '0.3rem' }}>
            Zero-cost purchase orders
          </div>
          <div style={{ display: 'flex', flexDirection: 'column', gap: '0.4rem' }}>
            {zeroCostPurchases.map((p) => (
              <div
                key={p.po_number}
                style={{
                  display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start',
                  padding: '0.4rem 0', borderBottom: '1px solid var(--paper-line)', fontSize: '0.86rem', gap: '0.6rem',
                }}
              >
                <div style={{ minWidth: 0 }}>
                  <div>{p.po_number} · {p.item_name}</div>
                  <div style={{ fontSize: '0.76rem', color: 'var(--ink-soft)' }}>
                    {p.supplier} · {p.quantity} units · {p.received ? 'received' : 'not yet received'}
                  </div>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
