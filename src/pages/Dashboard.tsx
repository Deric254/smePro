import { useEffect, useState, lazy, Suspense } from 'react';
import type { ReactNode } from 'react';
import { listModules, listRecords, getModuleSchema, runReport, listUsers, getBusinessInfo, getSettings, setSetting, getDebtSummary, getGrossProfitSummary, getBusinessPulse, getReportHighlights } from '../api';
import type { ModuleListItem } from '../types';
import type { DebtSummary, GrossProfitSummary, BusinessPulse, ReportHighlights } from '../api';
import { formatMoney } from '../lib/money';

// Lazy-loaded: it's the only place recharts is used, and that roughly
// doubles the bundle size — only downloaded when Dashboard is viewed.
const AnalyticsSection = lazy(() => import('../components/AnalyticsSection'));

function initials(name: string) {
  const words = name.split(/[\s/]+/).filter(Boolean);
  if (words.length === 1) return words[0].slice(0, 2).toUpperCase();
  return (words[0][0] + words[1][0]).toUpperCase();
}

// Money fields are always integer cents (see lib/money.ts) regardless
// of whether the value looks whole — only the field's declared type
// (isMoney) can be used to decide, never the number's own shape.
function formatMetricValue(value: number, isMoney: boolean, currency: string): string {
  if (isMoney) return formatMoney(value, currency);
  return Number.isInteger(value) ? value.toLocaleString() : value.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 });
}

interface ModuleStat {
  module: ModuleListItem;
  recordCount: number | null;
  metricValue: number | null;
  metricLabel: string | null;
  metricIsMoney: boolean;
}

