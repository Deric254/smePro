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
}: {
  modules: ModuleListItem[];
  selected: string | null;
  onSelect: (id: string) => void;
  businessName: string;
}) {
  return (
    <nav style={styles.wrap}>
      <div style={styles.header}>
        <div style={styles.wordmark}>SME Pro</div>
        <div style={styles.bizName}>{businessName}</div>
      </div>

      <div style={styles.list}>
        {modules.filter((m) => m.enabled).map((m) => (
          <button
            key={m.id}
            onClick={() => onSelect(m.id)}
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
          onClick={() => onSelect('__admin__')}
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
  wrap: {
    width: 220, flexShrink: 0, borderRight: '1px solid var(--paper-line)',
    display: 'flex', flexDirection: 'column', height: '100vh', position: 'sticky', top: 0,
  },
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
