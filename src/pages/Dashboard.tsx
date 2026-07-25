import { useEffect, useState } from 'react';
import { listModules, listRecords, getModuleSchema, runReport, listUsers, getVendorLicenseStatus, getSettings, setSetting } from '../api';
import type { ModuleListItem } from '../types';

function initials(name: string) {
  const words = name.split(/[\s/]+/).filter(Boolean);
  if (words.length === 1) return words[0].slice(0, 2).toUpperCase();
  return (words[0][0] + words[1][0]).toUpperCase();
}

function formatMetricValue(value: number): string {
  // Whole numbers stay whole (unit counts, headcounts); anything with
  // meaningful fractional cents (money) keeps 2 decimals. Avoids both
  // "3.00 team members" and "revenue: 2400" losing its cents silently.
  return Number.isInteger(value) ? value.toLocaleString() : value.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 });
}

interface ModuleStat {
  module: ModuleListItem;
  recordCount: number | null;
  metricValue: number | null;
  metricLabel: string | null;
}

export default function Dashboard({ businessName, onSelectModule, onOpenAdmin }: {
  businessName: string;
  onSelectModule: (id: string) => void;
  onOpenAdmin: () => void;
}) {
  const [stats, setStats] = useState<ModuleStat[]>([]);
  const [loading, setLoading] = useState(true);
  const [userCount, setUserCount] = useState<number | null>(null);
  const [licensed, setLicensed] = useState<boolean | null>(null);
  const [checklistDismissed, setChecklistDismissed] = useState<boolean | null>(null);

  useEffect(() => {
    let cancelled = false;

    listModules().then(async (res) => {
      const enabled: ModuleListItem[] = res.modules.filter((m: ModuleListItem) => m.enabled);
      const withStats = await Promise.all(
        enabled.map(async (module): Promise<ModuleStat> => {
          let recordCount: number | null = null;
          let metricValue: number | null = null;
          let metricLabel: string | null = null;
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
            }
          } catch { /* module has no metric, or this role can't read it — falls back to record count below */ }
          return { module, recordCount, metricValue, metricLabel };
        })
      );
      if (!cancelled) { setStats(withStats); setLoading(false); }
    }).catch(() => { if (!cancelled) setLoading(false); });

    // Best-effort — Staff/some roles won't have permission for these,
    // and that's fine, the dashboard just quietly shows less.
    listUsers().then((r) => { if (!cancelled) setUserCount(r.users.filter((u: { active: boolean }) => u.active).length); }).catch(() => {});
    getVendorLicenseStatus().then((r) => { if (!cancelled) setLicensed(r.licensed); }).catch(() => {});
    getSettings().then((s) => { if (!cancelled) setChecklistDismissed(s.onboarding_dismissed === 'true'); }).catch(() => { if (!cancelled) setChecklistDismissed(false); });

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

      {showChecklist && (
        <div className="card" style={styles.checklistCard}>
          <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start' }}>
            <h3 style={{ margin: '0 0 0.9rem' }}>Getting started</h3>
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
          <ChecklistItem
            done={licensed === true}
            label={licensed ? 'Licensed' : 'Activate your license when ready'}
            detail="Your business runs on a free trial until then — nothing is blocked in the meantime."
            onClick={onOpenAdmin}
          />
        </div>
      )}

      <h3 style={{ margin: '1.6rem 0 0.8rem' }}>Your modules</h3>
      {loading ? (
        <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>Loading…</div>
      ) : stats.length === 0 ? (
        <div className="card">
          No modules are enabled yet.{' '}
          <button className="btn btn-outline" style={{ marginLeft: '0.4rem' }} onClick={onOpenAdmin}>Go to Admin to enable one</button>
        </div>
      ) : (
        <div style={styles.grid}>
          {stats.map(({ module, recordCount, metricValue, metricLabel }) => (
            <button key={module.id} className="card" style={styles.tile} onClick={() => onSelectModule(module.id)}>
              <span className="stamp-badge" style={{ width: '2.4rem', height: '2.4rem', fontSize: '0.85rem', color: 'var(--stamp)', flexShrink: 0 }}>
                {initials(module.display_name)}
              </span>
              <div style={{ textAlign: 'left', minWidth: 0 }}>
                <div style={{ fontWeight: 600, fontSize: '0.92rem' }}>{module.display_name}</div>
                {metricValue !== null && metricLabel ? (
                  <div style={{ fontSize: '0.9rem', fontWeight: 600, color: 'var(--stamp)', marginTop: '0.15rem' }}>
                    {formatMetricValue(metricValue)}
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
  header: { marginBottom: '1.2rem' },
  eyebrow: { fontSize: '0.72rem', letterSpacing: '0.08em', textTransform: 'uppercase', color: 'var(--ink-soft)' },
  checklistCard: { marginBottom: '1.2rem' },
  dismissBtn: { padding: '0.25em 0.6em', fontSize: '0.76rem' },
  checklistItem: { display: 'flex', gap: '0.7rem', alignItems: 'flex-start', padding: '0.55rem 0' },
  checkCircle: {
    width: '1.3rem', height: '1.3rem', borderRadius: '999px', border: '1.5px solid var(--ink-faint)',
    display: 'flex', alignItems: 'center', justifyContent: 'center', fontSize: '0.75rem', flexShrink: 0, marginTop: '0.05rem',
    color: 'var(--paper)',
  },
  checkCircleDone: { background: 'var(--ok)', borderColor: 'var(--ok)' },
  grid: { display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(220px, 1fr))', gap: '0.8rem' },
  tile: { display: 'flex', alignItems: 'center', gap: '0.8rem', textAlign: 'left', cursor: 'pointer' },
};
