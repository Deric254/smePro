import type { ExpiringBatch } from '../api';
import { formatMoney } from '../lib/money';

export default function ExpiringBatchesCard({ items, currency }: { items: ExpiringBatch[]; currency: string }) {
  if (items.length === 0) {
    return (
      <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
        <div style={{ fontWeight: 600, marginBottom: '0.2rem' }}>Expiring batches</div>
        <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>Nothing expiring soon.</div>
      </div>
    );
  }

  const urgency = (days: number) => (days <= 3 ? 'var(--danger, #c0392b)' : days <= 10 ? 'var(--warning, #b8860b)' : 'var(--ink-soft)');

  return (
    <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
      <div style={{ fontWeight: 600, marginBottom: '0.2rem' }}>Expiring batches</div>
      <div style={{ color: 'var(--ink-soft)', fontSize: '0.82rem', marginBottom: '0.4rem' }}>
        These sell first under FEFO, but they're worth checking on directly if time is short.
      </div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: '0.4rem' }}>
        {items.map((it) => (
          <div
            key={it.batch_id}
            style={{
              display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start',
              padding: '0.4rem 0', borderBottom: '1px solid var(--paper-line)', fontSize: '0.86rem', gap: '0.6rem',
            }}
          >
            <div style={{ minWidth: 0 }}>
              <div>{it.item_name}</div>
              <div style={{ fontSize: '0.76rem', color: 'var(--ink-soft)' }}>
                {it.quantity_remaining} units · expires {it.expiry_date}
              </div>
            </div>
            <div style={{ textAlign: 'right', flexShrink: 0, fontSize: '0.8rem' }}>
              <div style={{ color: urgency(it.days_to_expiry), fontWeight: 600 }}>
                {it.days_to_expiry <= 0 ? 'expired' : `${it.days_to_expiry}d left`}
              </div>
              <div style={{ color: 'var(--ink-soft)' }}>{formatMoney(it.unit_price, currency)}/unit</div>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
