import { useCallback, useEffect, useState } from 'react';
import type { FormEvent } from 'react';
import { API_BASE } from '../api';
import { confirmDialog } from './ConfirmDialog';

// Owner-only licence manager (opened from the "Licences" button, which only the
// claimed owner copy shows; the local API refuses everyone else anyway).
// The owner key is only ever sent to the local app to unlock; it is held in memory
// there, never saved, and locks again after 30 idle minutes. See licence_admin.rs.

type Customer = {
  id: string; name: string; code: string; note: string; created: string;
  expires: string | null; status: 'active' | 'expiring' | 'expired' | 'none'; days_left: number | null;
  licence: string | null; paid: string[]; paid_for_month: boolean; last_issued: string | null;
};
type Summary = { total: number; active: number; expiring: number; expired: number; paid_for_month: number; unpaid_for_month: number };
type ListData = { month: string; summary: Summary; customers: Customer[] };
type Issued = { name: string; licence: string };

async function call(path: string, body: Record<string, unknown> = {}, method = 'POST'): Promise<{ status: number; data: any }> {
  const init: RequestInit = { method, cache: 'no-store', headers: { 'Content-Type': 'application/json' } };
  if (method !== 'GET') init.body = JSON.stringify(body);
  const res = await fetch(`${API_BASE}/licence/admin/${path}`, init);
  let data: any = null;
  try { data = await res.json(); } catch { /* empty */ }
  return { status: res.status, data };
}

