import { useEffect, useState } from 'react';

// These imports only resolve inside the actual Tauri app (they call into
// the Rust plugins registered in main.rs) — this component is a no-op
// when the frontend is loaded in a plain browser during web development,
// since `check()` will simply reject and we swallow that silently.
export default function UpdateChecker() {
  const [available, setAvailable] = useState<{ version: string; body?: string } | null>(null);
  const [installing, setInstalling] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    (async () => {
      try {
        const { check } = await import('@tauri-apps/plugin-updater');
        const update = await check();
        if (update) {
          setAvailable({ version: update.version, body: update.body });
        }
      } catch {
        // Not running inside Tauri (e.g. plain browser dev mode), or no
        // update available / update server unreachable — none of these
        // should interrupt normal use of the app.
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

  if (!available) return null;

  return (
    <div style={styles.banner} className="card">
      <div>
        <strong style={{ fontSize: '0.88rem' }}>Update available — v{available.version}</strong>
        {available.body && <div style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', marginTop: '0.2rem' }}>{available.body}</div>}
        {error && <div style={{ fontSize: '0.78rem', color: 'var(--stamp)', marginTop: '0.2rem' }}>{error}</div>}
      </div>
      <button className="btn btn-stamp" onClick={handleInstall} disabled={installing}>
        {installing ? 'Installing…' : 'Install & Restart'}
      </button>
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  banner: {
    // calc() with env(safe-area-inset-bottom) keeps this clear of
    // Android's on-screen nav bar/gesture strip — see the matching
    // comment in mobile.css for why this is needed at all. env()
    // resolves to 0 on desktop, so this is a no-op there.
    position: 'fixed', bottom: 'calc(1.6rem + env(safe-area-inset-bottom))', left: 'calc(1.6rem + env(safe-area-inset-left))', maxWidth: 360,
    display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: '1rem',
    zIndex: 30, borderColor: 'var(--stamp)',
  },
};
