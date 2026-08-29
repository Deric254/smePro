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
import StockTakePage from './pages/StockTake';
import Sidebar from './components/Sidebar';
import AiFloatingButton from './components/AiFloatingButton';
import UpdateChecker from './components/UpdateChecker';
import AndroidUpdateChecker from './components/AndroidUpdateChecker';
import { hasSession, listModules, clearSession, getSetupStatus, getBusinessInfo, getSettings, logout } from './api';
import type { ModuleListItem } from './types';
import { retryOnConnectionFailure } from './lib/retry';

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
    retryOnConnectionFailure(() => getSetupStatus())
      .then((res) => setNeedsSetup(!res.has_business))
      .catch(() => setNeedsSetup(false)) // if the check itself fails, fall back to the normal login screen rather than trapping the user
      .finally(() => setCheckingSetup(false));
  }, [loggedIn]);

  const loadModules = useCallback(async () => {
    try {
      const res = await retryOnConnectionFailure(() => listModules());
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
      retryOnConnectionFailure(() => getBusinessInfo()).then((info) => setBusinessName(info.name)).catch(() => {});
      getSettings().then((s) => {
        document.documentElement.dataset.theme = s.theme && s.theme !== 'ledger' ? s.theme : '';
      }).catch(() => {}); // purely cosmetic (custom theme vs the default) — not worth retrying against the startup race the other three calls above guard against
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

      {/* maxWidth was 980 — far narrower than the 1920px-class monitor
          these screenshots came from. Every grid on this page (the
          KPI row, the two-chart row, the module tiles) is already
          `repeat(auto-fit/auto-fill, minmax(...))` — capping the
          available width that low forces them to wrap into more ROWS
          than the actual screen has room for side by side, which is
          the real reason the Dashboard needed scrolling: not the
          padding, the wasted horizontal space. Widening this doesn't
          change a single number or a single query — it just lets the
          same grids that were already responsive actually use the
          width they have. Padding also trimmed slightly (1.6/2rem →
          1.2/1.6rem) — comfortable, not cramped, but no longer eating
          more vertical space than the content itself needs. */}
      <main className="app-main-content" style={{ flex: 1, padding: '1.2rem 1.6rem', maxWidth: 1400 }}>
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
          <AdminPanel tab={adminTab} onModulesChanged={loadModules} />
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
          modules.some((m) => m.id === 'inventory' && m.enabled) ? (
            <PointOfSale
              onNavigateToBranding={() => {
                setAdminTab('business');
                setSelected('__admin__');
              }}
            />
          ) : (
            <ServiceSale />
          )
        ) : selected === '__customers__' ? (
          <Customers />
        ) : selected === '__stocktake__' ? (
          <StockTakePage />
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

      {/* Fixed to the viewport, not scrolled with the page content —
          a sibling of <main>, not a child of it, is what makes that
          possible. Purely additive alongside the hamburger drawer
          above: every one of these four is already reachable from
          there too (Home, Sell, and Customers are the drawer's own
          first three items; Ask AI is the same badge already sitting
          in the topbar) — this doesn't replace or restructure any of
          that navigation, it just puts the handful of things someone
          reaches for constantly within one tap, without opening the
          drawer at all, on the platform where that drawer is the
          only other way to navigate at all. Hidden entirely on
          desktop (see the plain, non-media-query `.app-bottom-tabbar
          { display: none }` rule in index.css) — the sidebar there is
          already permanently visible, so there's no gap for this to
          fill. */}
      <nav className="app-bottom-tabbar">
        <button className={!selected ? 'active' : ''} onClick={() => setSelected('')} aria-label="Home">
          <span className="tab-icon" aria-hidden>⌂</span>
          Home
        </button>
        <button className={selected === '__pos__' ? 'active' : ''} onClick={() => setSelected('__pos__')} aria-label="Sell">
          <span className="tab-icon" aria-hidden>$</span>
          Sell
        </button>
        <button className={selected === '__customers__' ? 'active' : ''} onClick={() => setSelected('__customers__')} aria-label="Customers">
          <span className="tab-icon" aria-hidden>♥</span>
          Customers
        </button>
        <button className={aiOpen ? 'active' : ''} onClick={() => setAiOpen(true)} aria-label="Ask AI">
          <span className="tab-icon" aria-hidden>AI</span>
          Ask AI
        </button>
      </nav>

      <AiFloatingButton open={aiOpen} onClose={() => setAiOpen(false)} />
      <UpdateChecker />
        <AndroidUpdateChecker />
    </div>
  );
}
