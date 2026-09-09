import type { ItemProfit } from '../api';
import { formatMoney } from '../lib/money';

// Same "real fraction, not a hidden flag" honesty as the Dashboard's
// GrossProfitKpi — an item's margin built on partial cost data stays
// visibly partial, per item, rather than blending into one business-
// wide number that could look more complete than it is.
export default function ItemMarginCard({ items, currency }: { items: ItemProfit[]; currency: string }) {
  if (items.length === 0) {
    return (
      <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
        <div style={{ fontWeight: 600, marginBottom: '0.2rem' }}>Margin by item</div>
        <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>No sales yet.</div>
      </div>
    );
  }

  return (
    <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
      <div style={{ fontWeight: 600, marginBottom: '0.2rem' }}>Margin by item</div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: '0.4rem' }}>
        {items.map((it) => {
          const isProfit = it.profit_cents >= 0;
          const partialCost = it.cost_bearing_sales_count < it.sales_count;
          return (
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
                  {it.sales_count} sale{it.sales_count === 1 ? '' : 's'}
                  {partialCost ? ` · ${it.cost_bearing_sales_count} with real cost data` : ''}
                </div>
              </div>
              <div style={{ textAlign: 'right', flexShrink: 0 }}>
                <div style={{ fontWeight: 600, color: isProfit ? 'var(--ink)' : 'var(--stamp)' }}>
                  {isProfit ? '+' : '−'}{formatMoney(Math.abs(it.profit_cents), currency)}
                </div>
                <div style={{ fontSize: '0.76rem', color: 'var(--ink-soft)' }}>
                  {it.margin_pct === null ? 'no revenue' : `${it.margin_pct.toFixed(1)}% margin`}
                </div>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
