import { useEffect, useState } from 'react';
import { PAY, openLink, payMail, payRef, proofSentAt, rememberProofSent } from '../lib/pay';
import type { Plan } from '../lib/pay';

// The one pay window of the app: PayPal details for a licence plan, the reference to put in the
// PayPal note, and the proof email. Opened from the licence screen (LicenceGate).

export function PlanButtons({ code, onPick }: { code: string; onPick: (plan: Plan) => void }) {
  return (
    <div style={{ display: 'flex', gap: '0.5rem', marginBottom: '0.5rem' }}>
      {(Object.keys(PAY.plans) as Plan[]).map((plan) => (
        <button key={plan} type="button" className="btn btn-stamp" disabled={!code} style={{ flex: 1, justifyContent: 'center', flexDirection: 'column' }} onClick={() => onPick(plan)}>
          {PAY.plans[plan].label}
          <span style={{ fontWeight: 400, fontSize: '0.7rem' }}>Pay with PayPal</span>
        </button>
      ))}
    </div>
  );
}

function CopyRow({ label, value }: { label: string; value: string }) {
  const [copied, setCopied] = useState(false);
  async function copy() {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch { /* clipboard not available: the text is on screen */ }
  }
  return (
    <div style={{ marginTop: '0.6rem' }}>
      <div style={{ fontSize: '0.68rem', fontWeight: 700, letterSpacing: '0.08em', textTransform: 'uppercase', color: 'var(--ink-soft)', marginBottom: '0.25rem' }}>{label}</div>
      <div style={{ display: 'flex', gap: '0.5rem', alignItems: 'center', justifyContent: 'space-between', border: '1px solid var(--paper-line)', borderRadius: 3, background: 'var(--paper)', padding: '0.45em 0.7em', wordBreak: 'break-all' }}>
        <b className="mono" style={{ fontSize: '0.9rem' }}>{value}</b>
        <button type="button" className="btn btn-outline" style={{ fontSize: '0.75rem', padding: '0.25em 0.7em' }} onClick={copy}>{copied ? 'Copied' : 'Copy'}</button>
      </div>
    </div>
  );
}

export default function PayWindow({ code, plan, onClose }: { code: string; plan: Plan; onClose: () => void }) {
  const p = PAY.plans[plan];
  const ref = payRef(code, plan);
  const mail = payMail(code, plan);
  const [sentAt, setSentAt] = useState<number | null>(() => proofSentAt(ref));
  const [again, setAgain] = useState(false);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose(); };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  async function open(url: string) {
    setFailed(!(await openLink(url)));
  }

  async function sendProof(url: string) {
    if (await openLink(url)) {
      rememberProofSent(ref);
      setSentAt(Date.now());
      setFailed(false);
    } else {
      setFailed(true);
    }
  }

  const alreadySent = sentAt !== null && !again;
  return (
    <div style={styles.overlay} onClick={onClose}>
      <div role="dialog" aria-modal="true" aria-labelledby="pay-title" className="card" style={styles.card} onClick={(e) => e.stopPropagation()}>
        <button type="button" onClick={onClose} aria-label="Close" style={styles.close}>&times;</button>
        <h2 id="pay-title" style={{ margin: 0 }}>Pay with PayPal</h2>
        <div style={{ fontSize: '0.8rem', color: 'var(--ink-soft)', margin: '0.15rem 0 0.4rem' }}>{PAY.app} licence, {p.name}</div>
        <div style={{ fontSize: '2rem', fontWeight: 700 }}>${p.amount} <span style={{ fontSize: '0.9rem', fontWeight: 400 }}>USD</span></div>
        {alreadySent ? (
          <>
            <div role="status" style={styles.notice}>
              <b>Proof already sent on {new Date(sentAt).toLocaleDateString()}.</b>{' '}
              Please do not pay again for the same thing.
            </div>
            <div style={{ display: 'flex', gap: '0.5rem', marginTop: '0.9rem' }}>
              <button type="button" className="btn btn-outline" style={styles.half} onClick={onClose}>Close</button>
              <button type="button" className="btn btn-outline" style={styles.half} onClick={() => setAgain(true)}>I need to pay again</button>
            </div>
          </>
        ) : (
          <>
            <CopyRow label="Send to (PayPal)" value={PAY.email} />
            <CopyRow label="Put this in the PayPal note" value={ref} />
            <ol style={styles.steps}>
              <li><b>Open PayPal</b> and send exactly <b>${p.amount} USD</b> to the email above.</li>
              <li><b>Take a screenshot</b> of the PayPal confirmation. It must show the amount, the date and the <b>transaction ID</b>.</li>
              <li><b>Click "Email my proof"</b>. The email opens already written. Attach the screenshot and press Send. (The app cannot attach the file for you.)</li>
            </ol>
            {failed && <div role="alert" style={styles.error}>The link could not be opened. Send the screenshot to {PAY.email} with the reference above.</div>}
            <div style={{ display: 'flex', gap: '0.5rem' }}>
              <button type="button" className="btn btn-outline" style={styles.half} onClick={() => open(PAY.paypal)}>Open PayPal</button>
              <button type="button" className="btn btn-stamp" style={styles.half} onClick={() => sendProof(mail.mailto)}>Email my proof</button>
            </div>
            <div style={{ fontSize: '0.72rem', color: 'var(--ink-soft)', marginTop: '0.7rem' }}>
              Email app not opening?{' '}
              <button type="button" onClick={() => sendProof(mail.gmail)} style={styles.link}>Write it in Gmail on the web</button>
              , or send the screenshot to <b>{PAY.email}</b> with the reference above.
            </div>
          </>
        )}
      </div>
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  overlay: { position: 'fixed', inset: 0, zIndex: 1100, display: 'flex', alignItems: 'center', justifyContent: 'center', padding: '1rem', overflow: 'auto', background: 'rgba(0,0,0,0.55)' },
  card: { position: 'relative', width: '100%', maxWidth: 440 },
  close: { position: 'absolute', top: 6, right: 12, background: 'none', border: 'none', fontSize: '1.5rem', color: 'var(--ink-soft)', cursor: 'pointer' },
  notice: { marginTop: '0.8rem', padding: '0.6em 0.8em', border: '1px solid var(--warn)', background: 'var(--warn-wash)', borderRadius: 3, fontSize: '0.85rem' },
  steps: { margin: '0.9rem 0', paddingLeft: '1.2rem', fontSize: '0.85rem', lineHeight: 1.55 },
  error: { background: 'var(--stamp-wash)', color: 'var(--stamp)', padding: '0.5em 0.7em', borderRadius: 3, fontSize: '0.85rem', marginBottom: '0.7rem' },
  half: { flex: 1, justifyContent: 'center' },
  link: { background: 'none', border: 'none', padding: 0, font: 'inherit', textDecoration: 'underline', cursor: 'pointer', color: 'inherit' },
};
