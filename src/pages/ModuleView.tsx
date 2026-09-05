import { useEffect, useMemo, useRef, useState } from 'react';
import {
  getModuleSchema, listRecords, createRecord, updateRecord, deleteRecord, exportModule,
  downloadImportTemplate, importExcel,
  runReport, exportReport, listUnits, listCurrencies, runForecast, createInvoice, getBusinessInfo,
  receiveStock, repackStock, settleDebt, ApiError,
} from '../api';
import type { NewInvoiceItem, ImportExcelResult } from '../api';
import type { ModuleSchema, Record_, FieldDef, Unit, Currency } from '../types';
import { formatMoney, parseMoneyInput } from '../lib/money';
import InvoiceView from '../components/InvoiceView';
import ReceiptView from '../components/ReceiptView';
import DebtSummaryWidget from '../components/DebtSummary';

// Some fields must only ever change through a specific, purpose-built
// backend action — never through the generic create/edit form — because
// that action does more than set the field: purchasing's `received`
// flag is set atomically alongside a real inventory quantity/cost
// update inside receiving.rs::receive(), and letting the generic form
// touch it would silently skip all of that (receiving.rs's own doc
// comment calls this out explicitly as the one gap its "atomic receive"
// guarantee doesn't cover). Excluded from both create and edit — even
// creating a purchase order as already-received would be the same
// bypass as editing one into that state.
//
// debt_credit's `settled` is the same situation: settling atomically
// posts the real cash movement to Bookkeeping inside
// debt_settlement.rs::settle(), and the generic form touching it
// directly would silently skip that (debt_settlement.rs's own doc
// comment calls out the same limitation receiving.rs does — this
// hides the field from the intended path, it can't forbid a raw API
// call from a role that still holds "update").
//
// inventory's `quantity` is the same category of field, but stricter
// still: every inventory item starts at zero stock, full stop, on
// BOTH create and edit — sell (pos.rs), receive (receiving.rs), refund
// (refund.rs), and repack (repack.rs) are the only paths that should
// ever move a stock level, each with its own oversell/floor
// protections a plain field edit doesn't have. Unlike
// `received`/`settled`, this one doesn't even need the `isEditing`
// distinction — there's no legitimate caller-supplied opening count on
// this form; stock enters the system exactly one way, by Purchasing
// receiving an order. The real enforcement lives server-side in
// crud.rs (`create()` forces quantity to 0; `is_single_record_edit_blocked_field`
// blocks it on update); hiding the input here is just so nobody sees a
// field they can't actually change.
//
// purchasing's `po_number` joins this list for the same "generated,
// never hand-typed" reason as `received`, just generated at creation
// instead of by a separate action — see crud.rs::create's purchasing
// block and db_migrations.rs's v14 for the full story of why this
// field exists at all (it's what fixed same-supplier Excel imports
// silently colliding with each other).
function isActionManagedField(moduleId: string, fieldName: string): boolean {
  return (moduleId === 'purchasing' && (fieldName === 'received' || fieldName === 'po_number'))
    || (moduleId === 'debt_credit' && fieldName === 'settled')
    || (moduleId === 'debt_credit' && (fieldName === 'payment_method' || fieldName === 'source_order_id'))
    // debt_credit's `entry_number` joins this list for the same
    // "generated, never hand-typed" reason as purchasing's `po_number`
    // just above — see crud.rs::create's debt_credit block and
    // db_migrations.rs's v16 for why this field exists at all (it's
    // what makes an Excel re-import of Debt & Credit able to safely
    // match an existing row without matching on `party_name`, which
    // one party can legitimately have many separate entries under).
    || (moduleId === 'debt_credit' && fieldName === 'entry_number')
    || (moduleId === 'inventory' && fieldName === 'quantity');
}

// Reads a File into a bare base64 string (no "data:...;base64," prefix
// — the backend expects raw base64, same contract as the logo upload
// in BusinessBranding.tsx, just pulled out into a reusable helper
// here since this is the second place that needs it).
function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const result = reader.result as string;
      resolve(result.split(',')[1] ?? '');
    };
    reader.onerror = () => reject(new Error('could not read the file'));
    reader.readAsDataURL(file);
  });
}

