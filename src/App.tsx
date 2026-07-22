import { useEffect, useState, useCallback } from 'react';
import Login from './pages/Login';
import FirstRunSetup from './pages/FirstRunSetup';
import ModuleView from './pages/ModuleView';
import AdminPanel from './pages/AdminPanel';
import Sidebar from './components/Sidebar';
import LicenseBanner from './components/LicenseBanner';
import AiFloatingButton from './components/AiFloatingButton';
import UpdateChecker from './components/UpdateChecker';
import AndroidUpdateChecker from './components/AndroidUpdateChecker';
import { hasSession, listModules, clearSession, getSetupStatus, getBusinessInfo, getSettings, logout } from './api';
import type { ModuleListItem } from './types';

export default function App() {
  const [checkingSetup, setCheckingSetup] = useState(true);
  const [needsSetup, setNeedsSetup] = useState(false);
  const [loggedIn, setLoggedIn] = useState(hasSession());
  const [modules, setModules] = useState<ModuleListItem[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [businessName, setBusinessName] = useState('');

  // On launch, ask the backend whether this install has ever had a
  // business created — this is what decides between the first-run
  // wizard and the normal login screen. Only relevant when nobody is
  // already logged in; a returning, logged-in user skips straight past
  // this check.
  useEffect(() => {
    if (loggedIn) { setCheckingSetup(false); return; }
    getSetupStatus()
      .then((res) => setNeedsSetup(!res.has_business))
      .catch(() => setNeedsSetup(false)) // if the check itself fails, fall back to the normal login screen rather than trapping the user
      .finally(() => setCheckingSetup(false));
  }, [loggedIn]);

  const loadModules = useCallback(async () => {
    try {
      const res = await listModules();
      setModules(res.modules);
      if (!selected && res.modules.length > 0) {
        const firstEnabled = res.modules.find((m: ModuleListItem) => m.enabled);
        if (firstEnabled) setSelected(firstEnabled.id);
      }
    } catch {
      setLoadError('Could not load modules. Is the local server running?');
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (loggedIn) {
      loadModules();
      getBusinessInfo().then((info) => setBusinessName(info.name)).catch(() => {});
      getSettings().then((s) => {
        document.documentElement.dataset.theme = s.theme && s.theme !== 'ledger' ? s.theme : '';
      }).catch(() => {});
    }
  }, [loggedIn, loadModules]);

  async function handleLogout() {
    try {
      await logout();
    } catch {
      // Even if telling the server fails (e.g. it's already unreachable),
      // still clear local state below — the user should never be stuck
      // "logged in" on their own screen just because a network call failed.
    }
    clearSession();
    setLoggedIn(false);
    setSelected(null);
    setModules([]);
    document.documentElement.dataset.theme = '';
  }

  if (checkingSetup) {
    return null; // avoid a flash of the wrong screen while the check is in flight
  }

  if (needsSetup && !loggedIn) {
    return (
      <>
        <FirstRunSetup onComplete={() => { setNeedsSetup(false); setLoggedIn(true); }} />
        <UpdateChecker />
        <AndroidUpdateChecker />
      </>
    );
  }

  if (!loggedIn) {
    return (
      <>
        <Login onLoggedIn={() => setLoggedIn(true)} />
        <UpdateChecker />
        <AndroidUpdateChecker />
      </>
    );
  }

  return (
    <div style={{ display: 'flex' }}>
      <Sidebar modules={modules} selected={selected} onSelect={setSelected} businessName={businessName || '…'} />

      <main style={{ flex: 1, padding: '1.6rem 2rem', maxWidth: 980 }}>
        <div style={{ display: 'flex', justifyContent: 'flex-end', marginBottom: '0.6rem' }}>
          <button className="btn btn-outline" onClick={handleLogout} style={{ fontSize: '0.8rem' }}>Sign out</button>
        </div>

        <LicenseBanner onChange={loadModules} />

        {loadError && (
          <div className="card" style={{ borderColor: 'var(--stamp)', color: 'var(--stamp)' }}>{loadError}</div>
        )}

        {selected === '__admin__' ? (
          <AdminPanel />
        ) : selected ? (
          <ModuleView moduleId={selected} />
        ) : (
          <div className="card">No modules are enabled yet for this business.</div>
        )}
      </main>

      <AiFloatingButton />
      <UpdateChecker />
        <AndroidUpdateChecker />
    </div>
  );
}
