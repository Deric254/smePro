import { useEffect, useState } from 'react';
import { getDebtSummary, getBusinessInfo, ApiError } from '../api';
import type { DebtSummary as DebtSummaryData } from '../api';
import { formatMoney } from '../lib/money';

// Shown at the top of the Debt & Credit module screen (see ModuleView).
// Deliberately its own fetch against a real backend aggregate
// (debt_settlement::summary), not a client-side reduce over whatever
// page of records ModuleView already has loaded — see the comment on
// that endpoint for why: the generic record list caps at 1000 rows,
// which would make a client-side sum quietly wrong for a business
// with more open debt than that. "Clean truthful" numbers here means
// numbers computed over every row, every time.
export default function DebtSummaryWidget({ refreshKey }: { refreshKey: number }) {
  const [data, setData] = useState<DebtSummaryData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [currency, setCurrency] = useState('USD');

  useEffect(() => {
    getDebtSummary()
      .then((d) => setData(d))
      .catch((err) => setError(err instanceof ApiError ? err.message : 'Could not load debt summary'));
  }, [refreshKey]);

  useEffect(() => {
    // Business currency isn't part of the summary response — fetch it
    // the same way PointOfSale.tsx does, rather than assuming a
    // default that would be quietly wrong for any business not on
    // USD (this app runs in Kenya — KES — among other places).
    getBusinessInfo()
      .then((b: any) => { if (b?.currency) setCurrency(b.currency); })
      .catch(() => {}); // default 'USD' stands if this fails — matches every other screen's fallback
  }, []);

  if (error) {
    return <div className="card" style={{ marginBottom: '1.2rem', color: 'var(--stamp)' }}>{error}</div>;
  }
  if (!data) return null;

  const hasOverdue = data.overdue_count > 0;

  return (
    <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(200px, 1fr))', gap: '0.9rem', marginBottom: '1.2rem' }}>
      <SummaryTile
        label="Owed to you (unpaid)"
        amount={data.owed_to_business_unpaid}
        count={data.owed_to_business_unpaid_count}
        currency={currency}
      />
      <SummaryTile
        label="You owe (unpaid)"
        amount={data.owed_by_business_unpaid}
        count={data.owed_by_business_unpaid_count}
        currency={currency}
      />
      <SummaryTile
        label="Due within 7 days"
        amount={data.due_soon_amount}
        count={data.due_soon_count}
        currency={currency}
        tone={data.due_soon_count > 0 ? 'warn' : 'default'}
      />
      <SummaryTile
        label="Overdue"
        amount={data.overdue_amount}
        count={data.overdue_count}
        currency={currency}
        tone={hasOverdue ? 'alarm' : 'default'}
      />
    </div>
  );
}

function SummaryTile({
  label, amount, count, currency, tone = 'default',
}: { label: string; amount: number; count: number; currency: string; tone?: 'default' | 'warn' | 'alarm' }) {
  const toneStyles: Record<string, React.CSSProperties> = {
    default: {},
    warn: { borderColor: '#c9a13b', background: '#fdf6e3' },
    alarm: { borderColor: 'var(--stamp)', background: 'var(--stamp-wash)' },
  };
  return (
    <div className="card" style={{ padding: '0.9rem 1rem', ...toneStyles[tone] }}>
      <div style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', display: 'flex', alignItems: 'center', gap: '0.35rem' }}>
        {tone === 'alarm' && count > 0 && <span aria-hidden style={{ color: 'var(--stamp)' }}>⚠</span>}
        {label}
      </div>
      <div style={{ fontSize: '1.4rem', fontWeight: 600, marginTop: '0.15rem' }}>{formatMoney(amount, currency)}</div>
      <div style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', marginTop: '0.1rem' }}>
        {count} record{count === 1 ? '' : 's'}
      </div>
    </div>
  );
}
