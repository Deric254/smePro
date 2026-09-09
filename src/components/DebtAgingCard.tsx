import type { DebtAgingSummary } from '../api';
import { formatMoney } from '../lib/money';

export default function DebtAgingCard({ aging, currency }: { aging: DebtAgingSummary; currency: string }) {
  const buckets = [
    { label: '1–30 days', amount: aging.bucket_1_30_amount, count: aging.bucket_1_30_count },
    { label: '31–60 days', amount: aging.bucket_31_60_amount, count: aging.bucket_31_60_count },
    { label: '61–90 days', amount: aging.bucket_61_90_amount, count: aging.bucket_61_90_count },
    { label: '90+ days', amount: aging.bucket_90_plus_amount, count: aging.bucket_90_plus_count },
  ];

  if (aging.total_overdue_count === 0) {
    return (
      <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
        <div style={{ fontWeight: 600, marginBottom: '0.2rem' }}>Debtor aging</div>
        <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>Nothing overdue right now.</div>
      </div>
    );
  }

  return (
    <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
      <div style={{ fontWeight: 600, marginBottom: '0.2rem' }}>Debtor aging</div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: '0.4rem' }}>
        {buckets.map((b) => (
          <div
            key={b.label}
            style={{
              display: 'flex', justifyContent: 'space-between',
              padding: '0.4rem 0', borderBottom: '1px solid var(--paper-line)', fontSize: '0.86rem',
            }}
          >
            <span>{b.label}</span>
            <span style={{ color: 'var(--ink-soft)' }}>
              {b.count} · <strong style={{ color: 'var(--ink)' }}>{formatMoney(b.amount, currency)}</strong>
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}
