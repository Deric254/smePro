import type { ExpiringBatch, ExpiryTotals } from '../api';
import { formatMoney } from '../lib/money';

export default function ExpiringBatchesCard({ items, summary, currency }: { items: ExpiringBatch[]; summary: ExpiryTotals; currency: string }) {
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
      {summary.expired_units > 0 && (
        <div
          style={{
            border: '1px solid var(--danger, #c0392b)', borderRadius: 4, padding: '0.5rem 0.7rem',
            marginBottom: '0.5rem', fontSize: '0.84rem', color: 'var(--danger, #c0392b)',
          }}
        >
          <strong>{summary.expired_units} expired unit{summary.expired_units === 1 ? '' : 's'}</strong>
          {' '}across {summary.expired_items} item{summary.expired_items === 1 ? '' : 's'} ({summary.expired_batches} batch{summary.expired_batches === 1 ? '' : 'es'}),
          {' '}worth {formatMoney(summary.expired_cost_value, currency)} at cost. They can't be sold — clear them with
          {' '}&ldquo;Write off expired&rdquo; in Inventory.
        </div>
      )}
      <div style={{ color: 'var(--ink-soft)', fontSize: '0.82rem', marginBottom: '0.4rem' }}>
        {summary.expiring_units > 0
          ? `${summary.expiring_units} more unit${summary.expiring_units === 1 ? '' : 's'} (${summary.expiring_batches} batch${summary.expiring_batches === 1 ? '' : 'es'}) expiring within 30 days; they sell soonest-first.`
          : 'Nothing else expiring within 30 days.'}
        {items.length < summary.expired_batches + summary.expiring_batches && ` Showing the first ${items.length} batches.`}
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
                {it.days_to_expiry < 0 ? 'expired' : it.days_to_expiry === 0 ? 'expires today' : `${it.days_to_expiry}d left`}
              </div>
              <div style={{ color: 'var(--ink-soft)' }}>{formatMoney(it.unit_price, currency)}/unit</div>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
