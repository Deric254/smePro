import { useEffect, useState, lazy, Suspense } from 'react';
import { listModules, listRecords, getModuleSchema, runReport, listUsers, getBusinessInfo, getSettings, setSetting, getDebtSummary } from '../api';
import type { ModuleListItem } from '../types';
import type { DebtSummary } from '../api';
import { formatMoney } from '../lib/money';

// Lazy-loaded specifically because it's the only thing in the app that
// pulls in recharts, which roughly doubles the JS bundle on its own —
// no reason every screen (login, POS, admin) should pay that cost just
// because the Dashboard exists somewhere in the app. This chunk only
// downloads the moment someone actually views the dashboard with sales
// data, not before.
const AnalyticsSection = lazy(() => import('../components/AnalyticsSection'));

function initials(name: string) {
  const words = name.split(/[\s/]+/).filter(Boolean);
  if (words.length === 1) return words[0].slice(0, 2).toUpperCase();
  return (words[0][0] + words[1][0]).toUpperCase();
}

// Whole numbers stay whole (unit counts, headcounts). A metric whose
// measure field is "money" is ALWAYS integer cents — see
// src/lib/money.ts — and must go through formatMoney regardless of
// whether the raw number happens to look whole (e.g. exactly $4,500.00
// is stored as 450000, which is itself an integer, so "is this value
// a whole number" can never be used to detect money — only the
// field's own declared type can).
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

export default function Dashboard({ businessName, onSelectModule, onOpenAdmin }: {
  businessName: string;
  onSelectModule: (id: string) => void;
  onOpenAdmin: () => void;
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
            if (metric) {
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
    // Also best-effort: fails silently (module not enabled, or this
    // role lacks "read" on it) and the KPI card below just doesn't
    // render — same as every module tile already does when its own
    // metric fetch fails.
    getDebtSummary().then((d) => { if (!cancelled) setDebtSummary(d); }).catch(() => {});

    return () => { cancelled = true; };
  }, []);

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

      {debtSummary && <DebtStandingKpi data={debtSummary} currency={currency} onOpen={() => onSelectModule('debt_credit')} />}

      {stats.some((s) => s.module.id === 'sales') && (
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

// Answers the one question "debt standing" actually means, at a
// glance, with no ambiguity: net position (owed to you minus what you
// owe — positive means you're net owed money, negative means you owe
// more than you're owed) and an unmissable overdue alarm if anything
// has actually gone past its due date. Both numbers come straight from
// debt_settlement::summary — real SQL SUM/COUNT over the whole table,
// not a client-side estimate — so this is the same truthful total
// shown on the Debt & Credit module's own summary tiles, just
// distilled to the single figure that matters most from the
// dashboard.
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
        width: '100%', textAlign: 'left', cursor: 'pointer', marginBottom: '0.9rem', padding: '0.8rem 1.1rem',
        ...(hasOverdue ? { borderColor: 'var(--stamp)', background: 'var(--stamp-wash)' } : {}),
      }}
    >
      <div>
        <div style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', textTransform: 'uppercase', letterSpacing: '0.04em' }}>
          Debt standing
        </div>
        <div style={{ fontSize: '1.7rem', fontWeight: 700, marginTop: '0.15rem' }}>
          {netPosition >= 0 ? '+' : '−'}{formatMoney(Math.abs(netPosition), currency)}
        </div>
        <div style={{ fontSize: '0.82rem', color: 'var(--ink-soft)', marginTop: '0.1rem' }}>
          {netPosition >= 0
            ? `Net owed to you, across ${openCount} open record${openCount === 1 ? '' : 's'}`
            : `Net you owe, across ${openCount} open record${openCount === 1 ? '' : 's'}`}
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
