import { useEffect, useState } from 'react';
import { getIntelligenceReport } from '../api';
import type { IntelligenceReport, IntelligenceArea, IntelligenceInsight, IntelligenceKpi } from '../api';
import { formatMoney } from '../lib/money';

const PERIODS = [7, 30, 90];

const SEVERITY_COLOR: Record<string, string> = {
  problem: 'var(--stamp)',
  watch: 'var(--warn)',
  good: 'var(--ok)',
  info: 'var(--ink-faint)',
};

function severityRank(s: string): number {
  return s === 'problem' ? 0 : s === 'watch' ? 1 : s === 'info' ? 2 : 3;
}

function fmtValue(unit: string, v: number | null | undefined, currency: string): string {
  if (v === null || v === undefined) return '—';
  if (unit === 'money') return formatMoney(Math.round(v), currency);
  if (unit === 'percent') return `${v.toFixed(1)}%`;
  return Math.round(v).toLocaleString();
}

function fmtChange(k: { unit: string; value?: number | null; current?: number; previous?: number | null; change_pct?: number | null }): string | null {
  const cur = k.value ?? k.current;
  if (cur === null || cur === undefined || k.previous === null || k.previous === undefined) return null;
  if (k.unit === 'percent') {
    const d = cur - k.previous;
    return `${d >= 0 ? '+' : ''}${d.toFixed(1)} pts`;
  }
  if (k.change_pct === null || k.change_pct === undefined) return null;
  return `${k.change_pct >= 0 ? '+' : ''}${k.change_pct.toFixed(1)}%`;
}

function KpiTile({ k, currency }: { k: IntelligenceKpi; currency: string }) {
  const change = fmtChange(k);
  return (
    <div style={{ minWidth: 0 }}>
      <div style={{ fontSize: '0.74rem', color: 'var(--ink-soft)' }}>{k.label}</div>
      <div style={{ fontWeight: 600 }}>{fmtValue(k.unit, k.value, currency)}</div>
      {k.previous !== null && k.previous !== undefined && (
        <div style={{ fontSize: '0.72rem', color: 'var(--ink-soft)' }}>
          was {fmtValue(k.unit, k.previous, currency)}{change ? ` (${change})` : ''}
        </div>
      )}
    </div>
  );
}

function InsightBlock({ i, currency }: { i: IntelligenceInsight; currency: string }) {
  const cmp = i.comparison;
  const cmpChange = cmp ? fmtChange({ unit: cmp.unit, current: cmp.current, previous: cmp.previous, change_pct: cmp.change_pct }) : null;
  return (
    <div style={{ padding: '0.7rem 0', borderTop: '1px solid var(--paper-line)' }}>
      <div style={{ display: 'flex', gap: '0.5rem', alignItems: 'flex-start' }}>
        <span
          aria-hidden
          style={{ width: 9, height: 9, borderRadius: '50%', background: SEVERITY_COLOR[i.severity] ?? 'var(--ink-faint)', marginTop: '0.4rem', flexShrink: 0 }}
        />
        <div style={{ minWidth: 0 }}>
          <div style={{ fontWeight: 600, fontSize: '0.9rem' }}>{i.title}</div>
          <div style={{ fontSize: '0.85rem', marginTop: '0.15rem' }}>{i.what_happened}</div>
          <div style={{ fontSize: '0.85rem', marginTop: '0.3rem' }}>
            <span style={{ color: 'var(--ink-soft)' }}>Suggested action: </span>{i.recommended_action}
          </div>
          <details style={{ marginTop: '0.4rem', fontSize: '0.82rem' }}>
            <summary style={{ cursor: 'pointer', color: 'var(--ink-soft)' }}>Evidence and details</summary>
            <div style={{ marginTop: '0.4rem', display: 'flex', flexDirection: 'column', gap: '0.35rem' }}>
              <div>
                {i.evidence.map((e, n) => (
                  <div key={n} style={{ display: 'flex', justifyContent: 'space-between', gap: '0.8rem', padding: '0.15rem 0' }}>
                    <span style={{ color: 'var(--ink-soft)' }}>{e.label}</span>
                    <span style={{ textAlign: 'right' }}>{e.value}</span>
                  </div>
                ))}
              </div>
              <div><span style={{ color: 'var(--ink-soft)' }}>Period: </span>{i.period.label} ({i.period.start} to {i.period.end})</div>
              {cmp && (
                <div>
                  <span style={{ color: 'var(--ink-soft)' }}>Compared with {cmp.period.label.toLowerCase()} ({cmp.period.start} to {cmp.period.end}): </span>
                  {cmp.label} {fmtValue(cmp.unit, cmp.current, currency)} vs {fmtValue(cmp.unit, cmp.previous, currency)}{cmpChange ? ` (${cmpChange})` : ''}
                </div>
              )}
              <div><span style={{ color: 'var(--ink-soft)' }}>Why it matters: </span>{i.why_it_matters}</div>
              {i.impact && (
                <div>
                  <span style={{ color: 'var(--ink-soft)' }}>Potential impact: </span>
                  {formatMoney(i.impact.amount_cents, currency)} — {i.impact.basis}
                </div>
              )}
              <div>
                <span style={{ color: 'var(--ink-soft)' }}>Confidence: </span>{i.confidence.level}
                {i.confidence.limitations.length > 0 && (
                  <ul style={{ margin: '0.2rem 0 0', paddingLeft: '1.1rem', color: 'var(--ink-soft)' }}>
                    {i.confidence.limitations.map((l, n) => <li key={n}>{l}</li>)}
                  </ul>
                )}
              </div>
              {i.follow_up && <div><span style={{ color: 'var(--ink-soft)' }}>Follow up: </span>{i.follow_up}</div>}
            </div>
          </details>
        </div>
      </div>
    </div>
  );
}

