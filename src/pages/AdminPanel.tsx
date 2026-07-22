import { useEffect, useState } from 'react';
import {
  listRoles, createRole, deleteRole, setRoleAdminFlag, getRolePermissions, setRolePermissions,
  listUsers, createUser, setUserRole, deactivateUser,
  listUnits, createUnit, deleteUnit,
  listCurrencies, createCurrency, deleteCurrency,
  getSettings, setSetting,
  getVendorLicenseStatus, redeemVendorKey,
  listModules, getModuleSchema,
  ApiError,
} from '../api';
import type { Role, UserAccount, Unit, Currency, ModuleListItem } from '../types';

type Tab = 'roles' | 'users' | 'units' | 'currencies' | 'settings' | 'license';

const TABS: { id: Tab; label: string }[] = [
  { id: 'roles', label: 'Roles' },
  { id: 'users', label: 'Users' },
  { id: 'units', label: 'Units' },
  { id: 'currencies', label: 'Currencies' },
  { id: 'settings', label: 'Theme & Settings' },
  { id: 'license', label: 'Vendor License' },
];

export default function AdminPanel() {
  const [tab, setTab] = useState<Tab>('roles');

  return (
    <div>
      <div style={styles.headerRow}>
        <h2>Admin</h2>
        <div style={styles.tabs}>
          {TABS.map((t) => (
            <button key={t.id} className={tab === t.id ? 'btn' : 'btn btn-outline'} onClick={() => setTab(t.id)}>
              {t.label}
            </button>
          ))}
        </div>
      </div>

      {tab === 'roles' && <RolesTab />}
      {tab === 'users' && <UsersTab />}
      {tab === 'units' && <UnitsTab />}
      {tab === 'currencies' && <CurrenciesTab />}
      {tab === 'settings' && <SettingsTab />}
      {tab === 'license' && <VendorLicenseTab />}
    </div>
  );
}

function ErrorBox({ error }: { error: string | null }) {
  if (!error) return null;
  return <div style={styles.error}>{error}</div>;
}

// ------------------------------------------------------------- Roles

