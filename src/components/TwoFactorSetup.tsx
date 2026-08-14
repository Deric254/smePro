import { useState, useEffect } from 'react';
import { getToken, API_BASE as API } from '../api';

export default function TwoFactorSetup() {
  const [status, setStatus] = useState<{enabled:boolean} | null>(null);
  const [setupData, setSetupData] = useState<{secret:string; qr_uri:string; recovery_codes:string[]} | null>(null);
  const [code, setCode] = useState('');
  const [loading, setLoading] = useState(false);
  const [message, setMessage] = useState('');
  const [step, setStep] = useState<'start' | 'verify' | 'done'>('start');

  // Disabling requires the CURRENT TOTP code — same rule the backend
  // enforces (totp.rs: "Disabling 2FA requires the current TOTP code,
  // not just password"), kept as a separate flow/state from setup
  // since it's a different action entirely, not a continuation of it.
  const [showDisable, setShowDisable] = useState(false);
  const [disableCode, setDisableCode] = useState('');
  const [disabling, setDisabling] = useState(false);
  const [disableMessage, setDisableMessage] = useState('');

  useEffect(() => {
    fetch(`${API}/auth/2fa/status`, {
      cache: 'no-store', headers: { Authorization: `Bearer ${getToken()}` } })
      .then(r => r.json())
      .then(setStatus);
  }, []);

  const handleDisable = async () => {
    setDisabling(true);
    setDisableMessage('');
    try {
      const res = await fetch(`${API}/auth/2fa/disable`, {
      cache: 'no-store',
        method: 'POST',
        headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${getToken()}` },
        body: JSON.stringify({ code: disableCode }),
      });
      const data = await res.json();
      if (data.error) throw new Error(data.error);
      setStatus({ enabled: false });
      setShowDisable(false);
      setDisableCode('');
      setStep('start');
      setMessage('2FA has been disabled for your account.');
    } catch (e: any) {
      setDisableMessage(e.message);
    } finally {
      setDisabling(false);
    }
  };

  const handleStart = async () => {
    setLoading(true);
    try {
      const res = await fetch(`${API}/auth/2fa/setup`, {
      cache: 'no-store',
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
      cache: 'no-store',
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
          <p style={{ fontSize: 13, color: '#555', marginTop: 8, marginBottom: 16 }}>
            Your account is protected with an authenticator app.
          </p>
          {!showDisable ? (
            <button
              onClick={() => setShowDisable(true)}
              style={{
                padding: '8px 16px', borderRadius: 8, border: '1px solid #c0392b', background: '#fff',
                color: '#c0392b', fontSize: 13, fontWeight: 600, cursor: 'pointer',
              }}
            >
              Disable 2FA
            </button>
          ) : (
            <div style={{ padding: 16, background: '#fdeaea', borderRadius: 8, border: '1px solid #e0a0a0' }}>
              <p style={{ fontSize: 13, color: '#7a2020', marginBottom: 10 }}>
                Enter your current 6-digit authenticator code to confirm.
              </p>
              {disableMessage && <div style={{ fontSize: 12, color: '#c0392b', marginBottom: 8 }}>{disableMessage}</div>}
              <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                <input
                  type="text"
                  value={disableCode}
                  onChange={e => setDisableCode(e.target.value.replace(/\D/g, '').slice(0, 6))}
                  placeholder="000000"
                  style={{
                    width: 100, padding: '8px 10px', borderRadius: 8, border: '1px solid #ccc',
                    fontSize: 16, fontFamily: 'monospace', textAlign: 'center', letterSpacing: 3,
                  }}
                />
                <button
                  onClick={handleDisable}
                  disabled={disabling || disableCode.length !== 6}
                  style={{
                    padding: '8px 16px', borderRadius: 8, border: 'none', background: '#c0392b',
                    color: '#fff', fontSize: 13, fontWeight: 600,
                    opacity: disableCode.length !== 6 ? 0.5 : 1, cursor: disableCode.length !== 6 ? 'not-allowed' : 'pointer',
                  }}
                >
                  {disabling ? 'Disabling…' : 'Confirm disable'}
                </button>
                <button
                  onClick={() => { setShowDisable(false); setDisableCode(''); setDisableMessage(''); }}
                  style={{ padding: '8px 12px', borderRadius: 8, border: 'none', background: 'transparent', fontSize: 13, cursor: 'pointer' }}
                >
                  Cancel
                </button>
              </div>
            </div>
          )}
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
