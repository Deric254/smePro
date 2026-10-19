import { useCallback, useEffect, useState } from 'react';
import type { FormEvent, ReactNode } from 'react';
import { API_BASE } from '../api';
import { retryOnConnectionFailure } from '../lib/retry';
import UpdateChecker from './UpdateChecker';
import LicenceManager from './LicenceManager';

// Licence gate (desktop and Android). The real enforcement is in the local API
// (see src-tauri/src/licensing.rs: an unlicensed copy gets nothing from it);
// this component only shows the activation screen and the renewal reminder.
//
// Every release build serves the licence routes. A 404 only happens for dev
// binaries built without them, and is treated as "nothing to gate".
// If the local server can't be reached at all, the app is shown as usual:
// every screen in it already reports that, and the API stays closed anyway.

type LicenceState = {
  ok: boolean;
  kind: 'owner' | 'licensed' | 'unlicensed' | 'unconfigured';
  message: string;
  install_id?: string;
  expires?: string;
  days_left?: number;
  user?: string;
};

const RENEW_WARNING_DAYS = 14;

async function licenceRequest(path: string, init?: RequestInit): Promise<{ status: number; body: any }> {
  const res = await fetch(`${API_BASE}${path}`, { ...init, cache: 'no-store', headers: { 'Content-Type': 'application/json' } });
  let body: any = null;
  try { body = await res.json(); } catch { /* 404 pages etc. */ }
  return { status: res.status, body };
}

export default function LicenceGate({ children }: { children: ReactNode }) {
  const [state, setState] = useState<LicenceState | null>(null);
  const [gated, setGated] = useState(true);
  const [renewOpen, setRenewOpen] = useState(false);
  const [managerOpen, setManagerOpen] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const { status, body } = await retryOnConnectionFailure(() => licenceRequest('/licence/state'));
      if (status === 404) { setGated(false); return; }
      if (status === 200 && body) setState(body as LicenceState);
    } catch {
      setGated(false);
    }
  }, []);

  useEffect(() => {
    refresh();
    // Catches a licence running out while the app stays open.
    const timer = window.setInterval(refresh, 60_000);
    window.addEventListener('focus', refresh);
    return () => { window.clearInterval(timer); window.removeEventListener('focus', refresh); };
  }, [refresh]);

  if (!gated) return <>{children}</>;
  if (!state) return null;

  if (!state.ok) {
    return (
      <>
        <ActivationScreen state={state} onChanged={(s) => setState(s)} />
        {/* An unlicensed or expired copy must still be able to update itself. */}
        <UpdateChecker />
      </>
    );
  }

  const soon = state.kind === 'licensed' && typeof state.days_left === 'number' && state.days_left <= RENEW_WARNING_DAYS;
  return (
    <>
      {children}
      {/* Only the claimed owner copy shows this; the local API refuses everyone else too. */}
      {state.kind === 'owner' && (
        <>
          <button type="button" onClick={() => setManagerOpen(true)} style={styles.ownerTab} title="Licence manager (owner only)">Licences</button>
          {managerOpen && <LicenceManager onClose={() => setManagerOpen(false)} />}
        </>
      )}
      {soon && (
        <div role="status" style={styles.renewBar}>
          <span>
            Your licence {state.days_left! <= 0 ? 'ends today' : `ends in ${state.days_left} day${state.days_left === 1 ? '' : 's'}`}
            {state.expires ? ` (${state.expires})` : ''}. Ask the owner for a renewal.
          </span>
          <button type="button" className="btn btn-outline" style={styles.renewBtn} onClick={() => setRenewOpen(true)}>Renew</button>
        </div>
      )}
      {renewOpen && (
        <div style={styles.overlay} onClick={() => setRenewOpen(false)}>
          <div onClick={(e) => e.stopPropagation()} style={{ width: '100%', maxWidth: 420 }}>
            <ActivationScreen
              state={state}
              renewal
              onChanged={(s) => { setState(s); if (s.ok) setRenewOpen(false); }}
              onCancel={() => setRenewOpen(false)}
            />
          </div>
        </div>
      )}
    </>
  );
}

