import { useEffect, useState } from 'react';
import { listModules, getModuleSchema, getBusinessInfo, getDebtSummary, getGrossProfitSummary } from '../api';
import type { DebtSummary, GrossProfitSummary } from '../api';
import type { ModuleListItem, ModuleSchema } from '../types';
import { formatMoney } from '../lib/money';
import { ReportPanel } from './ModuleView';

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
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;

    getBusinessInfo().then((b: any) => { if (!cancelled && b?.currency) setCurrency(b.currency); }).catch(() => {});
    getDebtSummary().then((d) => { if (!cancelled) setDebtSummary(d); }).catch(() => {});
    getGrossProfitSummary().then((p) => { if (!cancelled) setGrossProfit(p); }).catch(() => {});

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
            {!grossProfit.has_cost_data && grossProfit.sales_count > 0 && (
              <div style={{ fontSize: '0.76rem', color: 'var(--ink-soft)', marginTop: '0.35rem' }}>
                No cost data recorded yet on these sales — margin will fill in as new sales happen.
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