const monthKey = (d: Date) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}`;
function shiftMonth(key: string, by: number) {
  const [y, m] = key.split('-').map(Number);
  return monthKey(new Date(y, m - 1 + by, 1));
}
function monthLabel(key: string) {
  const [y, m] = key.split('-').map(Number);
  return new Date(y, m - 1, 1).toLocaleString(undefined, { month: 'long', year: 'numeric' });
}

export default function LicenceManager({ onClose }: { onClose: () => void }) {
  const [unlocked, setUnlocked] = useState<boolean | null>(null);
  const [ownerKey, setOwnerKey] = useState('');
  const [data, setData] = useState<ListData | null>(null);
  const [month, setMonth] = useState(monthKey(new Date()));
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [issued, setIssued] = useState<Issued | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const [form, setForm] = useState({ code: '', name: '', note: '', days: '31' });
  const [renewId, setRenewId] = useState<string | null>(null);
  const [renewDays, setRenewDays] = useState('31');

  // Any answer that says "locked" sends the screen back to the unlock form.
  const handle = useCallback((status: number, d: any): boolean => {
    if (status === 401) { setUnlocked(false); setData(null); setError(d?.error ?? 'Locked.'); return false; }
    if (status >= 400) { setError(d?.error ?? 'That did not work. Please try again.'); return false; }
    return true;
  }, []);

  const load = useCallback(async (m: string) => {
    try {
      const { status, data: d } = await call('list', { month: m });
      if (handle(status, d)) { setData(d); setError(null); }
    } catch { setError('Could not reach the app. Close it and open it again.'); }
  }, [handle]);

  useEffect(() => {
    (async () => {
      try {
        const { data: s } = await call('status', {}, 'GET');
        setUnlocked(!!s?.unlocked);
        if (s?.unlocked) await load(month);
      } catch { setUnlocked(false); }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function run<T>(fn: () => Promise<T>) {
    setBusy(true); setError(null);
    try { return await fn(); } catch { setError('Could not reach the app. Close it and open it again.'); } finally { setBusy(false); }
  }

  async function unlock(e: FormEvent) {
    e.preventDefault();
    await run(async () => {
      const { status, data: d } = await call('unlock', { owner_key: ownerKey });
      if (status === 200) { setOwnerKey(''); setUnlocked(true); await load(month); } else setError(d?.error ?? 'Could not unlock.');
    });
  }

  async function lock() {
    await run(async () => { await call('lock'); setUnlocked(false); setData(null); setIssued(null); });
  }

  function readKeyFile(file: File | undefined) {
    if (!file) return;
    const reader = new FileReader();
    reader.onload = () => setOwnerKey(String(reader.result ?? '').trim());
    reader.readAsText(file);
  }

  async function changeMonth(by: number) {
    const m = shiftMonth(month, by);
    setMonth(m);
    await run(() => load(m));
  }

  async function addCustomer(e: FormEvent) {
    e.preventDefault();
    await run(async () => {
      const { status, data: d } = await call('add', { ...form, days: Number(form.days), month });
      if (handle(status, d)) {
        setData(d); setIssued({ name: d.customer, licence: d.licence });
        setForm({ code: '', name: '', note: '', days: form.days });
      }
    });
  }

  async function tick(c: Customer, paid: boolean) {
    if (!paid && !(await confirmDialog({
      title: `Un-tick ${monthLabel(month)} for ${c.name}?`,
      message: 'This only corrects your record. A licence that was already sent keeps working until its date.',
      confirmLabel: 'Un-tick',
    }))) return;
    await run(async () => {
      const { status, data: d } = await call('paid', { id: c.id, month, paid });
      if (handle(status, d)) { setData(d); if (d.licence) setIssued({ name: c.name, licence: d.licence }); }
    });
  }

  async function renew(c: Customer) {
    await run(async () => {
      const { status, data: d } = await call('renew', { id: c.id, days: Number(renewDays), month });
      if (handle(status, d)) { setData(d); setRenewId(null); setIssued({ name: c.name, licence: d.licence }); }
    });
  }

  async function remove(c: Customer) {
    if (!(await confirmDialog({
      title: `Remove ${c.name} from the list?`,
      message: 'This removes the record only. Their licence keeps working until it expires.',
      confirmLabel: 'Remove', tone: 'danger',
    }))) return;
    await run(async () => {
      const { status, data: d } = await call('remove', { id: c.id, month });
      if (handle(status, d)) setData(d);
    });
  }

  async function exportCsv() {
    await run(async () => {
      const { status, data: d } = await call('export', { month });
      if (!handle(status, d)) return;
      const blob = new Blob([d.csv], { type: 'text/csv;charset=utf-8' });
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url; a.download = d.filename; a.click();
      URL.revokeObjectURL(url);
    });
  }

  async function copy(text: string, tag: string) {
    try { await navigator.clipboard.writeText(text); setCopied(tag); window.setTimeout(() => setCopied(null), 1500); } catch { /* shown on screen to copy by hand */ }
  }

  const s = data?.summary;
  return (
    <div style={styles.overlay}>
      <div className="card" style={styles.panel}>
        <div style={styles.head}>
          <div>
            <div style={styles.eyebrow}>Owner only</div>
            <h2 style={{ margin: 0 }}>Licences</h2>
          </div>
          <div style={{ display: 'flex', gap: '0.5rem' }}>
            {unlocked && <button type="button" className="btn btn-outline" onClick={lock} disabled={busy}>Lock</button>}
            <button type="button" className="btn btn-outline" onClick={onClose}>Close</button>
          </div>
        </div>

        {error && <div role="alert" style={styles.error}>{error}</div>}

        {unlocked === null && <p style={styles.muted}>Loading…</p>}

        {unlocked === false && (
          <form onSubmit={unlock} style={styles.form}>
            <p style={styles.muted}>
              Unlock with your owner key to make licences. The key is held in memory only (never saved),
              and the manager locks again after 30 idle minutes or when you close the app.
            </p>
            <label style={styles.label}>
              Choose your key file (smp-owner.key)
              <input type="file" onChange={(e) => readKeyFile(e.target.files?.[0])} style={{ display: 'block', marginTop: '0.3rem' }} />
            </label>
            <label style={styles.label}>
              …or paste the key
              <input className="mono" type="password" value={ownerKey} onChange={(e) => setOwnerKey(e.target.value)}
                autoComplete="off" spellCheck={false} style={{ width: '100%', marginTop: '0.3rem' }} />
            </label>
            <button type="submit" className="btn btn-stamp" disabled={busy || !ownerKey.trim()}>Unlock</button>
          </form>
        )}

        {unlocked && data && s && (
          <>
            <div style={styles.cards}>
              <Stat label="Total customers" value={s.total} />
              <Stat label="Licensed now" value={s.active} />
              <Stat label="Expiring in 7 days" value={s.expiring} warn={s.expiring > 0} />
              <Stat label="Expired" value={s.expired} warn={s.expired > 0} />
              <Stat label={`Paid ${monthLabel(month)}`} value={s.paid_for_month} />
              <Stat label="Not paid yet" value={s.unpaid_for_month} warn={s.unpaid_for_month > 0} />
            </div>

            <form onSubmit={addCustomer} style={styles.addRow}>
              <input className="mono" placeholder="Activation code" value={form.code} required style={{ flex: '1 1 150px' }}
                onChange={(e) => setForm({ ...form, code: e.target.value })} autoComplete="off" spellCheck={false} />
              <input placeholder="Customer name" value={form.name} required maxLength={60} style={{ flex: '1 1 150px' }}
                onChange={(e) => setForm({ ...form, name: e.target.value })} />
              <input placeholder="Note (optional)" value={form.note} maxLength={200} style={{ flex: '1 1 130px' }}
                onChange={(e) => setForm({ ...form, note: e.target.value })} />
              <input type="number" min={1} max={365} title="Days the first licence lasts" value={form.days} style={{ width: 80 }}
                onChange={(e) => setForm({ ...form, days: e.target.value })} />
              <button type="submit" className="btn btn-stamp" disabled={busy || !form.code.trim() || !form.name.trim()}>Generate licence</button>
            </form>

            {issued && (
              <div style={styles.issued}>
                <div style={{ marginBottom: '0.4rem' }}><strong>Licence for {issued.name}.</strong> Send them only this text:</div>
                <textarea readOnly className="mono" rows={3} value={issued.licence} style={{ width: '100%' }} onFocus={(e) => e.currentTarget.select()} />
                <div style={{ display: 'flex', gap: '0.5rem', marginTop: '0.4rem' }}>
                  <button type="button" className="btn btn-stamp" onClick={() => copy(issued.licence, 'issued')}>{copied === 'issued' ? 'Copied' : 'Copy licence'}</button>
                  <button type="button" className="btn btn-outline" onClick={() => setIssued(null)}>Done</button>
                </div>
              </div>
            )}

            <div style={styles.monthRow}>
              <button type="button" className="btn btn-outline" onClick={() => changeMonth(-1)} disabled={busy} aria-label="Previous month">‹</button>
              <strong style={{ minWidth: 140, textAlign: 'center' }}>{monthLabel(month)}</strong>
              <button type="button" className="btn btn-outline" onClick={() => changeMonth(1)} disabled={busy} aria-label="Next month">›</button>
              <span style={{ flex: 1 }} />
              <button type="button" className="btn btn-outline" onClick={exportCsv} disabled={busy || s.total === 0}>Export (Excel / CSV)</button>
            </div>
            <p style={styles.hint}>
              Tick a month when the customer has paid: a fresh licence is made, valid to the end of the latest paid month plus 5 days.
              Send it to them; they paste it into the Renew box.
            </p>

            {data.customers.length === 0 ? (
              <p style={styles.muted}>No customers yet. Add the first one above using the activation code they send you.</p>
            ) : (
              <div style={{ overflowX: 'auto' }}>
                <table className="data-table" style={{ width: '100%' }}>
                  <thead>
                    <tr>
                      <th>Customer</th><th>Code</th><th>Licence ends</th>
                      <th style={{ textAlign: 'center' }}>Paid {monthLabel(month)}</th><th />
                    </tr>
                  </thead>
                  <tbody>
                    {data.customers.map((c) => (
                      <tr key={c.id}>
                        <td>
                          <div>{c.name}</div>
                          {c.note && <div style={styles.small}>{c.note}</div>}
                          {c.paid.length > 0 && <div style={styles.small}>Paid: {c.paid.join(', ')}</div>}
                        </td>
                        <td className="mono" style={{ whiteSpace: 'nowrap' }}>{c.code}</td>
                        <td style={{ whiteSpace: 'nowrap' }}>
                          {c.expires ?? '—'} <StatusChip c={c} />
                        </td>
                        <td style={{ textAlign: 'center' }}>
                          <input type="checkbox" checked={c.paid_for_month} disabled={busy}
                            aria-label={`${c.name} paid ${monthLabel(month)}`} onChange={(e) => tick(c, e.target.checked)} />
                        </td>
                        <td style={{ whiteSpace: 'nowrap' }}>
                          {renewId === c.id ? (
                            <span style={{ display: 'inline-flex', gap: '0.3rem', alignItems: 'center' }}>
                              <input type="number" min={1} max={365} value={renewDays} style={{ width: 70 }} onChange={(e) => setRenewDays(e.target.value)} />
                              <button type="button" className="btn btn-stamp" disabled={busy} onClick={() => renew(c)}>Make</button>
                              <button type="button" className="btn btn-outline" onClick={() => setRenewId(null)}>Cancel</button>
                            </span>
                          ) : (
                            <span style={{ display: 'inline-flex', gap: '0.3rem' }}>
                              {c.licence && <button type="button" className="btn btn-outline" onClick={() => copy(c.licence!, c.id)}>{copied === c.id ? 'Copied' : 'Copy licence'}</button>}
                              <button type="button" className="btn btn-outline" onClick={() => { setRenewId(c.id); setRenewDays('31'); }}>Renew…</button>
                              <button type="button" className="btn btn-outline" onClick={() => remove(c)}>Remove</button>
                            </span>
                          )}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
            <p style={styles.hint}>
              The customer list is saved on this computer only and is not part of business backups: use Export from time to time.
            </p>
          </>
        )}
      </div>
    </div>
  );
}

function Stat({ label, value, warn }: { label: string; value: number; warn?: boolean }) {
  return (
    <div style={{ ...styles.stat, borderColor: warn ? 'var(--warn)' : 'var(--paper-line)' }}>
      <div style={{ fontSize: '1.4rem', fontWeight: 600 }}>{value}</div>
      <div style={styles.small}>{label}</div>
    </div>
  );
}

function StatusChip({ c }: { c: Customer }) {
  const map = {
    active: { text: 'active', bg: 'var(--ok-wash)', fg: 'var(--ok)' },
    expiring: { text: `${c.days_left ?? 0}d left`, bg: 'var(--warn-wash)', fg: 'var(--warn)' },
    expired: { text: 'expired', bg: 'var(--stamp-wash)', fg: 'var(--stamp)' },
    none: { text: 'no licence', bg: 'var(--paper)', fg: 'var(--ink-soft)' },
  }[c.status];
  return <span style={{ background: map.bg, color: map.fg, borderRadius: 999, padding: '0.1em 0.6em', fontSize: '0.72rem', marginLeft: '0.3rem' }}>{map.text}</span>;
}

const styles: Record<string, React.CSSProperties> = {
  overlay: { position: 'fixed', inset: 0, zIndex: 1100, background: 'rgba(20,26,30,0.55)', display: 'flex', alignItems: 'flex-start', justifyContent: 'center', padding: '1.5rem', overflowY: 'auto' },
  panel: { width: '100%', maxWidth: 980, display: 'flex', flexDirection: 'column', gap: '0.9rem' },
  head: { display: 'flex', justifyContent: 'space-between', alignItems: 'center', gap: '1rem' },
  eyebrow: { fontSize: '0.72rem', letterSpacing: '0.08em', textTransform: 'uppercase', color: 'var(--ink-soft)' },
  form: { display: 'flex', flexDirection: 'column', gap: '0.8rem', maxWidth: 460 },
  label: { fontSize: '0.85rem', display: 'block' },
  muted: { fontSize: '0.9rem', color: 'var(--ink-soft)', margin: 0, lineHeight: 1.45 },
  hint: { fontSize: '0.78rem', color: 'var(--ink-soft)', margin: 0, lineHeight: 1.4 },
  small: { fontSize: '0.74rem', color: 'var(--ink-soft)' },
  error: { background: 'var(--stamp-wash)', color: 'var(--stamp)', padding: '0.5em 0.7em', borderRadius: 3, fontSize: '0.85rem' },
  cards: { display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(130px, 1fr))', gap: '0.6rem' },
  stat: { border: '1px solid var(--paper-line)', borderRadius: 3, padding: '0.6rem 0.8rem', background: 'var(--paper)' },
  addRow: { display: 'flex', flexWrap: 'wrap', gap: '0.5rem', alignItems: 'center' },
  issued: { border: '1px solid var(--ok)', background: 'var(--ok-wash)', borderRadius: 3, padding: '0.7rem 0.8rem', fontSize: '0.88rem' },
  monthRow: { display: 'flex', alignItems: 'center', gap: '0.5rem', flexWrap: 'wrap' },
};
