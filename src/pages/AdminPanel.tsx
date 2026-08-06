import { useEffect, useState } from 'react';
import {
  listRoles, createRole, deleteRole, setRoleAdminFlag, getRolePermissions, setRolePermissions,
  listUsers, createUser, setUserRole, deactivateUser,
  listUnits, createUnit, deleteUnit,
  listCurrencies, createCurrency, deleteCurrency,
  getSettings, setSetting,
  getVendorLicenseStatus, redeemVendorKey,
  createBackup, restoreBackup,
  getPaymentHistory, initiateStripeCheckout, initiateMpesaPayment,
  getAuditLog,
  listNotifications, sendNotification,
  changeBusinessType,
  listModules, getModuleSchema, enableModule,
  ApiError,
} from '../api';
import type { PaymentHistoryEntry, AuditLogEntry, NotificationRecord } from '../api';
import type { Role, UserAccount, Unit, Currency, ModuleListItem } from '../types';
import BusinessBranding from '../components/BusinessBranding';
import TwoFactorSetup from '../components/TwoFactorSetup';

type Tab = 'roles' | 'users' | 'units' | 'currencies' | 'settings' | 'license' | 'backup' | 'billing' | 'audit' | 'notifications' | 'business' | 'security';

const TABS: { id: Tab; label: string }[] = [
  { id: 'roles', label: 'Roles' },
  { id: 'users', label: 'Users' },
  { id: 'units', label: 'Units' },
  { id: 'currencies', label: 'Currencies' },
  { id: 'settings', label: 'Theme & Settings' },
  { id: 'business', label: 'Business' },
  { id: 'security', label: 'Security' },
  { id: 'notifications', label: 'Notifications' },
  { id: 'billing', label: 'Billing' },
  { id: 'license', label: 'Vendor License' },
  { id: 'backup', label: 'Backup & Restore' },
  { id: 'audit', label: 'Audit Log' },
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
      {tab === 'business' && <BusinessTab />}
      {tab === 'security' && <TwoFactorSetup />}
      {tab === 'notifications' && <NotificationsTab />}
      {tab === 'license' && <VendorLicenseTab />}
      {tab === 'billing' && <BillingTab />}
      {tab === 'backup' && <BackupTab />}
      {tab === 'audit' && <AuditLogTab />}
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

function BusinessTab() {
  const [modules, setModules] = useState<ModuleListItem[]>([]);
  const [enabling, setEnabling] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = () => { listModules().then((r) => setModules(r.modules)).catch(() => {}); };
  useEffect(refresh, []);

  const invoiceEnabled = modules.some((m) => m.id === 'invoice' && m.enabled);

  async function handleEnableInvoices() {
    setEnabling(true);
    setError(null);
    try {
      await enableModule('invoice');
      refresh();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not enable Invoices module');
    } finally {
      setEnabling(false);
    }
  }

  return (
    <div>
      <BusinessBranding />

      <div className="card" style={{ marginTop: '1.2rem' }}>
        <h3 style={{ marginTop: 0 }}>Additional modules</h3>
        <p style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>
          Modules beyond your business type's starting set can be turned on individually.
        </p>
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '0.6rem 0', borderTop: '1px solid var(--paper-line)' }}>
          <div>
            <strong>Invoices</strong>
            <div style={{ fontSize: '0.8rem', color: 'var(--ink-soft)' }}>Create, send, and track customer invoices.</div>
          </div>
          {invoiceEnabled ? (
            <span style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>Enabled</span>
          ) : (
            <button className="btn" onClick={handleEnableInvoices} disabled={enabling}>
              {enabling ? 'Enabling…' : 'Enable'}
            </button>
          )}
        </div>
        {error && <div style={{ color: 'var(--stamp)', fontSize: '0.85rem', marginTop: '0.5rem' }}>{error}</div>}
      </div>
    </div>
  );
}