function AreaBlock({ area, currency }: { area: IntelligenceArea; currency: string }) {
  const [open, setOpen] = useState(false);
  const worst = area.insights.length ? [...area.insights].sort((a, b) => severityRank(a.severity) - severityRank(b.severity))[0].severity : null;
  const summary = area.status === 'unavailable'
    ? 'Not available'
    : area.insights.length === 0
      ? 'No findings'
      : `${area.insights.length} finding${area.insights.length === 1 ? '' : 's'}`;
  return (
    <div style={{ borderTop: '1px solid var(--paper-line)' }}>
      <button
        onClick={() => setOpen(!open)}
        style={{ width: '100%', textAlign: 'left', padding: '0.7rem 0', cursor: 'pointer', display: 'flex', justifyContent: 'space-between', alignItems: 'center', gap: '0.6rem' }}
      >
        <span style={{ display: 'flex', alignItems: 'center', gap: '0.5rem', fontWeight: 600, fontSize: '0.9rem' }}>
          <span aria-hidden style={{ width: 9, height: 9, borderRadius: '50%', background: worst ? SEVERITY_COLOR[worst] : 'transparent', border: worst ? 'none' : '1px solid var(--paper-line)' }} />
          {area.title}
        </span>
        <span style={{ color: 'var(--ink-soft)', fontWeight: 400, fontSize: '0.8rem' }}>
          {summary}{area.status === 'partial' ? ' · partial' : ''} {open ? '−' : '+'}
        </span>
      </button>
      {open && (
        <div style={{ paddingBottom: '0.8rem' }}>
          {area.notes.filter(Boolean).map((n, idx) => (
            <div key={idx} style={{ fontSize: '0.82rem', color: 'var(--ink-soft)', marginBottom: '0.3rem' }}>{n}</div>
          ))}
          {area.kpis.length > 0 && (
            <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(140px, 1fr))', gap: '0.7rem', margin: '0.3rem 0 0.4rem' }}>
              {area.kpis.map((k) => <KpiTile key={k.label} k={k} currency={currency} />)}
            </div>
          )}
          {area.insights.map((i) => <InsightBlock key={i.kind} i={i} currency={currency} />)}
          {area.insights.length === 0 && area.status !== 'unavailable' && (
            <div style={{ fontSize: '0.82rem', color: 'var(--ink-soft)', padding: '0.4rem 0' }}>Nothing unusual found for this period.</div>
          )}
          {area.missing.length > 0 && (
            <div style={{ marginTop: '0.4rem', fontSize: '0.8rem', color: 'var(--ink-soft)' }}>
              <div style={{ fontWeight: 600 }}>{area.status === 'unavailable' ? 'Why it isn\'t available' : 'What\'s missing'}</div>
              <ul style={{ margin: '0.2rem 0 0', paddingLeft: '1.1rem' }}>
                {area.missing.map((m, n) => <li key={n}>{m}</li>)}
              </ul>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

export default function IntelligenceSection({ currency }: { currency: string }) {
  const [days, setDays] = useState(30);
  const [report, setReport] = useState<IntelligenceReport | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    // Never leave the previous period's figures on screen while the new
    // period loads.
    setReport(null);
    setError(null);
    getIntelligenceReport(days)
      .then((r) => { if (!cancelled) setReport(r); })
      .catch(() => { if (!cancelled) setError('Business intelligence could not be loaded right now. Please try again in a moment.'); });
    return () => { cancelled = true; };
  }, [days]);

  return (
    <div>
      <div style={{ display: 'flex', gap: '0.4rem', flexWrap: 'wrap', alignItems: 'center', marginBottom: '0.5rem' }}>
        {PERIODS.map((p) => (
          <button key={p} className={p === days ? 'btn' : 'btn btn-outline'} onClick={() => setDays(p)}>
            {p} days
          </button>
        ))}
      </div>
      {error && <div style={{ color: 'var(--stamp)', fontSize: '0.85rem' }}>{error}</div>}
      {!error && !report && <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>Loading…</div>}
      {report && (
        <div>
          <div style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', marginBottom: '0.3rem' }}>
            {report.period.start} to {report.period.end}, compared with {report.previous_period.start} to {report.previous_period.end}
          </div>
          {report.areas.map((a) => <AreaBlock key={a.id} area={a} currency={currency} />)}
        </div>
      )}
    </div>
  );
}
