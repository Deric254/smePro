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

  // Share of revenue across this displayed list, not of the whole
  // business — there's no total-revenue figure passed in here, and
  // implying "% of all sales" from a top-10 slice would be a false
  // precision. Labeled "of shown" below so that distinction stays
  // visible instead of silently assumed.
  const totalShown = customers.reduce((sum, c) => sum + c.value, 0);

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
            <div style={{ textAlign: 'right', flexShrink: 0 }}>
              <div style={{ fontWeight: 600 }}>{formatMoney(c.value, currency)}</div>
              {totalShown > 0 && (
                <div style={{ fontSize: '0.74rem', color: 'var(--ink-soft)' }}>
                  {((c.value / totalShown) * 100).toFixed(1)}% of shown
                </div>
              )}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
