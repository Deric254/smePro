import { useState } from 'react';
import type { ModuleListItem } from '../types';
import type { Tab as AdminTab } from '../pages/AdminPanel';
import { ADMIN_TABS } from '../pages/AdminPanel';
import AccountMenu from './AccountMenu';

function initials(name: string) {
  const words = name.split(/[\s/]+/).filter((w) => /[a-zA-Z]/.test(w));
  if (words.length === 0) return '?';
  if (words.length === 1) return words[0].slice(0, 2).toUpperCase();
  return (words[0][0] + words[1][0]).toUpperCase();
}

export default function Sidebar({
  modules,
  selected,
  onSelect,
  businessName,
  mobileOpen = false,
  onCloseMobile,
  onSignOut,
  adminTab,
  onSelectAdminTab,
}: {
  modules: ModuleListItem[];
  selected: string | null;
  onSelect: (id: string) => void;
  businessName: string;
  mobileOpen?: boolean;
  onCloseMobile?: () => void;
  onSignOut: () => void;
  adminTab: AdminTab;
  onSelectAdminTab: (tab: AdminTab) => void;
}) {
  // Remembers whether "Operations" is expanded across visits — a
  // once-off collapse shouldn't reset itself every time the app opens.
  const [operationsOpen, setOperationsOpen] = useState(() => {
    try { return localStorage.getItem('sidebar_operations_open') !== 'false'; } catch { return true; }
  });

  function toggleOperations() {
    setOperationsOpen((prev) => {
      const next = !prev;
      try { localStorage.setItem('sidebar_operations_open', String(next)); } catch { /* not critical */ }
      return next;
    });
  }

  // Same collapsible pattern as "Operations" above, for Admin — this
  // is what "part of the sidebar" actually means: Admin's own
  // sections navigate from here now, not from a tab-strip that used
  // to eat the whole top of the screen on a phone before any content
  // was even visible.
  const [adminOpen, setAdminOpen] = useState(() => {
    try { return localStorage.getItem('sidebar_admin_open') === 'true'; } catch { return false; }
  });

  function toggleAdmin() {
    setAdminOpen((prev) => {
      const next = !prev;
      try { localStorage.setItem('sidebar_admin_open', String(next)); } catch { /* not critical */ }
      return next;
    });
  }

  function selectAdminTab(t: AdminTab) {
    onSelectAdminTab(t);
    select('__admin__');
  }

  // Tapping any nav item closes the drawer on mobile — on desktop
  // onCloseMobile is either absent or a harmless no-op, since the
  // sidebar isn't a drawer there in the first place.
  function select(id: string) {
    onSelect(id);
    onCloseMobile?.();
  }

  const enabledModules = modules.filter((m) => m.enabled);
  // A module stays visible in its group even while collapsed, IF it's
  // the one currently open — collapsing "Operations" while you're
  // sitting inside Inventory shouldn't make Inventory disappear from
  // the nav and leave you with no way to tell where you are.
  const activeModuleIsHidden = !operationsOpen && enabledModules.some((m) => m.id === selected);

  return (
    <nav className={`app-sidebar${mobileOpen ? ' mobile-open' : ''}`}>
      <div style={styles.header}>
        <div style={styles.wordmark}>SME Pro</div>
        <div style={styles.bizName}>{businessName}</div>
      </div>

      <div style={styles.list}>
        <button
          onClick={() => select('')}
          style={{ ...styles.item, ...(!selected ? styles.itemActive : {}) }}
        >
          <span
            className="stamp-badge"
            style={{
              width: '1.9rem', height: '1.9rem', fontSize: '0.72rem',
              color: !selected ? 'var(--stamp)' : 'var(--ink-faint)',
            }}
          >
            ⌂
          </span>
          <span>Home</span>
        </button>

        <button
          onClick={() => select('__pos__')}
          style={{ ...styles.item, ...(selected === '__pos__' ? styles.itemActive : {}) }}
        >
          <span
            className="stamp-badge"
            style={{
              width: '1.9rem', height: '1.9rem', fontSize: '0.72rem',
              color: selected === '__pos__' ? 'var(--stamp)' : 'var(--ink-faint)',
            }}
          >
            $
          </span>
          <span>Sell</span>
        </button>

        <button
          onClick={() => select('__customers__')}
          style={{ ...styles.item, ...(selected === '__customers__' ? styles.itemActive : {}) }}
        >
          <span
            className="stamp-badge"
            style={{
              width: '1.9rem', height: '1.9rem', fontSize: '0.72rem',
              color: selected === '__customers__' ? 'var(--stamp)' : 'var(--ink-faint)',
            }}
          >
            ♥
          </span>
          <span>Customers</span>
        </button>

        {enabledModules.length > 0 && (
          <>
            <button onClick={toggleOperations} style={styles.groupHeader}>
              <span style={{ transform: operationsOpen ? 'rotate(90deg)' : 'none', transition: 'transform 0.15s ease', display: 'inline-block', fontSize: '0.7rem' }}>▶</span>
              <span>Operations</span>
            </button>

            {(operationsOpen || activeModuleIsHidden) && enabledModules.map((m) => (
              <button
                key={m.id}
                onClick={() => select(m.id)}
                style={{ ...styles.item, ...styles.subItem, ...(selected === m.id ? styles.itemActive : {}) }}
              >
                <span
                  className="stamp-badge"
                  style={{
                    width: '1.7rem', height: '1.7rem', fontSize: '0.65rem',
                    color: selected === m.id ? 'var(--stamp)' : 'var(--ink-faint)',
                  }}
                >
                  {initials(m.display_name)}
                </span>
                <span>{m.display_name}</span>
              </button>
            ))}
          </>
        )}

        <button onClick={toggleAdmin} style={styles.groupHeader}>
          <span style={{ transform: adminOpen ? 'rotate(90deg)' : 'none', transition: 'transform 0.15s ease', display: 'inline-block', fontSize: '0.7rem' }}>▶</span>
          <span>Admin</span>
        </button>

        {adminOpen && ADMIN_TABS.map((t) => (
          <button
            key={t.id}
            onClick={() => selectAdminTab(t.id)}
            style={{ ...styles.item, ...styles.subItem, ...(selected === '__admin__' && adminTab === t.id ? styles.itemActive : {}) }}
          >
            <span
              className="stamp-badge"
              style={{
                width: '1.7rem', height: '1.7rem', fontSize: '0.65rem',
                color: selected === '__admin__' && adminTab === t.id ? 'var(--stamp)' : 'var(--ink-faint)',
              }}
            >
              {initials(t.label)}
            </span>
            <span>{t.label}</span>
          </button>
        ))}
      </div>

      <div style={styles.footer}>
        <AccountMenu onSignOut={onSignOut} />
      </div>
    </nav>
  );
}

