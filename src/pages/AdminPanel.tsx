import { useEffect, useState } from 'react';
import {
  listRoles, createRole, deleteRole, setRoleAdminFlag, getRolePermissions, setRolePermissions,
  listUsers, createUser, setUserRole, deactivateUser,
  listUnits, createUnit, deleteUnit,
  listCurrencies, createCurrency, deleteCurrency,
  getSettings, setSetting,
  getAiSettings,
  createBackup, restoreBackup,
  getAuditLog,
  listNotifications, sendNotification, sendLowStockAlert,
  getCurrencyRates, convertCurrency, refreshCurrencyRates,
  listTaxRates, setTaxRate, computeTax,
  changeBusinessType,
  listModules, listAvailableModules, getModuleSchema, enableModule, disableModule,
  ApiError,
} from '../api';
import type { AuditLogEntry, NotificationRecord, AiSettingsStatus, CurrencyRate, TaxComputeItem, TaxComputeResult, AvailableModule } from '../api';
import type { Role, UserAccount, Unit, Currency, ModuleListItem } from '../types';
import { formatMoney, parseMoneyInput } from '../lib/money';
import { parseBackendTimestamp } from '../lib/date';
import BusinessBranding from '../components/BusinessBranding';
import TwoFactorSetup from '../components/TwoFactorSetup';

export type Tab = 'roles' | 'users' | 'units' | 'currencies' | 'tax' | 'settings' | 'backup' | 'audit' | 'notifications' | 'business' | 'security' | 'ai' | 'network';

export const ADMIN_TABS: { id: Tab; label: string }[] = [
  { id: 'roles', label: 'Roles' },
  { id: 'users', label: 'Users' },
  { id: 'units', label: 'Units' },
  { id: 'currencies', label: 'Currencies' },
  { id: 'tax', label: 'Tax Rates' },
  { id: 'settings', label: 'Theme & Settings' },
  { id: 'business', label: 'Business' },
  { id: 'ai', label: 'AI Settings' },
  { id: 'security', label: 'Security' },
  { id: 'network', label: 'Network' },
  { id: 'notifications', label: 'Notifications' },
  { id: 'backup', label: 'Backup & Restore' },
  { id: 'audit', label: 'Audit Log' },
];

