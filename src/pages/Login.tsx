import { useState } from 'react';
import type { FormEvent } from 'react';
import { login, setSession, ApiError } from '../api';

export default function Login({ onLoggedIn }: { onLoggedIn: () => void }) {
  const [businessId, setBusinessId] = useState(localStorage.getItem('erp_business_id') || '');
  const [username, setUsername] = useState('nia');
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    setError(null);
    setLoading(true);
    try {
      const { token } = await login(username, password, businessId);
      setSession(token, businessId);
      onLoggedIn();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not reach the server');
    } finally {
      setLoading(false);
    }
  }

  return (
    <div style={styles.wrap}>
      <div className="card" style={styles.card}>
        <div style={styles.stampRow}>
          <div className="stamp-badge" style={{ color: 'var(--stamp)', width: '3.2rem', height: '3.2rem', fontSize: '1.3rem' }}>
            MN
          </div>
          <div>
            <div style={styles.eyebrow}>SME Pro</div>
            <h1 style={{ margin: 0 }}>Sign in</h1>
          </div>
        </div>

        <form onSubmit={handleSubmit} style={styles.form}>
          <div>
            <label htmlFor="biz">Business ID</label>
            <input id="biz" value={businessId} onChange={(e) => setBusinessId(e.target.value)} required style={styles.input} />
          </div>
          <div>
            <label htmlFor="user">Username</label>
            <input id="user" value={username} onChange={(e) => setUsername(e.target.value)} required style={styles.input} />
          </div>
          <div>
            <label htmlFor="pass">Password</label>
            <input id="pass" type="password" value={password} onChange={(e) => setPassword(e.target.value)} required style={styles.input} />
          </div>

          {error && <div style={styles.error}>{error}</div>}

          <button type="submit" className="btn btn-stamp" disabled={loading} style={{ width: '100%', justifyContent: 'center', marginTop: '0.4rem' }}>
            {loading ? 'Signing in…' : 'Sign in'}
          </button>
        </form>
      </div>
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  wrap: { minHeight: '100vh', display: 'flex', alignItems: 'center', justifyContent: 'center', padding: '1.5rem' },
  card: { width: '100%', maxWidth: 360 },
  stampRow: { display: 'flex', alignItems: 'center', gap: '0.9rem', marginBottom: '1.4rem' },
  eyebrow: { fontSize: '0.72rem', letterSpacing: '0.08em', textTransform: 'uppercase', color: 'var(--ink-soft)' },
  form: { display: 'flex', flexDirection: 'column', gap: '0.9rem' },
  input: { width: '100%' },
  error: { background: 'var(--stamp-wash)', color: 'var(--stamp)', padding: '0.5em 0.7em', borderRadius: 3, fontSize: '0.85rem' },
};