function SettingsTab() {
  const [theme, setTheme] = useState('ledger');
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [updateStatus, setUpdateStatus] = useState<'idle' | 'checking' | 'available' | 'none' | 'error'>('idle');
  const [updateVersion, setUpdateVersion] = useState<string | null>(null);
  const [businessType, setBusinessType] = useState('retail');
  const [changingType, setChangingType] = useState(false);
  const [typeChangedModules, setTypeChangedModules] = useState<string[] | null>(null);
  const [typeError, setTypeError] = useState<string | null>(null);

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

  async function handleChangeBusinessType() {
    setChangingType(true);
    setTypeError(null);
    setTypeChangedModules(null);
    try {
      const res = await changeBusinessType(businessType);
      setTypeChangedModules(res.enabled_modules);
    } catch (err) {
      setTypeError(err instanceof ApiError ? err.message : 'Could not change business type');
    } finally {
      setChangingType(false);
    }
  }

  async function checkForUpdates() {
    setUpdateStatus('checking');
    try {
      const { check } = await import('@tauri-apps/plugin-updater');
      const update = await check();
      if (update) {
        setUpdateVersion(update.version);
        setUpdateStatus('available');
      } else {
        setUpdateStatus('none');
      }
    } catch {
      // Either not running inside the desktop app, or genuinely
      // couldn't reach the update server — same message either way,
      // since a customer doesn't need to know which.
      setUpdateStatus('error');
    }
  }

  return (
    <>
      <div className="card" style={{ marginBottom: '1rem' }}>
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

      <div className="card" style={{ marginBottom: '1rem' }}>
        <h3 style={{ marginTop: 0 }}>Business type</h3>
        <p style={{ fontSize: '0.85rem', color: 'var(--ink-soft)', lineHeight: 1.5 }}>
          Changes the sensible starting set of modules enabled for a business like this.
          Doesn't remove or hide anything already in use — only adds modules that make
          sense for the new type and aren't already on.
        </p>
        <ErrorBox error={typeError} />
        {typeChangedModules && (
          <div style={{ color: 'var(--ok)', fontSize: '0.85rem', marginBottom: '0.7rem' }}>
            Updated — enabled modules: {typeChangedModules.join(', ')}
          </div>
        )}
        <div style={{ display: 'flex', gap: '0.6rem', alignItems: 'flex-end' }}>
          <div style={{ flex: 1 }}>
            <label>Type</label>
            <select value={businessType} onChange={(e) => setBusinessType(e.target.value)} style={{ width: '100%' }}>
              <option value="retail">Retail / General Store</option>
              <option value="food">Food / Restaurant</option>
              <option value="services">Services</option>
              <option value="manufacturing">Manufacturing</option>
            </select>
          </div>
          <button className="btn btn-stamp" onClick={handleChangeBusinessType} disabled={changingType}>
            {changingType ? 'Applying…' : 'Apply'}
          </button>
        </div>
      </div>

      <div className="card">
        <h3 style={{ marginTop: 0 }}>Software updates</h3>
        <p style={{ fontSize: '0.85rem', color: 'var(--ink-soft)', lineHeight: 1.5 }}>
          The app checks for updates automatically each time it starts. Use this if you want
          to check right now instead of waiting for the next launch.
        </p>
        <button className="btn btn-outline" onClick={checkForUpdates} disabled={updateStatus === 'checking'}>
          {updateStatus === 'checking' ? 'Checking…' : 'Check for updates now'}
        </button>
        {updateStatus === 'available' && (
          <div style={{ marginTop: '0.7rem', color: 'var(--stamp)', fontSize: '0.85rem' }}>
            Version {updateVersion} is available — restart the app to see the install prompt.
          </div>
        )}
        {updateStatus === 'none' && (
          <div style={{ marginTop: '0.7rem', color: 'var(--ok)', fontSize: '0.85rem' }}>You're on the latest version.</div>
        )}
        {updateStatus === 'error' && (
          <div style={{ marginTop: '0.7rem', color: 'var(--ink-soft)', fontSize: '0.85rem' }}>
            Couldn't check right now — this also happens normally in a browser/dev preview, not just on a real connection issue.
          </div>
        )}
      </div>
    </>
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
              placeholder="SPK-XXXXX-XXXXX-XXXXX-..."
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

// -------------------------------------------------------------- Billing

function BillingTab() {
  const [history, setHistory] = useState<PaymentHistoryEntry[]>([]);
  const [loadingHistory, setLoadingHistory] = useState(true);
  const [provider, setProvider] = useState<'stripe' | 'mpesa'>('stripe');
  const [purpose, setPurpose] = useState<'activation' | 'subscription'>('subscription');
  const [amount, setAmount] = useState('');
  const [currency, setCurrency] = useState('usd');
  const [phone, setPhone] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [mpesaMessage, setMpesaMessage] = useState<string | null>(null);

  const refreshHistory = () => {
    setLoadingHistory(true);
    getPaymentHistory().then((r) => setHistory(r.payments)).catch(() => {}).finally(() => setLoadingHistory(false));
  };
  useEffect(() => { refreshHistory(); }, []);

  async function handlePay(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    setMpesaMessage(null);
    const parsedAmount = parseFloat(amount);
    if (!parsedAmount || parsedAmount <= 0) { setError('Enter a valid amount.'); return; }
    setBusy(true);
    try {
      if (provider === 'stripe') {
        const res = await initiateStripeCheckout(purpose, parsedAmount, currency);
        if (res.checkout_url) {
          // Real payment page — leaves the app deliberately. Opened in
          // the system browser, not inside the app's own webview, since
          // that's where a saved card / Apple Pay / etc. already works.
          try {
            const { openUrl } = await import('@tauri-apps/plugin-opener');
            await openUrl(res.checkout_url);
          } catch {
            window.open(res.checkout_url, '_blank');
          }
        }
      } else {
        if (!phone.trim()) { setError('Enter the M-Pesa phone number (format: 2547XXXXXXXX).'); setBusy(false); return; }
        const res = await initiateMpesaPayment(purpose, parsedAmount, phone.trim());
        setMpesaMessage(res.message || 'Check your phone to complete the M-Pesa payment.');
      }
      setAmount('');
      refreshHistory();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not start the payment');
    } finally {
      setBusy(false);
    }
  }

  return (
    <div>
      <div className="card" style={{ marginBottom: '1rem' }}>
        <h3 style={{ marginTop: 0 }}>Make a payment</h3>
        <p style={{ fontSize: '0.8rem', color: 'var(--ink-soft)', marginTop: 0 }}>
          This is a real charge through Stripe or M-Pesa — different from the free trial or a
          vendor-issued license key. Use this if your vendor has set up real billing for this
          install.
        </p>
        <ErrorBox error={error} />
        {mpesaMessage && <div style={{ color: 'var(--stamp)', fontSize: '0.85rem', marginBottom: '0.8rem' }}>{mpesaMessage}</div>}
        <form onSubmit={handlePay} style={styles.formGrid}>
          <div>
            <label>Provider</label>
            <select value={provider} onChange={(e) => setProvider(e.target.value as 'stripe' | 'mpesa')} style={{ width: '100%' }}>
              <option value="stripe">Card (Stripe)</option>
              <option value="mpesa">M-Pesa</option>
            </select>
          </div>
          <div>
            <label>Purpose</label>
            <select value={purpose} onChange={(e) => setPurpose(e.target.value as 'activation' | 'subscription')} style={{ width: '100%' }}>
              <option value="subscription">Monthly subscription</option>
              <option value="activation">One-time activation</option>
            </select>
          </div>
          <div>
            <label>Amount</label>
            <input type="number" min="0" step="0.01" value={amount} onChange={(e) => setAmount(e.target.value)} required style={{ width: '100%' }} />
          </div>
          {provider === 'stripe' ? (
            <div>
              <label>Currency</label>
              <input value={currency} onChange={(e) => setCurrency(e.target.value.toLowerCase())} maxLength={3} style={{ width: '100%' }} />
            </div>
          ) : (
            <div>
              <label>M-Pesa phone number</label>
              <input value={phone} onChange={(e) => setPhone(e.target.value)} placeholder="2547XXXXXXXX" style={{ width: '100%' }} />
            </div>
          )}
        </form>
        <button className="btn btn-stamp" style={{ marginTop: '0.8rem' }} onClick={handlePay} disabled={busy}>
          {busy ? 'Starting…' : provider === 'stripe' ? 'Continue to payment' : 'Send payment request'}
        </button>
      </div>

      <h3 style={{ margin: '1.2rem 0 0.8rem' }}>Payment history</h3>
      <div className="card" style={{ padding: 0, overflowX: 'auto' }}>
        <table style={styles.table}>
          <thead>
            <tr>
              <th style={styles.th}>Date</th>
              <th style={styles.th}>Provider</th>
              <th style={styles.th}>Purpose</th>
              <th style={styles.th}>Amount</th>
              <th style={styles.th}>Status</th>
            </tr>
          </thead>
          <tbody>
            {loadingHistory ? (
              <tr><td style={styles.td} colSpan={5}>Loading…</td></tr>
            ) : history.length === 0 ? (
              <tr><td style={styles.td} colSpan={5}>No payments yet.</td></tr>
            ) : (
              history.map((p) => (
                <tr key={p.reference}>
                  <td style={styles.td}>{new Date(p.created_at).toLocaleDateString()}</td>
                  <td style={styles.td}>{p.provider}</td>
                  <td style={styles.td}>{p.purpose}</td>
                  <td className="mono" style={styles.td}>{p.amount.toFixed(2)} {p.currency.toUpperCase()}</td>
                  <td style={styles.td}>
                    <span className={`status-pill ${p.status === 'completed' ? 'status-active' : p.status === 'failed' ? 'status-inactive' : ''}`}>
                      {p.status}
                    </span>
                  </td>
                </tr>
              ))
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}

// --------------------------------------------------------------- Backup

function BackupTab() {
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [downloaded, setDownloaded] = useState(false);

  const [restoreFile, setRestoreFile] = useState<{ database_base64: string; key_hex: string; created_at?: string; name: string } | null>(null);
  const [confirmText, setConfirmText] = useState('');
  const [restoring, setRestoring] = useState(false);
  const [restoreStaged, setRestoreStaged] = useState(false);
  const [restoreError, setRestoreError] = useState<string | null>(null);

  async function handleCreateBackup() {
    setCreating(true);
    setError(null);
    setDownloaded(false);
    try {
      const data = await createBackup();
      const blob = new Blob([JSON.stringify(data)], { type: 'application/json' });
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      const dateStamp = new Date(data.created_at).toISOString().slice(0, 10);
      a.href = url;
      a.download = `sme-pro-backup-${dateStamp}.json`;
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      URL.revokeObjectURL(url);
      setDownloaded(true);
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not create the backup');
    } finally {
      setCreating(false);
    }
  }

  function handleFileSelect(e: React.ChangeEvent<HTMLInputElement>) {
    setRestoreError(null);
    setRestoreStaged(false);
    setConfirmText('');
    const file = e.target.files?.[0];
    if (!file) return;
    const reader = new FileReader();
    reader.onload = () => {
      try {
        const parsed = JSON.parse(reader.result as string);
        if (!parsed.database_base64 || !parsed.key_hex) {
          setRestoreError('This does not look like a valid SME Pro backup file.');
          return;
        }
        setRestoreFile({ ...parsed, name: file.name });
      } catch {
        setRestoreError('Could not read this file — it may be corrupted or not a valid backup.');
      }
    };
    reader.readAsText(file);
  }

  async function handleRestore() {
    if (!restoreFile) return;
    setRestoring(true);
    setRestoreError(null);
    try {
      await restoreBackup({ database_base64: restoreFile.database_base64, key_hex: restoreFile.key_hex });
      setRestoreStaged(true);
    } catch (err) {
      setRestoreError(err instanceof ApiError ? err.message : 'Could not restore this backup');
    } finally {
      setRestoring(false);
    }
  }

  async function handleRestartNow() {
    try {
      const { relaunch } = await import('@tauri-apps/plugin-process');
      await relaunch();
    } catch {
      // Not running inside the desktop app (e.g. browser dev mode) —
      // nothing more this can do; the message below already covers it.
    }
  }

  return (
    <div>
      <div className="card" style={{ marginBottom: '1rem' }}>
        <h3 style={{ marginTop: 0 }}>Back up your business</h3>
        <p style={{ fontSize: '0.85rem', color: 'var(--ink-soft)', lineHeight: 1.5 }}>
          Downloads your entire business database — every record, every user, every setting —
          as one file, still encrypted. Store it somewhere safe: a cloud drive, a USB stick,
          anywhere other than only this computer. If this machine is ever lost, stolen, or its
          disk fails, this file is how you get everything back.
        </p>
        <ErrorBox error={error} />
        {downloaded && <div style={{ color: 'var(--ok)', fontSize: '0.85rem', marginBottom: '0.7rem' }}>Backup downloaded.</div>}
        <button className="btn btn-stamp" onClick={handleCreateBackup} disabled={creating}>
          {creating ? 'Creating backup…' : 'Download backup now'}
        </button>
      </div>

      <div className="card">
        <h3 style={{ marginTop: 0 }}>Restore from a backup</h3>
        <p style={{ fontSize: '0.85rem', color: 'var(--ink-soft)', lineHeight: 1.5 }}>
          <strong>This replaces everything currently in this app</strong> with what's in the
          backup file — every record, every user, every setting reverts to exactly how it was
          when that backup was made. Anything created since then is gone. Only do this if
          that's really what you want.
        </p>
        <ErrorBox error={restoreError} />

        {!restoreStaged ? (
          <>
            <input type="file" accept=".json,application/json" onChange={handleFileSelect} style={{ marginBottom: '0.8rem' }} />
            {restoreFile && (
              <div style={styles.restorePreview}>
                <div style={{ fontSize: '0.85rem' }}>
                  <strong>{restoreFile.name}</strong>
                  {restoreFile.created_at && (
                    <span style={{ color: 'var(--ink-soft)' }}> — backed up {new Date(restoreFile.created_at).toLocaleString()}</span>
                  )}
                </div>
                <label style={{ display: 'block', marginTop: '0.8rem' }}>
                  Type <span className="mono">RESTORE</span> to confirm — this cannot be undone
                </label>
                <input value={confirmText} onChange={(e) => setConfirmText(e.target.value)} style={{ width: '100%', marginTop: '0.3rem' }} />
                <button
                  className="btn btn-stamp"
                  style={{ marginTop: '0.8rem' }}
                  disabled={confirmText !== 'RESTORE' || restoring}
                  onClick={handleRestore}
                >
                  {restoring ? 'Restoring…' : 'Restore this backup'}
                </button>
              </div>
            )}
          </>
        ) : (
          <div>
            <div style={{ color: 'var(--ok)', fontSize: '0.9rem', fontWeight: 600, marginBottom: '0.5rem' }}>
              Backup staged successfully.
            </div>
            <p style={{ fontSize: '0.85rem', color: 'var(--ink-soft)' }}>
              The restore will finish the next time the app starts. Restart now to complete it.
            </p>
            <button className="btn btn-stamp" onClick={handleRestartNow}>Restart now</button>
          </div>
        )}
      </div>
    </div>
  );
}


// ------------------------------------------------------------ Audit Log

function AuditLogTab() {
  const [entries, setEntries] = useState<AuditLogEntry[]>([]);
  const [users, setUsers] = useState<Record<string, string>>({});
  const [moduleFilter, setModuleFilter] = useState('');
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    listUsers().then((r) => {
      const map: Record<string, string> = {};
      r.users.forEach((u: UserAccountLike) => { map[u.id] = u.username; });
      setUsers(map);
    }).catch(() => {});
  }, []);

  useEffect(() => {
    setLoading(true);
    setError(null);
    getAuditLog(moduleFilter || undefined)
      .then((r) => setEntries(r.entries))
      .catch((err) => setError(err instanceof ApiError ? err.message : 'Could not load the audit log'))
      .finally(() => setLoading(false));
  }, [moduleFilter]);

  const moduleOptions = Array.from(new Set(entries.map((e) => e.module_id))).sort();

  return (
    <div>
      <p style={{ fontSize: '0.85rem', color: 'var(--ink-soft)', marginTop: 0 }}>
        Every create, update, delete, and admin action anywhere in this app, automatically —
        not something anyone has to remember to turn on. This is the full accountability
        trail: who did what, and when.
      </p>
      <div style={{ marginBottom: '0.8rem' }}>
        <label>Filter by module</label>
        <select value={moduleFilter} onChange={(e) => setModuleFilter(e.target.value)} style={{ marginLeft: '0.6rem' }}>
          <option value="">All</option>
          {moduleOptions.map((m) => <option key={m} value={m}>{m}</option>)}
        </select>
      </div>
      <ErrorBox error={error} />
      <div className="card" style={{ padding: 0, overflowX: 'auto' }}>
        <table style={styles.table}>
          <thead>
            <tr>
              <th style={styles.th}>When</th>
              <th style={styles.th}>Who</th>
              <th style={styles.th}>Module</th>
              <th style={styles.th}>Action</th>
              <th style={styles.th}>Details</th>
            </tr>
          </thead>
          <tbody>
            {loading ? (
              <tr><td style={styles.td} colSpan={5}>Loading…</td></tr>
            ) : entries.length === 0 ? (
              <tr><td style={styles.td} colSpan={5}>No activity recorded yet.</td></tr>
            ) : (
              entries.map((e) => (
                <tr key={e.id}>
                  <td style={styles.td} className="mono">{new Date(e.timestamp).toLocaleString()}</td>
                  <td style={styles.td}>{e.user_id ? (users[e.user_id] ?? 'Unknown user') : 'System'}</td>
                  <td style={styles.td}>{e.module_id}</td>
                  <td style={styles.td}>{e.action}</td>
                  <td style={styles.td} className="mono">
                    {e.details ? JSON.stringify(e.details).slice(0, 80) : e.record_id ? `record: ${e.record_id.slice(0, 8)}…` : '—'}
                  </td>
                </tr>
              ))
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}

type UserAccountLike = { id: string; username: string };

// -------------------------------------------------------- Notifications

function NotificationsTab() {
  const [channel, setChannel] = useState<'whatsapp' | 'sms'>('whatsapp');
  const [recipient, setRecipient] = useState('');
  const [message, setMessage] = useState('');
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [sent, setSent] = useState(false);
  const [history, setHistory] = useState<NotificationRecord[]>([]);
  const [loadingHistory, setLoadingHistory] = useState(true);

  const refresh = () => {
    setLoadingHistory(true);
    listNotifications().then((r) => setHistory(r.notifications)).catch(() => {}).finally(() => setLoadingHistory(false));
  };
  useEffect(() => { refresh(); }, []);

  async function handleSend(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    setSent(false);
    setSending(true);
    try {
      await sendNotification(channel, recipient, message);
      setMessage('');
      setSent(true);
      refresh();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not send this message');
    } finally {
      setSending(false);
    }
  }

  return (
    <div>
      <div className="card" style={{ marginBottom: '1rem' }}>
        <h3 style={{ marginTop: 0 }}>Send a message</h3>
        <p style={{ fontSize: '0.8rem', color: 'var(--ink-soft)', marginTop: 0 }}>
          Sends a real WhatsApp or SMS message via this business's configured Twilio account.
          Without one configured, messages are logged here but not actually delivered — ask
          whoever set up this install whether that's connected yet.
        </p>
        <ErrorBox error={error} />
        {sent && <div style={{ color: 'var(--ok)', fontSize: '0.85rem', marginBottom: '0.7rem' }}>Sent.</div>}
        <form onSubmit={handleSend} style={styles.formGrid}>
          <div>
            <label>Channel</label>
            <select value={channel} onChange={(e) => setChannel(e.target.value as 'whatsapp' | 'sms')} style={{ width: '100%' }}>
              <option value="whatsapp">WhatsApp</option>
              <option value="sms">SMS</option>
            </select>
          </div>
          <div>
            <label>Recipient phone number</label>
            <input value={recipient} onChange={(e) => setRecipient(e.target.value)} placeholder="+2547XXXXXXXX" required style={{ width: '100%' }} />
          </div>
          <div style={{ gridColumn: '1 / -1' }}>
            <label>Message</label>
            <textarea value={message} onChange={(e) => setMessage(e.target.value)} required rows={3} style={{ width: '100%' }} />
          </div>
        </form>
        <button className="btn btn-stamp" style={{ marginTop: '0.8rem' }} onClick={handleSend} disabled={sending}>
          {sending ? 'Sending…' : 'Send'}
        </button>
      </div>

      <h3 style={{ margin: '1.2rem 0 0.8rem' }}>Recent messages</h3>
      <div className="card" style={{ padding: 0, overflowX: 'auto' }}>
        <table style={styles.table}>
          <thead>
            <tr>
              <th style={styles.th}>When</th>
              <th style={styles.th}>Channel</th>
              <th style={styles.th}>To</th>
              <th style={styles.th}>Message</th>
              <th style={styles.th}>Status</th>
            </tr>
          </thead>
          <tbody>
            {loadingHistory ? (
              <tr><td style={styles.td} colSpan={5}>Loading…</td></tr>
            ) : history.length === 0 ? (
              <tr><td style={styles.td} colSpan={5}>No messages sent yet.</td></tr>
            ) : (
              history.map((n) => (
                <tr key={n.id}>
                  <td style={styles.td} className="mono">{new Date(n.created_at).toLocaleString()}</td>
                  <td style={styles.td}>{n.channel}</td>
                  <td style={styles.td} className="mono">{n.recipient}</td>
                  <td style={styles.td}>{n.message.length > 60 ? n.message.slice(0, 60) + '…' : n.message}</td>
                  <td style={styles.td}>
                    <span className={`status-pill ${n.status === 'sent' ? 'status-active' : ''}`}>{n.status}</span>
                  </td>
                </tr>
              ))
            )}
          </tbody>
        </table>
      </div>
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
  restorePreview: { padding: '0.9rem', border: '1px solid var(--paper-line)', borderRadius: 3, background: 'var(--stamp-wash)' },
};
