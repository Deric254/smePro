import { useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import { runReport, getBusinessInfo, getProfitByCategory, getDayOfWeekPattern } from '../api';
import type { CategoryProfit, DayOfWeekPattern } from '../api';
import TimeSlicer, { defaultRange } from './TimeSlicer';
import type { DateRange } from './TimeSlicer';
import { formatMoney } from '../lib/money';
import {
  BarChart, Bar, LineChart, Line, XAxis, YAxis, CartesianGrid, Tooltip, ResponsiveContainer, LabelList,
  PieChart, Pie, Cell,
} from 'recharts';

// Fixed palette (not derived from data) so a given payment method or
// item reads the same color across every chart.
const PALETTE = ['var(--stamp)', '#7c9885', '#c98a4b', '#5b7b9a', '#a15c5c', '#8a7ca8'];

type Bucket = 'day' | 'week' | 'month';

// Turns the backend's raw bucket key ("2026-08-11" or "2026-08") into
// a readable chart-axis label ("Aug 11", "Aug 2026", "Aug 11–17").
// Parsed/formatted in UTC since these are calendar dates with no time
// component.
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
    // Repeat the month name only if the week crosses into a new one.
    const endStr = start.getUTCMonth() === end.getUTCMonth()
      ? String(end.getUTCDate())
      : end.toLocaleDateString(undefined, { month: 'short', day: 'numeric', timeZone: 'UTC' });
    return `${startStr}–${endStr}`;
  }
  const d = new Date(`${label}T00:00:00Z`);
  return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric', timeZone: 'UTC' });
}

