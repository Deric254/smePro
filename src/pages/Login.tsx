import { useEffect, useState } from 'react';
import type { FormEvent } from 'react';
import { login, setSession, clearSession, logout, getTerms, acceptTerms, getResolvedBusinessId, getPublicBranding, getSecurityQuestions, recoverViaSecurityQuestions, recoverViaAdminCode, API_BASE, ApiError } from '../api';

type Mode = 'login' | 'terms' | 'recover-questions' | 'recover-admin-code' | 'recover-success';

export default function Login({ onLoggedIn }: { onLoggedIn: () => void }) {
  const [businessId, setBusinessId] = useState(localStorage.getItem('erp_business_id') || '');
  const [businessIdKnown, setBusinessIdKnown] = useState(false);
  const [resolving, setResolving] = useState(true);
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [mode, setMode] = useState<Mode>('login');

  const [recoverUsername, setRecoverUsername] = useState('');
  const [answer1, setAnswer1] = useState('');
  const [answer2, setAnswer2] = useState('');
  const [adminCode, setAdminCode] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [confirmNewPassword, setConfirmNewPassword] = useState('');

  const [fetchedQuestions, setFetchedQuestions] = useState<{ question1: string | null; question2: string | null } | null>(null);
  const [questionsLoading, setQuestionsLoading] = useState(false);

  const [branding, setBranding] = useState<{ name: string | null; logo_url: string | null; slogan: string | null }>({ name: null, logo_url: null, slogan: null });

  const [termsText, setTermsText] = useState('');
  const [termsLoading, setTermsLoading] = useState(false);
  const [termsAccepting, setTermsAccepting] = useState(false);

  useEffect(() => {
    getPublicBranding().then(setBranding).catch(() => {});
  }, []);

  useEffect(() => {
    if (mode !== 'terms') return;
    setTermsLoading(true);
    setError(null);
    getTerms()
      .then((r) => setTermsText(r.text))
      .catch((err) => setError(err instanceof ApiError ? err.message : 'Could not load the Terms & Conditions'))
      .finally(() => setTermsLoading(false));
  }, [mode]);

  function completeLogin(token: string, termsAccepted: boolean) {
    setSession(token, businessId);
    if (termsAccepted) {
      onLoggedIn();
    } else {
      setMode('terms');
    }
  }

  async function handleAcceptTerms() {
    setError(null);
    setTermsAccepting(true);
    try {
      await acceptTerms();
      onLoggedIn();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not record your acceptance — please try again');
    } finally {
      setTermsAccepting(false);
    }
  }

  function handleDeclineTerms() {
    logout().catch(() => {});
    clearSession();
    switchMode('login');
  }

  useEffect(() => {
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
      completeLogin(result.token, result.terms_accepted);
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not reach the server');
    } finally {
      setLoading(false);
    }
  }

  function resetRecoveryFields() {
    setRecoverUsername(''); setAnswer1(''); setAnswer2(''); setAdminCode('');
    setNewPassword(''); setConfirmNewPassword(''); setError(null);
    setFetchedQuestions(null); setQuestionsLoading(false);
  }

  function switchMode(next: Mode) {
    resetRecoveryFields();
    setMode(next);
  }

  async function handleFindQuestions() {
    setError(null);
    setQuestionsLoading(true);
    try {
      const result = await getSecurityQuestions(businessId, recoverUsername);
      setFetchedQuestions(result);
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not look up security questions');
    } finally {
      setQuestionsLoading(false);
    }
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
          {branding.logo_url ? (
            <img
              src={`${API_BASE}${branding.logo_url}`}
              alt={branding.name || 'Business logo'}
              style={{ width: '3.2rem', height: '3.2rem', objectFit: 'contain' }}
            />
          ) : (
            <div className="stamp-badge" style={{ color: 'var(--stamp)', width: '3.2rem', height: '3.2rem', fontSize: '1.3rem' }}>
              SP
            </div>
          )}
          <div>
            <div style={styles.eyebrow}>{branding.name || 'SME Pro'}</div>
            <h1 style={{ margin: 0 }}>{mode === 'login' ? 'Sign in' : mode === 'terms' ? 'Terms & Conditions' : 'Reset your password'}</h1>
            {branding.slogan && mode === 'login' && (
              <div style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', fontStyle: 'italic', marginTop: '0.15rem' }}>{branding.slogan}</div>
            )}
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

        {mode === 'terms' && (
          <div style={styles.form}>
            <div style={styles.hint}>
              Please review and accept the Terms &amp; Conditions to continue.
            </div>
            <div style={styles.termsBox}>
              {termsLoading ? 'Loading…' : termsText}
            </div>

            {error && <div style={styles.error}>{error}</div>}

            <button
              type="button"
              className="btn btn-stamp"
              disabled={termsLoading || termsAccepting || !termsText}
              onClick={handleAcceptTerms}
              style={{ width: '100%', justifyContent: 'center' }}
            >
              {termsAccepting ? 'Recording…' : 'I accept'}
            </button>
            <button type="button" onClick={handleDeclineTerms} style={styles.linkBtn}>
              Not now — log out
            </button>
          </div>
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
              <input
                value={recoverUsername}
                onChange={(e) => { setRecoverUsername(e.target.value); setFetchedQuestions(null); }}
                required
                style={styles.input}
              />
            </div>

            {mode === 'recover-questions' ? (
              fetchedQuestions === null ? (
                <>
                  <div style={styles.hint}>Enter your username, then we'll show you the security questions you set up.</div>
                  {error && <div style={styles.error}>{error}</div>}
                  <button
                    type="button"
                    className="btn btn-stamp"
                    disabled={questionsLoading || !recoverUsername.trim()}
                    onClick={handleFindQuestions}
                    style={{ width: '100%', justifyContent: 'center' }}
                  >
                    {questionsLoading ? 'Looking up…' : 'Find my account'}
                  </button>
                </>
              ) : (
                <>
                  <div style={styles.hint}>Answer both security questions you set up when this account was created.</div>
                  <div>
                    <label>{fetchedQuestions.question1 || 'Answer to question 1'}</label>
                    <input value={answer1} onChange={(e) => setAnswer1(e.target.value)} required style={styles.input} autoFocus />
                  </div>
                  <div>
                    <label>{fetchedQuestions.question2 || 'Answer to question 2'}</label>
                    <input value={answer2} onChange={(e) => setAnswer2(e.target.value)} required style={styles.input} />
                  </div>
                </>
              )
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

            {(mode === 'recover-admin-code' || fetchedQuestions !== null) && (
              <>
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
              </>
            )}
            <button type="button" onClick={() => switchMode('login')} style={styles.linkBtn}>
              Back to sign in
            </button>
          </form>
        )}

        {mode === 'recover-success' && (
          <div style={styles.form}>
            <div style={{ color: 'var(--ok)', fontWeight: 600 }}>Password reset successfully.</div>
            <p style={{ fontSize: '0.85rem', color: 'var(--ink-soft)' }}>
              All devices have been signed out.
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
  termsBox: {
    maxHeight: '45vh',
    overflowY: 'auto',
    whiteSpace: 'pre-wrap',
    fontSize: '0.8rem',
    lineHeight: 1.5,
    padding: '0.8em 0.9em',
    border: '1px solid var(--paper-line)',
    borderRadius: 4,
    background: 'var(--paper)',
  },
  error: { background: 'var(--stamp-wash)', color: 'var(--stamp)', padding: '0.5em 0.7em', borderRadius: 3, fontSize: '0.85rem' },
  linkBtn: { background: 'none', border: 'none', color: 'var(--ink-soft)', fontSize: '0.8rem', cursor: 'pointer', textAlign: 'center', textDecoration: 'underline', padding: '0.3rem' },
  tabRow: { display: 'flex', gap: '0.5rem' },
  tabBtn: { flex: 1, fontSize: '0.8rem', padding: '0.4em 0.5em' },
};
