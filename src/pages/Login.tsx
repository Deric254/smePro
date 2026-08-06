import { useEffect, useState } from 'react';
import type { FormEvent } from 'react';
import { login, login2fa, setSession, getResolvedBusinessId, recoverViaSecurityQuestions, recoverViaAdminCode, ApiError } from '../api';

type Mode = 'login' | '2fa' | 'recover-questions' | 'recover-admin-code' | 'recover-success';

export default function Login({ onLoggedIn }: { onLoggedIn: () => void }) {
  const [businessId, setBusinessId] = useState(localStorage.getItem('erp_business_id') || '');
  const [businessIdKnown, setBusinessIdKnown] = useState(false);
  const [resolving, setResolving] = useState(true);
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [mode, setMode] = useState<Mode>('login');
  const [tempToken, setTempToken] = useState('');
  const [twoFaCode, setTwoFaCode] = useState('');

  // Recovery form state — shared field names across both methods where
  // they overlap (username, new password) to keep this simple.
  const [recoverUsername, setRecoverUsername] = useState('');
  const [answer1, setAnswer1] = useState('');
  const [answer2, setAnswer2] = useState('');
  const [adminCode, setAdminCode] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [confirmNewPassword, setConfirmNewPassword] = useState('');

  useEffect(() => {
    // The overwhelmingly common case: one installed copy of this app,
    // one business, forever. Nobody running a shop should have to know
    // or paste a UUID just to sign in — that's an implementation
    // detail, not something to memorize. Only falls back to asking for
    // it explicitly in the rare case where this install genuinely has
    // more than one business and the app can't safely guess which one.
    getResolvedBusinessId()
      .then((r) => {
        if (r.business_id) {
          setBusinessId(r.business_id);
          setBusinessIdKnown(true);
        }
      })
      .catch(() => {})
      .finally(() => setResolving(false));
  }, []);

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    setError(null);
    setLoading(true);
    try {
      const result = await login(username, password, businessId);
      if (result.requires_2fa) {
        setTempToken(result.temp_token);
        setMode('2fa');
        return;
      }
      setSession(result.token, businessId);
      onLoggedIn();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not reach the server');
    } finally {
      setLoading(false);
    }
  }

  async function handleTwoFaSubmit(e: FormEvent) {
    e.preventDefault();
    setError(null);
    setLoading(true);
    try {
      const { token } = await login2fa(tempToken, twoFaCode);
      setSession(token, businessId);
      onLoggedIn();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not verify code');
    } finally {
      setLoading(false);
    }
  }

  function resetRecoveryFields() {
    setRecoverUsername(''); setAnswer1(''); setAnswer2(''); setAdminCode('');
    setNewPassword(''); setConfirmNewPassword(''); setError(null);
    setTempToken(''); setTwoFaCode('');
  }

  function switchMode(next: Mode) {
    resetRecoveryFields();
    setMode(next);
  }

  async function handleRecoverySubmit(e: FormEvent) {
    e.preventDefault();
    setError(null);
    if (newPassword.length < 8) { setError('New password must be at least 8 characters.'); return; }
    if (newPassword !== confirmNewPassword) { setError('Passwords do not match.'); return; }
    setLoading(true);
    try {
      if (mode === 'recover-questions') {
        await recoverViaSecurityQuestions(businessId, {
          username: recoverUsername, answer1, answer2, new_password: newPassword,
        });
      } else if (mode === 'recover-admin-code') {
        await recoverViaAdminCode(businessId, {
          username: recoverUsername, admin_code: adminCode, new_password: newPassword,
        });
      }
      setMode('recover-success');
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Recovery failed');
    } finally {
      setLoading(false);
    }
  }

  if (resolving) return null;

  return (
    <div style={styles.wrap}>
      <div className="card" style={styles.card}>
        <div style={styles.stampRow}>
          <div className="stamp-badge" style={{ color: 'var(--stamp)', width: '3.2rem', height: '3.2rem', fontSize: '1.3rem' }}>
            SP
          </div>
          <div>
            <div style={styles.eyebrow}>SME Pro</div>
            <h1 style={{ margin: 0 }}>{mode === 'login' ? 'Sign in' : mode === '2fa' ? 'Verify your identity' : 'Reset your password'}</h1>
          </div>
        </div>

        {mode === 'login' && (
          <form onSubmit={handleSubmit} style={styles.form}>
            {!businessIdKnown && (
              <div>
                <label htmlFor="biz">Business ID</label>
                <input id="biz" value={businessId} onChange={(e) => setBusinessId(e.target.value)} required style={styles.input} />
                <div style={styles.hint}>
                  This install has more than one business set up — enter the ID for the one you're signing into.
                </div>
              </div>
            )}
            <div>
              <label htmlFor="user">Username</label>
              <input id="user" value={username} onChange={(e) => setUsername(e.target.value)} required style={styles.input} autoFocus />
            </div>
            <div>
              <label htmlFor="pass">Password</label>
              <input id="pass" type="password" value={password} onChange={(e) => setPassword(e.target.value)} required style={styles.input} />
            </div>

            {error && <div style={styles.error}>{error}</div>}

            <button type="submit" className="btn btn-stamp" disabled={loading} style={{ width: '100%', justifyContent: 'center', marginTop: '0.4rem' }}>
              {loading ? 'Signing in…' : 'Sign in'}
            </button>

            <button type="button" onClick={() => switchMode('recover-questions')} style={styles.linkBtn}>
              Forgot your password?
            </button>
          </form>
        )}

        {mode === '2fa' && (
          <form onSubmit={handleTwoFaSubmit} style={styles.form}>
            <div style={styles.hint}>
              Enter the 6-digit code from your authenticator app.
            </div>
            <div>
              <label htmlFor="totp">Authentication code</label>
              <input
                id="totp"
                className="mono"
                value={twoFaCode}
                onChange={(e) => setTwoFaCode(e.target.value)}
                required
                autoFocus
                inputMode="numeric"
                maxLength={8}
                style={styles.input}
              />
            </div>

            {error && <div style={styles.error}>{error}</div>}

            <button type="submit" className="btn btn-stamp" disabled={loading} style={{ width: '100%', justifyContent: 'center' }}>
              {loading ? 'Verifying…' : 'Verify'}
            </button>
            <button type="button" onClick={() => switchMode('login')} style={styles.linkBtn}>
              Back to sign in
            </button>
          </form>
        )}

        {(mode === 'recover-questions' || mode === 'recover-admin-code') && (
          <form onSubmit={handleRecoverySubmit} style={styles.form}>
            <div style={styles.tabRow}>
              <button
                type="button"
                className={mode === 'recover-questions' ? 'btn' : 'btn btn-outline'}
                style={styles.tabBtn}
                onClick={() => switchMode('recover-questions')}
              >
                Security questions
              </button>
              <button
                type="button"
                className={mode === 'recover-admin-code' ? 'btn' : 'btn btn-outline'}
                style={styles.tabBtn}
                onClick={() => switchMode('recover-admin-code')}
              >
                Admin recovery code
              </button>
            </div>

            <div>
              <label>Username</label>
              <input value={recoverUsername} onChange={(e) => setRecoverUsername(e.target.value)} required style={styles.input} />
            </div>

            {mode === 'recover-questions' ? (
              <>
                <div style={styles.hint}>Answer both security questions you set up when this account was created.</div>
                <div>
                  <label>Answer to question 1</label>
                  <input value={answer1} onChange={(e) => setAnswer1(e.target.value)} required style={styles.input} />
                </div>
                <div>
                  <label>Answer to question 2</label>
                  <input value={answer2} onChange={(e) => setAnswer2(e.target.value)} required style={styles.input} />
                </div>
              </>
            ) : (
              <>
                <div style={styles.hint}>
                  The admin recovery code was shown once, to the Owner, when this business was
                  first set up — it's the ultimate fallback if security question answers are
                  also forgotten.
                </div>
                <div>
                  <label>Admin recovery code</label>
                  <input className="mono" value={adminCode} onChange={(e) => setAdminCode(e.target.value)} required style={styles.input} />
                </div>
              </>
            )}

            <div>
              <label>New password</label>
              <input type="password" value={newPassword} onChange={(e) => setNewPassword(e.target.value)} required minLength={8} style={styles.input} />
            </div>
            <div>
              <label>Confirm new password</label>
              <input type="password" value={confirmNewPassword} onChange={(e) => setConfirmNewPassword(e.target.value)} required style={styles.input} />
            </div>

            {error && <div style={styles.error}>{error}</div>}

            <button type="submit" className="btn btn-stamp" disabled={loading} style={{ width: '100%', justifyContent: 'center' }}>
              {loading ? 'Resetting…' : 'Reset password'}
            </button>
            <button type="button" onClick={() => switchMode('login')} style={styles.linkBtn}>
              Back to sign in
            </button>
          </form>
        )}

        {mode === 'recover-success' && (
          <div style={styles.form}>
            <div style={{ color: 'var(--ok)', fontWeight: 600 }}>Password reset successfully.</div>
            <p style={{ fontSize: '0.85rem', color: 'var(--ink-soft)' }}>
              You can now sign in with your new password. For your security, every device
              previously signed in to this account has been signed out.
            </p>
            <button type="button" className="btn btn-stamp" style={{ width: '100%', justifyContent: 'center' }} onClick={() => switchMode('login')}>
              Back to sign in
            </button>
          </div>
        )}
      </div>
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  wrap: { minHeight: '100vh', display: 'flex', alignItems: 'center', justifyContent: 'center', padding: '1.5rem' },
  card: { width: '100%', maxWidth: 380 },
  stampRow: { display: 'flex', alignItems: 'center', gap: '0.9rem', marginBottom: '1.4rem' },
  eyebrow: { fontSize: '0.72rem', letterSpacing: '0.08em', textTransform: 'uppercase', color: 'var(--ink-soft)' },
  form: { display: 'flex', flexDirection: 'column', gap: '0.9rem' },
  input: { width: '100%' },
  hint: { fontSize: '0.78rem', color: 'var(--ink-soft)', marginTop: '0.3rem', lineHeight: 1.4 },
  error: { background: 'var(--stamp-wash)', color: 'var(--stamp)', padding: '0.5em 0.7em', borderRadius: 3, fontSize: '0.85rem' },
  linkBtn: { background: 'none', border: 'none', color: 'var(--ink-soft)', fontSize: '0.8rem', cursor: 'pointer', textAlign: 'center', textDecoration: 'underline', padding: '0.3rem' },
  tabRow: { display: 'flex', gap: '0.5rem' },
  tabBtn: { flex: 1, fontSize: '0.8rem', padding: '0.4em 0.5em' },
};