// The only file that imports recharts — lazy-loaded from Dashboard.tsx
// so other screens don't pay for the bundle size.
export default function AnalyticsSection() {
  const [range, setRange] = useState<DateRange>(defaultRange());
  const [revenue, setRevenue] = useState<number | null>(null);
  const [orderCount, setOrderCount] = useState<number | null>(null);
  const [avgSale, setAvgSale] = useState<number | null>(null);
  // Revenue minus cost_at_sale over the same `range`, same filter, same
  // report engine and same Promise.all as the Revenue KPI beside it —
  // so the two always describe exactly the same set of sales.
  const [profit, setProfit] = useState<number | null>(null);
  const [series, setSeries] = useState<{ label: string; value: number }[]>([]);
  const [topItems, setTopItems] = useState<{ label: string; value: number }[]>([]);
  const [paymentMix, setPaymentMix] = useState<{ label: string; value: number }[]>([]);
  const [bucket, setBucket] = useState<Bucket>('day');
  const [loading, setLoading] = useState(true);
  const [currency, setCurrency] = useState('USD');
  // Scoped to `range`, same as everything else in this section — see
  // profit::by_category's own doc comment on why it filters on
  // created_at, the same range field the sibling category breakdowns
  // just below (revenue by item_name, by payment_method) already use.
  // Reset to null on every range change first, same null-until-real
  // discipline as the rest of this component, so the card shows its
  // own "Loading…" state rather than the previous period's numbers
  // while the new ones are in flight.
  const [categoryProfit, setCategoryProfit] = useState<CategoryProfit[] | null>(null);
  // Scoped to `range`, same as categoryProfit above — the TimeSlicer
  // should move every chart on this page the same way, day-of-week
  // included. The backend (sales_patterns::day_of_week_pattern) takes
  // a lookback DAY COUNT, not a start/end pair, so `range` is
  // converted to one below (dayPatternLookbackDays) rather than
  // passed straight through — and clamped to the same [7, 365] floor
  // the backend itself enforces, since a single day's or week's worth
  // of history genuinely isn't enough to call a weekday "pattern" (a
  // 1-day slice would just show one bar, everything else at zero).
  // The label under the chart shows the real, possibly-clamped count
  // actually sent, so a short slice is never captioned with a window
  // wider than what it actually used. Same null-until-real-data
  // discipline as everything else here.
  const [dayPattern, setDayPattern] = useState<DayOfWeekPattern[] | null>(null);
  const [dayPatternLookbackDays, setDayPatternLookbackDays] = useState(90);

  useEffect(() => {
    setCategoryProfit(null);
    getProfitByCategory({ start: range.start, end: range.end }).then((r) => setCategoryProfit(r.categories)).catch(() => setCategoryProfit([]));
  }, [range]);

  useEffect(() => {
    setDayPattern(null);
    const spanDays = Math.round((new Date(range.end).getTime() - new Date(range.start).getTime()) / 86_400_000) + 1;
    const days = Math.min(365, Math.max(7, spanDays));
    setDayPatternLookbackDays(days);
    getDayOfWeekPattern(days).then((r) => setDayPattern(r.items)).catch(() => setDayPattern([]));
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
      // Already sorted DESC by the aggregate on the backend (see
      // report.rs's Category dimension) — top sellers first, no
      // client-side sort needed, just slice to a chart-sized top N.
      runReport('sales', { agg: 'sum', measure: 'revenue', dimension: 'category', field: 'item_name', start: range.start, end: range.end }),
      runReport('sales', { agg: 'sum', measure: 'revenue', dimension: 'category', field: 'payment_method', start: range.start, end: range.end }),
      runReport('sales', { agg: 'sum', measure: 'cost_at_sale', dimension: 'none', start: range.start, end: range.end }),
    ])
      .then(([rev, count, avg, trend, items, payments, cost]) => {
        if (cancelled) return;
        setRevenue(rev.report?.[0]?.value ?? 0);
        setProfit((rev.report?.[0]?.value ?? 0) - (cost.report?.[0]?.value ?? 0));
        setOrderCount(count.report?.[0]?.value ?? 0);
        setAvgSale(avg.report?.[0]?.value ?? 0);
        setSeries((trend.report ?? []).map((p: { label: string; value: number }) => ({ label: p.label, value: p.value })));
        // Raised from a hard cap of 6 to 20: with the chart below now
        // scrolling internally as this list grows (see that card's
        // own comment), there's no longer a layout reason to hide
        // items past the 6th-best seller — a business with a dozen
        // real sellers should be able to see all of them, not just
        // whichever 6 happened to be on top. 20 is still a cap, just
        // a much less arbitrary one — plenty for even a large catalog
        // without turning this into an unbounded, ever-taller list.
        setTopItems((items.report ?? []).slice(0, 20));
        // "(not set)" is report.rs's own label for a group with no
        // value for the field being grouped on (e.g. a sale with no
        // payment_method recorded) — already a real, non-empty string
        // from the backend, nothing to substitute here.
        setPaymentMix((payments.report ?? []).map((p: { label: string; value: number }) => ({ label: p.label, value: p.value })));
      })
      .catch(() => { if (!cancelled) { setRevenue(0); setProfit(0); setOrderCount(0); setAvgSale(0); setSeries([]); setTopItems([]); setPaymentMix([]); } })
      .finally(() => { if (!cancelled) setLoading(false); });

    return () => { cancelled = true; };
  }, [range]);

  // Cheap client-side sums of data already on screen — used to show
  // each bar/slice's share of the visible total in its tooltip, no
  // extra fetch needed.
  const seriesTotal = series.reduce((sum, p) => sum + p.value, 0);
  const topItemsTotal = topItems.reduce((sum, p) => sum + p.value, 0);
  const paymentMixTotal = paymentMix.reduce((sum, p) => sum + p.value, 0);

  return (
    <div style={{ marginBottom: '1rem' }}>
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', flexWrap: 'wrap', gap: '0.6rem', marginBottom: '0.7rem' }}>
        <h3 style={{ margin: 0 }}>Business at a glance</h3>
        <TimeSlicer value={range} onChange={setRange} />
      </div>

      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(160px, 1fr))', gap: '0.5rem', marginBottom: '0.5rem' }}>
        <KpiCard label={`Revenue — ${range.label}`} value={revenue} loading={loading} format="money" currency={currency} compact />
        <KpiCard label="Sales" value={orderCount} loading={loading} format="count" currency={currency} compact />
        <KpiCard label="Average sale" value={avgSale} loading={loading} format="money" currency={currency} compact />
        <KpiCard label={`Profit — ${range.label}`} value={profit} loading={loading} format="money" currency={currency} compact />
      </div>

      {/* 180 → 260: the space these KPI tiles and the Dashboard-level
          cards above this section now give up (condensed padding,
          merged pulse/highlights row) is spent here instead — this
          trend chart is the first thing a person looks at, so it gets
          the real estate. margin.top stays 26px, same reasoning as
          before (label-clipping headroom); the extra height is pure
          bar-area gain, not a proportional scale-up of the margins. */}
      <div className="card" style={{ height: 260 }}>
        {loading ? (
          <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>Loading…</div>
        ) : series.length === 0 ? (
          <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>No sales in this period yet.</div>
        ) : (
          // A short "Custom" range bucketed by day can still mean
          // dozens of bars (up to ~62 before the bucket auto-switches
          // to week/month — see the bucket calculation above). Squeezed
          // into a fixed-width chart, that many bars' value labels
          // start overlapping into unreadable noise — exactly the
          // "mess" this is meant to prevent as real usage accumulates
          // more days of data. `max(100%, ...)` (a native CSS
          // function, not a JS Math.max — needed here specifically
          // because it can compare a percentage against a pixel value,
          // which JS can't) means: fill the full card width when there
          // are few enough bars to look right doing that, but once
          // there isn't room for every bar's minimum legible width
          // (50px), grow the chart itself past the card's width instead
          // of shrinking the bars — and let this wrapper's own
          // horizontal scrollbar handle the rest, smoothly, the same
          // way "Top sellers" now scrolls vertically for the same
          // reason.
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
                    // Beyond the bare revenue figure, shows what share of
                    // the whole visible range that one bar represents —
                    // seriesTotal is the same series already on screen,
                    // just summed once, so this is free (no extra fetch)
                    // and always in sync with what's plotted.
                    formatter={(v) => [
                      `${formatMoney(Number(v), currency)} (${seriesTotal > 0 ? ((Number(v) / seriesTotal) * 100).toFixed(1) : '0'}% of range)`,
                      'Revenue',
                    ]}
                  />
                  {/* One color per bar (cycling PALETTE) instead of a
                      single flat fill — makes it easier to keep your
                      place scanning across a long, scrollable range,
                      and matches the categorical coloring already used
                      by the pie chart below. Sign/magnitude here is
                      shown by height, not color, so there's no semantic
                      meaning lost by varying it. */}
                  <Bar dataKey="value" radius={[3, 3, 0, 0]}>
                    {series.map((_, i) => (
                      <Cell key={i} fill={PALETTE[i % PALETTE.length]} />
                    ))}
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
            // Same reasoning as the revenue trend chart above, just
            // vertical instead of horizontal: with the cap on this
            // list raised from 6 to 20 (see where topItems is set),
            // a real catalog's full list of sellers can be taller
            // than this card has room for. Rather than compressing
            // every bar to fit — illegible once there are more than
            // 5 or 6 — each bar gets a fixed, always-readable 26px
            // row, the chart's own height grows past the visible
            // window once there are enough items, and this wrapper's
            // scrollbar handles the rest smoothly. Math.max keeps it
            // filling the full available height (no scroll) when
            // there are only a couple of items.
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
                      // Without this, recharts auto-thins ticks it
                      // guesses would overlap at this row height and
                      // silently drops some category labels (the bar
                      // still renders, its name just doesn't) — forcing
                      // every tick to render is what the fixed 26px-
                      // per-row height above is already sized for.
                      interval={0}
                    />
                    <Tooltip
                      contentStyle={{ background: 'var(--paper-card)', border: '1px solid var(--paper-line)', fontSize: '0.82rem' }}
                      formatter={(v) => [
                        `${formatMoney(Number(v), currency)} (${topItemsTotal > 0 ? ((Number(v) / topItemsTotal) * 100).toFixed(1) : '0'}% of top ${topItems.length})`,
                        'Revenue',
                      ]}
                    />
                    <Bar dataKey="value" radius={[0, 3, 3, 0]}>
                      {topItems.map((_, i) => (
                        <Cell key={i} fill={PALETTE[i % PALETTE.length]} />
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
                  formatter={(v) => `${formatMoney(Number(v), currency)} (${paymentMixTotal > 0 ? ((Number(v) / paymentMixTotal) * 100).toFixed(1) : '0'}%)`}
                />
              </PieChart>
            </ResponsiveContainer>
          )}
        </div>
      </div>

      {/* Profit by category now shares its row with the day-of-week
          pattern chart instead of running full width alone — halved so
          there's room for that second, equally important view right
          beside it, same auto-fit two-up grid as the "Top sellers /
          Payment method" row above. Follows the TimeSlicer, same as
          everything else in this section (see categoryProfit's own
          comment above on why). Two-tone bars (profit vs. loss) instead
          of the single-color palette the pie chart above uses, since
          sign is the one thing this chart needs to communicate before
          any of its magnitudes. */}
      <div
        style={{
          display: 'grid',
          gridTemplateColumns: 'repeat(auto-fit, minmax(280px, 1fr))',
          gap: '0.7rem',
          marginTop: '0.7rem',
        }}
      >
      <div className="card" style={{ height: 240, display: 'flex', flexDirection: 'column' }}>
        <div style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', marginBottom: '0.4rem', flexShrink: 0 }}>Profit by category — {range.label}</div>
        {categoryProfit === null ? (
          <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>Loading…</div>
        ) : categoryProfit.length === 0 ? (
          <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>No categorized sales in this period yet.</div>
        ) : (
          // Upright columns (not horizontal rows) so this doesn't
          // read as a second copy of "Top sellers" beside/above it.
          // Same scroll-once-it-doesn't-fit approach as the revenue
          // trend chart, just keyed to category count: each column
          // keeps a legible 80px, and the chart grows past the card
          // width (horizontal scroll) rather than squeezing names.
          <div style={{ overflowX: 'auto', width: '100%', flex: 1, minHeight: 0 }}>
            <div style={{ height: '100%', width: `max(100%, ${categoryProfit.length * 80}px)` }}>
              <ResponsiveContainer width="100%" height="100%">
                <BarChart
                  data={categoryProfit.map((c) => ({ label: c.category, value: c.profit_cents }))}
                  margin={{ top: 26, right: 12, left: 4, bottom: 4 }}
                >
                  <CartesianGrid strokeDasharray="3 3" stroke="var(--paper-line)" />
                  {/* interval={0}: force every category name to render
                      — recharts otherwise silently thins ticks it
                      guesses would overlap (the bug already fixed on
                      "Top sellers"). */}
                  <XAxis dataKey="label" tick={{ fontSize: 10, fill: 'var(--ink-soft)' }} interval={0} />
                  <YAxis tick={{ fontSize: 11, fill: 'var(--ink-soft)' }} tickFormatter={(v) => formatMoney(v, currency)} />
                  <Tooltip
                    contentStyle={{ background: 'var(--paper-card)', border: '1px solid var(--paper-line)', fontSize: '0.82rem' }}
                    formatter={(v) => [`${formatMoney(Math.abs(Number(v)), currency)} ${Number(v) >= 0 ? 'profit' : 'loss'}`, 'Result']}
                  />
                  {/* Each category keeps its own PALETTE color; a loss
                      still overrides to red, since sign has to read
                      instantly. */}
                  <Bar dataKey="value" radius={[3, 3, 0, 0]}>
                    {categoryProfit.map((c, i) => (
                      <Cell key={c.category} fill={c.profit_cents >= 0 ? PALETTE[i % PALETTE.length] : '#a15c5c'} />
                    ))}
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

      {/* The other important chart this row was made room for: the
          day-of-week pattern behind the "Busiest: <day>" line already
          shown in the pulse row above (see Dashboard.tsx) — that text
          names the busiest day but never showed the shape of the whole
          week, which this chart does. Drawn as a line rather than bars
          — every other chart on this dashboard is already a bar or pie,
          and a line reads the week-shape (rise/fall from day to day)
          more directly than seven disconnected columns would. Follows
          the TimeSlicer like every other chart here (see dayPattern's
          own comment above) — the label states the real lookback used,
          which only diverges from `range` itself when the slicer picked
          fewer than 7 days. Sunday→Saturday order comes straight from
          the backend (sales_patterns::day_of_week_pattern), not
          re-sorted here. */}
      <div className="card" style={{ height: 240, display: 'flex', flexDirection: 'column' }}>
        <div style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', marginBottom: '0.4rem', flexShrink: 0 }}>
          Avg revenue by day of week — last {dayPatternLookbackDays} day{dayPatternLookbackDays === 1 ? '' : 's'}
        </div>
        {dayPattern === null ? (
          <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>Loading…</div>
        ) : dayPattern.every((d) => d.occurrences === 0) ? (
          <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>Not enough sales history yet.</div>
        ) : (
          <ResponsiveContainer width="100%" height="100%">
            <LineChart
              data={dayPattern.map((d) => ({ label: d.day_name, value: d.avg_revenue_cents }))}
              margin={{ top: 22, right: 12, left: 4, bottom: 4 }}
            >
              <CartesianGrid strokeDasharray="3 3" stroke="var(--paper-line)" />
              <XAxis dataKey="label" tick={{ fontSize: 11, fill: 'var(--ink-soft)' }} tickFormatter={(v: string) => v.slice(0, 3)} interval={0} />
              <YAxis tick={{ fontSize: 11, fill: 'var(--ink-soft)' }} tickFormatter={(v) => formatMoney(v, currency)} />
              <Tooltip
                contentStyle={{ background: 'var(--paper-card)', border: '1px solid var(--paper-line)', fontSize: '0.82rem' }}
                formatter={(v) => [`${formatMoney(Number(v), currency)} avg`, 'Revenue']}
              />
              <Line
                type="monotone"
                dataKey="value"
                stroke="var(--stamp)"
                strokeWidth={2}
                dot={{ r: 3, fill: 'var(--stamp)' }}
                activeDot={{ r: 5 }}
              >
                <LabelList
                  dataKey="value"
                  position="top"
                  formatter={(v: ReactNode) => formatMoney(Number(v), currency)}
                  style={{ fontSize: 9, fill: 'var(--ink-soft)' }}
                />
              </Line>
            </LineChart>
          </ResponsiveContainer>
        )}
      </div>
      </div>
    </div>
  );
}

function KpiCard({ label, value, loading, format, currency, compact = false }: { label: string; value: number | null; loading: boolean; format: 'money' | 'count'; currency: string; compact?: boolean }) {
  return (
    <div className="card" style={{ padding: compact ? '0.5rem 0.8rem' : '0.7rem 0.9rem' }}>
      <div style={{ fontSize: '0.7rem', color: 'var(--ink-soft)', textTransform: 'uppercase', letterSpacing: '0.04em' }}>{label}</div>
      <div style={{ fontSize: compact ? '1.25rem' : '1.5rem', fontWeight: 600, color: 'var(--stamp)', marginTop: compact ? '0.1rem' : '0.2rem' }}>
        {loading || value === null ? '—' : format === 'money' ? formatMoney(value, currency) : value.toLocaleString()}
      </div>
    </div>
  );
}
