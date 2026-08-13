import { useEffect, useState, useCallback } from 'react';
import Login from './pages/Login';
import FirstRunSetup from './pages/FirstRunSetup';
import ModuleView from './pages/ModuleView';
import AdminPanel from './pages/AdminPanel';
import type { Tab as AdminTab } from './pages/AdminPanel';
import Dashboard from './pages/Dashboard';
import PointOfSale from './pages/PointOfSale';
import ServiceSale from './pages/ServiceSale';
import Customers from './pages/Customers';
import Sidebar from './components/Sidebar';
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
  const [mobileMenuOpen, setMobileMenuOpen] = useState(false);
  const [aiOpen, setAiOpen] = useState(false);
  // Which Admin section is showing — now driven entirely by the
  // sidebar's own collapsible "Admin" group (the same pattern as
  // "Operations"), not an internal tab-strip inside AdminPanel
  // itself. Defaults to 'roles', matching what AdminPanel used to
  // default to on its own.
  const [adminTab, setAdminTab] = useState<AdminTab>('roles');

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
      // Deliberately NOT auto-selecting the first module anymore — a
      // brand new user landing straight in an arbitrary module's raw
      // data table, with no context on what it is or what to do, was
      // the single biggest "directionless" complaint. `selected` stays
      // null, which renders the Dashboard below instead.
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
      <Sidebar
        modules={modules}
        selected={selected}
        onSelect={setSelected}
        businessName={businessName || '…'}
        mobileOpen={mobileMenuOpen}
        onCloseMobile={() => setMobileMenuOpen(false)}
        onSignOut={handleLogout}
        adminTab={adminTab}
        onSelectAdminTab={setAdminTab}
        onOpenAi={() => setAiOpen(true)}
      />
      <div className={`app-sidebar-backdrop${mobileMenuOpen ? ' mobile-open' : ''}`} onClick={() => setMobileMenuOpen(false)} />

      <main className="app-main-content" style={{ flex: 1, padding: '1.6rem 2rem', maxWidth: 980 }}>
        <div className="app-mobile-topbar">
          <button className="app-hamburger" onClick={() => setMobileMenuOpen(true)} aria-label="Open menu">☰</button>
          <div style={{ fontWeight: 600, flex: 1 }}>{businessName || 'SME Pro'}</div>
          <button
            onClick={() => setAiOpen(true)}
            aria-label="Ask AI"
            style={{ background: 'none', border: 'none', padding: 0, cursor: 'pointer' }}
          >
            <span className="stamp-badge" style={{ width: '2rem', height: '2rem', fontSize: '0.65rem', color: 'var(--ink-faint)' }}>AI</span>
          </button>
        </div>

        {loadError && (
          <div className="card" style={{ borderColor: 'var(--stamp)', color: 'var(--stamp)' }}>{loadError}</div>
        )}

        {selected === '__admin__' ? (
          <AdminPanel tab={adminTab} />
        ) : selected === '__pos__' ? (
          // PointOfSale's checkout hard-requires every line to
          // reference a real inventory record (see pos.rs) — it
          // fundamentally cannot work for a business with no
          // Inventory module enabled (services, consulting, anything
          // without stock). ServiceSale writes to the exact same
          // "sales" table through the plain generic create endpoint
          // instead, with no inventory dependency, and still gets a
          // real receipt out of it (receipt.rs only cares about the
          // shared order_id, not how the rows were created).
          modules.some((m) => m.id === 'inventory' && m.enabled) ? <PointOfSale /> : <ServiceSale />
        ) : selected === '__customers__' ? (
          <Customers />
        ) : selected ? (
          <ModuleView moduleId={selected} />
        ) : (
          <Dashboard
            businessName={businessName}
            onSelectModule={setSelected}
            onOpenAdmin={() => setSelected('__admin__')}
          />
        )}
      </main>

      <AiFloatingButton open={aiOpen} onClose={() => setAiOpen(false)} />
      <UpdateChecker />
        <AndroidUpdateChecker />
    </div>
  );
}
