import { useEffect, useState } from 'react';
import {
  listModules, getModuleSchema, getBusinessInfo, getDebtSummary, getGrossProfitSummary,
  getBasketAffinity, getProfitByItem, getDebtAging, getSlowMovers, getStockRunway,
  getRefundRateByItem, getDayOfWeekPattern, getHourOfDayPattern, getWeeklyTrend, getMonthlyTrend, getSeasonalPattern, runReport,
} from '../api';
import type {
  DebtSummary, GrossProfitSummary, BasketPair, ItemProfit, DebtAgingSummary,
  SlowMover, StockRunway, RefundRate, DayOfWeekPattern, HourOfDayPattern, PeriodTrendPoint, SeasonalMonthPattern,
} from '../api';
import type { ModuleListItem, ModuleSchema } from '../types';
import { formatMoney } from '../lib/money';
import { ReportPanel } from './ModuleView';
import BasketAffinityCard from '../components/BasketAffinityCard';
import ItemMarginCard from '../components/ItemMarginCard';
import DebtAgingCard from '../components/DebtAgingCard';
import SlowMoversCard from '../components/SlowMoversCard';
import StockRunwayCard from '../components/StockRunwayCard';
import RefundRateCard from '../components/RefundRateCard';
import TopCustomersCard from '../components/TopCustomersCard';
import DayOfWeekCard from '../components/DayOfWeekCard';
import HourOfDayCard from '../components/HourOfDayCard';
import PeriodTrendCard from '../components/PeriodTrendCard';
import SeasonalCard from '../components/SeasonalCard';

