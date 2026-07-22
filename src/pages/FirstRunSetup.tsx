import { useState } from 'react';
import type { FormEvent } from 'react';
import { createBusiness, setSession, ApiError } from '../api';
import { login } from '../api';

const BUSINESS_TYPES = [
  { value: 'retail', label: 'Retail / General Store' },
  { value: 'food', label: 'Food / Restaurant' },
  { value: 'services', label: 'Services' },
  { value: 'manufacturing', label: 'Manufacturing' },
];

type Step = 'business' | 'owner' | 'recovery-code' | 'done';

export default function FirstRunSetup({ onComplete }: { onComplete: () => void }) {
  const [step, setStep] = useState<Step>('business');
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const [businessName, setBusinessName] = useState('');
  const [currency, setCurrency] = useState('USD');
  const [businessType, setBusinessType] = useState('retail');

  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [confirmPassword, setConfirmPassword] = useState('');
  const [q1, setQ1] = useState('');
  const [a1, setA1] = useState('');
  const [q2, setQ2] = useState('');
  const [a2, setA2] = useState('');

  const [adminCode, setAdminCode] = useState('');
  const [businessId, setBusinessId] = useState('');
  const [savedCodeConfirmed, setSavedCodeConfirmed] = useState(false);

  function handleBusinessStep(e: FormEvent) {
    e.preventDefault();
    if (!businessName.trim()) { setError('Business name is required.'); return; }
    setError(null);
    setStep('owner');
  }

  async function handleOwnerStep(e: FormEvent) {
    e.preventDefault();
    setError(null);
    if (password.length < 8) { setError('Password must be at least 8 characters.'); return; }
    if (password !== confirmPassword) { setError('Passwords do not match.'); return; }
    if (!q1.trim() || !a1.trim() || !q2.trim() || !a2.trim()) {
      setError('Both security questions and answers are required — this is how you recover your account if you forget your password.');
      return;
    }

    setLoading(true);
    try {
      const result = await createBusiness({
        business_name: businessName,
        currency,
        business_type: businessType,
        owner_username: username,
        owner_password: password,
        security_q1: q1,
        security_a1: a1,
        security_q2: q2,
        security_a2: a2,
      });
      setAdminCode(result.admin_recovery_code);
      setBusinessId(result.business_id);
      setStep('recovery-code');
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not create your business — is the app running correctly?');
    } finally {
      setLoading(false);
    }
  }

  async function handleFinish() {
    setLoading(true);
    setError(null);
    try {
      const { token } = await login(username, password, businessId);
      setSession(token, businessId);
      onComplete();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Setup finished, but automatic sign-in failed — please sign in manually.');
      setStep('done');
    } finally {
      setLoading(false);
    }
  }

  return (
    <div style={styles.wrap}>
      <div className="card" style={styles.card}>
        <div style={styles.stampRow}>
          <div className="stamp-badge" style={{ color: 'var(--stamp)', width: '3.2rem', height: '3.2rem', fontSize: '1.3rem' }}>
            L&C
          </div>
          <div>
            <div style={styles.eyebrow}>Welcome</div>
            <h1 style={{ margin: 0 }}>Set up your business</h1>
          </div>
        </div>

        {step === 'business' && (
          <form onSubmit={handleBusinessStep} style={styles.form}>
            <div>
              <label>Business name</label>
              <input value={businessName} onChange={(e) => setBusinessName(e.target.value)} required style={styles.input} placeholder="e.g. Mama Nia General Store" />
            </div>
            <div>
              <label>Currency</label>
              <input value={currency} onChange={(e) => setCurrency(e.target.value.toUpperCase())} maxLength={3} style={styles.input} placeholder="USD, KES, ..." />
            </div>
            <div>
              <label>What kind of business is this?</label>
              <select value={businessType} onChange={(e) => setBusinessType(e.target.value)} style={styles.input}>
                {BUSINESS_TYPES.map((t) => <option key={t.value} value={t.value}>{t.label}</option>)}
              </select>
              <div style={styles.hint}>This picks sensible starting modules for you — you can change them anytime.</div>
            </div>
            {error && <div style={styles.error}>{error}</div>}
            <button className="btn btn-stamp" type="submit" style={{ width: '100%', justifyContent: 'center' }}>Continue</button>
          </form>
        )}

        {step === 'owner' && (
          <form onSubmit={handleOwnerStep} style={styles.form}>
            <div style={styles.hint}>Create your owner account — this has full access to everything.</div>
            <div>
              <label>Username</label>
              <input value={username} onChange={(e) => setUsername(e.target.value)} required style={styles.input} />
            </div>
            <div>
              <label>Password (min. 8 characters)</label>
              <input type="password" value={password} onChange={(e) => setPassword(e.target.value)} required style={styles.input} />
            </div>
            <div>
              <label>Confirm password</label>
              <input type="password" value={confirmPassword} onChange={(e) => setConfirmPassword(e.target.value)} required style={styles.input} />
            </div>
            <div style={styles.hint}>Security questions — used to recover your account if you forget your password.</div>
            <div>
              <label>Security question 1</label>
              <input value={q1} onChange={(e) => setQ1(e.target.value)} required style={styles.input} placeholder="e.g. First pet's name?" />
              <input value={a1} onChange={(e) => setA1(e.target.value)} required style={{ ...styles.input, marginTop: '0.4rem' }} placeholder="Answer" />
            </div>
            <div>
              <label>Security question 2</label>
              <input value={q2} onChange={(e) => setQ2(e.target.value)} required style={styles.input} placeholder="e.g. Mother's maiden name?" />
              <input value={a2} onChange={(e) => setA2(e.target.value)} required style={{ ...styles.input, marginTop: '0.4rem' }} placeholder="Answer" />
            </div>
            {error && <div style={styles.error}>{error}</div>}
            <div style={{ display: 'flex', gap: '0.6rem' }}>
              <button type="button" className="btn btn-outline" onClick={() => setStep('business')}>Back</button>
              <button className="btn btn-stamp" type="submit" disabled={loading} style={{ flex: 1, justifyContent: 'center' }}>
                {loading ? 'Creating…' : 'Create business'}
              </button>
            </div>
          </form>
        )}

        {step === 'recovery-code' && (
          <div style={styles.form}>
            <div style={styles.warningBox}>
              <strong>Save this admin recovery code now.</strong>
              <p style={{ margin: '0.5rem 0 0', fontSize: '0.85rem' }}>
                This is the last-resort way to recover access if you forget your password
                <em> and</em> your security question answers. It is shown <strong>exactly once</strong> —
                write it down somewhere safe (not just a screenshot on this device).
              </p>
            </div>
            <div style={styles.codeBox} className="mono">{adminCode}</div>
            <label style={{ display: 'flex', alignItems: 'center', gap: '0.5rem', textTransform: 'none', fontSize: '0.88rem', cursor: 'pointer' }}>
              <input type="checkbox" checked={savedCodeConfirmed} onChange={(e) => setSavedCodeConfirmed(e.target.checked)} />
              I've saved this code somewhere safe
            </label>
            {error && <div style={styles.error}>{error}</div>}
            <button className="btn btn-stamp" onClick={handleFinish} disabled={!savedCodeConfirmed || loading} style={{ width: '100%', justifyContent: 'center' }}>
              {loading ? 'Finishing…' : 'Continue to my business'}
            </button>
          </div>
        )}

        {step === 'done' && (
          <div style={styles.form}>
            <div>Your business was created. Please sign in with the username and password you just set.</div>
            {error && <div style={styles.error}>{error}</div>}
            <button className="btn btn-stamp" onClick={onComplete} style={{ width: '100%', justifyContent: 'center' }}>Go to sign in</button>
          </div>
        )}
      </div>
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  wrap: { minHeight: '100vh', display: 'flex', alignItems: 'center', justifyContent: 'center', padding: '1.5rem' },
  card: { width: '100%', maxWidth: 420 },
  stampRow: { display: 'flex', alignItems: 'center', gap: '0.9rem', marginBottom: '1.4rem' },
  eyebrow: { fontSize: '0.72rem', letterSpacing: '0.08em', textTransform: 'uppercase', color: 'var(--ink-soft)' },
  form: { display: 'flex', flexDirection: 'column', gap: '0.9rem' },
  input: { width: '100%' },
  hint: { fontSize: '0.8rem', color: 'var(--ink-soft)', lineHeight: 1.4 },
  error: { background: 'var(--stamp-wash)', color: 'var(--stamp)', padding: '0.5em 0.7em', borderRadius: 3, fontSize: '0.85rem' },
  warningBox: { background: 'var(--warn-wash)', color: 'var(--warn)', padding: '0.8em 1em', borderRadius: 3, fontSize: '0.85rem' },
  codeBox: {
    fontSize: '1.3rem', textAlign: 'center', letterSpacing: '0.1em', padding: '0.8em',
    background: 'var(--paper)', border: '1px dashed var(--ink-faint)', borderRadius: 3, color: 'var(--ink)',
  },
};