function RolesTab() {
  const [roles, setRoles] = useState<Role[]>([]);
  const [newName, setNewName] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [selectedRole, setSelectedRole] = useState<Role | null>(null);

  const refresh = () => listRoles().then((r) => setRoles(r.roles)).catch(() => {});
  useEffect(() => { refresh(); }, []);

  async function handleCreate(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      await createRole(newName);
      setNewName('');
      await refresh();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not create role');
    }
  }

  async function handleDelete(role: Role) {
    setError(null);
    try {
      await deleteRole(role.id);
      await refresh();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not delete role');
    }
  }

  async function toggleAdmin(role: Role) {
    setError(null);
    try {
      await setRoleAdminFlag(role.id, !role.can_administer);
      await refresh();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not update role');
    }
  }

  return (
    <div>
      <ErrorBox error={error} />
      <form onSubmit={handleCreate} className="card" style={{ display: 'flex', gap: '0.6rem', alignItems: 'flex-end', marginBottom: '1rem' }}>
        <div style={{ flex: 1 }}>
          <label>New role name</label>
          <input value={newName} onChange={(e) => setNewName(e.target.value)} placeholder="e.g. Cashier, Accountant, Supervisor" style={{ width: '100%' }} required />
        </div>
        <button className="btn btn-stamp" type="submit">Add role</button>
      </form>

      <div className="card" style={{ padding: 0, overflowX: 'auto', marginBottom: '1rem' }}>
        <table style={styles.table}>
          <thead>
            <tr>
              <th style={styles.th}>Role</th>
              <th style={styles.th}>Admin tier</th>
              <th style={styles.th} />
            </tr>
          </thead>
          <tbody>
            {roles.map((r) => (
              <tr key={r.id}>
                <td style={styles.td}>
                  {r.name} {r.is_system && <span style={styles.badge}>protected</span>}
                </td>
                <td style={styles.td}>
                  <label style={{ display: 'inline-flex', alignItems: 'center', gap: '0.4em', textTransform: 'none', fontSize: '0.85rem' }}>
                    <input type="checkbox" checked={r.can_administer} disabled={r.is_system} onChange={() => toggleAdmin(r)} />
                    can manage settings/payments
                  </label>
                </td>
                <td style={styles.td}>
                  <div style={{ display: 'flex', gap: '0.4rem' }}>
                    {!r.is_system && (
                      <button className="btn btn-outline" style={styles.smallBtn} onClick={() => setSelectedRole(r)}>Permissions</button>
                    )}
                    {!r.is_system && (
                      <button className="btn btn-outline" style={styles.smallBtn} onClick={() => handleDelete(r)}>Delete</button>
                    )}
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {selectedRole && <PermissionEditor role={selectedRole} onClose={() => setSelectedRole(null)} />}
    </div>
  );
}

function PermissionEditor({ role, onClose }: { role: Role; onClose: () => void }) {
  const [modules, setModules] = useState<ModuleListItem[]>([]);
  const [moduleId, setModuleId] = useState('');
  const [actions, setActions] = useState<string[]>([]);
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  useEffect(() => { listModules().then((r) => setModules(r.modules.filter((m: ModuleListItem) => m.enabled))).catch(() => {}); }, []);

  useEffect(() => {
    if (!moduleId) return;
    setSaved(false);
    Promise.all([getModuleSchema(moduleId), getRolePermissions(role.id)])
      .then(([schema, perms]) => {
        setActions(schema.actions);
        setChecked(new Set(perms[moduleId] ?? []));
      })
      .catch((e) => setError(e instanceof ApiError ? e.message : 'Could not load permissions'));
  }, [moduleId, role.id]);

  function toggle(action: string) {
    setChecked((prev) => {
      const next = new Set(prev);
      if (next.has(action)) next.delete(action); else next.add(action);
      return next;
    });
  }

  async function save() {
    setError(null);
    try {
      await setRolePermissions(role.id, moduleId, Array.from(checked));
      setSaved(true);
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not save permissions');
    }
  }

  return (
    <div className="card">
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '0.8rem' }}>
        <h3>Permissions for {role.name}</h3>
        <button className="btn btn-outline" style={styles.smallBtn} onClick={onClose}>Close</button>
      </div>
      <ErrorBox error={error} />
      <label>Module</label>
      <select value={moduleId} onChange={(e) => setModuleId(e.target.value)} style={{ marginBottom: '0.8rem' }}>
        <option value="">Choose a module…</option>
        {modules.map((m) => <option key={m.id} value={m.id}>{m.display_name}</option>)}
      </select>

      {moduleId && (
        <>
          <div style={{ display: 'flex', gap: '1rem', flexWrap: 'wrap', marginBottom: '0.9rem' }}>
            {actions.map((a) => (
              <label key={a} style={{ display: 'flex', alignItems: 'center', gap: '0.4em', textTransform: 'none', fontSize: '0.88rem' }}>
                <input type="checkbox" checked={checked.has(a)} onChange={() => toggle(a)} />
                {a}
              </label>
            ))}
          </div>
          <button className="btn btn-stamp" onClick={save}>Save permissions</button>
          {saved && <span style={{ marginLeft: '0.7rem', color: 'var(--ok)', fontSize: '0.85rem' }}>Saved.</span>}
        </>
      )}
    </div>
  );
}

// ------------------------------------------------------------- Users

