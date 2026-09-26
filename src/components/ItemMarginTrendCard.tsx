import type { ItemMarginTrend } from '../api';
import { formatMoney } from '../lib/money';

// "Which SKUs are secretly losers" — current 30-day window vs the 30
// days before it, per item. See profit::by_item_trend on the backend
// for exactly what counts as each window and why is_losing_money is
// held to a stricter bar than margin_pct.
export default function ItemMarginTrendCard({ items, currency }: { items: ItemMarginTrend[]; currency: string }) {
  return (
    <div className="card" style={{ marginTop: '0.9rem' }}>
      <div style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', textTransform: 'uppercase', letterSpacing: '0.04em', marginBottom: '0.5rem' }}>
        Margin trend — last 30 days vs the 30 before
      </div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: '0.4rem' }}>
        {items.map((it) => {
          const isProfit = it.current_profit_cents >= 0;
          const change = it.margin_pct_change_pts;
          return (
            <div key={it.item_name} style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'baseline', gap: '0.6rem', fontSize: '0.86rem' }}>
              <div style={{ minWidth: 0 }}>
                <span>{it.item_name}</span>
                {it.is_losing_money && (
                  <span style={{ marginLeft: '0.4rem', fontSize: '0.72rem', fontWeight: 600, color: 'var(--stamp)' }}>losing money</span>
                )}
                <span style={{ fontSize: '0.76rem', color: 'var(--ink-soft)', marginLeft: '0.4rem' }}>
                  {it.current_sales_count} sale{it.current_sales_count === 1 ? '' : 's'}
                </span>
              </div>
              <div style={{ textAlign: 'right', flexShrink: 0 }}>
                <span style={{ fontWeight: 600, color: isProfit ? 'var(--ink)' : 'var(--stamp)' }}>
                  {isProfit ? '+' : '−'}{formatMoney(Math.abs(it.current_profit_cents), currency)}
                </span>
                {change !== null && (
                  <span style={{ fontWeight: 400, fontSize: '0.76rem', color: change >= 0 ? 'var(--ink-soft)' : 'var(--stamp)', marginLeft: '0.4rem' }}>
                    {change >= 0 ? '+' : ''}{change.toFixed(1)} pts
                  </span>
                )}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
