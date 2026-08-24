import { useEffect, useState } from 'react';
import { listCustomers, getCustomer, getBusinessInfo } from '../api';
import type { CustomerSummary, CustomerDetail } from '../api';
import { formatMoney } from '../lib/money';
import { formatBackendDate, formatBackendDateTime } from '../lib/date';

export default function Customers() {
  const [customers, setCustomers] = useState<CustomerSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<CustomerDetail | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [search, setSearch] = useState('');
  const [currency, setCurrency] = useState('USD');

  useEffect(() => {
    getBusinessInfo().then((b: any) => { if (b?.currency) setCurrency(b.currency); }).catch(() => {});
  }, []);

  useEffect(() => {
    listCustomers()
      .then((r) => setCustomers(r.customers))
      .catch(() => setError('Could not load customers'))
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    if (!selectedId) { setDetail(null); return; }
    setDetailLoading(true);
    getCustomer(selectedId)
      .then(setDetail)
      .catch(() => setError('Could not load that customer'))
      .finally(() => setDetailLoading(false));
  }, [selectedId]);

  const filtered = customers.filter((c) => {
    const q = search.toLowerCase();
    return !q || (c.name ?? '').toLowerCase().includes(q) || (c.phone ?? '').includes(q);
  });

  const totalLtv = customers.reduce((sum, c) => sum + c.lifetime_value, 0);

  if (selectedId) {
    return (
      <div>
        <button className="btn btn-outline" style={{ marginBottom: '1rem' }} onClick={() => setSelectedId(null)}>← Back to customers</button>
        {detailLoading ? (
          <div style={{ color: 'var(--ink-soft)' }}>Loading…</div>
        ) : detail ? (
          <div>
            <div style={{ display: 'flex', alignItems: 'center', gap: '0.9rem', marginBottom: '1.2rem' }}>
              <span className="stamp-badge" style={{ width: '3rem', height: '3rem', fontSize: '1rem', color: 'var(--stamp)' }}>
                {(detail.name || detail.phone || '?').slice(0, 2).toUpperCase()}
              </span>
              <div>
                <h1 style={{ margin: 0 }}>{detail.name || detail.phone}</h1>
                <div style={{ fontSize: '0.85rem', color: 'var(--ink-soft)' }}>
                  {detail.phone ? `${detail.phone} · ` : 'No phone on file (matched by name) · '}
                  customer since {formatBackendDate(detail.customer_since)}
                </div>
              </div>
            </div>

            <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(150px, 1fr))', gap: '0.8rem', marginBottom: '1.4rem' }}>
              <div className="card" style={{ padding: '0.9rem 1rem' }}>
                <div style={{ fontSize: '0.72rem', color: 'var(--ink-soft)', textTransform: 'uppercase' }}>Lifetime value</div>
                <div style={{ fontSize: '1.5rem', fontWeight: 600, color: 'var(--stamp)' }}>{formatMoney(detail.lifetime_value, currency)}</div>
              </div>
              <div className="card" style={{ padding: '0.9rem 1rem' }}>
                <div style={{ fontSize: '0.72rem', color: 'var(--ink-soft)', textTransform: 'uppercase' }}>Orders</div>
                <div style={{ fontSize: '1.5rem', fontWeight: 600 }}>{detail.order_count}</div>
              </div>
            </div>

            <h3>Purchase history</h3>
            <table style={{ width: '100%', borderCollapse: 'collapse' }}>
              <thead>
                <tr style={{ borderBottom: '2px solid var(--ink)', fontSize: '0.75rem', textTransform: 'uppercase', color: 'var(--ink-soft)' }}>
                  <th style={{ textAlign: 'left', padding: '0.5rem' }}>Item</th>
                  <th style={{ textAlign: 'right', padding: '0.5rem' }}>Qty</th>
                  <th style={{ textAlign: 'right', padding: '0.5rem' }}>Amount</th>
                  <th style={{ textAlign: 'right', padding: '0.5rem' }}>Date</th>
                </tr>
              </thead>
              <tbody>
                {detail.purchases.map((p, i) => (
                  <tr key={i} style={{ borderBottom: '1px solid var(--paper-line)' }}>
                    <td style={{ padding: '0.5rem' }}>{p.item_name}</td>
                    <td style={{ textAlign: 'right', padding: '0.5rem' }} className="mono">{p.quantity}</td>
                    <td style={{ textAlign: 'right', padding: '0.5rem' }} className="mono">{formatMoney(p.revenue, currency)}</td>
                    <td style={{ textAlign: 'right', padding: '0.5rem', fontSize: '0.82rem', color: 'var(--ink-soft)' }}>{formatBackendDateTime(p.date)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <div style={{ color: 'var(--stamp)' }}>{error || 'Customer not found'}</div>
        )}
      </div>
    );
  }

  return (
    <div>
      <h1>Customers</h1>
      <p style={{ color: 'var(--ink-soft)', fontSize: '0.85rem', marginTop: '-0.6rem' }}>
        Anyone who gave a name or phone at checkout — sorted by how much they've spent with you.
        Sales where nobody offered their details stay anonymous, same as always.
      </p>

      {customers.length > 0 && (
        <div className="card" style={{ marginBottom: '1rem', padding: '0.9rem 1rem', display: 'inline-block' }}>
          <div style={{ fontSize: '0.72rem', color: 'var(--ink-soft)', textTransform: 'uppercase' }}>Total tracked lifetime value</div>
          <div style={{ fontSize: '1.4rem', fontWeight: 600, color: 'var(--stamp)' }}>{formatMoney(totalLtv, currency)}</div>
        </div>
      )}

      <input
        placeholder="Search by name or phone…"
        value={search}
        onChange={(e) => setSearch(e.target.value)}
        style={{ width: '100%', maxWidth: 320, marginBottom: '1rem', display: 'block' }}
      />

      {loading ? (
        <div style={{ color: 'var(--ink-soft)' }}>Loading…</div>
      ) : error ? (
        <div style={{ color: 'var(--stamp)' }}>{error}</div>
      ) : filtered.length === 0 ? (
        <div className="card">
          {customers.length === 0
            ? "No customers yet — they're captured automatically the first time someone gives a name or phone at checkout."
            : 'No customers match that search.'}
        </div>
      ) : (
        <table style={{ width: '100%', borderCollapse: 'collapse' }}>
          <thead>
            <tr style={{ borderBottom: '2px solid var(--ink)', fontSize: '0.75rem', textTransform: 'uppercase', color: 'var(--ink-soft)' }}>
              <th style={{ textAlign: 'left', padding: '0.5rem' }}>Name</th>
              <th style={{ textAlign: 'left', padding: '0.5rem' }}>Phone</th>
              <th style={{ textAlign: 'right', padding: '0.5rem' }}>Orders</th>
              <th style={{ textAlign: 'right', padding: '0.5rem' }}>Lifetime value</th>
              <th style={{ textAlign: 'right', padding: '0.5rem' }}>Last purchase</th>
            </tr>
          </thead>
          <tbody>
            {filtered.map((c) => (
              <tr
                key={c.id}
                onClick={() => setSelectedId(c.id)}
                style={{ borderBottom: '1px solid var(--paper-line)', cursor: 'pointer' }}
                onMouseEnter={(e) => (e.currentTarget.style.background = 'var(--stamp-wash)')}
                onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
              >
                <td style={{ padding: '0.5rem', fontWeight: 600 }}>{c.name || <span style={{ color: 'var(--ink-faint)', fontWeight: 400 }}>—</span>}</td>
                <td style={{ padding: '0.5rem' }} className="mono">{c.phone || <span style={{ color: 'var(--ink-faint)', fontFamily: 'var(--font-body)' }}>—</span>}</td>
                <td style={{ textAlign: 'right', padding: '0.5rem' }} className="mono">{c.order_count}</td>
                <td style={{ textAlign: 'right', padding: '0.5rem', fontWeight: 600, color: 'var(--stamp)' }} className="mono">{formatMoney(c.lifetime_value, currency)}</td>
                <td style={{ textAlign: 'right', padding: '0.5rem', fontSize: '0.82rem', color: 'var(--ink-soft)' }}>
                  {c.last_purchase_at ? formatBackendDate(c.last_purchase_at) : '—'}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