// Navigation into a specific admin section now lives entirely in the
// sidebar (a collapsible "Admin" group, the same pattern as
// "Operations" — see Sidebar.tsx), not in a horizontal tab-strip
// crammed at the top of this page. That strip used to wrap across
// many rows on a phone screen before any actual content was visible
// — genuinely "no space left for navigation" on mobile, which is
// exactly the problem this removes. `tab` is now fully controlled by
// whatever the sidebar has selected.
export default function AdminPanel({ tab, onModulesChanged }: { tab: Tab; onModulesChanged?: () => void }) {
  return (
    <div>
      <h2 style={{ marginTop: 0 }}>{ADMIN_TABS.find((t) => t.id === tab)?.label ?? 'Admin'}</h2>

      {tab === 'roles' && <RolesTab />}
      {tab === 'users' && <UsersTab />}
      {tab === 'units' && <UnitsTab />}
      {tab === 'currencies' && <CurrenciesTab />}
      {tab === 'tax' && <TaxRatesTab />}
      {tab === 'settings' && <SettingsTab />}
      {tab === 'business' && <BusinessTab onModulesChanged={onModulesChanged} />}
      {tab === 'security' && <TwoFactorSetup />}
      {tab === 'network' && <NetworkTab />}
      {tab === 'ai' && <AiSettingsTab />}
      {tab === 'notifications' && <NotificationsTab />}
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
      <select value={moduleId} onChange={(e) => setModuleId(e.target.value)} style={{ width: 'auto', marginBottom: '0.8rem' }}>
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

  // Exchange rates + converter — see currency.rs. Rates are cached
  // server-side and only refreshed on request, not on every load.
  const [ratesBase, setRatesBase] = useState('USD');
  const [rates, setRates] = useState<CurrencyRate[]>([]);
  const [ratesStale, setRatesStale] = useState(false);
  const [ratesLoading, setRatesLoading] = useState(false);
  const [ratesError, setRatesError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  const [convAmountText, setConvAmountText] = useState('100.00');
  const [convFrom, setConvFrom] = useState('USD');
  const [convTo, setConvTo] = useState('USD');
  const [convResult, setConvResult] = useState<string | null>(null);
  const [convError, setConvError] = useState<string | null>(null);
  const [converting, setConverting] = useState(false);

  const refresh = () => listCurrencies().then((r) => setCurrencies(r.currencies)).catch(() => {});
  useEffect(() => { refresh(); }, []);

  function loadRates(base: string) {
    setRatesLoading(true);
    setRatesError(null);
    getCurrencyRates(base)
      .then((r) => { setRates(r.rates); setRatesStale(r.stale); })
      .catch((err) => setRatesError(err instanceof ApiError ? err.message : 'Could not load exchange rates'))
      .finally(() => setRatesLoading(false));
  }
  useEffect(() => { loadRates(ratesBase); }, [ratesBase]);

  async function handleRefreshRates() {
    setRefreshing(true);
    setRatesError(null);
    try {
      await refreshCurrencyRates(ratesBase);
      loadRates(ratesBase);
    } catch (err) {
      // require_owner on the backend means a non-owner sees a clear
      // 403 here rather than a hidden/disabled button — simpler than
      // duplicating the ownership check client-side for a rarely-hit
      // permission edge case in an already admin-only tab.
      setRatesError(err instanceof ApiError ? err.message : 'Could not refresh rates');
    } finally {
      setRefreshing(false);
    }
  }

  async function handleConvert() {
    setConvError(null);
    setConvResult(null);
    const cents = parseMoneyInput(convAmountText, convFrom);
    if (cents === null) {
      setConvError('Enter a valid amount.');
      return;
    }
    setConverting(true);
    try {
      const { result } = await convertCurrency(convFrom, convTo, cents);
      setConvResult(`${formatMoney(cents, convFrom)} ${convFrom} = ${formatMoney(result, convTo)} ${convTo}`);
    } catch (err) {
      setConvError(err instanceof ApiError ? err.message : 'Could not convert — is a rate available for this pair?');
    } finally {
      setConverting(false);
    }
  }

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

      <div className="card" style={{ marginTop: '1.2rem' }}>
        <h3 style={{ marginTop: 0 }}>Exchange rates</h3>
        <div style={{ display: 'flex', gap: '0.6rem', alignItems: 'flex-end', marginBottom: '0.8rem' }}>
          <div style={{ width: 100 }}>
            <label>Base currency</label>
            <input value={ratesBase} onChange={(e) => setRatesBase(e.target.value.toUpperCase())} style={{ width: '100%' }} />
          </div>
          <button className="btn btn-outline" onClick={handleRefreshRates} disabled={refreshing}>
            {refreshing ? 'Refreshing…' : 'Refresh rates'}
          </button>
          {ratesStale && !ratesLoading && (
            <span style={{ fontSize: '0.78rem', color: 'var(--stamp)' }}>These rates look stale — consider refreshing.</span>
          )}
        </div>
        <ErrorBox error={ratesError} />
        {ratesLoading ? (
          <div style={{ fontSize: '0.85rem', color: 'var(--ink-soft)' }}>Loading…</div>
        ) : rates.length === 0 ? (
          <div style={{ fontSize: '0.85rem', color: 'var(--ink-soft)' }}>No cached rates for {ratesBase} yet — try refreshing.</div>
        ) : (
          <table style={styles.table}>
            <thead><tr><th style={styles.th}>To</th><th style={styles.th}>Rate</th><th style={styles.th}>Fetched</th></tr></thead>
            <tbody>
              {rates.map((r) => (
                <tr key={r.to_currency}>
                  <td className="mono" style={styles.td}>{r.to_currency}</td>
                  <td className="mono" style={styles.td}>{r.rate}</td>
                  {/* fetched_at is Unix epoch SECONDS from the backend
                      (currency.rs's SystemTime::…as_secs()) — JS Date
                      expects MILLISECONDS, so this was previously
                      rendering dates near January 1970 instead of the
                      real fetch time. */}
                  <td style={styles.td}>{new Date(r.fetched_at * 1000).toLocaleString()}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      <div className="card" style={{ marginTop: '1.2rem' }}>
        <h3 style={{ marginTop: 0 }}>Convert</h3>
        <div style={{ display: 'flex', gap: '0.6rem', alignItems: 'flex-end', flexWrap: 'wrap' }}>
          <div style={{ width: 120 }}>
            <label>Amount</label>
            <input
              type="text"
              inputMode="decimal"
              value={convAmountText}
              onChange={(e) => setConvAmountText(e.target.value)}
              style={{ width: '100%' }}
            />
          </div>
          <div style={{ width: 90 }}>
            <label>From</label>
            <input value={convFrom} onChange={(e) => setConvFrom(e.target.value.toUpperCase())} style={{ width: '100%' }} />
          </div>
          <div style={{ width: 90 }}>
            <label>To</label>
            <input value={convTo} onChange={(e) => setConvTo(e.target.value.toUpperCase())} style={{ width: '100%' }} />
          </div>
          <button className="btn btn-stamp" onClick={handleConvert} disabled={converting}>
            {converting ? 'Converting…' : 'Convert'}
          </button>
        </div>
        {convError && <div style={{ color: 'var(--stamp)', fontSize: '0.85rem', marginTop: '0.5rem' }}>{convError}</div>}
        {convResult && <div style={{ fontSize: '0.9rem', marginTop: '0.5rem', fontWeight: 600 }}>{convResult}</div>}
      </div>
    </div>
  );
}

// ----------------------------------------------------------- Tax Rates

function TaxRatesTab() {
  const [rates, setRates] = useState<{ category: string; rate: number }[]>([]);
  const [category, setCategory] = useState('');
  const [rateText, setRateText] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const refresh = () => listTaxRates().then((r) => setRates(r.rates)).catch(() => {});
  useEffect(() => { refresh(); }, []);

  async function handleSetRate(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    const rate = parseFloat(rateText);
    if (!category.trim() || Number.isNaN(rate) || rate < 0) {
      setError('Enter a category name and a rate (e.g. 16 for 16%).');
      return;
    }
    setSaving(true);
    try {
      await setTaxRate(category.trim(), rate);
      setCategory('');
      setRateText('');
      await refresh();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not save this rate — owner permission required.');
    } finally {
      setSaving(false);
    }
  }

  // Calculator/preview state
  const [calcItems, setCalcItems] = useState<{ category: string; unit_price_text: string; quantity: number }[]>([
    { category: '', unit_price_text: '0.00', quantity: 1 },
  ]);
  const [taxInclusive, setTaxInclusive] = useState(false);
  const [calcResult, setCalcResult] = useState<TaxComputeResult | null>(null);
  const [calcError, setCalcError] = useState<string | null>(null);
  const [calculating, setCalculating] = useState(false);

  function updateCalcItem(i: number, patch: Partial<{ category: string; unit_price_text: string; quantity: number }>) {
    setCalcItems((prev) => prev.map((it, idx) => (idx === i ? { ...it, ...patch } : it)));
  }

  async function handleCompute() {
    setCalcError(null);
    setCalcResult(null);
    const parsedItems: TaxComputeItem[] = [];
    for (const it of calcItems) {
      if (!it.category.trim()) continue;
      const cents = parseMoneyInput(it.unit_price_text, 'USD');
      if (cents === null || !(it.quantity > 0)) {
        setCalcError(`"${it.category}" has an invalid price or quantity.`);
        return;
      }
      parsedItems.push({ category: it.category.trim(), unit_price: cents, quantity: it.quantity });
    }
    if (parsedItems.length === 0) {
      setCalcError('Add at least one line item with a category.');
      return;
    }
    setCalculating(true);
    try {
      setCalcResult(await computeTax(parsedItems, taxInclusive));
    } catch (err) {
      setCalcError(err instanceof ApiError ? err.message : 'Could not compute tax for these items');
    } finally {
      setCalculating(false);
    }
  }

  return (
    <div>
      <div className="card" style={{ borderColor: 'var(--stamp)', marginBottom: '1rem' }}>
        <strong style={{ color: 'var(--stamp)' }}>Heads up:</strong>{' '}
        <span style={{ fontSize: '0.85rem' }}>
          These per-category tax rates are <strong>not currently applied</strong> to real sales or invoices —
          Point of Sale and Invoicing both use a single flat tax rate instead, set under Admin → Business.
          What's below lets you configure and preview category rates, but changing them here won't change
          what a customer is actually charged yet. If you need per-category tax to actually apply at
          checkout, that's a real feature to build deliberately — ask, don't assume this page already does it.
        </span>
      </div>

      <div className="card" style={{ marginBottom: '1rem' }}>
        <h3 style={{ marginTop: 0 }}>Category rates</h3>
        <form onSubmit={handleSetRate} style={{ display: 'flex', gap: '0.6rem', alignItems: 'flex-end', marginBottom: '0.8rem' }}>
          <div style={{ flex: 1 }}>
            <label>Category</label>
            <input value={category} onChange={(e) => setCategory(e.target.value)} placeholder="Food" style={{ width: '100%' }} />
          </div>
          <div style={{ width: 100 }}>
            <label>Rate (%)</label>
            <input value={rateText} onChange={(e) => setRateText(e.target.value)} placeholder="16" style={{ width: '100%' }} />
          </div>
          <button className="btn btn-stamp" type="submit" disabled={saving}>{saving ? 'Saving…' : 'Set rate'}</button>
        </form>
        <ErrorBox error={error} />
        <table style={styles.table}>
          <thead><tr><th style={styles.th}>Category</th><th style={styles.th}>Rate</th></tr></thead>
          <tbody>
            {rates.length === 0 && <tr><td colSpan={2} style={styles.td}>No category rates set yet.</td></tr>}
            {rates.map((r) => (
              <tr key={r.category}>
                <td style={styles.td}>{r.category}</td>
                <td className="mono" style={styles.td}>{r.rate}%</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <div className="card">
        <h3 style={{ marginTop: 0 }}>Preview calculator</h3>
        {calcItems.map((it, i) => (
          <div key={i} style={{ display: 'flex', gap: '0.5rem', marginBottom: '0.5rem', alignItems: 'flex-end' }}>
            <div style={{ flex: 1 }}>
              <label>Category</label>
              <input value={it.category} onChange={(e) => updateCalcItem(i, { category: e.target.value })} style={{ width: '100%' }} />
            </div>
            <div style={{ width: 100 }}>
              <label>Unit price</label>
              <input
                type="text"
                inputMode="decimal"
                value={it.unit_price_text}
                onChange={(e) => updateCalcItem(i, { unit_price_text: e.target.value })}
                style={{ width: '100%' }}
              />
            </div>
            <div style={{ width: 70 }}>
              <label>Qty</label>
              <input
                type="number"
                min={1}
                value={it.quantity}
                onChange={(e) => updateCalcItem(i, { quantity: parseInt(e.target.value, 10) || 1 })}
                style={{ width: '100%' }}
              />
            </div>
          </div>
        ))}
        <button
          className="btn btn-outline"
          type="button"
          onClick={() => setCalcItems((prev) => [...prev, { category: '', unit_price_text: '0.00', quantity: 1 }])}
          style={{ marginBottom: '0.8rem' }}
        >
          + Add line
        </button>

        <label style={{ display: 'flex', alignItems: 'center', gap: '0.4rem', marginBottom: '0.8rem' }}>
          <input type="checkbox" checked={taxInclusive} onChange={(e) => setTaxInclusive(e.target.checked)} />
          Prices already include tax
        </label>

        <button className="btn btn-stamp" onClick={handleCompute} disabled={calculating}>
          {calculating ? 'Computing…' : 'Compute'}
        </button>

        {calcError && <div style={{ color: 'var(--stamp)', fontSize: '0.85rem', marginTop: '0.6rem' }}>{calcError}</div>}

        {calcResult && (
          <div style={{ marginTop: '0.8rem', fontSize: '0.85rem' }}>
            {calcResult.lines.map((l) => (
              <div key={l.category} style={{ display: 'flex', justifyContent: 'space-between' }}>
                <span>{l.category} ({l.rate}%)</span>
                <span className="mono">{formatMoney(l.taxable_amount, 'USD')} + {formatMoney(l.tax_amount, 'USD')} tax</span>
              </div>
            ))}
            <div style={{ borderTop: '1px solid var(--paper-line)', marginTop: '0.5rem', paddingTop: '0.5rem', fontWeight: 600, display: 'flex', justifyContent: 'space-between' }}>
              <span>Total</span>
              <span className="mono">{formatMoney(calcResult.total, 'USD')}</span>
            </div>
          </div>
        )}
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

function BusinessTab({ onModulesChanged }: { onModulesChanged?: () => void }) {
  const [modules, setModules] = useState<AvailableModule[]>([]);
  const [loading, setLoading] = useState(true);
  const [enablingId, setEnablingId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = () => {
    setLoading(true);
    listAvailableModules()
      .then((r) => setModules(r.modules))
      .catch(() => setError('Could not load the module list'))
      .finally(() => setLoading(false));
  };
  useEffect(refresh, []);

  async function handleEnable(moduleId: string) {
    setEnablingId(moduleId);
    setError(null);
    try {
      await enableModule(moduleId);
      refresh();
      // This screen's own `modules` list (above) is local to
      // BusinessTab and refreshing it here only updates what THIS
      // screen shows. The sidebar's "Operations" section (and the
      // gate on whether POS itself is even reachable — see App.tsx's
      // `modules.some(id === 'inventory' && enabled)` check) is driven
      // by a completely separate `modules` state living in App.tsx,
      // fetched exactly once at login and never again — so without
      // this, enabling a module here updates the database correctly
      // but the sidebar has no way of finding out, and looks like
      // "add module" silently did nothing until a full reload.
      onModulesChanged?.();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : `Could not enable ${moduleId}`);
    } finally {
      setEnablingId(null);
    }
  }

  async function handleDisable(moduleId: string) {
    setEnablingId(moduleId);
    setError(null);
    try {
      await disableModule(moduleId);
      refresh();
      onModulesChanged?.(); // same reasoning as handleEnable above
    } catch (err) {
      setError(err instanceof ApiError ? err.message : `Could not disable ${moduleId}`);
    } finally {
      setEnablingId(null);
    }
  }

  return (
    <div>
      <BusinessBranding />

      <div className="card" style={{ marginTop: '1.2rem' }}>
        <h3 style={{ marginTop: 0 }}>Additional modules</h3>
        <p style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>
          Your business type enables a sensible starting set — any of these can be turned on
          individually too, whether or not your type's preset included them.
        </p>
        {loading ? (
          <div style={{ fontSize: '0.85rem', color: 'var(--ink-soft)' }}>Loading…</div>
        ) : (
          modules.map((m) => (
            <div key={m.id} style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '0.6rem 0', borderTop: '1px solid var(--paper-line)' }}>
              <div>
                <strong>{m.display_name}</strong>
              </div>
              {m.enabled ? (
                <button className="btn btn-outline" onClick={() => handleDisable(m.id)} disabled={enablingId === m.id}>
                  {enablingId === m.id ? 'Disabling…' : 'Disable'}
                </button>
              ) : (
                <button className="btn" onClick={() => handleEnable(m.id)} disabled={enablingId === m.id}>
                  {enablingId === m.id ? 'Enabling…' : 'Enable'}
                </button>
              )}
            </div>
          ))
        )}
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

// --------------------------------------------------------------- Backup

function BackupTab() {
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [downloaded, setDownloaded] = useState(false);
  const [backupPassphrase, setBackupPassphrase] = useState('');

  const [restoreFile, setRestoreFile] = useState<{ database_base64: string; wrapped_key_base64: string; created_at?: string; name: string } | null>(null);
  const [restorePassphrase, setRestorePassphrase] = useState('');
  const [confirmText, setConfirmText] = useState('');
  const [restoring, setRestoring] = useState(false);
  const [restoreStaged, setRestoreStaged] = useState(false);
  const [restoreError, setRestoreError] = useState<string | null>(null);

  async function handleCreateBackup() {
    setCreating(true);
    setError(null);
    setDownloaded(false);
    try {
      const data = await createBackup(backupPassphrase);
      const blob = new Blob([JSON.stringify(data)], { type: 'application/json' });
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      const dateStamp = parseBackendTimestamp(data.created_at).toISOString().slice(0, 10);
      a.href = url;
      a.download = `sme-pro-backup-${dateStamp}.json`;
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      URL.revokeObjectURL(url);
      setDownloaded(true);
      setBackupPassphrase('');
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
    setRestorePassphrase('');
    const file = e.target.files?.[0];
    if (!file) return;
    const reader = new FileReader();
    reader.onload = () => {
      try {
        const parsed = JSON.parse(reader.result as string);
        if (!parsed.database_base64 || !parsed.wrapped_key_base64) {
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
      await restoreBackup({ database_base64: restoreFile.database_base64, wrapped_key_base64: restoreFile.wrapped_key_base64, passphrase: restorePassphrase });
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
        <label style={{ display: 'block', marginBottom: '0.4rem' }}>
          Backup passphrase (at least 8 characters)
        </label>
        <p style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', marginTop: 0, marginBottom: '0.5rem' }}>
          Protects this specific file — not your login password. Anyone who ever gets hold of the
          downloaded file cannot open it without this passphrase too. Write it down somewhere
          separate from the file itself; there is no way to recover it if it's lost.
        </p>
        <input
          type="password"
          value={backupPassphrase}
          onChange={(e) => setBackupPassphrase(e.target.value)}
          style={{ width: '100%', marginBottom: '0.8rem' }}
        />
        <button className="btn btn-stamp" onClick={handleCreateBackup} disabled={creating || backupPassphrase.length < 8}>
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
                    <span style={{ color: 'var(--ink-soft)' }}> — backed up {parseBackendTimestamp(restoreFile.created_at).toLocaleString()}</span>
                  )}
                </div>
                <label style={{ display: 'block', marginTop: '0.8rem' }}>
                  Backup passphrase
                </label>
                <input
                  type="password"
                  value={restorePassphrase}
                  onChange={(e) => setRestorePassphrase(e.target.value)}
                  style={{ width: '100%', marginTop: '0.3rem' }}
                />
                <label style={{ display: 'block', marginTop: '0.8rem' }}>
                  Type <span className="mono">RESTORE</span> to confirm — this cannot be undone
                </label>
                <input value={confirmText} onChange={(e) => setConfirmText(e.target.value)} style={{ width: '100%', marginTop: '0.3rem' }} />
                <button
                  className="btn btn-stamp"
                  style={{ marginTop: '0.8rem' }}
                  disabled={confirmText !== 'RESTORE' || restoring || !restorePassphrase}
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
        <select value={moduleFilter} onChange={(e) => setModuleFilter(e.target.value)} style={{ width: 'auto', marginLeft: '0.6rem' }}>
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
                  <td style={styles.td} className="mono">{parseBackendTimestamp(e.timestamp).toLocaleString()}</td>
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
  // Separate submitting/error state from the manual send form above —
  // the two actions can be triggered independently, and mixing their
  // in-flight/error state would show a spinner or error message on
  // the wrong button while the other one is what's actually running.
  const [sendingLowStock, setSendingLowStock] = useState(false);
  const [lowStockError, setLowStockError] = useState<string | null>(null);
  const [lowStockSent, setLowStockSent] = useState(false);

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

  async function handleSendLowStock() {
    if (!recipient) {
      setLowStockError('Enter a recipient phone number above first.');
      return;
    }
    setLowStockError(null);
    setLowStockSent(false);
    setSendingLowStock(true);
    try {
      // The message itself is composed server-side from the same
      // low-stock data the AI assistant and Dashboard already use —
      // see notifications::send_low_stock_alert — so there's nothing
      // else to gather here beyond channel + recipient.
      await sendLowStockAlert(channel, recipient);
      setLowStockSent(true);
      refresh();
    } catch (err) {
      setLowStockError(err instanceof ApiError ? err.message : 'Could not send the low-stock alert');
    } finally {
      setSendingLowStock(false);
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

      <div className="card" style={{ marginBottom: '1rem' }}>
        <h3 style={{ marginTop: 0 }}>Send low-stock alert</h3>
        <p style={{ fontSize: '0.8rem', color: 'var(--ink-soft)', marginTop: 0 }}>
          Sends a message to the recipient above listing every item currently at or below its reorder
          level, across all your modules — the same data the Dashboard and AI assistant already use, so
          there's nothing else to fill in beyond the channel and recipient above.
        </p>
        {lowStockError && <div style={{ color: 'var(--stamp)', fontSize: '0.85rem', marginBottom: '0.6rem' }}>{lowStockError}</div>}
        {lowStockSent && <div style={{ color: 'var(--ok)', fontSize: '0.85rem', marginBottom: '0.6rem' }}>Sent.</div>}
        <button className="btn btn-outline" onClick={handleSendLowStock} disabled={sendingLowStock}>
          {sendingLowStock ? 'Sending…' : 'Send low-stock alert'}
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
                  <td style={styles.td} className="mono">{parseBackendTimestamp(n.created_at).toLocaleString()}</td>
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

// --------------------------------------------------------- AI Settings

const PROVIDERS = [
  { id: 'nvidia', label: 'NVIDIA NIM', free: true, url: 'https://build.nvidia.com', keyField: 'nvidia_key_set' as const },
  { id: 'gemini', label: 'Google Gemini', free: true, url: 'https://aistudio.google.com', keyField: 'gemini_key_set' as const },
  { id: 'openai', label: 'OpenAI', free: false, url: 'https://platform.openai.com/api-keys', keyField: 'openai_key_set' as const },
  { id: 'claude', label: 'Claude (Anthropic)', free: false, url: 'https://console.anthropic.com', keyField: 'claude_key_set' as const },
];

function AiSettingsTab() {
  const [status, setStatus] = useState<AiSettingsStatus | null>(null);
  const [provider, setProvider] = useState('nvidia');
  const [keyInputs, setKeyInputs] = useState<Record<string, string>>({});
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    getAiSettings()
      .then((s) => { setStatus(s); setProvider(s.provider); })
      .catch(() => setError('Could not load AI settings'))
      .finally(() => setLoading(false));
  }, []);

  async function saveProvider(id: string) {
    setSaving('provider');
    setError(null);
    try {
      await setSetting('ai_provider', id);
      setProvider(id);
      setStatus((s) => (s ? { ...s, provider: id } : s));
      setSaved('provider');
      setTimeout(() => setSaved(null), 2000);
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not save');
    } finally {
      setSaving(null);
    }
  }

  async function saveKey(providerId: string) {
    const key = keyInputs[providerId]?.trim();
    if (!key) return;
    setSaving(providerId);
    setError(null);
    try {
      await setSetting(`ai_${providerId}_api_key`, key);
      setKeyInputs((k) => ({ ...k, [providerId]: '' }));
      const fresh = await getAiSettings();
      setStatus(fresh);
      setSaved(providerId);
      setTimeout(() => setSaved(null), 2000);
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not save key');
    } finally {
      setSaving(null);
    }
  }

  if (loading) return <div style={{ color: 'var(--ink-soft)' }}>Loading…</div>;

  return (
    <div>
      <div className="card" style={{ marginBottom: '1rem' }}>
        <h3 style={{ marginTop: 0 }}>AI assistant</h3>
        <p style={{ fontSize: '0.85rem', color: 'var(--ink-soft)', lineHeight: 1.5 }}>
          The AI assistant answers questions grounded in this business's own real data — it sees
          your actual inventory, sales, and records, not just a general description of the app.
          Pick a provider below and add its key. NVIDIA and Google both offer genuinely free tiers,
          no card required, if you want to try this at zero cost first.
        </p>
        <ErrorBox error={error} />

        <label>Active provider</label>
        <div style={{ display: 'flex', gap: '0.5rem', flexWrap: 'wrap', marginBottom: '1rem' }}>
          {PROVIDERS.map((p) => (
            <button
              key={p.id}
              className={provider === p.id ? 'btn btn-stamp' : 'btn btn-outline'}
              onClick={() => saveProvider(p.id)}
              disabled={saving === 'provider'}
            >
              {p.label}{p.free && <span style={{ fontSize: '0.7rem', opacity: 0.8 }}> · free</span>}
            </button>
          ))}
        </div>
        {saved === 'provider' && <div style={{ color: 'var(--ok)', fontSize: '0.85rem', marginBottom: '0.8rem' }}>Provider saved.</div>}
      </div>

      {PROVIDERS.map((p) => {
        const isSet = status?.[p.keyField];
        return (
          <div key={p.id} className="card" style={{ marginBottom: '0.8rem' }}>
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '0.5rem' }}>
              <div style={{ fontWeight: 600 }}>
                {p.label}
                {!p.free && <span style={{ fontSize: '0.72rem', color: 'var(--ink-soft)', fontWeight: 400 }}> — paid, no ongoing free tier</span>}
              </div>
              <span className={`status-pill ${isSet ? 'status-active' : 'status-inactive'}`}>
                {isSet ? 'Key configured' : 'Not configured'}
              </span>
            </div>
            <div style={{ display: 'flex', gap: '0.5rem' }}>
              <input
                type="password"
                placeholder={isSet ? 'Enter a new key to replace it' : 'Paste your API key here'}
                value={keyInputs[p.id] ?? ''}
                onChange={(e) => setKeyInputs((k) => ({ ...k, [p.id]: e.target.value }))}
                style={{ flex: 1 }}
              />
              <button className="btn btn-outline" onClick={() => saveKey(p.id)} disabled={saving === p.id || !keyInputs[p.id]?.trim()}>
                {saving === p.id ? 'Saving…' : 'Save'}
              </button>
            </div>
            {saved === p.id && <div style={{ color: 'var(--ok)', fontSize: '0.82rem', marginTop: '0.4rem' }}>Key saved.</div>}
            <a href={p.url} target="_blank" rel="noreferrer" style={{ fontSize: '0.78rem', display: 'inline-block', marginTop: '0.5rem' }}>
              Get a {p.free ? 'free' : ''} key at {p.url.replace('https://', '')} →
            </a>
          </div>
        );
      })}
    </div>
  );
}

// --------------------------------------------------------- Network

interface NetworkModeState {
  mode: 'standalone' | 'host' | 'client';
  host_address: string | null;
}

// Reads/writes device network mode via Tauri's IPC (not the HTTP API —
// see network_mode.rs's own doc comment on why: a "client" device may
// have no local server running at all to ask over HTTP). Every change
// here needs an app restart to actually take effect, since the server
// bind address and whether a local database even opens are decided
// once at startup (see lib.rs's setup()) — deliberately not attempting
// a live in-place teardown/rebind of a running server and database
// connection, which is a much larger source of bugs for very little
// benefit over "restart the app."
function NetworkTab() {
  const [state, setState] = useState<NetworkModeState | null>(null);
  const [lanAddress, setLanAddress] = useState<string | null>(null);
  const [hostInput, setHostInput] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [pendingRestart, setPendingRestart] = useState(false);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        const current = await invoke<NetworkModeState>('get_network_mode');
        if (!cancelled) setState(current);
        if (current.mode === 'host') {
          const addr = await invoke<string | null>('get_lan_address');
          if (!cancelled) setLanAddress(addr);
        }
      } catch {
        // Not running inside Tauri (e.g. plain browser dev) — network
        // mode has no meaning there, so this tab just shows nothing
        // rather than a confusing error about a feature that only
        // exists in the real desktop/Android app.
        if (!cancelled) setState({ mode: 'standalone', host_address: null });
      }
    })();
    return () => { cancelled = true; };
  }, []);

  async function applyMode(mode: 'standalone' | 'host' | 'client', hostAddress?: string) {
    setError(null);
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      await invoke('set_network_mode', { mode, hostAddress: hostAddress ?? null });
      setPendingRestart(true);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Could not save this');
    }
  }

  async function handleRestart() {
    try {
      const { relaunch } = await import('@tauri-apps/plugin-process');
      await relaunch();
    } catch {
      setError('Please close and reopen the app for this to take effect.');
    }
  }

  if (!state) return <p style={{ color: 'var(--ink-soft)' }}>Loading…</p>;

  if (pendingRestart) {
    return (
      <div className="card" style={{ maxWidth: 420 }}>
        <p>Saved. Restart the app for this to take effect.</p>
        <button className="btn btn-stamp" onClick={handleRestart}>Restart now</button>
      </div>
    );
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: '1rem', maxWidth: 480 }}>
      <p style={{ color: 'var(--ink-soft)', fontSize: '0.88rem' }}>
        By default every device runs its own separate copy of this business, with no connection to any other device.
        To have several devices on the same WiFi share one live, real-time copy instead, pick one device to be the host
        (usually a desktop/laptop that's reliably on) and connect the rest to it as clients.
      </p>

      <div className="card">
        <strong>Currently: {state.mode === 'standalone' ? 'Standalone (own copy)' : state.mode === 'host' ? 'Hosting for other devices' : `Connected to ${state.host_address}`}</strong>
      </div>

      {error && <div style={{ color: 'var(--stamp)', fontSize: '0.85rem' }}>{error}</div>}

      {state.mode !== 'standalone' && (
        <button className="btn btn-outline" onClick={() => applyMode('standalone')}>
          Switch back to standalone (own copy)
        </button>
      )}

      {state.mode !== 'host' && (
        <div className="card">
          <strong style={{ display: 'block', marginBottom: '0.4rem' }}>Host this business</strong>
          <p style={{ fontSize: '0.82rem', color: 'var(--ink-soft)' }}>
            Other devices on this WiFi will connect to this one for live data. Keep this device on and connected
            while others need real-time access.
          </p>
          <button className="btn btn-stamp" onClick={() => applyMode('host')}>Make this device the host</button>
        </div>
      )}
      {state.mode === 'host' && (
        <div className="card">
          <strong>Other devices should connect to:</strong>
          <div style={{ fontFamily: 'var(--font-mono)', fontSize: '1.1rem', marginTop: '0.4rem' }}>
            {lanAddress ? `${lanAddress}:8080` : 'Could not detect a network address'}
          </div>
        </div>
      )}

      {state.mode !== 'client' && (
        <div className="card">
          <strong style={{ display: 'block', marginBottom: '0.4rem' }}>Connect to another device</strong>
          <p style={{ fontSize: '0.82rem', color: 'var(--ink-soft)' }}>
            Enter the host address shown on the device you want to connect to.
          </p>
          <input
            value={hostInput}
            onChange={(e) => setHostInput(e.target.value)}
            placeholder="192.168.1.42:8080"
            style={{ width: '100%', marginBottom: '0.5rem' }}
          />
          <button className="btn btn-stamp" onClick={() => applyMode('client', hostInput.trim())} disabled={!hostInput.trim()}>
            Connect
          </button>
        </div>
      )}
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  table: { width: '100%', borderCollapse: 'collapse', fontSize: '0.86rem' },
  th: { textAlign: 'left', padding: '0.6rem 0.8rem', borderBottom: '1px solid var(--paper-line)', fontSize: '0.72rem', textTransform: 'uppercase', letterSpacing: '0.03em', color: 'var(--ink-soft)' },
  td: { padding: '0.55rem 0.8rem', borderBottom: '1px solid var(--paper-line)' },
  error: { background: 'var(--stamp-wash)', color: 'var(--stamp)', padding: '0.5em 0.7em', borderRadius: 3, fontSize: '0.85rem', marginBottom: '0.8rem' },
  formGrid: { display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(160px, 1fr))', gap: '0.8rem' },
  smallBtn: { padding: '0.3em 0.7em', fontSize: '0.78rem' },
  badge: { fontSize: '0.65rem', color: 'var(--ink-faint)', textTransform: 'uppercase', letterSpacing: '0.03em', marginLeft: '0.4em' },
  restorePreview: { padding: '0.9rem', border: '1px solid var(--paper-line)', borderRadius: 3, background: 'var(--stamp-wash)' },
};