function UsersTab() {
  const [users, setUsers] = useState<UserAccount[]>([]);
  const [roles, setRoles] = useState<Role[]>([]);
  const [showForm, setShowForm] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [form, setForm] = useState({ username: '', password: '', role_id: '', security_q1: '', security_a1: '', security_q2: '', security_a2: '' });

  const refresh = () => Promise.all([listUsers(), listRoles()]).then(([u, r]) => { setUsers(u.users); setRoles(r.roles); }).catch(() => {});
  useEffect(() => { refresh(); }, []);

  async function handleCreate(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      await createUser(form);
      setForm({ username: '', password: '', role_id: '', security_q1: '', security_a1: '', security_q2: '', security_a2: '' });
      setShowForm(false);
      await refresh();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not create user');
    }
  }

  async function handleRoleChange(u: UserAccount, roleId: string) {
    setError(null);
    try {
      await setUserRole(u.id, roleId);
      await refresh();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not reassign role');
    }
  }

  async function handleDeactivate(u: UserAccount) {
    setError(null);
    try {
      await deactivateUser(u.id);
      await refresh();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not deactivate user');
    }
  }

  return (
    <div>
      <ErrorBox error={error} />
      <div style={{ marginBottom: '1rem' }}>
        <button className="btn btn-stamp" onClick={() => setShowForm((v) => !v)}>{showForm ? 'Cancel' : '+ New user'}</button>
      </div>

      {showForm && (
        <form onSubmit={handleCreate} className="card" style={{ marginBottom: '1rem' }}>
          <div style={styles.formGrid}>
            <div>
              <label>Username *</label>
              <input value={form.username} onChange={(e) => setForm((p) => ({ ...p, username: e.target.value }))} required style={{ width: '100%' }} />
            </div>
            <div>
              <label>Password *</label>
              <input type="password" value={form.password} onChange={(e) => setForm((p) => ({ ...p, password: e.target.value }))} required minLength={8} style={{ width: '100%' }} />
            </div>
            <div>
              <label>Role *</label>
              <select value={form.role_id} onChange={(e) => setForm((p) => ({ ...p, role_id: e.target.value }))} required style={{ width: '100%' }}>
                <option value="">Choose a role…</option>
                {roles.map((r) => <option key={r.id} value={r.id}>{r.name}</option>)}
              </select>
            </div>
            <div>
              <label>Security question 1 *</label>
              <input value={form.security_q1} onChange={(e) => setForm((p) => ({ ...p, security_q1: e.target.value }))} required style={{ width: '100%' }} />
            </div>
            <div>
              <label>Answer 1 *</label>
              <input value={form.security_a1} onChange={(e) => setForm((p) => ({ ...p, security_a1: e.target.value }))} required style={{ width: '100%' }} />
            </div>
            <div>
              <label>Security question 2 *</label>
              <input value={form.security_q2} onChange={(e) => setForm((p) => ({ ...p, security_q2: e.target.value }))} required style={{ width: '100%' }} />
            </div>
            <div>
              <label>Answer 2 *</label>
              <input value={form.security_a2} onChange={(e) => setForm((p) => ({ ...p, security_a2: e.target.value }))} required style={{ width: '100%' }} />
            </div>
          </div>
          <button className="btn btn-stamp" type="submit" style={{ marginTop: '0.8rem' }}>Create user</button>
        </form>
      )}

      <div className="card" style={{ padding: 0, overflowX: 'auto' }}>
        <table style={styles.table}>
          <thead>
            <tr>
              <th style={styles.th}>Username</th>
              <th style={styles.th}>Role</th>
              <th style={styles.th}>Status</th>
              <th style={styles.th} />
            </tr>
          </thead>
          <tbody>
            {users.map((u) => (
              <tr key={u.id}>
                <td style={styles.td}>{u.username}</td>
                <td style={styles.td}>
                  <select
                    value={roles.find((r) => r.name === u.role)?.id ?? ''}
                    disabled={!u.active}
                    onChange={(e) => handleRoleChange(u, e.target.value)}
                  >
                    {roles.map((r) => <option key={r.id} value={r.id}>{r.name}</option>)}
                  </select>
                </td>
                <td style={styles.td}>{u.active ? <span className="status-pill status-active">Active</span> : <span className="status-pill status-inactive">Deactivated</span>}</td>
                <td style={styles.td}>
                  {u.active && <button className="btn btn-outline" style={styles.smallBtn} onClick={() => handleDeactivate(u)}>Deactivate</button>}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

// ------------------------------------------------------------- Units

function UnitsTab() {
  const [units, setUnits] = useState<Unit[]>([]);
  const [name, setName] = useState('');
  const [abbr, setAbbr] = useState('');
  const [error, setError] = useState<string | null>(null);

  const refresh = () => listUnits().then((r) => setUnits(r.units)).catch(() => {});
  useEffect(() => { refresh(); }, []);

  async function handleCreate(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      await createUnit(name, abbr || undefined);
      setName(''); setAbbr('');
      await refresh();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not add unit');
    }
  }

  async function handleDelete(u: Unit) {
    setError(null);
    try {
      await deleteUnit(u.id);
      await refresh();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not delete unit');
    }
  }

  return (
    <div>
      <ErrorBox error={error} />
      <form onSubmit={handleCreate} className="card" style={{ display: 'flex', gap: '0.6rem', alignItems: 'flex-end', marginBottom: '1rem' }}>
        <div style={{ flex: 1 }}>
          <label>Unit name</label>
          <input value={name} onChange={(e) => setName(e.target.value)} placeholder="e.g. Sack of 90kg" required style={{ width: '100%' }} />
        </div>
        <div style={{ width: 100 }}>
          <label>Abbreviation</label>
          <input value={abbr} onChange={(e) => setAbbr(e.target.value)} placeholder="sack" style={{ width: '100%' }} />
        </div>
        <button className="btn btn-stamp" type="submit">Add unit</button>
      </form>

      <div className="card" style={{ padding: 0, overflowX: 'auto' }}>
        <table style={styles.table}>
          <thead><tr><th style={styles.th}>Name</th><th style={styles.th}>Abbreviation</th><th style={styles.th} /></tr></thead>
          <tbody>
            {units.map((u) => (
              <tr key={u.id}>
                <td style={styles.td}>{u.name}</td>
                <td className="mono" style={styles.td}>{u.abbreviation || '—'}</td>
                <td style={styles.td}><button className="btn btn-outline" style={styles.smallBtn} onClick={() => handleDelete(u)}>Delete</button></td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

// -------------------------------------------------------- Currencies

function CurrenciesTab() {
  const [currencies, setCurrencies] = useState<Currency[]>([]);
  const [code, setCode] = useState('');
  const [symbol, setSymbol] = useState('');
  const [name, setName] = useState('');
  const [error, setError] = useState<string | null>(null);

  const refresh = () => listCurrencies().then((r) => setCurrencies(r.currencies)).catch(() => {});
  useEffect(() => { refresh(); }, []);

  async function handleCreate(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      await createCurrency(code, symbol || undefined, name || undefined);
      setCode(''); setSymbol(''); setName('');
      await refresh();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not add currency');
    }
  }

  async function handleDelete(c: Currency) {
    setError(null);
    try {
      await deleteCurrency(c.id);
      await refresh();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not delete currency');
    }
  }

  return (
    <div>
      <ErrorBox error={error} />
      <form onSubmit={handleCreate} className="card" style={{ display: 'flex', gap: '0.6rem', alignItems: 'flex-end', marginBottom: '1rem' }}>
        <div style={{ width: 100 }}>
          <label>Code *</label>
          <input value={code} onChange={(e) => setCode(e.target.value)} placeholder="XOF" required style={{ width: '100%' }} />
        </div>
        <div style={{ width: 100 }}>
          <label>Symbol</label>
          <input value={symbol} onChange={(e) => setSymbol(e.target.value)} placeholder="CFA" style={{ width: '100%' }} />
        </div>
        <div style={{ flex: 1 }}>
          <label>Name</label>
          <input value={name} onChange={(e) => setName(e.target.value)} placeholder="West African CFA franc" style={{ width: '100%' }} />
        </div>
        <button className="btn btn-stamp" type="submit">Add currency</button>
      </form>

      <div className="card" style={{ padding: 0, overflowX: 'auto' }}>
        <table style={styles.table}>
          <thead><tr><th style={styles.th}>Code</th><th style={styles.th}>Symbol</th><th style={styles.th}>Name</th><th style={styles.th} /></tr></thead>
          <tbody>
            {currencies.map((c) => (
              <tr key={c.id}>
                <td className="mono" style={styles.td}>{c.code}</td>
                <td style={styles.td}>{c.symbol || '—'}</td>
                <td style={styles.td}>{c.name || '—'}</td>
                <td style={styles.td}><button className="btn btn-outline" style={styles.smallBtn} onClick={() => handleDelete(c)}>Delete</button></td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

// --------------------------------------------------------- Settings

const THEMES = [
  { id: 'ledger', label: 'Classic (default)' },
  { id: 'dark_ledger', label: 'Dark Classic' },
  { id: 'sea_glass', label: 'Sea Glass' },
];

function SettingsTab() {
  const [theme, setTheme] = useState('ledger');
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    getSettings().then((s) => { if (s.theme) setTheme(s.theme); }).catch(() => {});
  }, []);

  async function applyTheme(id: string) {
    setTheme(id);
    setSaved(false);
    setError(null);
    document.documentElement.dataset.theme = id === 'ledger' ? '' : id;
    try {
      await setSetting('theme', id);
      setSaved(true);
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not save theme');
    }
  }

  return (
    <div className="card">
      <ErrorBox error={error} />
      <label>Theme</label>
      <div style={{ display: 'flex', gap: '0.6rem', flexWrap: 'wrap', marginTop: '0.4rem' }}>
        {THEMES.map((t) => (
          <button
            key={t.id}
            className={theme === t.id ? 'btn' : 'btn btn-outline'}
            onClick={() => applyTheme(t.id)}
          >
            {t.label}
          </button>
        ))}
      </div>
      {saved && <div style={{ marginTop: '0.7rem', color: 'var(--ok)', fontSize: '0.85rem' }}>Saved — applies for everyone on this install.</div>}
    </div>
  );
}

// --------------------------------------------------- Vendor License

function VendorLicenseTab() {
  const [status, setStatus] = useState<{ licensed: boolean; key_id?: string; activated_at?: string } | null>(null);
  const [key, setKey] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = () => getVendorLicenseStatus().then(setStatus).catch(() => {});
  useEffect(() => { refresh(); }, []);

  async function handleRedeem(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    setBusy(true);
    try {
      await redeemVendorKey(key);
      setKey('');
      await refresh();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not redeem key');
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="card">
      <ErrorBox error={error} />
      {status?.licensed ? (
        <div>
          <span className="status-pill status-active">Licensed</span>
          <div style={{ marginTop: '0.6rem', fontSize: '0.85rem', color: 'var(--ink-soft)' }} className="mono">
            key_id: {status.key_id} · activated: {status.activated_at}
          </div>
        </div>
      ) : (
        <form onSubmit={handleRedeem} style={{ display: 'flex', gap: '0.6rem', alignItems: 'flex-end' }}>
          <div style={{ flex: 1 }}>
            <label>License key</label>
            <input
              className="mono"
              value={key}
              onChange={(e) => setKey(e.target.value.toUpperCase())}
              placeholder="LKC-XXXX-XXXX-XXXX-XXXX"
              style={{ width: '100%' }}
              required
            />
          </div>
          <button className="btn btn-stamp" type="submit" disabled={busy}>{busy ? 'Redeeming…' : 'Redeem'}</button>
        </form>
      )}
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  headerRow: { display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: '0.9rem', flexWrap: 'wrap', gap: '0.6rem' },
  tabs: { display: 'flex', gap: '0.4rem', flexWrap: 'wrap' },
  table: { width: '100%', borderCollapse: 'collapse', fontSize: '0.86rem' },
  th: { textAlign: 'left', padding: '0.6rem 0.8rem', borderBottom: '1px solid var(--paper-line)', fontSize: '0.72rem', textTransform: 'uppercase', letterSpacing: '0.03em', color: 'var(--ink-soft)' },
  td: { padding: '0.55rem 0.8rem', borderBottom: '1px solid var(--paper-line)' },
  error: { background: 'var(--stamp-wash)', color: 'var(--stamp)', padding: '0.5em 0.7em', borderRadius: 3, fontSize: '0.85rem', marginBottom: '0.8rem' },
  formGrid: { display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(160px, 1fr))', gap: '0.8rem' },
  smallBtn: { padding: '0.3em 0.7em', fontSize: '0.78rem' },
  badge: { fontSize: '0.65rem', color: 'var(--ink-faint)', textTransform: 'uppercase', letterSpacing: '0.03em', marginLeft: '0.4em' },
};
