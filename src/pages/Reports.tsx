import { useEffect, useState } from 'react';
import { listModules, getModuleSchema, getBusinessInfo, getDebtSummary, getGrossProfitSummary, getBasketAffinity, getProfitByItem, getDebtAging, getSlowMovers, getRefundRateByItem, runReport } from '../api';
import type { DebtSummary, GrossProfitSummary, BasketPair, ItemProfit, DebtAgingSummary, SlowMover, RefundRate } from '../api';
import type { ModuleListItem, ModuleSchema } from '../types';
import { formatMoney } from '../lib/money';
import { ReportPanel } from './ModuleView';
import BasketAffinityCard from '../components/BasketAffinityCard';
import ItemMarginCard from '../components/ItemMarginCard';
import DebtAgingCard from '../components/DebtAgingCard';
import SlowMoversCard from '../components/SlowMoversCard';
import RefundRateCard from '../components/RefundRateCard';
import TopCustomersCard from '../components/TopCustomersCard';

// One screen for every report in the system, for decision-making —
// before this, the only way to see a module's own report was to open
// that module and click its "Report" tab, one module at a time, with
// no single place showing the business's overall numbers together.
// This reuses ModuleView's own ReportPanel unchanged (same measure/
// aggregation/slice/export controls a person already knows from
// there) rather than building a second, different reporting UI —
// consistency over novelty.
export default function Reports() {
  const [modules, setModules] = useState<ModuleListItem[]>([]);
  const [schemas, setSchemas] = useState<Record<string, ModuleSchema>>({});
  const [expanded, setExpanded] = useState<string | null>(null);
  const [currency, setCurrency] = useState('USD');
  const [debtSummary, setDebtSummary] = useState<DebtSummary | null>(null);
  const [grossProfit, setGrossProfit] = useState<GrossProfitSummary | null>(null);
  // null = not fetched yet / sales not enabled / no permission — same
  // best-effort discipline as grossProfit/debtSummary above, the card
  // just doesn't render if this stays null.
  const [basketPairs, setBasketPairs] = useState<BasketPair[] | null>(null);
  // Same best-effort discipline again.
  const [itemMargins, setItemMargins] = useState<ItemProfit[] | null>(null);
  const [debtAging, setDebtAging] = useState<DebtAgingSummary | null>(null);
  const [slowMovers, setSlowMovers] = useState<SlowMover[] | null>(null);
  const [refundRates, setRefundRates] = useState<RefundRate[] | null>(null);
  const [topCustomers, setTopCustomers] = useState<{ label: string; value: number }[] | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;

    getBusinessInfo().then((b: any) => { if (!cancelled && b?.currency) setCurrency(b.currency); }).catch(() => {});
    getDebtSummary().then((d) => { if (!cancelled) setDebtSummary(d); }).catch(() => {});
    getGrossProfitSummary().then((p) => { if (!cancelled) setGrossProfit(p); }).catch(() => {});
    getBasketAffinity({ limit: 10 }).then((r) => { if (!cancelled) setBasketPairs(r.pairs); }).catch(() => {});
    getProfitByItem(10).then((r) => { if (!cancelled) setItemMargins(r.items); }).catch(() => {});
    getDebtAging().then((a) => { if (!cancelled) setDebtAging(a); }).catch(() => {});
    getSlowMovers(30, 10).then((r) => { if (!cancelled) setSlowMovers(r.items); }).catch(() => {});
    getRefundRateByItem(10).then((r) => { if (!cancelled) setRefundRates(r.items); }).catch(() => {});
    // No dedicated endpoint — same generic report engine
    // AnalyticsSection.tsx already uses for top-selling items, grouped
    // by customer instead of item_name. Already sorted DESC
    // server-side; sliced to top 10 here purely for display.
    runReport('sales', { agg: 'sum', measure: 'revenue', dimension: 'category', field: 'customer' })
      .then((r) => { if (!cancelled) setTopCustomers((r.report ?? []).slice(0, 10)); })
      .catch(() => {});

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

  const reportable = modules.filter((m) => schemas[m.id]?.my_permissions.includes('export') || schemas[m.id]?.actions.includes('export'));

  return (
    <div>
      <h2 style={{ marginTop: 0 }}>Reports</h2>
      <p style={{ color: 'var(--ink-soft)', marginTop: '-0.4rem' }}>
        Every report in the system, in one place — pick a section below to slice it by date, item, or customer.
      </p>

      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(220px, 1fr))', gap: '0.9rem', marginBottom: '1.6rem' }}>
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
      </div>

      {(basketPairs || itemMargins || debtAging || slowMovers || refundRates || topCustomers) && (
        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(320px, 1fr))', gap: '0.9rem', marginBottom: '1.6rem' }}>
          {itemMargins && <ItemMarginCard items={itemMargins} currency={currency} />}
          {basketPairs && <BasketAffinityCard pairs={basketPairs} currency={currency} />}
          {topCustomers && <TopCustomersCard customers={topCustomers} currency={currency} />}
          {debtAging && <DebtAgingCard aging={debtAging} currency={currency} />}
          {slowMovers && <SlowMoversCard items={slowMovers} currency={currency} />}
          {refundRates && <RefundRateCard items={refundRates} currency={currency} />}
        </div>
      )}

      {loading ? (
        <div style={{ color: 'var(--ink-soft)' }}>Loading…</div>
      ) : reportable.length === 0 ? (
        <div style={{ color: 'var(--ink-soft)' }}>No modules with reporting enabled for your role yet.</div>
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: '0.6rem' }}>
          {reportable.map((m) => {
            const schema = schemas[m.id];
            const isOpen = expanded === m.id;
            return (
              <div key={m.id} className="card" style={{ padding: 0, overflow: 'hidden' }}>
                <button
                  onClick={() => setExpanded(isOpen ? null : m.id)}
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
