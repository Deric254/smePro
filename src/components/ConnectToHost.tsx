import { useState } from 'react';
import { apiBaseForHost } from '../api';

// Shown before App ever mounts, when this device is configured as a
// LAN "client" (see network_mode.rs) but hasn't been told which
// device to connect to yet — entering "connect to another device" in
// Admin → Network without immediately also giving it an address would
// otherwise leave the app with nowhere to send its very first request.
export default function ConnectToHost({ onConnected }: { onConnected: (address: string) => void }) {
  const [address, setAddress] = useState('');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleConnect(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = address.trim();
    if (!trimmed) return;
    const normalized = apiBaseForHost(trimmed);
    setSaving(true);
    setError(null);
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      // host_address is stored WITHOUT a scheme (just "ip:port") —
      // setApiBase (called by main.tsx on the next launch, and by
      // onConnected right now) is what adds "http://", so there's
      // exactly one place that decides the scheme rather than two
      // copies that could drift.
      await invoke('set_network_mode', { mode: 'client', hostAddress: normalized.replace(/^https?:\/\//i, '') });
      onConnected(normalized);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Could not save this — try again');
      setSaving(false);
    }
  }

  return (
    <div style={styles.wrap}>
      <div className="card" style={styles.card}>
        <h2 style={{ marginTop: 0 }}>Connect to your business</h2>
        <p style={{ color: 'var(--ink-soft)', fontSize: '0.9rem' }}>
          This device is set to connect to another device on your network instead of running its own copy.
          On the host device, open Admin → Network to see the address to enter here.
        </p>
        <form onSubmit={handleConnect}>
          <label>Host address</label>
          <input
            value={address}
            onChange={(e) => setAddress(e.target.value)}
            placeholder="192.168.1.42:8080"
            style={{ width: '100%' }}
            autoFocus
          />
          {error && <p style={{ color: 'var(--stamp)', fontSize: '0.85rem' }}>{error}</p>}
          <button className="btn btn-stamp" type="submit" disabled={saving} style={{ marginTop: '0.8rem', width: '100%' }}>
            {saving ? 'Connecting…' : 'Connect'}
          </button>
        </form>
      </div>
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  wrap: { display: 'flex', alignItems: 'center', justifyContent: 'center', minHeight: '100vh', padding: '1.5rem' },
  card: { maxWidth: 380, width: '100%' },
};
