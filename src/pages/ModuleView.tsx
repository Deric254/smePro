import { useEffect, useMemo, useRef, useState } from 'react';
import {
  getModuleSchema, listRecords, createRecord, deleteRecord, exportModule,
  runReport, exportReport, listUnits, listCurrencies, runForecast, createInvoice, ApiError,
} from '../api';
import type { NewInvoiceItem } from '../api';
import type { ModuleSchema, Record_, FieldDef, Unit, Currency } from '../types';
import OcrImport from './OcrImport';
import InvoiceView from '../components/InvoiceView';

export default function ModuleView({ moduleId }: { moduleId: string }) {
  const [schema, setSchema] = useState<ModuleSchema | null>(null);
  const [records, setRecords] = useState<Record_[]>([]);
  const [search, setSearch] = useState('');
  const [showForm, setShowForm] = useState(false);
  const [showOcrImport, setShowOcrImport] = useState(false);
  const [formValues, setFormValues] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);
  const [tab, setTab] = useState<'records' | 'report'>('records');
  const [loading, setLoading] = useState(true);
  const [units, setUnits] = useState<Unit[]>([]);
  const [currencies, setCurrencies] = useState<Currency[]>([]);

  useEffect(() => {
    setLoading(true);
    setError(null);
    setTab('records');
    Promise.all([getModuleSchema(moduleId), listRecords(moduleId)])
      .then(([s, r]) => {
        setSchema(s);
        setRecords(r.records);
        // Only fetch the reference-data lists this module actually needs
        // — a module with no unit/currency fields shouldn't pay for it.
        const needsUnits = s.fields.some((f: FieldDef) => f.type === 'unit');
        const needsCurrencies = s.fields.some((f: FieldDef) => f.type === 'currency');
        if (needsUnits) listUnits().then((res) => setUnits(res.units)).catch(() => {});
        if (needsCurrencies) listCurrencies().then((res) => setCurrencies(res.currencies)).catch(() => {});
      })
      .catch((e) => setError(e instanceof ApiError ? e.message : 'Failed to load module'))
      .finally(() => setLoading(false));
  }, [moduleId]);

  const [searching, setSearching] = useState(false);
  const [viewingInvoiceId, setViewingInvoiceId] = useState<string | null>(null);
  const [showInvoiceForm, setShowInvoiceForm] = useState(false);
  const skipNextSearch = useRef(true);

  async function refreshRecords(searchTerm?: string) {
    const r = await listRecords(moduleId, searchTerm);
    setRecords(r.records);
  }

  // The initial module load (above) already fetches records once for
  // the empty-search state — without this, opening a module would
  // trigger a second, redundant fetch of the exact same data 300ms
  // later. Reset whenever the module changes, since that's a genuinely
  // new "initial load" this same logic applies to again.
  useEffect(() => { skipNextSearch.current = true; }, [moduleId]);

  // Live search: fires automatically ~300ms after typing stops, not on
  // Enter/submit. Debounced rather than firing on every keystroke —
  // typing "milk" shouldn't be four separate requests for "m", "mi",
  // "mil", "milk". Cancels a still-pending timer if the user keeps
  // typing before it fires, and ignores a stale in-flight response
  // that resolves after a newer search has already started (the
  // classic "typed fast, an old slow response overwrites new results"
  // race a naive debounce misses).
  useEffect(() => {
    if (!schema) return; // don't search before the module has even loaded
    if (skipNextSearch.current) { skipNextSearch.current = false; return; }
    let cancelled = false;
    setSearching(true);
    const timer = setTimeout(async () => {
      try {
        const r = await listRecords(moduleId, search || undefined);
        if (!cancelled) setRecords(r.records);
      } catch {
        // A failed live search shouldn't blank the list or show an
        // error banner for what's often just a mid-typing hiccup —
        // the existing records just stay as they were.
      } finally {
        if (!cancelled) setSearching(false);
      }
    }, 300);
    return () => { cancelled = true; clearTimeout(timer); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [search, moduleId, schema]);

  async function handleCreate(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      const payload: Record<string, unknown> = {};
      for (const f of schema!.fields) {
        const raw = formValues[f.name];
        if (raw === undefined || raw === '') continue;
        payload[f.name] = f.type === 'integer' ? parseInt(raw, 10)
          : f.type === 'real' ? parseFloat(raw)
          : f.type === 'boolean' ? raw === 'true'
          : raw;
      }
      await createRecord(moduleId, payload);
      setFormValues({});
      setShowForm(false);
      await refreshRecords();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not create record');
    }
  }

  async function handleDelete(id: string) {
    try {
      await deleteRecord(moduleId, id);
      await refreshRecords();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not delete record');
    }
  }

  const columns = useMemo(() => schema?.fields.map((f) => f.name) ?? [], [schema]);
  const canDelete = schema?.my_permissions.includes('delete');
  const canExport = schema?.my_permissions.includes('export');
  const canCreate = schema?.my_permissions.includes('create');

  if (loading) return <div style={{ padding: '1rem', color: 'var(--ink-soft)' }}>Loading…</div>;
  if (!schema) return <div style={{ padding: '1rem' }}>{error || 'Module not found'}</div>;

  return (
    <div>
      <div style={styles.headerRow}>
        <h2>{schema.display_name}</h2>
        <div style={styles.tabs}>
          <button className={tab === 'records' ? 'btn' : 'btn btn-outline'} onClick={() => setTab('records')}>Records</button>
          <button className={tab === 'report' ? 'btn' : 'btn btn-outline'} onClick={() => setTab('report')}>Report</button>
        </div>
      </div>

      {error && <div style={styles.error}>{error}</div>}

      {tab === 'records' ? (
        <>
          <div style={styles.toolbar}>
            <div style={{ display: 'flex', alignItems: 'center', gap: '0.5rem', flex: 1, position: 'relative' }}>
              <input
                placeholder="Search…"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                style={{ flex: 1, maxWidth: 260 }}
              />
              {searching && <span style={{ fontSize: '0.75rem', color: 'var(--ink-faint)' }}>searching…</span>}
            </div>
            <div style={{ display: 'flex', gap: '0.5rem' }}>
              {canExport && <button className="btn btn-outline" onClick={() => exportModule(moduleId)}>Export to Excel</button>}
              {moduleId === 'invoice' ? (
                canCreate && <button className="btn btn-stamp" onClick={() => setShowInvoiceForm((v) => !v)}>{showInvoiceForm ? 'Cancel' : '+ New invoice'}</button>
              ) : (
                <>
                  {canCreate && <button className="btn btn-outline" onClick={() => setShowOcrImport(true)}>Import from photo</button>}
                  {canCreate && <button className="btn btn-stamp" onClick={() => setShowForm((v) => !v)}>{showForm ? 'Cancel' : '+ New'}</button>}
                </>
              )}
            </div>
          </div>

          {moduleId === 'invoice' && showInvoiceForm && (
            <NewInvoiceForm
              onCreated={() => { setShowInvoiceForm(false); refreshRecords(); }}
              onCancel={() => setShowInvoiceForm(false)}
            />
          )}

          {showOcrImport && schema && (
            <OcrImport
              moduleId={moduleId}
              fields={schema.fields}
              onClose={() => setShowOcrImport(false)}
              onImported={() => { refreshRecords(); }}
            />
          )}

          {showForm && moduleId !== 'invoice' && (
            <form onSubmit={handleCreate} className="card" style={styles.form}>
              <div style={styles.formGrid}>
                {schema.fields.map((f) => (
                  <FieldInput key={f.name} field={f} value={formValues[f.name] ?? ''} units={units} currencies={currencies} onChange={(v) => setFormValues((p) => ({ ...p, [f.name]: v }))} />
                ))}
              </div>
              <button className="btn btn-stamp" type="submit" style={{ marginTop: '0.8rem' }}>Save</button>
            </form>
          )}

          <div className="card" style={{ padding: 0, overflowX: 'auto' }}>
            <table style={styles.table}>
              <thead>
                <tr>
                  {columns.map((c) => <th key={c} style={styles.th}>{c.replace(/_/g, ' ')}</th>)}
                  {canDelete && <th style={styles.th} />}
                </tr>
              </thead>
              <tbody>
                {records.length === 0 && (
                  <tr><td colSpan={columns.length + 1} style={styles.empty}>No records yet — add the first one above.</td></tr>
                )}
                {records.map((r) => (
                  <tr key={r.id}>
                    {columns.map((c) => (
                      <td key={c} className={typeof r[c] === 'number' ? 'mono' : ''} style={styles.td}>
                        {formatCell(r[c])}
                      </td>
                    ))}
                    {moduleId === 'invoice' && (
                      <td style={styles.td}>
                        <button className="btn btn-stamp" style={{ padding: '0.3em 0.7em', fontSize: '0.78rem' }} onClick={() => setViewingInvoiceId(r.id)}>
                          View
                        </button>
                      </td>
                    )}
                    {canDelete && (
                      <td style={styles.td}>
                        <button className="btn btn-outline" style={{ padding: '0.3em 0.7em', fontSize: '0.78rem' }} onClick={() => handleDelete(r.id)}>Delete</button>
                      </td>
                    )}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      ) : (
        <ReportPanel moduleId={moduleId} schema={schema} canExport={!!canExport} />
      )}

      {viewingInvoiceId && (
        <InvoiceView invoiceId={viewingInvoiceId} onClose={() => setViewingInvoiceId(null)} />
      )}
    </div>
  );
}

function formatCell(v: unknown) {
  if (v === null || v === undefined) return <span style={{ color: 'var(--ink-faint)' }}>—</span>;
  return String(v);
}

function FieldInput({ field, value, units, currencies, onChange }: { field: FieldDef; value: string; units: Unit[]; currencies: Currency[]; onChange: (v: string) => void }) {
  const inputType = field.type === 'integer' || field.type === 'real' ? 'number' : field.type === 'date' ? 'date' : 'text';
  if (field.type === 'boolean') {
    return (
      <div>
        <label>{field.name.replace(/_/g, ' ')}</label>
        <select value={value} onChange={(e) => onChange(e.target.value)}>
          <option value="">—</option>
          <option value="true">Yes</option>
          <option value="false">No</option>
        </select>
      </div>
    );
  }
  if (field.type === 'unit') {
    return (
      <div>
        <label>{field.name.replace(/_/g, ' ')}{field.required ? ' *' : ''}</label>
        <select value={value} required={field.required} onChange={(e) => onChange(e.target.value)}>
          <option value="">—</option>
          {units.map((u) => <option key={u.id} value={u.name}>{u.name}{u.abbreviation ? ` (${u.abbreviation})` : ''}</option>)}
        </select>
        {units.length === 0 && (
          <div style={{ fontSize: '0.72rem', color: 'var(--ink-faint)', marginTop: '0.2em' }}>
            No units defined yet — add some under Admin → Units.
          </div>
        )}
      </div>
    );
  }
  if (field.type === 'currency') {
    return (
      <div>
        <label>{field.name.replace(/_/g, ' ')}{field.required ? ' *' : ''}</label>
        <select value={value} required={field.required} onChange={(e) => onChange(e.target.value)}>
          <option value="">—</option>
          {currencies.map((c) => <option key={c.id} value={c.code}>{c.code}{c.symbol ? ` (${c.symbol})` : ''}</option>)}
        </select>
        {currencies.length === 0 && (
          <div style={{ fontSize: '0.72rem', color: 'var(--ink-faint)', marginTop: '0.2em' }}>
            No currencies defined yet — add some under Admin → Currencies.
          </div>
        )}
      </div>
    );
  }
  return (
    <div>
      <label>{field.name.replace(/_/g, ' ')}{field.required ? ' *' : ''}</label>
      <input
        type={inputType}
        step={field.type === 'real' ? '0.01' : undefined}
        value={value}
        required={field.required}
        onChange={(e) => onChange(e.target.value)}
        style={{ width: '100%' }}
      />
    </div>
  );
}

function ReportPanel({ moduleId, schema, canExport }: { moduleId: string; schema: ModuleSchema; canExport: boolean }) {
  const numericFields = schema.fields.filter((f) => f.type === 'integer' || f.type === 'real');
  const categoryFields = schema.fields.filter((f) => f.type === 'text' || f.type === 'unit' || f.type === 'currency');
  const [agg, setAgg] = useState<'sum' | 'count' | 'avg'>('sum');
  const [measure, setMeasure] = useState(numericFields[0]?.name ?? '');
  const [dimension, setDimension] = useState<'none' | 'category' | 'time'>(categoryFields[0] ? 'category' : 'none');
  const [field, setField] = useState(categoryFields[0]?.name ?? '');
  const [bucket, setBucket] = useState('month');
  const [points, setPoints] = useState<{ label: string; value: number }[]>([]);
  const [error, setError] = useState<string | null>(null);

  async function run() {
    setError(null);
    try {
      const params: Record<string, string> = { agg };
      if (agg !== 'count') params.measure = measure;
      if (dimension === 'category') { params.dimension = 'category'; params.field = field; }
      if (dimension === 'time') { params.dimension = 'time'; params.bucket = bucket; }
      const res = await runReport(moduleId, params);
      setPoints(res.report);
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not run report');
    }
  }

  useEffect(() => { run(); /* eslint-disable-next-line */ }, []);

  const max = Math.max(1, ...points.map((p) => p.value));

  return (
    <>
    <div className="card">
      <div style={styles.reportControls}>
        <div>
          <label>Aggregate</label>
          <select value={agg} onChange={(e) => setAgg(e.target.value as any)}>
            <option value="sum">Sum</option>
            <option value="count">Count</option>
            <option value="avg">Average</option>
          </select>
        </div>
        {agg !== 'count' && (
          <div>
            <label>Of</label>
            <select value={measure} onChange={(e) => setMeasure(e.target.value)}>
              {numericFields.map((f) => <option key={f.name} value={f.name}>{f.name}</option>)}
            </select>
          </div>
        )}
        <div>
          <label>Slice by</label>
          <select value={dimension} onChange={(e) => setDimension(e.target.value as any)}>
            <option value="none">Total</option>
            {categoryFields.length > 0 && <option value="category">Category</option>}
            <option value="time">Time</option>
          </select>
        </div>
        {dimension === 'category' && (
          <div>
            <label>Field</label>
            <select value={field} onChange={(e) => setField(e.target.value)}>
              {categoryFields.map((f) => <option key={f.name} value={f.name}>{f.name}</option>)}
            </select>
          </div>
        )}
        {dimension === 'time' && (
          <div>
            <label>Bucket</label>
            <select value={bucket} onChange={(e) => setBucket(e.target.value)}>
              <option value="day">Day</option>
              <option value="week">Week</option>
              <option value="month">Month</option>
              <option value="quarter">Quarter</option>
              <option value="year">Year</option>
            </select>
          </div>
        )}
        <button className="btn btn-outline" onClick={run}>Run</button>
        {canExport && (
          <button
            className="btn btn-stamp"
            onClick={() => {
              const params: Record<string, string> = { agg };
              if (agg !== 'count') params.measure = measure;
              if (dimension === 'category') { params.dimension = 'category'; params.field = field; }
              if (dimension === 'time') { params.dimension = 'time'; params.bucket = bucket; }
              exportReport(moduleId, params);
            }}
          >
            Export to Excel
          </button>
        )}
      </div>

      {error && <div style={styles.error}>{error}</div>}

      <div style={{ marginTop: '1.2rem', display: 'flex', flexDirection: 'column', gap: '0.5rem' }}>
        {points.length === 0 && <div style={{ color: 'var(--ink-soft)', fontSize: '0.88rem' }}>No data yet.</div>}
        {points.map((p) => (
          <div key={p.label} style={styles.barRow}>
            <span style={{ width: 110, fontSize: '0.8rem', color: 'var(--ink-soft)', flexShrink: 0 }}>{p.label}</span>
            <div style={styles.barTrack}>
              <div style={{ ...styles.barFill, width: `${(p.value / max) * 100}%` }} />
            </div>
            <span className="mono" style={{ width: 80, textAlign: 'right', fontSize: '0.82rem' }}>{p.value.toLocaleString()}</span>
          </div>
        ))}
      </div>
      </div>
      {numericFields.length > 0 && <ForecastPanel moduleId={moduleId} numericFields={numericFields} />}
    </>
  );
}

function NewInvoiceForm({ onCreated, onCancel }: { onCreated: () => void; onCancel: () => void }) {
  const [customer, setCustomer] = useState('');
  const [customerEmail, setCustomerEmail] = useState('');
  const [customerPhone, setCustomerPhone] = useState('');
  const [dueDate, setDueDate] = useState('');
  const [notes, setNotes] = useState('');
  const [items, setItems] = useState<NewInvoiceItem[]>([{ description: '', quantity: 1, unit_price: 0 }]);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const subtotal = items.reduce((sum, it) => sum + (it.quantity || 0) * (it.unit_price || 0), 0);

  function updateItem(i: number, patch: Partial<NewInvoiceItem>) {
    setItems((prev) => prev.map((it, idx) => (idx === i ? { ...it, ...patch } : it)));
  }
  function addItem() {
    setItems((prev) => [...prev, { description: '', quantity: 1, unit_price: 0 }]);
  }
  function removeItem(i: number) {
    setItems((prev) => prev.filter((_, idx) => idx !== i));
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    const cleanItems = items.filter((it) => it.description.trim() && it.quantity > 0);
    if (cleanItems.length === 0) {
      setError('Add at least one line item with a description and quantity.');
      return;
    }
    setSaving(true);
    try {
      await createInvoice({
        customer,
        customer_email: customerEmail || undefined,
        customer_phone: customerPhone || undefined,
        due_date: dueDate,
        items: cleanItems,
        notes: notes || undefined,
      });
      onCreated();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not create the invoice');
    } finally {
      setSaving(false);
    }
  }

  return (
    <form onSubmit={handleSubmit} className="card" style={{ marginBottom: '1rem' }}>
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(180px, 1fr))', gap: '0.8rem', marginBottom: '0.8rem' }}>
        <div>
          <label>Customer</label>
          <input value={customer} onChange={(e) => setCustomer(e.target.value)} required style={{ width: '100%' }} />
        </div>
        <div>
          <label>Email (optional)</label>
          <input type="email" value={customerEmail} onChange={(e) => setCustomerEmail(e.target.value)} style={{ width: '100%' }} />
        </div>
        <div>
          <label>Phone (optional)</label>
          <input value={customerPhone} onChange={(e) => setCustomerPhone(e.target.value)} style={{ width: '100%' }} />
        </div>
        <div>
          <label>Due date</label>
          <input type="date" value={dueDate} onChange={(e) => setDueDate(e.target.value)} required style={{ width: '100%' }} />
        </div>
      </div>

      <label>Line items</label>
      {items.map((it, i) => (
        <div key={i} style={{ display: 'flex', gap: '0.5rem', marginBottom: '0.5rem', alignItems: 'center' }}>
          <input
            placeholder="Description"
            value={it.description}
            onChange={(e) => updateItem(i, { description: e.target.value })}
            style={{ flex: 2 }}
          />
          <input
            type="number"
            min={1}
            placeholder="Qty"
            value={it.quantity}
            onChange={(e) => updateItem(i, { quantity: Number(e.target.value) })}
            style={{ width: 70 }}
          />
          <input
            type="number"
            min={0}
            step="0.01"
            placeholder="Unit price"
            value={it.unit_price}
            onChange={(e) => updateItem(i, { unit_price: Number(e.target.value) })}
            style={{ width: 110 }}
          />
          <span className="mono" style={{ width: 90, textAlign: 'right', fontSize: '0.85rem', color: 'var(--ink-soft)' }}>
            {((it.quantity || 0) * (it.unit_price || 0)).toFixed(2)}
          </span>
          {items.length > 1 && (
            <button type="button" className="btn btn-outline" style={{ padding: '0.2em 0.5em', fontSize: '0.75rem' }} onClick={() => removeItem(i)}>×</button>
          )}
        </div>
      ))}
      <button type="button" className="btn btn-outline" style={{ fontSize: '0.8rem', marginBottom: '0.8rem' }} onClick={addItem}>+ Add line item</button>

      <div>
        <label>Notes (optional)</label>
        <input value={notes} onChange={(e) => setNotes(e.target.value)} style={{ width: '100%' }} />
      </div>

      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginTop: '1rem', paddingTop: '0.8rem', borderTop: '1px solid var(--paper-line)' }}>
        <div style={{ fontSize: '0.95rem', fontWeight: 600 }}>
          Subtotal: <span className="mono">{subtotal.toFixed(2)}</span>
          <span style={{ fontSize: '0.75rem', fontWeight: 400, color: 'var(--ink-soft)', marginLeft: '0.5rem' }}>(tax applied automatically at your business's rate)</span>
        </div>
        <div style={{ display: 'flex', gap: '0.6rem' }}>
          <button type="button" className="btn btn-outline" onClick={onCancel}>Cancel</button>
          <button type="submit" className="btn btn-stamp" disabled={saving}>{saving ? 'Creating…' : 'Create invoice'}</button>
        </div>
      </div>

      {error && <div style={styles.error}>{error}</div>}
    </form>
  );
}

function ForecastPanel({ moduleId, numericFields }: { moduleId: string; numericFields: FieldDef[] }) {
  const [measure, setMeasure] = useState(numericFields[0]?.name ?? '');
  const [bucket, setBucket] = useState('month');
  const [method, setMethod] = useState<'moving_average' | 'exponential_smoothing'>('moving_average');
  const [result, setResult] = useState<{ forecast_next: number; method: string; history: { label: string; value: number }[] } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  async function run() {
    setLoading(true);
    setError(null);
    try {
      const res = await runForecast(moduleId, { measure, bucket, method });
      setResult(res);
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not run the forecast — needs a few periods of history to work from');
    } finally {
      setLoading(false);
    }
  }

  return (
    <div className="card" style={{ marginTop: '1rem' }}>
      <h3 style={{ marginTop: 0 }}>Forecast next period</h3>
      <p style={{ fontSize: '0.82rem', color: 'var(--ink-soft)', marginTop: '-0.4rem' }}>
        A plain-arithmetic projection from your own history — not a guess, not AI-invented. Needs a
        few periods of real data behind it to mean anything.
      </p>
      <div style={styles.reportControls}>
        <div>
          <label>Of</label>
          <select value={measure} onChange={(e) => setMeasure(e.target.value)}>
            {numericFields.map((f) => <option key={f.name} value={f.name}>{f.name}</option>)}
          </select>
        </div>
        <div>
          <label>Bucket</label>
          <select value={bucket} onChange={(e) => setBucket(e.target.value)}>
            <option value="day">Day</option>
            <option value="week">Week</option>
            <option value="month">Month</option>
            <option value="quarter">Quarter</option>
          </select>
        </div>
        <div>
          <label>Method</label>
          <select value={method} onChange={(e) => setMethod(e.target.value as typeof method)}>
            <option value="moving_average">Moving average</option>
            <option value="exponential_smoothing">Exponential smoothing (weights recent periods more)</option>
          </select>
        </div>
        <button className="btn btn-stamp" onClick={run} disabled={loading}>{loading ? 'Calculating…' : 'Forecast'}</button>
      </div>

      {error && <div style={styles.error}>{error}</div>}

      {result && (
        <div style={{ marginTop: '1rem' }}>
          <div style={{ fontSize: '1.6rem', fontWeight: 600, color: 'var(--stamp)' }}>
            {result.forecast_next.toLocaleString()}
          </div>
          <div style={{ fontSize: '0.78rem', color: 'var(--ink-soft)' }}>
            projected for the next {bucket}, based on {result.history.length} periods of history ({result.method})
          </div>
        </div>
      )}
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  headerRow: { display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: '0.9rem', flexWrap: 'wrap', gap: '0.6rem' },
  tabs: { display: 'flex', gap: '0.4rem' },
  toolbar: { display: 'flex', justifyContent: 'space-between', gap: '1rem', marginBottom: '0.9rem', flexWrap: 'wrap' },
  form: { marginBottom: '1rem' },
  formGrid: { display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(160px, 1fr))', gap: '0.8rem' },
  table: { width: '100%', borderCollapse: 'collapse', fontSize: '0.86rem' },
  th: { textAlign: 'left', padding: '0.6rem 0.8rem', borderBottom: '1px solid var(--paper-line)', fontSize: '0.72rem', textTransform: 'uppercase', letterSpacing: '0.03em', color: 'var(--ink-soft)' },
  td: { padding: '0.55rem 0.8rem', borderBottom: '1px solid var(--paper-line)' },
  empty: { padding: '1.4rem', textAlign: 'center', color: 'var(--ink-faint)' },
  error: { background: 'var(--stamp-wash)', color: 'var(--stamp)', padding: '0.5em 0.7em', borderRadius: 3, fontSize: '0.85rem', marginBottom: '0.8rem' },
  reportControls: { display: 'flex', gap: '0.9rem', flexWrap: 'wrap', alignItems: 'flex-end' },
  barRow: { display: 'flex', alignItems: 'center', gap: '0.7rem' },
  barTrack: { flex: 1, height: 10, background: 'var(--paper)', borderRadius: 5, overflow: 'hidden', border: '1px solid var(--paper-line)' },
  barFill: { height: '100%', background: 'var(--stamp)' },
};