export default function Dashboard({ businessName, onSelectModule, onOpenAdmin, canViewReports }: {
  businessName: string;
  onSelectModule: (id: string) => void;
  onOpenAdmin: () => void;
  canViewReports: boolean;
}) {
  const [stats, setStats] = useState<ModuleStat[]>([]);
  const [loading, setLoading] = useState(true);
  const [userCount, setUserCount] = useState<number | null>(null);
  const [checklistDismissed, setChecklistDismissed] = useState<boolean | null>(null);
  const [currency, setCurrency] = useState('USD');
  // null = not fetched yet / module not enabled / no permission — the
  // KPI card below only renders once this is a real object, so an
  // in-progress or failed fetch never shows a wrong or half-formed
  // number.
  const [debtSummary, setDebtSummary] = useState<DebtSummary | null>(null);
  // Same null-until-real-data discipline as debtSummary above.
  const [grossProfit, setGrossProfit] = useState<GrossProfitSummary | null>(null);
  // Same again — the previously AI-chat-only "business pulse" readout
  // (trend, forecast, low-stock/overdue flags), now surfaced directly
  // here so it doesn't require opening the AI panel and asking a
  // question first. See getBusinessPulse in api.ts.
  const [pulse, setPulse] = useState<BusinessPulse | null>(null);
  // One-line teasers pointing into the fuller Stock health / Sales
  // patterns sections on the Reports page — see report_highlights.rs.
  // Each field independently null when there's nothing worth
  // highlighting yet, same discipline as everything else here.
  const [highlights, setHighlights] = useState<ReportHighlights | null>(null);

  useEffect(() => {
    let cancelled = false;

    listModules().then(async (res) => {
      const enabled: ModuleListItem[] = res.modules.filter((m: ModuleListItem) => m.enabled);
      const withStats = await Promise.all(
        enabled.map(async (module): Promise<ModuleStat> => {
          let recordCount: number | null = null;
          let metricValue: number | null = null;
          let metricLabel: string | null = null;
          let metricIsMoney = false;
          try {
            const r = await listRecords(module.id);
            recordCount = r.records.length;
          } catch { /* leave null — the tile still renders, just without a count */ }
          try {
            const schema = await getModuleSchema(module.id);
            const metric = schema.dashboard_metric;
            // THE BUG THIS FIXES: this metric (e.g. Sales' own
            // "revenue this period") came from the exact same generic
            // report engine Reports.tsx and AnalyticsSection use, but
            // this call had no reports-access gate at all — any role
            // with plain `read` on a module got its business metric
            // shown on the Dashboard regardless of can_view_reports,
            // which defeats the entire point of that flag existing.
            // Skip the fetch outright rather than fetch-then-hide, so
            // a role without reports access never even asks for it.
            if (metric && canViewReports) {
              const report = await runReport(module.id, {
                agg: metric.aggregation,
                measure: metric.measure ?? '',
                dimension: 'none',
              });
              metricValue = report.report?.[0]?.value ?? 0;
              metricLabel = metric.label;
              metricIsMoney = schema.fields.find((f: { name: string; type: string }) => f.name === metric.measure)?.type === 'money';
            }
          } catch { /* module has no metric, or this role can't read it — falls back to record count below */ }
          return { module, recordCount, metricValue, metricLabel, metricIsMoney };
        })
      );
      if (!cancelled) { setStats(withStats); setLoading(false); }
    }).catch(() => { if (!cancelled) setLoading(false); });

    getBusinessInfo().then((b: any) => { if (!cancelled && b?.currency) setCurrency(b.currency); }).catch(() => {});

    // Best-effort — Staff/some roles won't have permission for these,
    // and that's fine, the dashboard just quietly shows less.
    listUsers().then((r) => { if (!cancelled) setUserCount(r.users.filter((u: { active: boolean }) => u.active).length); }).catch(() => {});
    getSettings().then((s) => { if (!cancelled) setChecklistDismissed(s.onboarding_dismissed === 'true'); }).catch(() => { if (!cancelled) setChecklistDismissed(false); });
    // Same reasoning as the per-module metric fetch above: these are
    // report/analytics-only widgets. Skipping the fetch entirely (not
    // fetch-then-hide) when the role can't view reports, consistent
    // with the dashboard_metric skip above — one rule, applied
    // everywhere business-performance data could otherwise leak onto
    // this screen regardless of what the backend itself enforces.
    if (canViewReports) {
      // Also best-effort: fails silently (module not enabled, or this
      // role lacks "read" on it) and the KPI card below just doesn't
      // render — same as every module tile already does when its own
      // metric fetch fails.
      getDebtSummary().then((d) => { if (!cancelled) setDebtSummary(d); }).catch(() => {});
      // Same best-effort fetch, same reason — Sales not enabled, or no
      // "read" permission on it, and the card below just doesn't render.
      getGrossProfitSummary().then((p) => { if (!cancelled) setGrossProfit(p); }).catch(() => {});
      // Same best-effort fetch, same reason. A `has_data: false` pulse
      // still renders (its own honest "not enough history yet" state,
      // set inside BusinessPulseCard) — only a hard fetch failure (e.g.
      // no permission at all) leaves this null and skips the card.
      getBusinessPulse().then((r) => { if (!cancelled) setPulse(r.business_pulse); }).catch(() => {});
      getReportHighlights().then((r) => { if (!cancelled) setHighlights(r); }).catch(() => {});
    }

    return () => { cancelled = true; };
  }, [canViewReports]);

  const hasAnyRecords = stats.some((s) => (s.recordCount ?? 0) > 0);
  const showChecklist = checklistDismissed === false;

  async function dismissChecklist() {
    setChecklistDismissed(true);
    try { await setSetting('onboarding_dismissed', 'true'); } catch { /* not critical if this fails to save */ }
  }

  return (
    <div>
      <div style={styles.header}>
        <div style={styles.eyebrow}>Welcome back</div>
        <h1 style={{ margin: '0.15rem 0 0' }}>{businessName || 'Your business'}</h1>
      </div>

      {(grossProfit || debtSummary) && (
        <div style={styles.kpiRow}>
          {grossProfit && <GrossProfitKpi data={grossProfit} currency={currency} onOpen={() => onSelectModule('sales')} />}
          {debtSummary && <DebtStandingKpi data={debtSummary} currency={currency} onOpen={() => onSelectModule('debt_credit')} />}
        </div>
      )}

      {/* Pulse and Highlights used to be two separate full-width cards
          stacked on top of each other, each just a sentence or two of
          content padded out to card size. Merged into one condensed
          row. Every underlying fetch (getBusinessPulse,
          getReportHighlights) and the has_data/null-until-real-data
          discipline are unchanged; this only changes how the same data
          is laid out. BusinessPulseCard itself is untouched — it's
          still used at full detail in the AI chat panel
          (AiFloatingButton.tsx) — this row reads pulse's fields
          directly instead of rendering that component.
          Every field BusinessPulse actually carries gets a slot here
          now — headline + trend, the forecast, low stock, and the top
          recommendation — plus the highlights' urgent-item/busiest-day,
          not just the subset that fit before. Built as an array and
          interspersed with a real "•" separator (was: bare flex gap,
          which reads as unrelated fragments rather than one readout)
          since the row has the width to spell each one out. */}
      {(pulse || (highlights && (highlights.most_urgent_item || highlights.busiest_day))) && (() => {
        const items: ReactNode[] = [];
        if (pulse && pulse.has_data) {
          items.push(
            <span key="revenue">
              <strong>{formatMoney(pulse.revenue_this_period_cents, currency)}</strong> this month
              {pulse.pct_change !== null && (
                <span style={{ color: pulse.pct_change >= 0 ? 'var(--ok)' : 'var(--stamp)', fontWeight: 600, marginLeft: '0.3rem' }}>
                  {pulse.pct_change >= 0 ? '↑' : '↓'} {Math.abs(pulse.pct_change).toFixed(0)}%
                </span>
              )}
            </span>
          );
          items.push(
            <span key="forecast" style={{ color: 'var(--ink-soft)' }}>
              Forecast: <span style={{ color: 'var(--ink)', fontWeight: 600 }}>{formatMoney(pulse.forecast_next_period_cents, currency)}</span> next period
            </span>
          );
          if (pulse.low_stock_count > 0) {
            items.push(<span key="lowstock" style={{ color: 'var(--stamp)', fontWeight: 600 }}>{pulse.low_stock_count} low stock</span>);
          }
          if (pulse.recommendations.length > 0) {
            items.push(<span key="rec" style={{ color: 'var(--ink-soft)' }}>{pulse.recommendations[0]}</span>);
          }
        }
        if (highlights?.most_urgent_item) {
          items.push(
            <span key="urgent">
              <span style={{ color: 'var(--stamp)', fontWeight: 600 }}>{Math.floor(highlights.most_urgent_item.days_of_stock_left)}d left</span>
              {' '}on {highlights.most_urgent_item.item_name}
            </span>
          );
        }
        if (highlights?.busiest_day) {
          items.push(
            <span key="busiest" style={{ color: 'var(--ink-soft)' }}>
              Busiest: <span style={{ color: 'var(--ink)', fontWeight: 600 }}>{highlights.busiest_day.day_name}</span>
            </span>
          );
        }
        return (
          <div className="card" style={{ marginBottom: '0.6rem', padding: '0.45rem 0.9rem', display: 'flex', alignItems: 'center', flexWrap: 'wrap', gap: '0.5rem', fontSize: '0.8rem' }}>
            {items.map((item, i) => (
              <span key={i} style={{ display: 'flex', alignItems: 'center', gap: '0.5rem' }}>
                {i > 0 && <span aria-hidden style={{ color: 'var(--ink-faint)' }}>•</span>}
                {item}
              </span>
            ))}
            <button
              onClick={() => onSelectModule('__reports__')}
              style={{ marginLeft: 'auto', background: 'none', border: 'none', color: 'var(--stamp)', fontWeight: 600, cursor: 'pointer', padding: 0, fontSize: '0.8rem' }}
            >
              Full reports →
            </button>
          </div>
        );
      })()}

      {canViewReports && stats.some((s) => s.module.id === 'sales') && (
        <Suspense fallback={<div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem', marginBottom: '1.6rem' }}>Loading analytics…</div>}>
          <AnalyticsSection />
        </Suspense>
      )}

      {showChecklist && (
        <div className="card" style={styles.checklistCard}>
          <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start' }}>
            <h3 style={{ margin: '0 0 0.6rem' }}>Getting started</h3>
            <button className="btn btn-outline" style={styles.dismissBtn} onClick={dismissChecklist}>Dismiss</button>
          </div>
          <ChecklistItem
            done={hasAnyRecords}
            label={hasAnyRecords ? 'Added your first record' : 'Add your first record'}
            detail="Pick any module below and create an entry — a product, a sale, whatever fits your business."
            onClick={() => stats[0] && onSelectModule(stats[0].module.id)}
          />
          <ChecklistItem
            done={(userCount ?? 1) > 1}
            label={(userCount ?? 1) > 1 ? 'Invited your team' : 'Invite your team'}
            detail="Add staff accounts with exactly the access they need — Admin → Users."
            onClick={onOpenAdmin}
          />
        </div>
      )}

      <h3 style={{ margin: '1rem 0 0.6rem' }}>Your modules</h3>
      {loading ? (
        <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>Loading…</div>
      ) : stats.length === 0 ? (
        <div className="card">
          No modules are enabled yet.{' '}
          <button className="btn btn-outline" style={{ marginLeft: '0.4rem' }} onClick={onOpenAdmin}>Go to Admin to enable one</button>
        </div>
      ) : (
        <div style={styles.grid}>
          {stats.map(({ module, recordCount, metricValue, metricLabel, metricIsMoney }) => (
            <button key={module.id} className="card" style={styles.tile} onClick={() => onSelectModule(module.id)}>
              <span className="stamp-badge" style={{ width: '2.4rem', height: '2.4rem', fontSize: '0.85rem', color: 'var(--stamp)', flexShrink: 0 }}>
                {initials(module.display_name)}
              </span>
              <div style={{ textAlign: 'left', minWidth: 0 }}>
                <div style={{ fontWeight: 600, fontSize: '0.92rem' }}>{module.display_name}</div>
                {metricValue !== null && metricLabel ? (
                  <div style={{ fontSize: '0.9rem', fontWeight: 600, color: 'var(--stamp)', marginTop: '0.15rem' }}>
                    {formatMetricValue(metricValue, metricIsMoney, currency)}
                    <span style={{ fontWeight: 400, fontSize: '0.76rem', color: 'var(--ink-soft)', marginLeft: '0.35rem' }}>{metricLabel}</span>
                  </div>
                ) : (
                  <div style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', marginTop: '0.15rem' }}>
                    {recordCount === null ? 'Open' : recordCount === 0 ? 'No records yet' : `${recordCount} record${recordCount === 1 ? '' : 's'}`}
                  </div>
                )}
              </div>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

// Gross profit KPI card. Shows the real cost-data coverage fraction
// whenever it's incomplete (not just a boolean "some missing" flag),
// so e.g. an 87.5% margin on 4 of 50 real-cost sales reads honestly.
// Condensed to a single subtitle line (was: margin line + a separate
// shrinkage-after-write-offs line + a separate cost-coverage aside).
// Nothing computed here changed and nothing is hidden permanently —
// the full breakdown (shrinkage-adjusted profit/margin, and the "only
// N of M sales have real cost data" coverage note) now lives in this
// card's title attribute, a native hover tooltip, so it's still one
// hover away instead of costing three lines of card height by default.
function GrossProfitKpi({ data, currency, onOpen }: { data: GrossProfitSummary; currency: string; onOpen: () => void }) {
  const isProfit = data.profit_cents >= 0;
  const missingCostCount = data.sales_count - data.cost_bearing_sales_count;

  // Ordered so the caveat about the number itself (which sales it's
  // even based on) comes before the caveat about a further adjustment
  // on top of it — these are two independent notes, not a chain, and
  // reading them in "what the number covers" → "what's been subtracted
  // from it since" order makes that clearer than the old shrinkage-
  // first order did.
  const tooltipParts: string[] = [];
  if (missingCostCount > 0) {
    tooltipParts.push(`Margin uses only ${data.cost_bearing_sales_count} of ${data.sales_count} sales with real cost data`);
  }
  if (data.shrinkage_cents > 0) {
    const afterLabel = `${data.profit_cents_after_shrinkage >= 0 ? '+' : '−'}${formatMoney(Math.abs(data.profit_cents_after_shrinkage), currency)}`;
    tooltipParts.push(
      `After ${formatMoney(data.shrinkage_cents, currency)} write-offs: ${afterLabel}` +
      (data.margin_pct_after_shrinkage !== null ? ` (${data.margin_pct_after_shrinkage.toFixed(1)}% margin)` : '')
    );
  }

  return (
    <button
      className="card"
      onClick={onOpen}
      title={tooltipParts.join('\n') || undefined}
      style={{
        display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: '1rem',
        width: '100%', textAlign: 'left', cursor: 'pointer', padding: '0.55rem 0.9rem',
        ...(!isProfit ? { borderColor: 'var(--stamp)', background: 'var(--stamp-wash)' } : {}),
      }}
    >
      <div>
        <div style={{ fontSize: '0.72rem', color: 'var(--ink-soft)', textTransform: 'uppercase', letterSpacing: '0.04em' }}>
          Gross profit
        </div>
        <div style={{ fontSize: '1.35rem', fontWeight: 700, marginTop: '0.1rem', ...(!isProfit ? { color: 'var(--stamp)' } : {}) }}>
          {isProfit ? '+' : '−'}{formatMoney(Math.abs(data.profit_cents), currency)}
        </div>
        <div style={{ fontSize: '0.76rem', color: 'var(--ink-soft)' }}>
          {data.margin_pct === null
            ? `Across ${data.sales_count} sale${data.sales_count === 1 ? '' : 's'} — no revenue yet`
            : `${data.margin_pct.toFixed(1)}% margin, ${data.sales_count} sale${data.sales_count === 1 ? '' : 's'}`}
          {tooltipParts.length > 0 ? ' · hover for detail' : ''}
        </div>
      </div>
    </button>
  );
}

// Net debt position (owed to business minus owed by business) plus an
// overdue alarm, sourced from debt_settlement::summary.
function DebtStandingKpi({ data, currency, onOpen }: { data: DebtSummary; currency: string; onOpen: () => void }) {
  const netPosition = data.owed_to_business_unpaid - data.owed_by_business_unpaid;
  const hasOverdue = data.overdue_count > 0;
  const openCount = data.owed_to_business_unpaid_count + data.owed_by_business_unpaid_count;

  return (
    <button
      className="card"
      onClick={onOpen}
      style={{
        display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: '1rem',
        width: '100%', textAlign: 'left', cursor: 'pointer', padding: '0.55rem 0.9rem',
        ...(hasOverdue ? { borderColor: 'var(--stamp)', background: 'var(--stamp-wash)' } : {}),
      }}
    >
      <div>
        <div style={{ fontSize: '0.72rem', color: 'var(--ink-soft)', textTransform: 'uppercase', letterSpacing: '0.04em' }}>
          Debt standing
        </div>
        <div style={{ fontSize: '1.35rem', fontWeight: 700, marginTop: '0.1rem' }}>
          {netPosition >= 0 ? '+' : '−'}{formatMoney(Math.abs(netPosition), currency)}
        </div>
        <div style={{ fontSize: '0.76rem', color: 'var(--ink-soft)' }}>
          {netPosition >= 0
            ? `Net owed to you, ${openCount} open record${openCount === 1 ? '' : 's'}`
            : `Net you owe, ${openCount} open record${openCount === 1 ? '' : 's'}`}
        </div>
      </div>
      {hasOverdue ? (
        <div style={{ textAlign: 'right', flexShrink: 0 }}>
          <div style={{ fontSize: '0.85rem', fontWeight: 700, color: 'var(--stamp)', display: 'flex', alignItems: 'center', gap: '0.3rem', justifyContent: 'flex-end' }}>
            <span aria-hidden>⚠</span> {data.overdue_count} overdue
          </div>
          <div style={{ fontSize: '0.9rem', fontWeight: 600, color: 'var(--stamp)' }}>{formatMoney(data.overdue_amount, currency)}</div>
        </div>
      ) : (
        <div style={{ fontSize: '0.82rem', color: 'var(--ok)', fontWeight: 600, flexShrink: 0 }}>Nothing overdue</div>
      )}
    </button>
  );
}

function ChecklistItem({ done, label, detail, onClick }: { done: boolean; label: string; detail: string; onClick: () => void }) {
  return (
    <div style={styles.checklistItem}>
      <span style={{ ...styles.checkCircle, ...(done ? styles.checkCircleDone : {}) }}>{done ? '✓' : ''}</span>
      <div style={{ flex: 1 }}>
        <button
          onClick={onClick}
          style={{ background: 'none', border: 'none', padding: 0, textAlign: 'left', cursor: 'pointer', fontSize: '0.9rem', fontWeight: 600, color: done ? 'var(--ink-soft)' : 'var(--ink)', textDecoration: done ? 'line-through' : 'none' }}
        >
          {label}
        </button>
        <div style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', marginTop: '0.1rem' }}>{detail}</div>
      </div>
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  header: { marginBottom: '0.9rem' },
  eyebrow: { fontSize: '0.72rem', letterSpacing: '0.08em', textTransform: 'uppercase', color: 'var(--ink-soft)' },
  // Lets the gross-profit and debt-standing KPI cards sit side by
  // side on wider windows instead of always stacking full-width, the
  // same auto-fit-grid approach AnalyticsSection's own KPI row already
  // uses — minmax(300px, 1fr) is wide enough that neither card's
  // content (value + sub-line + right-side overdue/coverage note)
  // wraps awkwardly, and it still collapses to one column below that.
  kpiRow: { display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(300px, 1fr))', gap: '0.6rem', marginBottom: '0.6rem' },
  checklistCard: { marginBottom: '0.9rem', padding: '0.9rem 1.1rem' },
  dismissBtn: { padding: '0.25em 0.6em', fontSize: '0.76rem' },
  checklistItem: { display: 'flex', gap: '0.7rem', alignItems: 'flex-start', padding: '0.4rem 0' },
  checkCircle: {
    width: '1.3rem', height: '1.3rem', borderRadius: '999px', border: '1.5px solid var(--ink-faint)',
    display: 'flex', alignItems: 'center', justifyContent: 'center', fontSize: '0.75rem', flexShrink: 0, marginTop: '0.05rem',
    color: 'var(--paper)',
  },
  checkCircleDone: { background: 'var(--ok)', borderColor: 'var(--ok)' },
  grid: { display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(220px, 1fr))', gap: '0.8rem' },
  tile: { display: 'flex', alignItems: 'center', gap: '0.8rem', textAlign: 'left', cursor: 'pointer' },
};