const styles: Record<string, React.CSSProperties> = {
  header: { padding: '1.4rem 1.2rem 1rem' },
  wordmark: { fontFamily: 'var(--font-display)', fontWeight: 600, fontSize: '1.1rem' },
  bizName: { fontSize: '0.75rem', color: 'var(--ink-soft)', marginTop: '0.15rem' },
  list: { display: 'flex', flexDirection: 'column', gap: '0.15rem', padding: '0.4rem 0.7rem', overflowY: 'auto', flex: 1 },
  footer: { padding: '0.4rem 0.7rem 0.9rem', borderTop: '1px solid var(--paper-line)' },
  item: {
    display: 'flex', alignItems: 'center', gap: '0.7rem', textAlign: 'left',
    background: 'transparent', border: 'none', borderRadius: 3, padding: '0.5rem 0.6rem',
    fontSize: '0.88rem', color: 'var(--ink)', fontFamily: 'var(--font-body)',
  },
  subItem: { paddingLeft: '0.9rem' },
  itemActive: { background: 'var(--stamp-wash)', fontWeight: 600 },
  groupHeader: {
    display: 'flex', alignItems: 'center', gap: '0.5rem', textAlign: 'left',
    background: 'transparent', border: 'none', padding: '0.7rem 0.6rem 0.35rem',
    fontSize: '0.7rem', color: 'var(--ink-faint)', fontFamily: 'var(--font-body)',
    textTransform: 'uppercase', letterSpacing: '0.06em', fontWeight: 600, marginTop: '0.3rem',
  },
};
