import { formatMoney } from '../lib/money';

// No new backend route for this one — report.rs's Category dimension
// already accepts any field on the module, and Sales already has a
// `customer` field, so this is the same call AnalyticsSection.tsx
// already makes for top-selling items, just grouped by customer
// instead of item_name. Already sorted DESC server-side.
export default function TopCustomersCard({ customers, currency }: { customers: { label: string; value: number }[]; currency: string }) {
  if (customers.length === 0) {
    return (
      <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
        <div style={{ fontWeight: 600, marginBottom: '0.2rem' }}>Top customers</div>
        <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>No sales with a customer recorded yet.</div>
      </div>
    );
  }

  return (
    <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
      <div style={{ fontWeight: 600, marginBottom: '0.2rem' }}>Top customers</div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: '0.4rem' }}>
        {customers.map((c) => (
          <div
            key={c.label}
            style={{
              display: 'flex', justifyContent: 'space-between',
              padding: '0.4rem 0', borderBottom: '1px solid var(--paper-line)', fontSize: '0.86rem', gap: '0.6rem',
            }}
          >
            <span>{c.label}</span>
            <span style={{ fontWeight: 600, flexShrink: 0 }}>{formatMoney(c.value, currency)}</span>
          </div>
        ))}
      </div>
    </div>
  );
}
