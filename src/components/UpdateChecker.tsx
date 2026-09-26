import { useEffect, useState } from 'react';
import DraggableBanner from './DraggableBanner';

export default function UpdateChecker() {
  const [available, setAvailable] = useState<{ version: string; body?: string } | null>(null);
  const [installing, setInstalling] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [dismissedVersion, setDismissedVersion] = useState<string | null>(null);

  useEffect(() => {
    (async () => {
      try {
        const { check } = await import('@tauri-apps/plugin-updater');
        const update = await check();
        if (update) {
          setAvailable({ version: update.version, body: update.body });
        }
      } catch {
      }
    })();
  }, []);

  async function handleInstall() {
    setInstalling(true);
    setError(null);
    try {
      const { check } = await import('@tauri-apps/plugin-updater');
      const { relaunch } = await import('@tauri-apps/plugin-process');
      const update = await check();
      if (update) {
        await update.downloadAndInstall();
        await relaunch();
      }
    } catch {
      setError('Could not install the update. You can keep using the app normally — try again later.');
      setInstalling(false);
    }
  }

  if (!available || available.version === dismissedVersion) return null;

  return (
    <DraggableBanner className="card update-banner" style={styles.banner} onDismiss={() => setDismissedVersion(available.version)}>
      <div>
        <strong style={{ fontSize: '0.88rem' }}>Update available — v{available.version}</strong>
        {available.body && <div style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', marginTop: '0.2rem' }}>{available.body}</div>}
        {error && <div style={{ fontSize: '0.78rem', color: 'var(--stamp)', marginTop: '0.2rem' }}>{error}</div>}
      </div>
      <button className="btn btn-stamp" onClick={handleInstall} disabled={installing}>
        {installing ? 'Installing…' : 'Install & Restart'}
      </button>
    </DraggableBanner>
  );
}

const styles: Record<string, React.CSSProperties> = {
  banner: {
    position: 'fixed', bottom: 'calc(1.6rem + var(--safe-bottom))', left: 'calc(1.6rem + env(safe-area-inset-left))', maxWidth: 360,
    display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: '1rem',
    zIndex: 30, borderColor: 'var(--stamp)',
  },
};
