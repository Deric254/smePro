import { useState, useEffect } from 'react';
import { getToken } from '../api';

const API = 'http://127.0.0.1:8080';

export default function TwoFactorSetup() {
  const [status, setStatus] = useState<{enabled:boolean} | null>(null);
  const [setupData, setSetupData] = useState<{secret:string; qr_uri:string; recovery_codes:string[]} | null>(null);
  const [code, setCode] = useState('');
  const [loading, setLoading] = useState(false);
  const [message, setMessage] = useState('');
  const [step, setStep] = useState<'start' | 'verify' | 'done'>('start');

  useEffect(() => {
    fetch(`${API}/auth/2fa/status`, { headers: { Authorization: `Bearer ${getToken()}` } })
      .then(r => r.json())
      .then(setStatus);
  }, []);

  const handleStart = async () => {
    setLoading(true);
    try {
      const res = await fetch(`${API}/auth/2fa/setup`, {
        method: 'POST',
        headers: { Authorization: `Bearer ${getToken()}` },
      });
      const data = await res.json();
      if (data.error) throw new Error(data.error);
      setSetupData(data);
      setStep('verify');
    } catch (e: any) {
      setMessage(e.message);
    } finally {
      setLoading(false);
    }
  };

  const handleVerify = async () => {
    setLoading(true);
    try {
      const res = await fetch(`${API}/auth/2fa/verify`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${getToken()}` },
        body: JSON.stringify({ code }),
      });
      const data = await res.json();
      if (data.error) throw new Error(data.error);
      setStatus({ enabled: true });
      setStep('done');
      setMessage('2FA enabled successfully! Save your recovery codes.');
    } catch (e: any) {
      setMessage(e.message);
    } finally {
      setLoading(false);
    }
  };

  if (!status) return <div>Loading…</div>;

  return (
    <div style={{ maxWidth: 480, padding: 20 }}>
      <h2 style={{ fontFamily: "Newsreader, Georgia, serif", fontSize: 22, marginBottom: 16 }}>
        Two-Factor Authentication
      </h2>

      {message && (
        <div style={{
          padding: '10px 14px', borderRadius: 8, marginBottom: 16,
          background: message.includes('Error') || message.includes('invalid') ? '#fdeaea' : '#eafaf1',
          color: message.includes('Error') || message.includes('invalid') ? '#c0392b' : '#27ae60',
          fontSize: 13,
        }}>{message}</div>
      )}

      {status.enabled ? (
        <div>
          <p style={{ color: '#27ae60', fontWeight: 600 }}>✓ 2FA is enabled</p>
          <p style={{ fontSize: 13, color: '#555', marginTop: 8 }}>
            Your account is protected with an authenticator app.
          </p>
        </div>
      ) : step === 'start' ? (
        <div>
          <p style={{ fontSize: 13, color: '#555', marginBottom: 16 }}>
            Add an extra layer of security. You'll need a code from your authenticator app every time you sign in.
          </p>
          <button
            onClick={handleStart}
            disabled={loading}
            style={{
              padding: '10px 20px', borderRadius: 8, border: 'none', background: '#1a1a1a',
              color: '#fff', fontSize: 13, fontWeight: 600, cursor: loading ? 'not-allowed' : 'pointer',
            }}
          >
            {loading ? 'Setting up…' : 'Set up 2FA'}
          </button>
        </div>
      ) : step === 'verify' && setupData ? (
        <div>
          <p style={{ fontSize: 13, marginBottom: 12 }}>
            <strong>Step 1:</strong> Scan this QR code with your authenticator app (Google Authenticator, Authy, etc.)
          </p>
          <div style={{
            padding: 16, background: '#fff', border: '1px solid #ddd', borderRadius: 8,
            marginBottom: 16, wordBreak: 'break-all', fontSize: 12, fontFamily: 'monospace',
          }}>
            {setupData.qr_uri}
          </div>
          <p style={{ fontSize: 13, marginBottom: 8 }}>
            <strong>Step 2:</strong> Enter the 6-digit code from your app
          </p>
          <input
            type="text"
            value={code}
            onChange={e => setCode(e.target.value.replace(/\D/g, '').slice(0, 6))}
            placeholder="000000"
            style={{
              width: 120, padding: '10px 12px', borderRadius: 8, border: '1px solid #ccc',
              fontSize: 18, fontFamily: 'monospace', textAlign: 'center', letterSpacing: 4,
              marginBottom: 16,
            }}
          />
          <div>
            <button
              onClick={handleVerify}
              disabled={loading || code.length !== 6}
              style={{
                padding: '10px 20px', borderRadius: 8, border: 'none', background: '#1a1a1a',
                color: '#fff', fontSize: 13, fontWeight: 600,
                opacity: code.length !== 6 ? 0.5 : 1, cursor: code.length !== 6 ? 'not-allowed' : 'pointer',
              }}
            >
              {loading ? 'Verifying…' : 'Enable 2FA'}
            </button>
          </div>

          <div style={{ marginTop: 24, padding: 16, background: '#fffbeb', borderRadius: 8, border: '1px solid #f59e0b' }}>
            <p style={{ fontSize: 12, fontWeight: 600, color: '#92400e', marginBottom: 8 }}>
              ⚠️ Save these recovery codes now
            </p>
            <p style={{ fontSize: 12, color: '#78350f', marginBottom: 8 }}>
              If you lose your phone, these codes are the only way to recover your account. Each code can only be used once.
            </p>
            <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 6 }}>
              {setupData.recovery_codes.map((c, i) => (
                <code key={i} style={{
                  fontSize: 12, fontFamily: 'monospace', padding: '4px 8px',
                  background: '#fff', borderRadius: 4, border: '1px solid #fcd34d',
                }}>{c}</code>
              ))}
            </div>
          </div>
        </div>
      ) : (
        <div>
          <p style={{ color: '#27ae60', fontWeight: 600 }}>✓ 2FA is now enabled!</p>
          <p style={{ fontSize: 13, color: '#555', marginTop: 8 }}>
            You'll be asked for a code from your authenticator app at your next login.
          </p>
        </div>
      )}
    </div>
  );
}