export default function ModuleView({ moduleId }: { moduleId: string }) {
  const [schema, setSchema] = useState<ModuleSchema | null>(null);
  const [records, setRecords] = useState<Record_[]>([]);
  const [search, setSearch] = useState('');
  const [showForm, setShowForm] = useState(false);
  // null = the form is creating a new record; a string = the form is
  // editing that existing record's id. Same form, same FieldInput
  // rendering, same money-field text-buffer discipline either way —
  // only the submit handler's target endpoint (createRecord vs
  // updateRecord) and the initial formValues differ.
  const [editingId, setEditingId] = useState<string | null>(null);
  const [showExcelImport, setShowExcelImport] = useState(false);
  const [excelKeyField, setExcelKeyField] = useState('');
  const [excelFile, setExcelFile] = useState<File | null>(null);
  const [excelImporting, setExcelImporting] = useState(false);
  const [excelError, setExcelError] = useState<string | null>(null);
  const [excelResult, setExcelResult] = useState<ImportExcelResult | null>(null);
  const [templateDownloading, setTemplateDownloading] = useState(false);
  const [templateStatus, setTemplateStatus] = useState<string | null>(null);

  async function handleDownloadTemplate() {
    setTemplateDownloading(true);
    setTemplateStatus(null);
    setExcelError(null);
    try {
      await downloadImportTemplate(moduleId);
      setTemplateStatus('Template downloaded.');
    } catch (err) {
      setExcelError(err instanceof ApiError ? err.message : 'Could not download the template');
    } finally {
      setTemplateDownloading(false);
    }
  }

  async function handleExcelImport() {
    if (!excelFile) return;
    setExcelImporting(true);
    setExcelError(null);
    setExcelResult(null);
    try {
      const base64 = await fileToBase64(excelFile);
      const result = await importExcel(moduleId, base64, excelKeyField || undefined);
      setExcelResult(result);
      await refreshRecords();
    } catch (err) {
      setExcelError(err instanceof ApiError ? err.message : 'Could not import this file');
    } finally {
      setExcelImporting(false);
    }
  }
  const [formValues, setFormValues] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);
  const [tab, setTab] = useState<'records' | 'report'>('records');
  const [loading, setLoading] = useState(true);
  const [units, setUnits] = useState<Unit[]>([]);
  const [currencies, setCurrencies] = useState<Currency[]>([]);
  const [inventoryItems, setInventoryItems] = useState<Record_[]>([]);
  // The business's own currency — needed everywhere a "money"-typed
  // field is parsed (form input) or displayed (records table), so
  // decimal places are correct for e.g. JPY (0dp) or KWD (3dp), not
  // just assumed to be USD's 2dp. See src/lib/money.ts.
  const [businessCurrency, setBusinessCurrency] = useState('USD');

  useEffect(() => {
    getBusinessInfo()
      .then((b: any) => { if (b?.currency) setBusinessCurrency(b.currency); })
      .catch(() => {});
  }, []);

  useEffect(() => {
    setLoading(true);
    setError(null);
    setTab('records');
    Promise.all([getModuleSchema(moduleId), listRecords(moduleId)])
      .then(([s, r]) => {
        setSchema(s);
        setRecords(r.records);
        // "repack" lives on inventory.json's own actions list, so when
        // this page IS Inventory, its own schema already has the
        // answer — no separate fetch needed (unlike "receive", which
        // is checked from the Purchasing page and needs Inventory's
        // permissions fetched separately, since that's a different
        // module's schema than the one loaded here).
        setInventoryCanRepack(moduleId === 'inventory' && s.my_permissions.includes('repack'));
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

  // Same reasoning as PointOfSale.tsx's own focus listener: this page
  // has no live push/sync mechanism telling it when something changed
  // elsewhere — another module's action, another window, or a second
  // device on the same LAN server (see API_BASE in api.ts). Ordinary
  // in-app navigation away and back already remounts this component
  // fresh with a brand new `moduleId` effect run above; this covers
  // the gap that alone doesn't: returning focus to the window while
  // still sitting on the same module screen the whole time.
  useEffect(() => {
    function onFocus() { refreshRecords(search || undefined); }
    window.addEventListener('focus', onFocus);
    return () => window.removeEventListener('focus', onFocus);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [moduleId, search]);

  useEffect(() => {
    if (moduleId !== 'purchasing') {
      setInventoryItems([]);
      return;
    }
    listRecords('inventory')
      .then((r) => setInventoryItems(r.records))
      .catch(() => setInventoryItems([]));
  }, [moduleId]);

  const [searching, setSearching] = useState(false);
  const [viewingInvoiceId, setViewingInvoiceId] = useState<string | null>(null);
  // Invoices auto-generated from a POS/service sale carry the sale's
  // own order_id in source_sale_id (see invoice::create_invoice_for_order)
  // — this lets the Invoices tab open the exact same original receipt
  // the till printed, so the two views can never show inconsistent
  // figures for the same sale.
  const [viewingReceiptOrderId, setViewingReceiptOrderId] = useState<string | null>(null);
  const [showInvoiceForm, setShowInvoiceForm] = useState(false);
  const skipNextSearch = useRef(true);

  // Receiving (purchasing -> inventory) and repacking (bulk -> retail
  // units) are each their own dedicated backend action — see
  // receiving.rs and repack.rs — not something the generic create/edit
  // form can do, since both atomically touch a SECOND record besides
  // the one being acted on. This page only shows the trigger buttons
  // when the signed-in user actually holds the specific RBAC action
  // each one requires (receiving.rs: "receive" on inventory; repack.rs:
  // "repack" on inventory) — both live on the Inventory module's
  // permission set regardless of which module's table you're currently
  // viewing, which is why receiving needs its own small fetch below
  // when you're on the Purchasing page rather than Inventory itself.
  const [inventoryCanReceive, setInventoryCanReceive] = useState(false);
  const [inventoryCanRepack, setInventoryCanRepack] = useState(false);

  const [receivingId, setReceivingId] = useState<string | null>(null);
  const [receiveQtyText, setReceiveQtyText] = useState('');
  const [receiveError, setReceiveError] = useState<string | null>(null);
  const [receiveSubmitting, setReceiveSubmitting] = useState(false);
  const [actionResult, setActionResult] = useState<string | null>(null);

  const [repackSourceId, setRepackSourceId] = useState<string | null>(null);
  const [repackTargetId, setRepackTargetId] = useState('');
  // Lets the modal create the target item in the same step instead of
  // requiring a separate trip to Inventory's own create form first —
  // see repack.rs's module doc comment for why. 'existing' is the
  // original behavior (repackTargetId); 'new' switches the target
  // picker for a name + selling-price pair instead.
  const [repackTargetMode, setRepackTargetMode] = useState<'existing' | 'new'>('existing');
  const [repackNewTargetName, setRepackNewTargetName] = useState('');
  const [repackNewTargetPriceText, setRepackNewTargetPriceText] = useState('');
  const [repackSourceQtyText, setRepackSourceQtyText] = useState('1');
  const [repackTargetQtyText, setRepackTargetQtyText] = useState('');
  const [repackNotes, setRepackNotes] = useState('');
  const [repackError, setRepackError] = useState<string | null>(null);
  const [repackSubmitting, setRepackSubmitting] = useState(false);

  // Settling a debt/credit record — same shape as receiving/repacking
  // above. See debt_settlement.rs.
  const [settlingId, setSettlingId] = useState<string | null>(null);
  const [settleError, setSettleError] = useState<string | null>(null);
  const [settleSubmitting, setSettleSubmitting] = useState(false);
  // Defaults to 'cash', same as PointOfSale.tsx's own payment-method
  // selector — matching the one other place in the app that already
  // asks this question, rather than defaulting to a blank choice that
  // would need an extra click before Confirm even does anything.
  const [settlePaymentMethod, setSettlePaymentMethod] = useState('cash');

  useEffect(() => {
    if (moduleId === 'inventory') return; // schema.my_permissions already covers this case directly
    if (moduleId !== 'purchasing') return; // receiving only ever gets triggered from the Purchasing list
    getModuleSchema('inventory')
      .then((s) => setInventoryCanReceive(s.my_permissions.includes('receive')))
      .catch(() => {}); // Inventory module not enabled, or this role can't see it — button just won't show
  }, [moduleId]);

  async function submitReceive() {
    if (!receivingId) return;
    const qty = receiveQtyText.trim() === '' ? undefined : parseInt(receiveQtyText, 10);
    if (receiveQtyText.trim() !== '' && (!Number.isInteger(qty) || (qty as number) <= 0)) {
      setReceiveError('Quantity received must be a positive whole number.');
      return;
    }
    setReceiveSubmitting(true);
    setReceiveError(null);
    try {
      const summary = await receiveStock(receivingId, qty);
      setActionResult(
        `Received ${summary.quantity_received} of "${summary.inventory_name}". New stock: ${summary.new_stock_level}. New average cost: ${formatMoney(summary.new_weighted_average_cost, businessCurrency)}.`
      );
      setReceivingId(null);
      setReceiveQtyText('');
      await refreshRecords();
    } catch (err) {
      setReceiveError(err instanceof ApiError ? err.message : 'Could not receive this purchase order');
    } finally {
      setReceiveSubmitting(false);
    }
  }

  async function submitRepack() {
    if (!repackSourceId) return;
    const sourceQty = parseInt(repackSourceQtyText, 10);
    const targetQty = parseInt(repackTargetQtyText, 10);
    if (!Number.isInteger(sourceQty) || sourceQty <= 0) {
      setRepackError('Quantity consumed must be a positive whole number.');
      return;
    }
    if (!Number.isInteger(targetQty) || targetQty <= 0) {
      setRepackError('Quantity produced must be a positive whole number.');
      return;
    }
    // Exactly one of the two ways to say what this repack produces —
    // mirrors the same either/or repack.rs itself enforces, checked
    // here too so the person sees the problem immediately rather than
    // waiting on a round trip to the backend for it.
    let newTargetPriceCents: number | undefined;
    if (repackTargetMode === 'new') {
      if (!repackNewTargetName.trim()) {
        setRepackError('Enter a name for the new item being created.');
        return;
      }
      newTargetPriceCents = parseMoneyInput(repackNewTargetPriceText, businessCurrency) ?? undefined;
      if (newTargetPriceCents == null) {
        setRepackError('Enter a selling price for the new item.');
        return;
      }
    } else if (!repackTargetId) {
      setRepackError('Select the item being produced, or switch to "Create a new item".');
      return;
    }
    setRepackSubmitting(true);
    setRepackError(null);
    try {
      const summary = await repackStock({
        source_record_id: repackSourceId,
        source_quantity: sourceQty,
        ...(repackTargetMode === 'new'
          ? { new_target_name: repackNewTargetName.trim(), new_target_unit_price: newTargetPriceCents }
          : { target_record_id: repackTargetId }),
        target_quantity_produced: targetQty,
        notes: repackNotes || undefined,
      });
      // The profit case for repacking, at today's prices — this is the
      // actual reason a business breaks bulk, so it's surfaced right
      // in the confirmation, not left for someone to work out by hand
      // from the two unit prices. Only shown when it's computable
      // (bulk_equivalent_value > 0, i.e. the source item actually has
      // a selling price set).
      const profitLine = typeof summary.repack_profit_uplift === 'number' && summary.repack_margin_uplift_pct != null
        ? ` ${summary.repack_profit_uplift >= 0 ? 'Profit uplift' : 'Profit reduction'}: ${formatMoney(Math.abs(summary.repack_profit_uplift), businessCurrency)} (${summary.repack_margin_uplift_pct >= 0 ? '+' : ''}${summary.repack_margin_uplift_pct.toFixed(1)}% vs. selling in bulk).`
        : '';
      // Rounding is never silently absorbed — if the weighted-average
      // cost calculation couldn't land on the exact cent, that's
      // spelled out here too, matching the labeled Bookkeeping entry
      // repack.rs posts for it.
      const roundingLine = summary.rounding_adjustment_cents
        ? ` (A ${formatMoney(Math.abs(summary.rounding_adjustment_cents), businessCurrency)} rounding ${summary.rounding_adjustment_cents > 0 ? 'loss' : 'gain'} was posted to Bookkeeping under Stock Revaluation.)`
        : '';
      const newItemLine = summary.target_created ? ' (new item created)' : '';
      setActionResult(
        `Repacked ${sourceQty} of "${summary.source_name}" into ${targetQty} of "${summary.target_name}"${newItemLine}. New cost: ${formatMoney(summary.target_unit_cost_after, businessCurrency)} each.${profitLine}${roundingLine}`
      );
      setRepackSourceId(null);
      setRepackTargetId('');
      setRepackTargetMode('existing');
      setRepackNewTargetName('');
      setRepackNewTargetPriceText('');
      setRepackSourceQtyText('1');
      setRepackTargetQtyText('');
      setRepackNotes('');
      await refreshRecords();
    } catch (err) {
      setRepackError(err instanceof ApiError ? err.message : 'Could not complete the repack');
    } finally {
      setRepackSubmitting(false);
    }
  }

  async function submitSettle() {
    if (!settlingId) return;
    setSettleSubmitting(true);
    setSettleError(null);
    try {
      const summary = await settleDebt(settlingId, settlePaymentMethod);
      setActionResult(
        summary.posted_to_bookkeeping_as
          ? `Marked "${summary.party_name}" settled and posted ${formatMoney(summary.amount, businessCurrency)} to Bookkeeping as ${summary.posted_to_bookkeeping_as}.`
          : `Marked "${summary.party_name}" settled.`
      );
      setSettlingId(null);
      await refreshRecords();
    } catch (err) {
      setSettleError(err instanceof ApiError ? err.message : 'Could not settle this record');
    } finally {
      setSettleSubmitting(false);
    }
  }

  // Bumped every time refreshRecords() runs for the Debt & Credit
  // module, so DebtSummaryWidget above re-fetches its totals in
  // lockstep with the records list — settle a debt, edit one, delete
  // one, or import a batch via Excel, and the summary tiles (and the
  // overdue alarm) update immediately alongside the table, not on the
  // next unrelated re-render.
  const [debtSummaryRefreshKey, setDebtSummaryRefreshKey] = useState(0);

  async function refreshRecords(searchTerm?: string) {
    const r = await listRecords(moduleId, searchTerm);
    setRecords(r.records);
    if (moduleId === 'debt_credit') setDebtSummaryRefreshKey((k) => k + 1);
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

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      // Editing sends only the fields actually present in formValues
      // (a real PATCH — see updateRecord/crud::update), which
      // naturally happens here too: startEdit only seeds formValues
      // with the record's existing values, so an edit payload already
      // contains every field, same as create's "every non-empty field"
      // behavior. The one difference that matters: on edit, a field
      // the person cleared to empty should still be sent (so they can
      // actually blank out an optional field), whereas on create an
      // empty field just means "not set yet, use the default" — so
      // only create skips empty strings.
      // Same rule for both create and edit: only fields with an
      // actual typed value are sent. This deliberately means editing
      // can't blank out an optional field back to empty through this
      // form (only change it to something else) — the alternative,
      // treating an empty box as "clear this", breaks per field type
      // (an empty numeric input becomes NaN, which JSON serializes as
      // null, which then fails backend validation with a confusing
      // error) rather than doing anything useful, so it's not
      // supported rather than half-supported.
      const payload: Record<string, unknown> = {};
      for (const f of schema!.fields) {
        const raw = formValues[f.name];
        if (raw === undefined || raw === '') continue;
        if (f.type === 'money') {
          const cents = parseMoneyInput(raw, businessCurrency);
          if (cents === null) {
            setError(`"${f.name.replace(/_/g, ' ')}" is not a valid amount.`);
            return;
          }
          payload[f.name] = cents;
          continue;
        }
        payload[f.name] = f.type === 'integer' ? parseInt(raw, 10)
          : f.type === 'real' ? parseFloat(raw)
          : f.type === 'boolean' ? raw === 'true'
          : raw;
      }
      if (editingId !== null) {
        await updateRecord(moduleId, editingId, payload);
      } else {
        await createRecord(moduleId, payload);
      }
      setFormValues({});
      setShowForm(false);
      setEditingId(null);
      await refreshRecords();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : editingId !== null ? 'Could not save changes' : 'Could not create record');
    }
  }

  // Money fields need their existing integer-cents value converted
  // back to a display decimal string ("1999" -> "19.99") before they
  // can sit in the same text-buffer input the create form uses — see
  // src/lib/money.ts. Every other field type is already the right
  // shape (or close enough) as a plain string.
  function startEdit(record: Record_) {
    const seeded: Record<string, string> = {};
    for (const f of schema!.fields) {
      const v = record[f.name];
      if (v === null || v === undefined) { seeded[f.name] = ''; continue; }
      seeded[f.name] = f.type === 'money' ? formatMoney(v as number, businessCurrency) : String(v);
    }
    setFormValues(seeded);
    setEditingId(record.id);
    setShowForm(true);
  }

  function cancelForm() {
    setFormValues({});
    setEditingId(null);
    setShowForm(false);
  }

  async function handleDelete(id: string) {
    try {
      await deleteRecord(moduleId, id);
      await refreshRecords();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not delete record');
    }
  }

  // source_order_id is an internal traceability pointer (see pos.rs /
  // debt_settlement.rs) — a raw UUID meaningful to the system, not to
  // a person reading this table. Every other field on every module is
  // shown; this is the one deliberate exception, for the same
  // "not ambiguous, not clutter" reason payment_method IS shown: one
  // is a real fact someone wants to see at a glance, the other is
  // plumbing.
  //
  // `invoice.items_json` joins this exception for a different but
  // related reason: it's not plumbing, it's real content (the line
  // items), but showing the raw JSON string itself in a plain table
  // cell is pure clutter — a person already sees those same line
  // items rendered properly (description/qty/price/amount) the moment
  // they open the invoice via InvoiceView, one click away in the same
  // row. Showing the raw JSON a second time here adds nothing but a
  // wall of `{"description":...}` text, and — being far longer than
  // every other column — is also what was forcing this table into
  // horizontal scroll on an otherwise perfectly ordinary-width screen.
  const columns = useMemo(
    () => schema?.fields.map((f) => f.name).filter((n) => n !== 'source_order_id' && !(moduleId === 'invoice' && n === 'items_json')) ?? [],
    [schema, moduleId]
  );
  const canDelete = schema?.my_permissions.includes('delete');
  const canExport = schema?.my_permissions.includes('export');
  const canCreate = schema?.my_permissions.includes('create');
  const canUpdate = schema?.my_permissions.includes('update');
  // "settle" lives on debt_credit's own actions list (see
  // debt_credit.json / debt_settlement.rs) and is only ever checked
  // from this page being the Debt & Credit module itself — same
  // situation the inventoryCanRepack comment above already describes
  // for "repack", so no separate schema fetch is needed here either.
  const canSettle = moduleId === 'debt_credit' && !!schema?.my_permissions.includes('settle');

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

      {moduleId === 'debt_credit' && tab === 'records' && <DebtSummaryWidget refreshKey={debtSummaryRefreshKey} />}

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
              {canCreate && moduleId !== 'invoice' && (
                <button className="btn btn-outline" onClick={() => { setShowExcelImport(true); setExcelResult(null); setExcelError(null); setExcelFile(null); setTemplateStatus(null); }}>
                  Import from Excel
                </button>
              )}
              {moduleId === 'invoice' ? (
                canCreate && <button className="btn btn-stamp" onClick={() => setShowInvoiceForm((v) => !v)}>{showInvoiceForm ? 'Cancel' : '+ New invoice'}</button>
              ) : (
                <>
                  {canCreate && (
                    <button className="btn btn-stamp" onClick={() => (showForm ? cancelForm() : setShowForm(true))}>
                      {showForm ? 'Cancel' : '+ New'}
                    </button>
                  )}
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

          {showForm && moduleId !== 'invoice' && (
            <form onSubmit={handleSubmit} className="card" style={styles.form}>
              {editingId !== null && (
                <div style={{ fontSize: '0.8rem', color: 'var(--ink-soft)', marginBottom: '0.6rem' }}>Editing record</div>
              )}
              {editingId !== null && moduleId === 'inventory' && (
                <div style={{ fontSize: '0.72rem', color: 'var(--ink-faint)', marginBottom: '0.6rem' }}>
                  Stock quantity isn't edited here — use Sell, Receive (via Purchasing), or Repack to change how much is in stock.
                </div>
              )}
              {editingId === null && moduleId === 'inventory' && (
                <div style={{ fontSize: '0.72rem', color: 'var(--ink-faint)', marginBottom: '0.6rem' }}>
                  New items start at 0 in stock — receive them through Purchasing to bring stock in.
                </div>
              )}
              <div style={styles.formGrid}>
                {moduleId === 'purchasing' && (
                  <PurchaseItemSelector
                    items={inventoryItems}
                    value={formValues.inventory_record_id ?? ''}
                    required
                    onChange={(id, name) => setFormValues((p) => ({ ...p, inventory_record_id: id, item_name: name }))}
                  />
                )}
                {schema.fields.filter((f) => !isActionManagedField(moduleId, f.name) && !(moduleId === 'purchasing' && (f.name === 'item_name' || f.name === 'inventory_record_id'))).map((f) => (
                  <FieldInput key={f.name} field={f} value={formValues[f.name] ?? ''} units={units} currencies={currencies} businessCurrency={businessCurrency} onChange={(v) => setFormValues((p) => ({ ...p, [f.name]: v }))} />
                ))}
              </div>
              <div style={{ display: 'flex', gap: '0.5rem', marginTop: '0.8rem' }}>
                <button className="btn btn-stamp" type="submit">{editingId !== null ? 'Save changes' : 'Save'}</button>
                {editingId !== null && <button className="btn btn-outline" type="button" onClick={cancelForm}>Cancel</button>}
              </div>
            </form>
          )}

          <div className="card" style={{ padding: 0, overflowX: 'auto' }}>
            <table style={styles.table}>
              <thead>
                <tr>
                  {columns.map((c) => <th key={c} style={styles.th}>{c.replace(/_/g, ' ')}</th>)}
                  {(canUpdate || canDelete || (moduleId === 'purchasing' && inventoryCanReceive) || (moduleId === 'inventory' && inventoryCanRepack) || canSettle) && moduleId !== 'invoice' && <th style={styles.th} />}
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
                        {formatCell(r[c], schema!.fields.find((f) => f.name === c)?.type, businessCurrency)}
                      </td>
                    ))}
                    {moduleId === 'invoice' && (
                      <td style={styles.td}>
                        <div style={{ display: 'flex', gap: '0.4rem', flexWrap: 'wrap' }}>
                          <button className="btn btn-stamp" style={{ padding: '0.3em 0.7em', fontSize: '0.78rem' }} onClick={() => setViewingInvoiceId(r.id)}>
                            View
                          </button>
                          {typeof r.source_sale_id === 'string' && r.source_sale_id && (
                            <button className="btn btn-outline" style={{ padding: '0.3em 0.7em', fontSize: '0.78rem' }} onClick={() => setViewingReceiptOrderId(r.source_sale_id as string)}>
                              Receipt
                            </button>
                          )}
                        </div>
                      </td>
                    )}
                    {moduleId !== 'invoice' && (canUpdate || canDelete || (moduleId === 'purchasing' && inventoryCanReceive) || (moduleId === 'inventory' && inventoryCanRepack) || canSettle) && (
                      <td style={styles.td}>
                        <div style={{ display: 'flex', gap: '0.4rem', flexWrap: 'wrap' }}>
                          {moduleId === 'purchasing' && inventoryCanReceive && !r.received && (
                            <button className="btn btn-stamp" style={{ padding: '0.3em 0.7em', fontSize: '0.78rem' }} onClick={() => { setReceivingId(r.id); setReceiveQtyText(''); setReceiveError(null); }}>
                              Receive
                            </button>
                          )}
                          {moduleId === 'inventory' && inventoryCanRepack && (
                            <button className="btn btn-outline" style={{ padding: '0.3em 0.7em', fontSize: '0.78rem' }} onClick={() => { setRepackSourceId(r.id); setRepackTargetId(''); setRepackTargetMode('existing'); setRepackNewTargetName(''); setRepackNewTargetPriceText(''); setRepackSourceQtyText('1'); setRepackTargetQtyText(''); setRepackNotes(''); setRepackError(null); }}>
                              Repack
                            </button>
                          )}
                          {canSettle && !r.settled && (
                            <button className="btn btn-stamp" style={{ padding: '0.3em 0.7em', fontSize: '0.78rem' }} onClick={() => { setSettlingId(r.id); setSettleError(null); setSettlePaymentMethod('cash'); }}>
                              Settle
                            </button>
                          )}
                          {canUpdate && (
                            <button className="btn btn-outline" style={{ padding: '0.3em 0.7em', fontSize: '0.78rem' }} onClick={() => startEdit(r)}>Edit</button>
                          )}
                          {canDelete && (
                            <button className="btn btn-outline" style={{ padding: '0.3em 0.7em', fontSize: '0.78rem' }} onClick={() => handleDelete(r.id)}>Delete</button>
                          )}
                        </div>
                      </td>
                    )}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      ) : (
        <ReportPanel moduleId={moduleId} schema={schema} canExport={!!canExport} businessCurrency={businessCurrency} />
      )}

      {viewingInvoiceId && (
        <InvoiceView invoiceId={viewingInvoiceId} onClose={() => setViewingInvoiceId(null)} onStatusChanged={refreshRecords} />
      )}

      {viewingReceiptOrderId && (
        <ReceiptView orderId={viewingReceiptOrderId} onClose={() => setViewingReceiptOrderId(null)} />
      )}

      {actionResult && (
        <div className="card" style={{ ...styles.error, background: 'var(--paper-highlight, #eef7ee)', color: 'var(--ink)', display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
          <span>{actionResult}</span>
          <button className="btn btn-outline" style={{ padding: '0.2em 0.6em', fontSize: '0.75rem' }} onClick={() => setActionResult(null)}>Dismiss</button>
        </div>
      )}

      {receivingId && (
        <div style={styles.overlay} onClick={() => setReceivingId(null)}>
          <div className="card" style={styles.modal} onClick={(e) => e.stopPropagation()}>
            <h3 style={{ marginTop: 0 }}>Receive stock</h3>
            <p style={{ fontSize: '0.85rem', color: 'var(--ink-soft)' }}>
              Marks this purchase order received and adds the stock to Inventory, recalculating its weighted-average
              cost. Leave the quantity blank to receive everything that was ordered, or enter a smaller number for a
              partial delivery.
            </p>
            <label>Quantity received (optional)</label>
            <input
              type="text"
              inputMode="numeric"
              placeholder="Full ordered quantity"
              value={receiveQtyText}
              onChange={(e) => setReceiveQtyText(e.target.value)}
              style={{ width: '100%' }}
            />
            {receiveError && <div style={styles.error}>{receiveError}</div>}
            <div style={styles.modalActions}>
              <button className="btn btn-outline" onClick={() => setReceivingId(null)} disabled={receiveSubmitting}>Cancel</button>
              <button className="btn btn-stamp" onClick={submitReceive} disabled={receiveSubmitting}>
                {receiveSubmitting ? 'Receiving…' : 'Confirm receipt'}
              </button>
            </div>
          </div>
        </div>
      )}

      {repackSourceId && (
        <div style={styles.overlay} onClick={() => setRepackSourceId(null)}>
          <div className="card" style={styles.modal} onClick={(e) => e.stopPropagation()}>
            <h3 style={{ marginTop: 0 }}>Repack / break bulk</h3>
            <p style={{ fontSize: '0.85rem', color: 'var(--ink-soft)' }}>
              Converts stock from "{String(records.find((r) => r.id === repackSourceId)?.name ?? 'this item')}" into a
              different retail unit — e.g. breaking a sack into loose kilogram bags.
            </p>
            <label>Produces (target item)</label>
            <div style={{ display: 'flex', gap: '1rem', marginBottom: '0.4rem', fontSize: '0.85rem' }}>
              <label style={{ display: 'flex', alignItems: 'center', gap: '0.3rem', fontWeight: 'normal' }}>
                <input
                  type="radio"
                  checked={repackTargetMode === 'existing'}
                  onChange={() => setRepackTargetMode('existing')}
                />
                An existing item
              </label>
              <label style={{ display: 'flex', alignItems: 'center', gap: '0.3rem', fontWeight: 'normal' }}>
                <input
                  type="radio"
                  checked={repackTargetMode === 'new'}
                  onChange={() => setRepackTargetMode('new')}
                />
                Create a new item
              </label>
            </div>
            {repackTargetMode === 'existing' ? (
              <select value={repackTargetId} onChange={(e) => setRepackTargetId(e.target.value)} style={{ width: '100%' }}>
                <option value="">Select the item being produced…</option>
                {records.filter((r) => r.id !== repackSourceId).map((r) => (
                  <option key={r.id} value={r.id}>{String(r.name ?? r.sku ?? r.id)}</option>
                ))}
              </select>
            ) : (
              <div style={{ display: 'flex', gap: '0.6rem' }}>
                <div style={{ flex: 2 }}>
                  <label style={{ fontSize: '0.8rem', fontWeight: 'normal' }}>Name</label>
                  <input
                    type="text"
                    placeholder="e.g. Rice — 1kg bag"
                    value={repackNewTargetName}
                    onChange={(e) => setRepackNewTargetName(e.target.value)}
                    style={{ width: '100%' }}
                  />
                </div>
                <div style={{ flex: 1 }}>
                  <label style={{ fontSize: '0.8rem', fontWeight: 'normal' }}>Selling price</label>
                  <input
                    type="text"
                    inputMode="decimal"
                    placeholder="0.00"
                    value={repackNewTargetPriceText}
                    onChange={(e) => setRepackNewTargetPriceText(e.target.value)}
                    style={{ width: '100%' }}
                  />
                </div>
              </div>
            )}
            {repackTargetMode === 'new' && (
              <p style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', marginTop: '0.3rem' }}>
                A SKU is generated automatically. Cost isn't asked for here — it's calculated from what this
                repack actually consumes.
              </p>
            )}
            <div style={{ display: 'flex', gap: '0.6rem', marginTop: '0.6rem' }}>
              <div style={{ flex: 1 }}>
                <label>Quantity consumed</label>
                <input type="text" inputMode="numeric" value={repackSourceQtyText} onChange={(e) => setRepackSourceQtyText(e.target.value)} style={{ width: '100%' }} />
              </div>
              <div style={{ flex: 1 }}>
                <label>Quantity produced</label>
                <input type="text" inputMode="numeric" value={repackTargetQtyText} onChange={(e) => setRepackTargetQtyText(e.target.value)} style={{ width: '100%' }} />
              </div>
            </div>
            <label style={{ marginTop: '0.6rem', display: 'block' }}>Notes (optional)</label>
            <input type="text" value={repackNotes} onChange={(e) => setRepackNotes(e.target.value)} style={{ width: '100%' }} />
            {repackError && <div style={styles.error}>{repackError}</div>}
            <div style={styles.modalActions}>
              <button className="btn btn-outline" onClick={() => setRepackSourceId(null)} disabled={repackSubmitting}>Cancel</button>
              <button
                className="btn btn-stamp"
                onClick={submitRepack}
                disabled={repackSubmitting || (repackTargetMode === 'existing' ? !repackTargetId : !repackNewTargetName.trim())}
              >
                {repackSubmitting ? 'Repacking…' : 'Confirm repack'}
              </button>
            </div>
          </div>
        </div>
      )}

      {settlingId && (
        <div style={styles.overlay} onClick={() => setSettlingId(null)}>
          <div className="card" style={styles.modal} onClick={(e) => e.stopPropagation()}>
            <h3 style={{ marginTop: 0 }}>Settle debt/credit</h3>
            {(() => {
              const r = records.find((rec) => rec.id === settlingId);
              const isIncome = r?.direction === 'owed_to_business';
              return (
                <p style={{ fontSize: '0.85rem', color: 'var(--ink-soft)' }}>
                  Marks "{String(r?.party_name ?? 'this record')}" as settled and posts{' '}
                  {formatMoney(Number(r?.amount ?? 0), businessCurrency)} to Bookkeeping as{' '}
                  {isIncome ? 'income (money received)' : 'an expense (money paid out)'}, if Bookkeeping is enabled.
                  This can't be undone from here — settling again once done isn't possible, to avoid posting the
                  same amount twice.
                </p>
              );
            })()}
            {settleError && <div style={styles.error}>{settleError}</div>}
            <div style={{ marginTop: '0.6rem' }}>
              <label>Payment method</label>
              <select value={settlePaymentMethod} onChange={(e) => setSettlePaymentMethod(e.target.value)} style={{ width: '100%' }}>
                <option value="cash">Cash</option>
                <option value="mobile_money">Mobile money</option>
                <option value="card">Card</option>
              </select>
            </div>
            <div style={styles.modalActions}>
              <button className="btn btn-outline" onClick={() => setSettlingId(null)} disabled={settleSubmitting}>Cancel</button>
              <button className="btn btn-stamp" onClick={submitSettle} disabled={settleSubmitting}>
                {settleSubmitting ? 'Settling…' : 'Confirm settlement'}
              </button>
            </div>
          </div>
        </div>
      )}

      {showExcelImport && schema && (
        <div style={styles.overlay} onClick={() => setShowExcelImport(false)}>
          <div className="card" style={styles.modal} onClick={(e) => e.stopPropagation()}>
            <h3 style={{ marginTop: 0 }}>Import from Excel</h3>
            {moduleId === 'inventory' ? (
              <p style={{ fontSize: '0.85rem', color: 'var(--ink-soft)' }}>
                Download the template below to add new items — it has no quantity column, since a new
                item always starts at zero stock. To do a stock take instead, use "Export to Excel" on
                existing records, correct the counted quantities in that file, then reimport it; a row
                whose SKU already exists will be rejected if it comes from the blank template.
              </p>
            ) : moduleId === 'purchasing' ? (
              <p style={{ fontSize: '0.85rem', color: 'var(--ink-soft)' }}>
                Download the template below to place new orders — every row becomes a brand new
                purchase order with its own PO number, and is received immediately: stock lands in
                Inventory and its cost is recalculated right away, no separate "Receive" click needed.
                To correct a mistake afterward, use "Export to Excel" instead, fix that row, and reimport
                it — matching is done by PO number, so the correction lands on the right order (note:
                once an order has been received this way, its quantity and cost can no longer be changed
                by reimporting, since Inventory has already been updated from the original figures — use
                Repack or an Inventory stock take to adjust from there instead).
              </p>
            ) : (
              <p style={{ fontSize: '0.85rem', color: 'var(--ink-soft)' }}>
                Download the template below, fill it in (or export your existing records and edit them),
                then upload it back here. Matching rows update the existing record instead of creating a
                duplicate.
              </p>
            )}
            <button className="btn btn-outline" onClick={handleDownloadTemplate} disabled={templateDownloading} style={{ marginBottom: '0.4rem' }}>
              {templateDownloading ? 'Downloading…' : 'Download template'}
            </button>
            {templateStatus && <div style={{ fontSize: '0.85rem', color: 'var(--ink-soft)', marginBottom: '0.8rem' }}>{templateStatus}</div>}

            <label>File to import</label>
            <input
              type="file"
              accept=".xlsx"
              onChange={(e) => setExcelFile(e.target.files?.[0] ?? null)}
              style={{ width: '100%' }}
            />

            {(() => {
              // Matching a re-uploaded row against an existing record
              // is only ever safe on a field the module actually
              // marked `unique` (see excel_import.rs's
              // `key_field_is_unique` comment for the full reasoning
              // and the exact bug this closes) — never just "whichever
              // field happens to be listed first." Purchasing (and any
              // similar append-only module with no unique field) has
              // nothing safe to match on at all, so there's no picker
              // to show; every row on that kind of module always
              // creates a new record, which is what re-importing a
              // transaction log should do anyway.
              const uniqueFields = schema.fields.filter((f) => f.unique);
              if (uniqueFields.length === 0) {
                return (
                  <div style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', marginTop: '0.6rem' }}>
                    This module has no unique field to match rows against, so every imported row creates a
                    new record — re-uploading the same file will create duplicates, not update anything.
                  </div>
                );
              }
              return (
                <>
                  <label style={{ marginTop: '0.6rem', display: 'block' }}>Match existing records by</label>
                  <select value={excelKeyField} onChange={(e) => setExcelKeyField(e.target.value)} style={{ width: '100%' }}>
                    <option value="">{uniqueFields[0].name} (default)</option>
                    {uniqueFields.map((f) => (
                      <option key={f.name} value={f.name}>{f.name.replace(/_/g, ' ')}</option>
                    ))}
                  </select>
                </>
              );
            })()}

            {excelError && <div style={styles.error}>{excelError}</div>}

            {excelResult && (
              <div style={{ marginTop: '0.8rem', fontSize: '0.85rem' }}>
                <div>
                  {excelResult.created} created, {excelResult.updated} updated.
                  {moduleId === 'purchasing' && excelResult.created > 0 ? ' New orders were received immediately — stock is already in Inventory.' : ''}
                </div>
                {excelResult.errors.length > 0 && (
                  <div style={{ marginTop: '0.4rem', color: 'var(--stamp)' }}>
                    {excelResult.errors.length} row(s) had problems:
                    <ul style={{ margin: '0.3rem 0 0', paddingLeft: '1.2rem' }}>
                      {excelResult.errors.slice(0, 10).map((e: { row: number; error: string }, i: number) => (
                        <li key={i}>Row {e.row}: {e.error}</li>
                      ))}
                    </ul>
                    {excelResult.errors.length > 10 && <div>…and {excelResult.errors.length - 10} more.</div>}
                  </div>
                )}
              </div>
            )}

            <div style={styles.modalActions}>
              <button className="btn btn-outline" onClick={() => setShowExcelImport(false)} disabled={excelImporting}>Close</button>
              <button className="btn btn-stamp" onClick={handleExcelImport} disabled={excelImporting || !excelFile}>
                {excelImporting ? 'Importing…' : 'Import'}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

function formatCell(v: unknown, fieldType?: string, currency?: string) {
  if (v === null || v === undefined) return <span style={{ color: 'var(--ink-faint)' }}>—</span>;
  if (fieldType === 'money' && typeof v === 'number') return formatMoney(v, currency ?? 'USD');
  return String(v);
}

function PurchaseItemSelector({ items, value, required, onChange }: { items: Record_[]; value: string; required?: boolean; onChange: (id: string, name: string) => void }) {
  return (
    <div>
      <label>Inventory item{required ? ' *' : ''}</label>
      <select
        value={value}
        required={required}
        onChange={(e) => {
          const item = items.find((record) => record.id === e.target.value);
          onChange(e.target.value, String(item?.name ?? ''));
        }}
      >
        <option value="">Select an item from Inventory...</option>
        {items.map((item) => (
          <option key={item.id} value={item.id}>
            {String(item.name ?? item.sku ?? item.id)} · {String(item.quantity ?? 0)} in stock
          </option>
        ))}
      </select>
      {items.length === 0 && (
        <div style={{ fontSize: '0.72rem', color: 'var(--ink-faint)', marginTop: '0.2em' }}>
          Create the item in Inventory first. New catalog items start at zero stock; receiving this purchase adds the delivered quantity.
        </div>
      )}
    </div>
  );
}

function FieldInput({ field, value, units, currencies, businessCurrency, onChange }: { field: FieldDef; value: string; units: Unit[]; currencies: Currency[]; businessCurrency: string; onChange: (v: string) => void }) {
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
  if (field.type === 'money') {
    // Plain text buffer, not a reformatted controlled value — typing
    // over a value that snaps back to "12.50" on every keystroke
    // fights the cursor (the same bug fixed in PointOfSale.tsx's
    // refund field and the invoice form above). Normalizes to a clean
    // decimal only on blur, and the actual integer-cents conversion
    // happens once, at submit time, via parseMoneyInput.
    return (
      <div>
        <label>{field.name.replace(/_/g, ' ')}{field.required ? ' *' : ''}</label>
        <input
          type="text"
          inputMode="decimal"
          value={value}
          required={field.required}
          onChange={(e) => onChange(e.target.value)}
          onBlur={() => {
            const parsed = parseMoneyInput(value, businessCurrency);
            if (parsed !== null) onChange(formatMoney(parsed, businessCurrency));
          }}
          style={{ width: '100%' }}
        />
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

export function ReportPanel({ moduleId, schema, canExport, businessCurrency }: { moduleId: string; schema: ModuleSchema; canExport: boolean; businessCurrency: string }) {
  const numericFields = schema.fields.filter((f) => f.type === 'integer' || f.type === 'real' || f.type === 'money');
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
  // Only meaningful when agg !== 'count' — a count is always a plain
  // number regardless of what field was picked as "measure" (which is
  // ignored for count anyway).
  const measureIsMoney = agg !== 'count' && numericFields.find((f) => f.name === measure)?.type === 'money';

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
            <span className="mono" style={{ width: 80, textAlign: 'right', fontSize: '0.82rem' }}>
              {measureIsMoney ? formatMoney(p.value, businessCurrency) : p.value.toLocaleString()}
            </span>
          </div>
        ))}
      </div>
      </div>
      {numericFields.length > 0 && <ForecastPanel moduleId={moduleId} numericFields={numericFields} businessCurrency={businessCurrency} />}
    </>
  );
}

// Editing state keeps the unit price as raw typed text — not
// integer cents directly — so the input never fights the user's
// cursor by reformatting mid-keystroke (the same bug class fixed in
// PointOfSale.tsx's refund amount field). Only converted to actual
// integer cents (via money.ts's strict parser) at submit time.
interface EditableInvoiceItem {
  description: string;
  quantity: number;
  unit_price_text: string;
}

function NewInvoiceForm({ onCreated, onCancel }: { onCreated: () => void; onCancel: () => void }) {
  const [customer, setCustomer] = useState('');
  const [customerEmail, setCustomerEmail] = useState('');
  const [customerPhone, setCustomerPhone] = useState('');
  const [dueDate, setDueDate] = useState('');
  const [notes, setNotes] = useState('');
  const [items, setItems] = useState<EditableInvoiceItem[]>([{ description: '', quantity: 1, unit_price_text: '0.00' }]);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [currency, setCurrency] = useState('USD');

  useEffect(() => {
    getBusinessInfo()
      .then((b: any) => { if (b?.currency) setCurrency(b.currency); })
      .catch(() => {}); // default 'USD' stands if this fails
  }, []);

  function lineCents(it: EditableInvoiceItem): number {
    const price = parseMoneyInput(it.unit_price_text, currency) ?? 0;
    return (it.quantity || 0) * price;
  }
  const subtotal = items.reduce((sum, it) => sum + lineCents(it), 0);

  function updateItem(i: number, patch: Partial<EditableInvoiceItem>) {
    setItems((prev) => prev.map((it, idx) => (idx === i ? { ...it, ...patch } : it)));
  }
  function addItem() {
    setItems((prev) => [...prev, { description: '', quantity: 1, unit_price_text: '0.00' }]);
  }
  function removeItem(i: number) {
    setItems((prev) => prev.filter((_, idx) => idx !== i));
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    const cleanItems: NewInvoiceItem[] = [];
    for (const it of items) {
      if (!it.description.trim() || !(it.quantity > 0)) continue;
      const cents = parseMoneyInput(it.unit_price_text, currency);
      if (cents === null || cents < 0) {
        setError(`"${it.description || 'A line item'}" has an invalid unit price.`);
        return;
      }
      cleanItems.push({ description: it.description, quantity: it.quantity, unit_price: cents });
    }
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
            type="text"
            inputMode="decimal"
            placeholder="Unit price"
            value={it.unit_price_text}
            onChange={(e) => updateItem(i, { unit_price_text: e.target.value })}
            onBlur={() => {
              const parsed = parseMoneyInput(it.unit_price_text, currency);
              if (parsed !== null) updateItem(i, { unit_price_text: formatMoney(parsed, currency) });
            }}
            style={{ width: 110 }}
          />
          <span className="mono" style={{ width: 90, textAlign: 'right', fontSize: '0.85rem', color: 'var(--ink-soft)' }}>
            {formatMoney(lineCents(it), currency)}
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
          Subtotal: <span className="mono">{formatMoney(subtotal, currency)}</span>
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

function ForecastPanel({ moduleId, numericFields, businessCurrency }: { moduleId: string; numericFields: FieldDef[]; businessCurrency: string }) {
  const [measure, setMeasure] = useState(numericFields[0]?.name ?? '');
  const [bucket, setBucket] = useState('month');
  const [method, setMethod] = useState<'moving_average' | 'exponential_smoothing'>('moving_average');
  const [result, setResult] = useState<{ forecast_next: number; method: string; history: { label: string; value: number }[] } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const measureIsMoney = numericFields.find((f) => f.name === measure)?.type === 'money';

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
            {measureIsMoney ? formatMoney(result.forecast_next, businessCurrency) : result.forecast_next.toLocaleString()}
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
  overlay: { position: 'fixed', inset: 0, background: 'rgba(0,0,0,0.45)', display: 'flex', alignItems: 'center', justifyContent: 'center', zIndex: 1000 },
  modal: { background: 'var(--paper-card)', borderRadius: 10, padding: '1.4rem', width: 440, maxWidth: '94vw', maxHeight: '90vh', overflowY: 'auto', boxShadow: '0 8px 32px rgba(0,0,0,0.25)' },
  modalActions: { display: 'flex', gap: '0.5rem', marginTop: '1rem', justifyContent: 'flex-end' },
};
