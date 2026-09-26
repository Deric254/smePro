import { useEffect, useRef, useState } from 'react';
import { getCurrentUser } from '../api';
import type { CurrentUser } from '../api';
import { retryOnConnectionFailure } from '../lib/retry';

function initials(name: string) {
  const words = name.split(/[\s/]+/).filter(Boolean);
  if (words.length === 0) return '?';
  if (words.length === 1) return words[0].slice(0, 2).toUpperCase();
  return (words[0][0] + words[1][0]).toUpperCase();
}

/**
 * The account/profile menu — a name/role badge that opens a small
 * popup with sign-out inside it. This replaces what used to be a
 * bare "Sign out" button floating at the top of every page with no
 * indication of who was even signed in — a real account menu needed
 * to exist before sign-out had anywhere sensible to live.
 */
export default function AccountMenu({ onSignOut }: { onSignOut: () => void }) {
  const [user, setUser] = useState<CurrentUser | null>(null);
  const [open, setOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    retryOnConnectionFailure(() => getCurrentUser()).then(setUser).catch(() => {}); // menu still works (badge just shows nothing) if this fails
  }, []);

  useEffect(() => {
    function onClickOutside(e: MouseEvent) {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    }
    if (open) {
      document.addEventListener('mousedown', onClickOutside);
      return () => document.removeEventListener('mousedown', onClickOutside);
    }
  }, [open]);

  const displayName = user?.username ?? '…';

  return (
    <div ref={menuRef} style={styles.wrap}>
      <button
        onClick={() => setOpen((v) => !v)}
        style={styles.trigger}
        aria-haspopup="true"
        aria-expanded={open}
      >
        <span className="stamp-badge" style={styles.avatar}>{initials(displayName)}</span>
        <span style={styles.nameCol}>
          <span style={styles.name}>{displayName}</span>
          {user?.role_name && <span style={styles.role}>{user.role_name}</span>}
        </span>
        <span style={styles.chevron}>{open ? '▲' : '▼'}</span>
      </button>

      {open && (
        <div style={styles.popup}>
          <div style={styles.popupHeader}>
            <div style={{ fontWeight: 600 }}>{displayName}</div>
            {user?.role_name && <div style={styles.role}>{user.role_name}{user.business_name ? ` · ${user.business_name}` : ''}</div>}
          </div>
          <button
            style={styles.signOutBtn}
            onClick={() => { setOpen(false); onSignOut(); }}
          >
            Sign out
          </button>
        </div>
      )}
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  wrap: { position: 'relative' },
  trigger: {
    display: 'flex', alignItems: 'center', gap: '0.6rem', textAlign: 'left',
    background: 'transparent', border: 'none', borderRadius: 6, padding: '0.4rem 0.5rem',
    fontFamily: 'var(--font-body)', cursor: 'pointer', width: '100%',
  },
  avatar: { width: '2rem', height: '2rem', fontSize: '0.75rem', flexShrink: 0 },
  nameCol: { display: 'flex', flexDirection: 'column', minWidth: 0, flex: 1 },
  name: { fontSize: '0.85rem', fontWeight: 600, color: 'var(--ink)', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' },
  role: { fontSize: '0.72rem', color: 'var(--ink-soft)' },
  chevron: { fontSize: '0.6rem', color: 'var(--ink-faint)' },
  popup: {
    position: 'absolute', bottom: 'calc(100% + 0.4rem)', left: 0, right: 0,
    background: 'var(--paper-card)', border: '1px solid var(--paper-line)', borderRadius: 8,
    boxShadow: '0 4px 16px rgba(0,0,0,0.12)', padding: '0.7rem', zIndex: 50,
  },
  popupHeader: { marginBottom: '0.6rem', paddingBottom: '0.6rem', borderBottom: '1px solid var(--paper-line)', fontSize: '0.85rem' },
  signOutBtn: {
    width: '100%', textAlign: 'left', background: 'transparent', border: 'none',
    borderRadius: 4, padding: '0.4rem 0.3rem', fontSize: '0.85rem', color: 'var(--stamp)',
    cursor: 'pointer', fontFamily: 'var(--font-body)',
  },
};
