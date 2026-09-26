import { useEffect, useState } from 'react';
import DraggableBanner from './DraggableBanner';

// Android-only auto-update path (desktop uses UpdateChecker.tsx +
// tauri-plugin-updater, which has no Android support). Built on
// http + fs + a custom install_apk command (see installer.rs).
//
// Untested on a real device (no Android SDK/emulator in this sandbox
// — see MOBILE.md/RELEASE.md). If install_apk fails, check:
// InstallerPlugin.kt's FileProvider authority matches applicationId,
// and whether appCacheDir() resolves to the right cache dir on-device.

type ReleaseAsset = { name: string; browser_download_url: string };
type ReleaseInfo = { tag_name: string; assets: ReleaseAsset[]; body?: string };

const REPO = 'Deric254/smePro';

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
  // Same version-scoped, session-only dismiss as UpdateChecker.tsx —
  // see that file's comment for why.
  const [dismissedVersion, setDismissedVersion] = useState<string | null>(null);

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
      const { invoke } = await import('@tauri-apps/api/core');

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
      // THE FIX: openPath() used to be called here, but on Android it
      // hands the package installer a raw file:// path — Android has
      // refused to let one app pass that to another since API 24, so
      // this silently failed after a full, successful download (see
      // installer.rs's doc comment for the full story, including the
      // still-open upstream issue confirming openPath() itself can't
      // do this). install_apk is a regular command (see lib.rs) that
      // hands off to InstallerPlugin.kt, which does the FileProvider
      // handoff that actually works. This is still the same point
      // where Android takes over with its own "Update this app?"
      // confirmation screen; that confirmation tap is a real OS
      // security requirement for any app not installed through the
      // Play Store, not something that can be skipped from here.
      await invoke('install_apk', { path: apkPath });
      setStatus('idle');
      setRelease(null);
    } catch (e) {
      setStatus('error');
      setError(e instanceof Error ? e.message : 'Could not download or install the update');
    }
  }

  if (!isAndroid || !release || release.tag_name === dismissedVersion) return null;

  return (
    <DraggableBanner className="card update-banner" style={styles.banner} onDismiss={() => setDismissedVersion(release.tag_name)}>
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
    </DraggableBanner>
  );
}

const styles: Record<string, React.CSSProperties> = {
  banner: {
    // Base (desktop / no in-app tab bar) position — see the matching
    // comment in UpdateChecker.tsx. On phone widths, .update-banner in
    // mobile.css overrides `bottom` to also clear the app's own bottom
    // tab bar. This banner is Android-only (see the component name),
    // so that phone-width override is not a hypothetical edge case —
    // it's THE case, every time this actually renders.
    position: 'fixed', bottom: 'calc(1.6rem + var(--safe-bottom))',
    left: 'calc(1.6rem + env(safe-area-inset-left))', right: 'calc(1.6rem + env(safe-area-inset-right))', maxWidth: 420,
    display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: '1rem',
    zIndex: 30, borderColor: 'var(--stamp)',
  },
};
