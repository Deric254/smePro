import type { StockRunway } from '../api';

export default function StockRunwayCard({ items }: { items: StockRunway[] }) {
  if (items.length === 0) {
    return (
      <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
        <div style={{ fontWeight: 600, marginBottom: '0.2rem' }}>Stock runway</div>
        <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>No stock on hand to estimate from.</div>
      </div>
    );
  }

  return (
    <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
      <div style={{ fontWeight: 600, marginBottom: '0.2rem' }}>Stock runway</div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: '0.4rem' }}>
        {items.map((it) => {
          const urgent = it.days_of_stock_left !== null && it.days_of_stock_left <= 7;
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
                  {it.quantity} in stock · {it.avg_daily_sales > 0 ? `${it.avg_daily_sales.toFixed(1)}/day` : 'no recent sales'}
                </div>
              </div>
              <div style={{ fontWeight: 600, flexShrink: 0, color: urgent ? 'var(--stamp)' : 'var(--ink)' }}>
                {it.days_of_stock_left === null ? 'n/a' : `${Math.floor(it.days_of_stock_left)}d left`}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
