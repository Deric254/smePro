import { useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import { runReport, getBusinessInfo } from '../api';
import TimeSlicer, { defaultRange } from './TimeSlicer';
import type { DateRange } from './TimeSlicer';
import { formatMoney } from '../lib/money';
import {
  BarChart, Bar, XAxis, YAxis, CartesianGrid, Tooltip, ResponsiveContainer, LabelList,
  PieChart, Pie, Cell,
} from 'recharts';

// A small, fixed palette reused across every chart on this page —
// deliberately not derived from data (e.g. hashing a label to a
// color), so the same payment method or item always reads the same
// color if it happens to appear in more than one chart.
const PALETTE = ['var(--stamp)', '#7c9885', '#c98a4b', '#5b7b9a', '#a15c5c', '#8a7ca8'];

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
  const [topItems, setTopItems] = useState<{ label: string; value: number }[]>([]);
  const [paymentMix, setPaymentMix] = useState<{ label: string; value: number }[]>([]);
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
      // Already sorted DESC by the aggregate on the backend (see
      // report.rs's Category dimension) — top sellers first, no
      // client-side sort needed, just slice to a chart-sized top N.
      runReport('sales', { agg: 'sum', measure: 'revenue', dimension: 'category', field: 'item_name', start: range.start, end: range.end }),
      runReport('sales', { agg: 'sum', measure: 'revenue', dimension: 'category', field: 'payment_method', start: range.start, end: range.end }),
    ])
      .then(([rev, count, avg, trend, items, payments]) => {
        if (cancelled) return;
        setRevenue(rev.report?.[0]?.value ?? 0);
        setOrderCount(count.report?.[0]?.value ?? 0);
        setAvgSale(avg.report?.[0]?.value ?? 0);
        setSeries((trend.report ?? []).map((p: { label: string; value: number }) => ({ label: p.label, value: p.value })));
        setTopItems((items.report ?? []).slice(0, 6));
        // "(not set)" is report.rs's own label for a group with no
        // value for the field being grouped on (e.g. a sale with no
        // payment_method recorded) — already a real, non-empty string
        // from the backend, nothing to substitute here.
        setPaymentMix((payments.report ?? []).map((p: { label: string; value: number }) => ({ label: p.label, value: p.value })));
      })
      .catch(() => { if (!cancelled) { setRevenue(0); setOrderCount(0); setAvgSale(0); setSeries([]); setTopItems([]); setPaymentMix([]); } })
      .finally(() => { if (!cancelled) setLoading(false); });

    return () => { cancelled = true; };
  }, [range]);

  return (
    <div style={{ marginBottom: '1rem' }}>
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', flexWrap: 'wrap', gap: '0.6rem', marginBottom: '0.7rem' }}>
        <h3 style={{ margin: 0 }}>Business at a glance</h3>
        <TimeSlicer value={range} onChange={setRange} />
      </div>

      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(160px, 1fr))', gap: '0.7rem', marginBottom: '0.7rem' }}>
        <KpiCard label={`Revenue — ${range.label}`} value={revenue} loading={loading} format="money" currency={currency} />
        <KpiCard label="Sales" value={orderCount} loading={loading} format="count" currency={currency} />
        <KpiCard label="Average sale" value={avgSale} loading={loading} format="money" currency={currency} />
      </div>

      {/* 220 → 180: the single largest fixed height on this page.
          Every value that used to need vertical room here (bar +
          value-label above it) still fits — margin.top was already
          generous (26px) specifically to stop label clipping; 180 just
          removes the leftover slack below that, not the room the
          labels actually need. */}
      <div className="card" style={{ height: 180 }}>
        {loading ? (
          <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>Loading…</div>
        ) : series.length === 0 ? (
          <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>No sales in this period yet.</div>
        ) : (
          <ResponsiveContainer width="100%" height="100%">
            {/* Recharts' inner <svg> clips anything outside its own
                pixel bounds (the browser's default `overflow: hidden`
                on nested svg elements) — the previous top:18 margin
                was too tight for the value-label text sitting above
                the tallest bar, which is exactly what was getting cut
                off. Room added on every side, not just the top, so a
                wide currency-formatted label on the first/last bar
                doesn't clip left/right either. */}
            <BarChart data={series} margin={{ top: 26, right: 12, left: 4, bottom: 4 }}>
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
              <Bar dataKey="value" fill="var(--stamp)" radius={[3, 3, 0, 0]}>
                <LabelList
                  dataKey="value"
                  position="top"
                  formatter={(v: ReactNode) => formatMoney(Number(v), currency)}
                  style={{ fontSize: 10, fill: 'var(--ink-soft)' }}
                />
              </Bar>
            </BarChart>
          </ResponsiveContainer>
        )}
      </div>

      <div
        style={{
          display: 'grid',
          gridTemplateColumns: 'repeat(auto-fit, minmax(280px, 1fr))',
          gap: '0.7rem',
          marginTop: '0.7rem',
        }}
      >
        {/* Same height as the pie chart card below it, deliberately —
            they sit in the same grid row and an explicit height on a
            grid item overrides the row's default stretch-to-match
            behavior, so two different heights here would visibly
            misalign the row. 220 (not the more aggressive 180/200 cut
            used elsewhere on this page) because the OTHER card in
            this row has fixed-pixel geometry (recharts' Pie
            outerRadius is an absolute px value, not responsive) that
            can't safely shrink past a certain point without clipping
            again — see that card's own comment. This bar chart itself
            would be fine smaller; it's kept in step with its row
            partner instead. */}
        <div className="card" style={{ height: 220 }}>
          <div style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', marginBottom: '0.4rem' }}>Top sellers by revenue</div>
          {loading ? (
            <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>Loading…</div>
          ) : topItems.length === 0 ? (
            <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>No sales in this period yet.</div>
          ) : (
            <ResponsiveContainer width="100%" height="90%">
              {/* right:40 was sized for the value label on a
                  medium-length bar; a long currency-formatted total
                  on the top row (the widest bar) needs more room than
                  that or its label clips against the SVG's right
                  edge. */}
              <BarChart data={topItems} layout="vertical" margin={{ top: 8, left: 8, right: 56, bottom: 4 }}>
                <CartesianGrid strokeDasharray="3 3" stroke="var(--paper-line)" horizontal={false} />
                <XAxis type="number" tick={{ fontSize: 11, fill: 'var(--ink-soft)' }} tickFormatter={(v) => formatMoney(v, currency)} />
                <YAxis
                  type="category"
                  dataKey="label"
                  width={110}
                  tick={{ fontSize: 11, fill: 'var(--ink-soft)' }}
                />
                <Tooltip
                  contentStyle={{ background: 'var(--paper-card)', border: '1px solid var(--paper-line)', fontSize: '0.82rem' }}
                  formatter={(v) => [formatMoney(Number(v), currency), 'Revenue']}
                />
                <Bar dataKey="value" fill="var(--stamp)" radius={[0, 3, 3, 0]}>
                  <LabelList
                    dataKey="value"
                    position="right"
                    formatter={(v: ReactNode) => formatMoney(Number(v), currency)}
                    style={{ fontSize: 10, fill: 'var(--ink-soft)' }}
                  />
                </Bar>
              </BarChart>
            </ResponsiveContainer>
          )}
        </div>

        {/* Same height as "Top sellers" above — see that card's own
            comment on why these two stay matched. Reduced from 260 to
            220, together with shrinking the Pie's own outerRadius/
            innerRadius/margin below by the same proportion — NOT
            reduced on its own. This chart's geometry is fixed pixels,
            not responsive to its container the way every other chart
            on this page is, so shrinking the card without also
            shrinking the circle would put the percentage labels right
            back outside the SVG's bounds — exactly the clipping bug
            already fixed once above. Smaller circle, same safe
            margin-to-label ratio, same guarantee. */}
        <div className="card" style={{ height: 220 }}>
          <div style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', marginBottom: '0.4rem' }}>Revenue by payment method</div>
          {loading ? (
            <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>Loading…</div>
          ) : paymentMix.length === 0 ? (
            <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>No sales in this period yet.</div>
          ) : (
            <ResponsiveContainer width="100%" height="90%">
              {/* Margin shrunk from 16 to 10 in proportion with the
                  smaller radius below — still enough room for the
                  percentage labels sitting just outside outerRadius,
                  just scaled down with everything else instead of
                  left oversized for a circle that's no longer as big. */}
              <PieChart margin={{ top: 10, right: 10, bottom: 10, left: 10 }}>
                <Pie
                  data={paymentMix}
                  dataKey="value"
                  nameKey="label"
                  innerRadius={28}
                  outerRadius={58}
                  label={(props: { name?: string; percent?: number }) => `${props.name ?? ''} ${((props.percent ?? 0) * 100).toFixed(0)}%`}
                  labelLine={false}
                >
                  {paymentMix.map((entry, i) => (
                    <Cell key={entry.label} fill={PALETTE[i % PALETTE.length]} />
                  ))}
                </Pie>
                <Tooltip
                  contentStyle={{ background: 'var(--paper-card)', border: '1px solid var(--paper-line)', fontSize: '0.82rem' }}
                  formatter={(v) => formatMoney(Number(v), currency)}
                />
              </PieChart>
            </ResponsiveContainer>
          )}
        </div>
      </div>
    </div>
  );
}

function KpiCard({ label, value, loading, format, currency }: { label: string; value: number | null; loading: boolean; format: 'money' | 'count'; currency: string }) {
  return (
    <div className="card" style={{ padding: '0.7rem 0.9rem' }}>
      <div style={{ fontSize: '0.72rem', color: 'var(--ink-soft)', textTransform: 'uppercase', letterSpacing: '0.04em' }}>{label}</div>
      <div style={{ fontSize: '1.5rem', fontWeight: 600, color: 'var(--stamp)', marginTop: '0.2rem' }}>
        {loading || value === null ? '—' : format === 'money' ? formatMoney(value, currency) : value.toLocaleString()}
      </div>
    </div>
  );
}
