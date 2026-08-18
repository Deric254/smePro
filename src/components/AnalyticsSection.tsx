import { useEffect, useState } from 'react';
import { runReport, getBusinessInfo } from '../api';
import TimeSlicer, { defaultRange } from './TimeSlicer';
import type { DateRange } from './TimeSlicer';
import { formatMoney } from '../lib/money';
import { BarChart, Bar, XAxis, YAxis, CartesianGrid, Tooltip, ResponsiveContainer } from 'recharts';

type Bucket = 'day' | 'week' | 'month';

// The backend hands back sortable-but-opaque bucket keys ("2026-08-11"
// for a day, "2026-08" for a month, and — since report.rs's week
// bucket is now the Monday date that starts the week — "2026-08-11"
// for a week too, meaning day/week share a raw format and only differ
// in how they're displayed here). This turns those into what someone
// actually wants to read on a chart axis: "Aug 11", "Aug 2026", or a
// real week range like "Aug 11–17".
//
// Parsed with an explicit UTC midnight timestamp and formatted back
// out in UTC deliberately — these are calendar dates with no time
// component, and letting the browser's local timezone touch them
// could shift a date backward by a day for anyone west of UTC.
function formatBucketLabel(bucket: Bucket, label: string): string {
  if (bucket === 'month') {
    const [year, month] = label.split('-').map(Number);
    const d = new Date(Date.UTC(year, month - 1, 1));
    return d.toLocaleDateString(undefined, { month: 'short', year: 'numeric', timeZone: 'UTC' });
  }
  if (bucket === 'week') {
    const start = new Date(`${label}T00:00:00Z`);
    const end = new Date(start.getTime() + 6 * 86_400_000);
    const startStr = start.toLocaleDateString(undefined, { month: 'short', day: 'numeric', timeZone: 'UTC' });
    // Only repeat the month name if the week actually crosses into a
    // new one ("Aug 29 – Sep 4") — otherwise it's just noise ("Aug 11
    // – Aug 17" is harder to scan at a glance than "Aug 11–17").
    const endStr = start.getUTCMonth() === end.getUTCMonth()
      ? String(end.getUTCDate())
      : end.toLocaleDateString(undefined, { month: 'short', day: 'numeric', timeZone: 'UTC' });
    return `${startStr}–${endStr}`;
  }
  const d = new Date(`${label}T00:00:00Z`);
  return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric', timeZone: 'UTC' });
}

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
  const [bucket, setBucket] = useState<Bucket>('day');
  const [loading, setLoading] = useState(true);
  const [currency, setCurrency] = useState('USD');

  useEffect(() => {
    getBusinessInfo().then((b: any) => { if (b?.currency) setCurrency(b.currency); }).catch(() => {});
  }, []);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);

    const daySpan = (new Date(range.end).getTime() - new Date(range.start).getTime()) / 86_400_000;
    const bucket: Bucket = daySpan > 62 ? 'month' : daySpan > 10 ? 'week' : 'day';
    setBucket(bucket);

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
        <KpiCard label={`Revenue — ${range.label}`} value={revenue} loading={loading} format="money" currency={currency} />
        <KpiCard label="Sales" value={orderCount} loading={loading} format="count" currency={currency} />
        <KpiCard label="Average sale" value={avgSale} loading={loading} format="money" currency={currency} />
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
              <XAxis
                dataKey="label"
                tick={{ fontSize: 11, fill: 'var(--ink-soft)' }}
                tickFormatter={(v) => formatBucketLabel(bucket, v)}
                interval="preserveStartEnd"
                minTickGap={24}
              />
              <YAxis tick={{ fontSize: 11, fill: 'var(--ink-soft)' }} tickFormatter={(v) => formatMoney(v, currency)} />
              <Tooltip
                contentStyle={{ background: 'var(--paper-card)', border: '1px solid var(--paper-line)', fontSize: '0.82rem' }}
                labelFormatter={(v) => formatBucketLabel(bucket, String(v))}
                formatter={(v) => [formatMoney(Number(v), currency), 'Revenue']}
              />
              <Bar dataKey="value" fill="var(--stamp)" radius={[3, 3, 0, 0]} />
            </BarChart>
          </ResponsiveContainer>
        )}
      </div>
    </div>
  );
}

function KpiCard({ label, value, loading, format, currency }: { label: string; value: number | null; loading: boolean; format: 'money' | 'count'; currency: string }) {
  return (
    <div className="card" style={{ padding: '0.9rem 1rem' }}>
      <div style={{ fontSize: '0.72rem', color: 'var(--ink-soft)', textTransform: 'uppercase', letterSpacing: '0.04em' }}>{label}</div>
      <div style={{ fontSize: '1.5rem', fontWeight: 600, color: 'var(--stamp)', marginTop: '0.2rem' }}>
        {loading || value === null ? '—' : format === 'money' ? formatMoney(value, currency) : value.toLocaleString()}
      </div>
    </div>
  );
}
