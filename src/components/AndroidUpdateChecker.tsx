import { useEffect, useState } from 'react';

// Everything in this file only does anything on Android — every other
// platform's early-return on the platform() check makes this a no-op.
// Desktop already has real auto-update via UpdateChecker.tsx +
// tauri-plugin-updater, which has no Android/iOS implementation at all,
// which is the whole reason this separate, Android-specific path
// exists: it's built from lower-level pieces (http + fs + opener)
// instead of the higher-level updater plugin.
//
// UNTESTED ON A REAL DEVICE — built in a sandbox with no Android SDK or
// emulator available (see MOBILE.md / RELEASE.md for the full context).
// Every piece here is real, documented Tauri v2 plugin usage, not
// invented API — but "compiles against the documented API" and
// "verified working on an actual phone" are different claims, and only
// the first one is true yet. If this doesn't work first try, the most
// likely culprit is the capabilities/default.json permission scopes
// (see the comment there) or the AndroidManifest FileProvider wiring
// (see mobile-android.sh / the CI workflow's "Apply Android manifest
// additions" step) — check those first before assuming the JS logic
// below is wrong.

type ReleaseAsset = { name: string; browser_download_url: string };
type ReleaseInfo = { tag_name: string; assets: ReleaseAsset[]; body?: string };

const REPO = 'Deric254/UniversalSME';

function isNewer(latest: string, current: string): boolean {
  const parse = (v: string) => v.replace(/^v/, '').split('.').map((n) => parseInt(n, 10) || 0);
  const [lMaj, lMin, lPatch] = parse(latest);
  const [cMaj, cMin, cPatch] = parse(current);
  if (lMaj !== cMaj) return lMaj > cMaj;
  if (lMin !== cMin) return lMin > cMin;
  return lPatch > cPatch;
}

export default function AndroidUpdateChecker() {
  const [isAndroid, setIsAndroid] = useState(false);
  const [release, setRelease] = useState<ReleaseInfo | null>(null);
  const [status, setStatus] = useState<'idle' | 'downloading' | 'installing' | 'error'>('idle');
  const [error, setError] = useState<string | null>(null);
  const [progress, setProgress] = useState(0);

  useEffect(() => {
    (async () => {
      try {
        const { platform } = await import('@tauri-apps/plugin-os');
        if (platform() !== 'android') return;
        setIsAndroid(true);

        const { getVersion } = await import('@tauri-apps/api/app');
        const { fetch: tauriFetch } = await import('@tauri-apps/plugin-http');

        const currentVersion = await getVersion();
        const res = await tauriFetch(`https://api.github.com/repos/${REPO}/releases/latest`, {
          headers: { Accept: 'application/vnd.github+json' },
        });
        if (!res.ok) return; // no releases yet, or offline — fail silently, not an error state
        const info: ReleaseInfo = await res.json();

        if (isNewer(info.tag_name, currentVersion) && info.assets.some((a) => a.name.endsWith('.apk'))) {
          setRelease(info);
        }
      } catch {
        // Not running inside Tauri (plain browser dev mode), offline,
        // or the GitHub API is unreachable — none of these should
        // interrupt normal use of the app.
      }
    })();
  }, []);

  async function handleUpdate() {
    if (!release) return;
    const apkAsset = release.assets.find((a) => a.name.endsWith('.apk'));
    if (!apkAsset) return;

    setStatus('downloading');
    setError(null);
    setProgress(0);

    try {
      const { fetch: tauriFetch } = await import('@tauri-apps/plugin-http');
      const { writeFile, mkdir, exists } = await import('@tauri-apps/plugin-fs');
      const { appCacheDir, join } = await import('@tauri-apps/api/path');
      const { openPath } = await import('@tauri-apps/plugin-opener');

      const res = await tauriFetch(apkAsset.browser_download_url);
      if (!res.ok || !res.body) throw new Error(`Download failed (HTTP ${res.status})`);

      // Stream + report progress rather than one big buffered await —
      // an APK is tens of MB on mobile data, a silent multi-minute
      // freeze with no feedback is a bad experience even if it would
      // eventually finish.
      const total = Number(res.headers.get('content-length') || 0);
      const reader = res.body.getReader();
      const chunks: Uint8Array[] = [];
      let received = 0;
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        chunks.push(value);
        received += value.length;
        if (total > 0) setProgress(Math.round((received / total) * 100));
      }
      const bytes = new Uint8Array(received);
      let offset = 0;
      for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }

      const cacheDir = await appCacheDir();
      if (!(await exists(cacheDir))) await mkdir(cacheDir, { recursive: true });
      const apkPath = await join(cacheDir, apkAsset.name);
      await writeFile(apkPath, bytes);

      setStatus('installing');
      // Hands off to Android's own package installer via FileProvider —
      // this is the point where Android takes over with its own "Update
      // this app?" confirmation screen. That confirmation tap is a real
      // OS security requirement for any app not installed through the
      // Play Store, not something that can be skipped from here.
      await openPath(apkPath);
      setStatus('idle');
      setRelease(null);
    } catch (e) {
      setStatus('error');
      setError(e instanceof Error ? e.message : 'Could not download or install the update');
    }
  }

  if (!isAndroid || !release) return null;

  return (
    <div style={styles.banner} className="card">
      <div style={{ flex: 1 }}>
        <strong style={{ fontSize: '0.88rem' }}>Update available — {release.tag_name}</strong>
        {status === 'downloading' && (
          <div style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', marginTop: '0.2rem' }}>
            Downloading… {progress > 0 ? `${progress}%` : ''}
          </div>
        )}
        {status === 'installing' && (
          <div style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', marginTop: '0.2rem' }}>
            Opening the installer — tap "Update" on the screen that appears.
          </div>
        )}
        {error && <div style={{ fontSize: '0.78rem', color: 'var(--stamp)', marginTop: '0.2rem' }}>{error}</div>}
      </div>
      <button className="btn btn-stamp" onClick={handleUpdate} disabled={status === 'downloading' || status === 'installing'}>
        {status === 'downloading' ? 'Downloading…' : status === 'installing' ? 'Installing…' : 'Update'}
      </button>
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  banner: {
    position: 'fixed', bottom: '1.6rem', left: '1.6rem', right: '1.6rem', maxWidth: 420,
    display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: '1rem',
    zIndex: 30, borderColor: 'var(--stamp)',
  },
};
