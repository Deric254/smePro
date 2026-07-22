import { useEffect, useState } from 'react';
import { getLicenseStatus, activateLicense, payLicense } from '../api';
import type { LicenseStatus } from '../types';

export default function LicenseBanner({ onChange }: { onChange?: () => void }) {
  const [status, setStatus] = useState<LicenseStatus | null>(null);
  const [busy, setBusy] = useState(false);

  async function refresh() {
    setStatus(await getLicenseStatus());
  }

  useEffect(() => { refresh(); }, []);

  async function handleActivate() {
    setBusy(true);
    try { await activateLicense(); await refresh(); onChange?.(); } finally { setBusy(false); }
  }
  async function handlePay() {
    setBusy(true);
    try { await payLicense(); await refresh(); onChange?.(); } finally { setBusy(false); }
  }

  if (!status) return null;

  if (status.status === 'active') return null; // nothing to say when everything is fine

  const copy: Record<string, { title: string; body: string; tone: string }> = {
    inactive: {
      title: 'Activation needed',
      body: 'One-time activation unlocks your subscription. Your data stays fully yours either way — export is just paused until activation.',
      tone: 'status-inactive',
    },
    grace: {
      title: `Payment due — ${(status as any).days_left} day${(status as any).days_left === 1 ? '' : 's'} left`,
      body: 'Everything keeps working normally. Exporting your data is paused until payment goes through.',
      tone: 'status-grace',
    },
    locked: {
      title: `Payment overdue by ${(status as any).days_overdue} day${(status as any).days_overdue === 1 ? '' : 's'}`,
      body: 'Your shop keeps running as normal. Only exporting your data is paused — pay to restore it.',
      tone: 'status-locked',
    },
  };
  const c = copy[status.status];

  return (
    <div className="card" style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: '1rem', marginBottom: '1rem', borderColor: 'var(--warn)' }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: '0.8rem' }}>
        <span className={`status-pill ${c.tone}`}>{status.status}</span>
        <div>
          <strong style={{ display: 'block', fontSize: '0.92rem' }}>{c.title}</strong>
          <span style={{ fontSize: '0.82rem', color: 'var(--ink-soft)' }}>{c.body}</span>
        </div>
      </div>
      <button className="btn btn-stamp" disabled={busy} onClick={status.status === 'inactive' ? handleActivate : handlePay}>
        {status.status === 'inactive' ? 'Activate' : 'Pay now'}
      </button>
    </div>
  );
}