// Same collapsible +/− pattern the per-module report list at the
// bottom of this page already used — extended here to every section
// on the page, not just that one. Collapsed by default, on purpose:
// this page now holds a lot of reports, and showing all of them
// expanded at once is exactly the cluttered view this was built to
// avoid. `onExpand` fires once, the first time a section opens — its
// data is fetched then, not on page load, so visiting Reports and
// glancing at the section titles costs nothing beyond the currency
// lookup and the module list itself.
function CollapsibleSection({ title, onExpand, loading, children }: {
  title: string;
  onExpand?: () => void;
  loading?: boolean;
  children: React.ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const [triggered, setTriggered] = useState(false);

  function toggle() {
    const next = !open;
    setOpen(next);
    if (next && !triggered) {
      setTriggered(true);
      onExpand?.();
    }
  }

  return (
    <div className="card" style={{ padding: 0, overflow: 'hidden', marginBottom: '0.6rem' }}>
      <button
        onClick={toggle}
        style={{
          width: '100%', textAlign: 'left', padding: '0.9rem 1.1rem', cursor: 'pointer',
          display: 'flex', justifyContent: 'space-between', alignItems: 'center', fontWeight: 600,
        }}
      >
        {title}
        <span style={{ color: 'var(--ink-soft)', fontWeight: 400 }}>{open ? '−' : '+'}</span>
      </button>
      {open && (
        <div style={{ padding: '0 1.1rem 1.1rem' }}>
          {!triggered ? null : loading ? <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>Loading…</div> : children}
        </div>
      )}
    </div>
  );
}

export default function Reports() {
  const [modules, setModules] = useState<ModuleListItem[]>([]);
  const [schemas, setSchemas] = useState<Record<string, ModuleSchema>>({});
  const [expandedModule, setExpandedModule] = useState<string | null>(null);
  const [currency, setCurrency] = useState('USD');
  const [loading, setLoading] = useState(true);

  // Each section: its own data slot, its own "is this section's fetch
  // in flight" flag. null data = not fetched yet, or the fetch found
  // nothing to show (both render the same "nothing here" state inside
  // each card component — see each card's own empty-state handling).
  const [debtSummary, setDebtSummary] = useState<DebtSummary | null>(null);
  const [grossProfit, setGrossProfit] = useState<GrossProfitSummary | null>(null);
  const [itemMargins, setItemMargins] = useState<ItemProfit[] | null>(null);
  const [profitabilityLoading, setProfitabilityLoading] = useState(false);

  const [basketPairs, setBasketPairs] = useState<BasketPair[] | null>(null);
  const [topCustomers, setTopCustomers] = useState<{ label: string; value: number }[] | null>(null);
  const [customersLoading, setCustomersLoading] = useState(false);

  const [slowMovers, setSlowMovers] = useState<SlowMover[] | null>(null);
  const [stockRunway, setStockRunway] = useState<StockRunway[] | null>(null);
  const [stockLoading, setStockLoading] = useState(false);

  const [debtAging, setDebtAging] = useState<DebtAgingSummary | null>(null);
  const [debtorsLoading, setDebtorsLoading] = useState(false);

  const [dayOfWeek, setDayOfWeek] = useState<DayOfWeekPattern[] | null>(null);
  const [hourOfDay, setHourOfDay] = useState<HourOfDayPattern[] | null>(null);
  const [weeklyTrend, setWeeklyTrend] = useState<PeriodTrendPoint[] | null>(null);
  const [monthlyTrend, setMonthlyTrend] = useState<PeriodTrendPoint[] | null>(null);
  const [seasonal, setSeasonal] = useState<SeasonalMonthPattern[] | null>(null);
  const [patternsLoading, setPatternsLoading] = useState(false);

  const [refundRates, setRefundRates] = useState<RefundRate[] | null>(null);
  const [refundsLoading, setRefundsLoading] = useState(false);

  useEffect(() => {
    let cancelled = false;

    getBusinessInfo().then((b: any) => { if (!cancelled && b?.currency) setCurrency(b.currency); }).catch(() => {});

    listModules().then(async (res) => {
      const enabled: ModuleListItem[] = res.modules.filter((m: ModuleListItem) => m.enabled);
      if (!cancelled) setModules(enabled);
      const loaded: Record<string, ModuleSchema> = {};
      await Promise.all(enabled.map(async (m) => {
        try {
          loaded[m.id] = await getModuleSchema(m.id);
        } catch { /* this role can't read this module's schema — leave it out, same as ModuleView would refuse to open it directly */ }
      }));
      if (!cancelled) { setSchemas(loaded); setLoading(false); }
    }).catch(() => { if (!cancelled) setLoading(false); });

    return () => { cancelled = true; };
  }, []);

  function loadProfitability() {
    setProfitabilityLoading(true);
    Promise.allSettled([
      getGrossProfitSummary().then(setGrossProfit),
      getProfitByItem(10).then((r) => setItemMargins(r.items)),
    ]).finally(() => setProfitabilityLoading(false));
  }

  function loadCustomers() {
    setCustomersLoading(true);
    Promise.allSettled([
      getBasketAffinity({ limit: 10 }).then((r) => setBasketPairs(r.pairs)),
      // No dedicated endpoint — same generic report engine
      // AnalyticsSection.tsx already uses for top-selling items,
      // grouped by customer instead of item_name. Already sorted DESC
      // server-side; sliced to top 10 here purely for display.
      runReport('sales', { agg: 'sum', measure: 'revenue', dimension: 'category', field: 'customer' })
        .then((r) => setTopCustomers((r.report ?? []).slice(0, 10))),
    ]).finally(() => setCustomersLoading(false));
  }

  function loadStockHealth() {
    setStockLoading(true);
    Promise.allSettled([
      getSlowMovers(30, 10).then((r) => setSlowMovers(r.items)),
      getStockRunway(30, 15).then((r) => setStockRunway(r.items)),
    ]).finally(() => setStockLoading(false));
  }

  function loadDebtors() {
    setDebtorsLoading(true);
    Promise.allSettled([
      getDebtSummary().then(setDebtSummary),
      getDebtAging().then(setDebtAging),
    ]).finally(() => setDebtorsLoading(false));
  }

  function loadPatterns() {
    setPatternsLoading(true);
    Promise.allSettled([
      getDayOfWeekPattern(90).then((r) => setDayOfWeek(r.items)),
      getHourOfDayPattern(30).then((r) => setHourOfDay(r.items)),
      getWeeklyTrend(12).then((r) => setWeeklyTrend(r.items)),
      getMonthlyTrend(12).then((r) => setMonthlyTrend(r.items)),
      getSeasonalPattern().then((r) => setSeasonal(r.items)),
    ]).finally(() => setPatternsLoading(false));
  }

  function loadRefunds() {
    setRefundsLoading(true);
    getRefundRateByItem(10).then((r) => setRefundRates(r.items)).finally(() => setRefundsLoading(false));
  }

  const reportable = modules.filter((m) => schemas[m.id]?.my_permissions.includes('export') || schemas[m.id]?.actions.includes('export'));

  return (
    <div>
      <h2 style={{ marginTop: 0 }}>Reports</h2>

      <CollapsibleSection title="Profitability" onExpand={loadProfitability} loading={profitabilityLoading}>
        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(min(280px, 100%), 1fr))', gap: '0.9rem' }}>
          {grossProfit && (
            <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
              <div style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', textTransform: 'uppercase', letterSpacing: '0.04em' }}>
                Gross profit (all-time)
              </div>
              <div style={{ fontSize: '1.5rem', fontWeight: 700, marginTop: '0.15rem' }}>
                {formatMoney(grossProfit.profit_cents, currency)}
              </div>
              <div style={{ fontSize: '0.8rem', color: 'var(--ink-soft)', marginTop: '0.1rem' }}>
                Revenue {formatMoney(grossProfit.revenue_cents, currency)} − Cost {formatMoney(grossProfit.cost_cents, currency)}
                {grossProfit.margin_pct !== null ? ` · ${grossProfit.margin_pct.toFixed(1)}% margin` : ''}
              </div>
              {grossProfit.cost_bearing_sales_count < grossProfit.sales_count && (
                <div style={{ fontSize: '0.76rem', color: 'var(--ink-soft)', marginTop: '0.35rem' }}>
                  Only {grossProfit.cost_bearing_sales_count} of {grossProfit.sales_count} sales have real cost data recorded — this margin is based on those only, not the full sales count.
                </div>
              )}
            </div>
          )}
          {itemMargins && <ItemMarginCard items={itemMargins} currency={currency} />}
        </div>
      </CollapsibleSection>

      <CollapsibleSection title="Customers" onExpand={loadCustomers} loading={customersLoading}>
        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(min(280px, 100%), 1fr))', gap: '0.9rem' }}>
          {topCustomers && <TopCustomersCard customers={topCustomers} currency={currency} />}
          {basketPairs && <BasketAffinityCard pairs={basketPairs} currency={currency} />}
        </div>
      </CollapsibleSection>

      <CollapsibleSection title="Stock health" onExpand={loadStockHealth} loading={stockLoading}>
        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(min(280px, 100%), 1fr))', gap: '0.9rem' }}>
          {stockRunway && <StockRunwayCard items={stockRunway} />}
          {slowMovers && <SlowMoversCard items={slowMovers} currency={currency} />}
        </div>
      </CollapsibleSection>

      <CollapsibleSection title="Debtors" onExpand={loadDebtors} loading={debtorsLoading}>
        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(min(280px, 100%), 1fr))', gap: '0.9rem' }}>
          {debtSummary && (
            <div className="card" style={{ padding: '0.9rem 1.1rem' }}>
              <div style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', textTransform: 'uppercase', letterSpacing: '0.04em' }}>
                Debt standing
              </div>
              <div style={{ fontSize: '1.5rem', fontWeight: 700, marginTop: '0.15rem' }}>
                {formatMoney(debtSummary.owed_to_business_unpaid - debtSummary.owed_by_business_unpaid, currency)}
              </div>
              <div style={{ fontSize: '0.8rem', color: 'var(--ink-soft)', marginTop: '0.1rem' }}>
                Owed to you {formatMoney(debtSummary.owed_to_business_unpaid, currency)} · You owe {formatMoney(debtSummary.owed_by_business_unpaid, currency)}
              </div>
              {debtSummary.overdue_count > 0 && (
                <div style={{ fontSize: '0.8rem', color: 'var(--stamp)', marginTop: '0.1rem' }}>
                  {debtSummary.overdue_count} overdue ({formatMoney(debtSummary.overdue_amount, currency)})
                </div>
              )}
            </div>
          )}
          {debtAging && <DebtAgingCard aging={debtAging} currency={currency} />}
        </div>
      </CollapsibleSection>

      <CollapsibleSection title="Sales patterns" onExpand={loadPatterns} loading={patternsLoading}>
        <div style={{ display: 'flex', flexDirection: 'column', gap: '0.75rem' }}>
          {dayOfWeek && <DayOfWeekCard items={dayOfWeek} currency={currency} />}
          {hourOfDay && <HourOfDayCard items={hourOfDay} currency={currency} />}
          {weeklyTrend && (
            <PeriodTrendCard
              title="Weekly trend (last 12 weeks)"
              items={weeklyTrend}
              currency={currency}
              formatLabel={(label) => `Wk of ${label.slice(5)}`}
            />
          )}
          {monthlyTrend && (
            <PeriodTrendCard
              title="Monthly trend (last 12 months)"
              items={monthlyTrend}
              currency={currency}
              formatLabel={(label) => label}
            />
          )}
          {seasonal && <SeasonalCard items={seasonal} currency={currency} />}
        </div>
      </CollapsibleSection>

      <CollapsibleSection title="Refunds" onExpand={loadRefunds} loading={refundsLoading}>
        {refundRates && <RefundRateCard items={refundRates} currency={currency} />}
      </CollapsibleSection>

      {loading ? (
        <div style={{ color: 'var(--ink-soft)', marginTop: '0.6rem' }}>Loading…</div>
      ) : reportable.length === 0 ? null : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: '0.6rem', marginTop: '0.6rem' }}>
          {reportable.map((m) => {
            const schema = schemas[m.id];
            const isOpen = expandedModule === m.id;
            return (
              <div key={m.id} className="card" style={{ padding: 0, overflow: 'hidden' }}>
                <button
                  onClick={() => setExpandedModule(isOpen ? null : m.id)}
                  style={{
                    width: '100%', textAlign: 'left', padding: '0.9rem 1.1rem', cursor: 'pointer',
                    display: 'flex', justifyContent: 'space-between', alignItems: 'center', fontWeight: 600,
                  }}
                >
                  {m.display_name}
                  <span style={{ color: 'var(--ink-soft)', fontWeight: 400 }}>{isOpen ? '−' : '+'}</span>
                </button>
                {isOpen && (
                  <div style={{ padding: '0 1.1rem 1.1rem' }}>
                    <ReportPanel
                      moduleId={m.id}
                      schema={schema}
                      canExport={!!schema?.my_permissions.includes('export')}
                      businessCurrency={currency}
                    />
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
