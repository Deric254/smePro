import type { ModuleListItem } from '../types';

function initials(name: string) {
  const words = name.split(/[\s/]+/).filter(Boolean);
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
}: {
  modules: ModuleListItem[];
  selected: string | null;
  onSelect: (id: string) => void;
  businessName: string;
  mobileOpen?: boolean;
  onCloseMobile?: () => void;
}) {
  // Tapping any nav item closes the drawer on mobile — on desktop
  // onCloseMobile is either absent or a harmless no-op, since the
  // sidebar isn't a drawer there in the first place.
  function select(id: string) {
    onSelect(id);
    onCloseMobile?.();
  }

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

        {modules.filter((m) => m.enabled).map((m) => (
          <button
            key={m.id}
            onClick={() => select(m.id)}
            style={{ ...styles.item, ...(selected === m.id ? styles.itemActive : {}) }}
          >
            <span
              className="stamp-badge"
              style={{
                width: '1.9rem', height: '1.9rem', fontSize: '0.72rem',
                color: selected === m.id ? 'var(--stamp)' : 'var(--ink-faint)',
              }}
            >
              {initials(m.display_name)}
            </span>
            <span>{m.display_name}</span>
          </button>
        ))}
      </div>

      <div style={styles.footer}>
        <button
          onClick={() => select('__admin__')}
          style={{ ...styles.item, ...(selected === '__admin__' ? styles.itemActive : {}) }}
        >
          <span
            className="stamp-badge"
            style={{
              width: '1.9rem', height: '1.9rem', fontSize: '0.72rem',
              color: selected === '__admin__' ? 'var(--stamp)' : 'var(--ink-faint)',
            }}
          >
            ⚙
          </span>
          <span>Admin</span>
        </button>
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
  itemActive: { background: 'var(--stamp-wash)', fontWeight: 600 },
};
