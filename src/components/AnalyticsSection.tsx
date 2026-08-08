import { useEffect, useState } from 'react';
import { runReport } from '../api';
import TimeSlicer, { defaultRange } from './TimeSlicer';
import type { DateRange } from './TimeSlicer';
import { BarChart, Bar, XAxis, YAxis, CartesianGrid, Tooltip, ResponsiveContainer } from 'recharts';

// Deliberately the ONLY file in the app that imports recharts. Kept
// separate on purpose — lazy-loaded from Dashboard.tsx via React.lazy
// — because recharts alone roughly doubles the JS bundle. Every other
// screen (login, POS, admin, module views) should stay fast and never
// pay that cost; only someone actually looking at the sales chart
// should trigger loading it.
export default function AnalyticsSection() {
  const [range, setRange] = useState<DateRange>(defaultRange());
  const [revenue, setRevenue] = useState<number | null>(null);
  const [orderCount, setOrderCount] = useState<number | null>(null);
  const [avgSale, setAvgSale] = useState<number | null>(null);
  const [series, setSeries] = useState<{ label: string; value: number }[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);

    const daySpan = (new Date(range.end).getTime() - new Date(range.start).getTime()) / 86_400_000;
    const bucket = daySpan > 62 ? 'month' : daySpan > 10 ? 'week' : 'day';

    Promise.all([
      runReport('sales', { agg: 'sum', measure: 'revenue', dimension: 'none', start: range.start, end: range.end }),
      runReport('sales', { agg: 'count', dimension: 'none', start: range.start, end: range.end }),
      runReport('sales', { agg: 'avg', measure: 'revenue', dimension: 'none', start: range.start, end: range.end }),
      runReport('sales', { agg: 'sum', measure: 'revenue', dimension: 'time', field: 'created_at', bucket, start: range.start, end: range.end }),
    ])
      .then(([rev, count, avg, trend]) => {
        if (cancelled) return;
        setRevenue(rev.report?.[0]?.value ?? 0);
        setOrderCount(count.report?.[0]?.value ?? 0);
        setAvgSale(avg.report?.[0]?.value ?? 0);
        setSeries((trend.report ?? []).map((p: { label: string; value: number }) => ({ label: p.label, value: p.value })));
      })
      .catch(() => { if (!cancelled) { setRevenue(0); setOrderCount(0); setAvgSale(0); setSeries([]); } })
      .finally(() => { if (!cancelled) setLoading(false); });

    return () => { cancelled = true; };
  }, [range]);

  return (
    <div style={{ marginBottom: '1.6rem' }}>
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', flexWrap: 'wrap', gap: '0.6rem', marginBottom: '0.9rem' }}>
        <h3 style={{ margin: 0 }}>Business at a glance</h3>
        <TimeSlicer value={range} onChange={setRange} />
      </div>

      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(160px, 1fr))', gap: '0.8rem', marginBottom: '0.9rem' }}>
        <KpiCard label={`Revenue — ${range.label}`} value={revenue} loading={loading} format="money" />
        <KpiCard label="Sales" value={orderCount} loading={loading} format="count" />
        <KpiCard label="Average sale" value={avgSale} loading={loading} format="money" />
      </div>

      <div className="card" style={{ height: 220 }}>
        {loading ? (
          <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>Loading…</div>
        ) : series.length === 0 ? (
          <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>No sales in this period yet.</div>
        ) : (
          <ResponsiveContainer width="100%" height="100%">
            <BarChart data={series}>
              <CartesianGrid strokeDasharray="3 3" stroke="var(--paper-line)" />
              <XAxis dataKey="label" tick={{ fontSize: 11, fill: 'var(--ink-soft)' }} />
              <YAxis tick={{ fontSize: 11, fill: 'var(--ink-soft)' }} />
              <Tooltip contentStyle={{ background: 'var(--paper-card)', border: '1px solid var(--paper-line)', fontSize: '0.82rem' }} />
              <Bar dataKey="value" fill="var(--stamp)" radius={[3, 3, 0, 0]} />
            </BarChart>
          </ResponsiveContainer>
        )}
      </div>
    </div>
  );
}

function KpiCard({ label, value, loading, format }: { label: string; value: number | null; loading: boolean; format: 'money' | 'count' }) {
  return (
    <div className="card" style={{ padding: '0.9rem 1rem' }}>
      <div style={{ fontSize: '0.72rem', color: 'var(--ink-soft)', textTransform: 'uppercase', letterSpacing: '0.04em' }}>{label}</div>
      <div style={{ fontSize: '1.5rem', fontWeight: 600, color: 'var(--stamp)', marginTop: '0.2rem' }}>
        {loading || value === null ? '—' : format === 'money' ? value.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 }) : value.toLocaleString()}
      </div>
    </div>
  );
}