function ActivationScreen({
  state, onChanged, renewal = false, onCancel,
}: {
  state: LicenceState;
  onChanged: (s: LicenceState) => void;
  renewal?: boolean;
  onCancel?: () => void;
}) {
  const [licence, setLicence] = useState('');
  const [ownerKey, setOwnerKey] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);

  async function submit(path: string, payload: Record<string, string>, e?: FormEvent) {
    e?.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const { status, body } = await licenceRequest(path, { method: 'POST', body: JSON.stringify(payload) });
      if (status === 200 && body) {
        setLicence('');
        setOwnerKey('');
        onChanged(body as LicenceState);
      } else {
        setError(body?.error ?? 'That did not work. Please try again.');
      }
    } catch {
      setError('Could not reach the app. Close it and open it again.');
    } finally {
      setBusy(false);
    }
  }

  async function copyCode() {
    try {
      await navigator.clipboard.writeText(state.install_id ?? '');
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch { /* clipboard not available: the code is on screen to read out */ }
  }

  if (state.kind === 'unconfigured') {
    return (
      <div style={renewal ? undefined : styles.wrap}>
        <div className="card" style={styles.card}>
          <h2 style={styles.title}>This copy can't be used</h2>
          <p style={styles.text}>{state.message}</p>
        </div>
      </div>
    );
  }

  return (
    <div style={renewal ? undefined : styles.wrap}>
      <div className="card" style={styles.card}>
        <div style={styles.eyebrow}>SME Pro</div>
        <h2 style={styles.title}>{renewal ? 'Renew your licence' : 'Activate this computer'}</h2>
        {!renewal && state.message && <p style={styles.text}>{state.message}</p>}
        <p style={styles.text}>
          Send this activation code to the owner. They will send you a licence for this computer.
        </p>
        <div style={styles.codeRow}>
          <code className="mono" style={styles.code}>{state.install_id ?? '…'}</code>
          <button type="button" className="btn btn-outline" style={styles.copyBtn} onClick={copyCode}>{copied ? 'Copied' : 'Copy'}</button>
        </div>

        <form onSubmit={(e) => submit('/licence/activate', { licence }, e)} style={styles.form}>
          <label style={styles.label}>
            Paste the licence the owner sent you
            <textarea
              className="mono"
              rows={4}
              value={licence}
              onChange={(e) => setLicence(e.target.value)}
              placeholder="SMP1.…"
              spellCheck={false}
              style={{ width: '100%', marginTop: '0.3rem' }}
              required
            />
          </label>
          {error && <div role="alert" style={styles.error}>{error}</div>}
          <button type="submit" className="btn btn-stamp" disabled={busy || !licence.trim()} style={{ width: '100%', justifyContent: 'center' }}>
            {renewal ? 'Renew' : 'Activate'}
          </button>
          {onCancel && (
            <button type="button" className="btn btn-outline" style={{ width: '100%', justifyContent: 'center' }} onClick={onCancel}>Cancel</button>
          )}
        </form>

        {!renewal && (
          <details style={{ marginTop: '1.2rem' }}>
            <summary style={styles.summary}>I am the owner of this app</summary>
            <form onSubmit={(e) => submit('/licence/claim-owner', { owner_key: ownerKey }, e)} style={{ ...styles.form, marginTop: '0.6rem' }}>
              <label style={styles.label}>
                Owner key (used once to prove it's you; it is never saved)
                <input
                  className="mono"
                  type="password"
                  value={ownerKey}
                  onChange={(e) => setOwnerKey(e.target.value)}
                  autoComplete="off"
                  spellCheck={false}
                  style={{ width: '100%', marginTop: '0.3rem' }}
                />
              </label>
              <button type="submit" className="btn btn-outline" disabled={busy || !ownerKey.trim()} style={{ width: '100%', justifyContent: 'center' }}>
                Use this computer as the owner copy
              </button>
            </form>
          </details>
        )}
      </div>
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  wrap: { minHeight: '100vh', display: 'flex', alignItems: 'center', justifyContent: 'center', padding: '1.5rem' },
  card: { width: '100%', maxWidth: 420 },
  eyebrow: { fontSize: '0.72rem', letterSpacing: '0.08em', textTransform: 'uppercase', color: 'var(--ink-soft)' },
  title: { margin: '0.2rem 0 0.8rem' },
  text: { fontSize: '0.9rem', lineHeight: 1.45, margin: '0 0 0.8rem' },
  codeRow: { display: 'flex', gap: '0.5rem', alignItems: 'center', marginBottom: '1.1rem' },
  code: { flex: 1, fontSize: '1.15rem', letterSpacing: '0.06em', padding: '0.5em 0.7em', border: '1px solid var(--paper-line)', borderRadius: 3, background: 'var(--paper)', userSelect: 'all' },
  copyBtn: { fontSize: '0.8rem', padding: '0.45em 0.8em' },
  form: { display: 'flex', flexDirection: 'column', gap: '0.8rem' },
  label: { fontSize: '0.85rem', display: 'block' },
  summary: { cursor: 'pointer', fontSize: '0.8rem', color: 'var(--ink-soft)' },
  error: { background: 'var(--stamp-wash)', color: 'var(--stamp)', padding: '0.5em 0.7em', borderRadius: 3, fontSize: '0.85rem' },
  renewBar: { position: 'fixed', left: 0, right: 0, bottom: 0, zIndex: 900, display: 'flex', alignItems: 'center', justifyContent: 'center', gap: '0.8rem', padding: '0.5rem 1rem', background: 'var(--warn-wash)', color: 'var(--ink)', borderTop: '1px solid var(--warn)', fontSize: '0.85rem' },
  ownerTab: { position: 'fixed', right: 0, top: '45%', zIndex: 800, writingMode: 'vertical-rl', padding: '0.6rem 0.3rem', fontSize: '0.72rem', letterSpacing: '0.06em', background: 'var(--paper-card)', color: 'var(--ink-soft)', border: '1px solid var(--paper-line)', borderRight: 'none', borderRadius: '4px 0 0 4px', cursor: 'pointer' },
  renewBtn: { fontSize: '0.8rem', padding: '0.25em 0.8em' },
  overlay: { position: 'fixed', inset: 0, zIndex: 1000, display: 'flex', alignItems: 'center', justifyContent: 'center', padding: '1.5rem', background: 'rgba(0,0,0,0.45)' },
};
