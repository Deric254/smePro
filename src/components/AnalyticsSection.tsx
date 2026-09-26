import { useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import { runReport, getBusinessInfo, getProfitByCategory } from '../api';
import type { CategoryProfit } from '../api';
import TimeSlicer, { defaultRange } from './TimeSlicer';
import type { DateRange } from './TimeSlicer';
import { formatMoney } from '../lib/money';
import {
  BarChart, Bar, XAxis, YAxis, CartesianGrid, Tooltip, ResponsiveContainer, LabelList,
  PieChart, Pie, Cell,
} from 'recharts';

const PALETTE = ['var(--stamp)', '#7c9885', '#c98a4b', '#5b7b9a', '#a15c5c', '#8a7ca8'];

type Bucket = 'day' | 'week' | 'month';

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
    const endStr = start.getUTCMonth() === end.getUTCMonth()
      ? String(end.getUTCDate())
      : end.toLocaleDateString(undefined, { month: 'short', day: 'numeric', timeZone: 'UTC' });
    return `${startStr}–${endStr}`;
  }
  const d = new Date(`${label}T00:00:00Z`);
  return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric', timeZone: 'UTC' });
}

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
  const [categoryProfit, setCategoryProfit] = useState<CategoryProfit[] | null>(null);

  useEffect(() => {
    setCategoryProfit(null);
    getProfitByCategory({ start: range.start, end: range.end }).then((r) => setCategoryProfit(r.categories)).catch(() => setCategoryProfit([]));
  }, [range]);

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
      runReport('sales', { agg: 'sum', measure: 'revenue', dimension: 'category', field: 'item_name', start: range.start, end: range.end }),
      runReport('sales', { agg: 'sum', measure: 'revenue', dimension: 'category', field: 'payment_method', start: range.start, end: range.end }),
    ])
      .then(([rev, count, avg, trend, items, payments]) => {
        if (cancelled) return;
        setRevenue(rev.report?.[0]?.value ?? 0);
        setOrderCount(count.report?.[0]?.value ?? 0);
        setAvgSale(avg.report?.[0]?.value ?? 0);
        setSeries((trend.report ?? []).map((p: { label: string; value: number }) => ({ label: p.label, value: p.value })));
        setTopItems((items.report ?? []).slice(0, 20));
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
          <div style={{ overflowX: 'auto', width: '100%', height: '100%' }}>
            <div style={{ height: '100%', width: `max(100%, ${series.length * 50}px)` }}>
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
            </div>
          </div>
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
            misalign the row. 240, not the 220 from the previous
            round: that value turned out to cut the pie chart's own
            margin too close for its largest slice's label (confirmed
            against an actual screenshot, not just reasoned about) —
            see that card's comment for the real fix. This bar chart
            itself would be fine at 220 or smaller; it's kept in step
            with its row partner instead. */}
        <div className="card" style={{ height: 240, display: 'flex', flexDirection: 'column' }}>
          <div style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', marginBottom: '0.4rem', flexShrink: 0 }}>Top sellers by revenue</div>
          {loading ? (
            <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>Loading…</div>
          ) : topItems.length === 0 ? (
            <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>No sales in this period yet.</div>
          ) : (
            <div style={{ flex: 1, minHeight: 0, overflowY: 'auto' }}>
              <div style={{ width: '100%', height: Math.max(topItems.length * 26, 150) }}>
                <ResponsiveContainer width="100%" height="100%">
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
              </div>
            </div>
          )}
        </div>

        {/* Same height as "Top sellers" above — see that card's own
            comment on why these two stay matched. 240 (up from the
            previous round's 220): that value cut this chart's own
            safety margin too close — the largest slice's percentage
            label was sitting right at the card's top edge in an
            actual screenshot. Radius and margin below are sized
            together for 240, with real headroom this time rather than
            a proportionally-shrunk guess. */}
        <div className="card" style={{ height: 240 }}>
          <div style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', marginBottom: '0.4rem' }}>Revenue by payment method</div>
          {loading ? (
            <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>Loading…</div>
          ) : paymentMix.length === 0 ? (
            <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>No sales in this period yet.</div>
          ) : (
            <ResponsiveContainer width="100%" height="90%">
              {/* Margin back up to 18 (from a too-tight 10) and the
                  circle itself sized down to match (62/30, from
                  62/28) — worked through properly this time: card 240
                  → ResponsiveContainer 90% ≈ 216px tall, minus 18px
                  margin each side ≈ 180px usable for the circle AND
                  its outward percentage labels combined. A 62px
                  outerRadius circle is 124px across, leaving ~28px
                  clear on every side for label text — comfortable
                  room, not a bare minimum. */}
              <PieChart margin={{ top: 18, right: 18, bottom: 18, left: 18 }}>
                <Pie
                  data={paymentMix}
                  dataKey="value"
                  nameKey="label"
                  innerRadius={30}
                  outerRadius={62}
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

      {/* Follows the TimeSlicer, same as everything else in this
          section (see categoryProfit's own comment above on why).
          Two-tone bars (profit vs. loss) instead of the single-color
          palette the pie chart above uses, since sign is the one thing
          this chart needs to communicate before any of its magnitudes. */}
      <div className="card" style={{ marginTop: '0.7rem', height: 240, display: 'flex', flexDirection: 'column' }}>
        <div style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', marginBottom: '0.4rem', flexShrink: 0 }}>Profit by category — {range.label}</div>
        {categoryProfit === null ? (
          <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>Loading…</div>
        ) : categoryProfit.length === 0 ? (
          <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>No categorized sales in this period yet.</div>
        ) : (
          <div style={{ flex: 1, minHeight: 0, overflowY: 'auto' }}>
            <div style={{ width: '100%', height: Math.max(categoryProfit.length * 26, 150) }}>
              <ResponsiveContainer width="100%" height="100%">
                <BarChart
                  data={categoryProfit.map((c) => ({ label: c.category, value: c.profit_cents }))}
                  layout="vertical"
                  margin={{ top: 8, left: 8, right: 56, bottom: 4 }}
                >
                  <CartesianGrid strokeDasharray="3 3" stroke="var(--paper-line)" horizontal={false} />
                  <XAxis type="number" tick={{ fontSize: 11, fill: 'var(--ink-soft)' }} tickFormatter={(v) => formatMoney(v, currency)} />
                  <YAxis type="category" dataKey="label" width={110} tick={{ fontSize: 11, fill: 'var(--ink-soft)' }} />
                  <Tooltip
                    contentStyle={{ background: 'var(--paper-card)', border: '1px solid var(--paper-line)', fontSize: '0.82rem' }}
                    formatter={(v) => [formatMoney(Number(v), currency), 'Profit']}
                  />
                  <Bar dataKey="value" radius={[0, 3, 3, 0]}>
                    {categoryProfit.map((c) => (
                      <Cell key={c.category} fill={c.profit_cents >= 0 ? 'var(--stamp)' : '#a15c5c'} />
                    ))}
                    <LabelList
                      dataKey="value"
                      position="right"
                      formatter={(v: ReactNode) => formatMoney(Number(v), currency)}
                      style={{ fontSize: 10, fill: 'var(--ink-soft)' }}
                    />
                  </Bar>
                </BarChart>
              </ResponsiveContainer>
            </div>
          </div>
        )}
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
