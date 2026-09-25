import { useEffect, useMemo, useRef, useState } from 'react';
import {
  getModuleSchema, listRecords, createRecord, updateRecord, deleteRecord, exportModule,
  downloadImportTemplate, importExcel,
  runReport, exportReport, listUnits, listCurrencies, runForecast, createInvoice, getBusinessInfo,
  repackStock, settleDebt, getBatches, updateBatchPrice, ApiError,
} from '../api';
import type { NewInvoiceItem, ImportExcelResult, BatchSummary } from '../api';
import type { ModuleSchema, Record_, FieldDef, Unit, Currency } from '../types';
import { formatMoney, parseMoneyInput } from '../lib/money';
import InvoiceView from '../components/InvoiceView';
import ReceiptView from '../components/ReceiptView';
import DebtSummaryWidget from '../components/DebtSummary';

// Fields that are only ever set by a dedicated backend action, not the
// generic form: purchasing's `received`/`po_number` (receiving.rs),
// debt_credit's `settled`/`payment_method`/`source_order_id`/`entry_number`
// (debt_settlement.rs), and inventory's `quantity` (stock only moves via
// sell/receive/refund/repack). Enforced server-side too — this just
// keeps the form from showing a field the user can't actually edit.
function isActionManagedField(moduleId: string, fieldName: string): boolean {
  return (moduleId === 'purchasing' && (fieldName === 'received' || fieldName === 'po_number'))
    || (moduleId === 'debt_credit' && fieldName === 'settled')
    || (moduleId === 'debt_credit' && (fieldName === 'payment_method' || fieldName === 'source_order_id'))
    || (moduleId === 'debt_credit' && fieldName === 'entry_number')
    || (moduleId === 'inventory' && fieldName === 'quantity');
}

// Reads a File into a bare base64 string (no data URL prefix).
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
  // null = creating a new record; a string = editing that record's id.
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
  // Values startEdit seeded the form with, so submit sends only what
  // actually changed (a real PATCH, not a full resubmission).
  const [originalFormValues, setOriginalFormValues] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);
  const [tab, setTab] = useState<'records' | 'report'>('records');
  const [loading, setLoading] = useState(true);
  const [units, setUnits] = useState<Unit[]>([]);
  const [currencies, setCurrencies] = useState<Currency[]>([]);
  const [inventoryItems, setInventoryItems] = useState<Record_[]>([]);
  // Needed for correct money-field decimal places (e.g. JPY 0dp, KWD 3dp).
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
        // Inventory's own schema already lists "repack" permissions.
        setInventoryCanRepack(moduleId === 'inventory' && s.my_permissions.includes('repack'));
        // Only fetch the reference-data lists this module actually needs.
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
  // elsewhere — another module's action, or another window. Ordinary
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
  const [inventoryCanRepack, setInventoryCanRepack] = useState(false);

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

  // Viewing/editing an item's batches. See batches.rs. Loaded fresh
  // every time the modal opens rather than kept in sync with the main
  // records list, since a batch's own quantity/cost/price live in a
  // separate table this page never otherwise fetches.
  const [viewingBatchesId, setViewingBatchesId] = useState<string | null>(null);
  const [batchSummary, setBatchSummary] = useState<BatchSummary | null>(null);
  const [batchesLoading, setBatchesLoading] = useState(false);
  const [batchesError, setBatchesError] = useState<string | null>(null);
  const [editingBatchId, setEditingBatchId] = useState<string | null>(null);
  const [batchPriceText, setBatchPriceText] = useState('');
  const [batchCostText, setBatchCostText] = useState('');
  const [batchSaveError, setBatchSaveError] = useState<string | null>(null);
  const [batchSaving, setBatchSaving] = useState(false);

  async function openBatches(id: string) {
    setViewingBatchesId(id);
    setBatchesLoading(true);
    setBatchesError(null);
    setBatchSummary(null);
    try {
      setBatchSummary(await getBatches(id));
    } catch (err) {
      setBatchesError(err instanceof ApiError ? err.message : 'Could not load batches for this item');
    } finally {
      setBatchesLoading(false);
    }
  }

  function startEditBatch(b: { id: string; unit_price: number; unit_cost: number }) {
    setEditingBatchId(b.id);
    // Was hardcoded (/100).toFixed(2) — wrong for any currency that
    // isn't 2-decimal (100x too small for JPY/UGX/RWF, 10x too large
    // for BHD/KWD/OMR/JOD). formatMoney is already what the read-only
    // view two lines away uses, and it's the same seed-an-editable-
    // text-input pattern already established in PointOfSale.tsx's
    // refund flow (setRefundAmountText(formatMoney(...))) — parseMoneyInput
    // below already round-trips whatever formatMoney produces.
    setBatchPriceText(formatMoney(b.unit_price, businessCurrency));
    setBatchCostText(formatMoney(b.unit_cost, businessCurrency));
    setBatchSaveError(null);
  }

  async function submitBatchPrice() {
    if (!editingBatchId || !viewingBatchesId) return;
    const price = parseMoneyInput(batchPriceText, businessCurrency);
    if (price === null) {
      setBatchSaveError('Selling price must be a valid amount.');
      return;
    }
    // Only sent when it's actually being changed from what the batch
    // already carries — same "omit unless changing" reasoning as
    // api.ts's updateBatchPrice itself: a Manager who never touches
    // cost should never trip the Owner-only check just because a cost
    // value was present in the form.
    const originalCost = batchSummary?.batches.find((b) => b.id === editingBatchId)?.unit_cost;
    const parsedCost = batchCostText.trim() === '' ? null : parseMoneyInput(batchCostText, businessCurrency);
    if (batchCostText.trim() !== '' && parsedCost === null) {
      setBatchSaveError('Cost must be a valid amount.');
      return;
    }
    const costOverride = parsedCost !== null && parsedCost !== originalCost ? parsedCost : undefined;
    setBatchSaving(true);
    setBatchSaveError(null);
    try {
      await updateBatchPrice(viewingBatchesId, editingBatchId, price, costOverride);
      setEditingBatchId(null);
      setBatchSummary(await getBatches(viewingBatchesId));
      await refreshRecords();
    } catch (err) {
      setBatchSaveError(err instanceof ApiError ? err.message : 'Could not update this batch');
    } finally {
      setBatchSaving(false);
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
      // An empty box means "not set yet, use the default" and is
      // never sent, on either create or edit — this deliberately means
      // editing can't blank out an optional field back to empty
      // through this form (only change it to something else). The
      // alternative, treating an empty box as "clear this", breaks per
      // field type (an empty numeric input becomes NaN, which JSON
      // serializes as null, which then fails backend validation with a
      // confusing error) rather than doing anything useful, so it's
      // not supported rather than half-supported.
      //
      // Create sends every non-empty field, same as before. Editing
      // additionally skips any field whose value is IDENTICAL to what
      // startEdit originally seeded it with — see originalFormValues'
      // own comment for why this is the fix, not just a nicety. Money
      // fields are compared as their parsed cents, not their raw typed
      // string, so "19.99" and "19.990" (or a locale that displays a
      // trailing zero differently) don't look like a change when they
      // aren't one.
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
          if (editingId !== null) {
            const originalCents = originalFormValues[f.name] !== undefined && originalFormValues[f.name] !== ''
              ? parseMoneyInput(originalFormValues[f.name], businessCurrency)
              : null;
            if (originalCents === cents) continue;
          }
          payload[f.name] = cents;
          continue;
        }
        if (editingId !== null && raw === originalFormValues[f.name]) continue;
        payload[f.name] = f.type === 'integer' ? parseInt(raw, 10)
          : f.type === 'real' ? parseFloat(raw)
          : f.type === 'boolean' ? raw === 'true'
          : raw;
      }
      // expiry_date: same reason as PurchaseItemSelector's
      // inventory_record_id/item_name aren't set via the loop above —
      // it isn't in schema.fields (see the input's own comment above)
      // so the loop never picks it up. Create-only, matching the
      // input being hidden on edit: create_and_receive (receiving.rs)
      // is the only path that ever reads this key, and it only runs
      // on create.
      if (moduleId === 'purchasing' && editingId === null && formValues.expiry_date) {
        payload.expiry_date = formValues.expiry_date;
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
    setOriginalFormValues(seeded);
    setEditingId(record.id);
    setShowForm(true);
  }

  function cancelForm() {
    setFormValues({});
    setOriginalFormValues({});
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
  // Invoice-only column collapse: customer_email/customer_phone fold into
  // the existing `customer` cell, and tax_rate folds into the existing
  // `tax_amount` cell (rendered as "rate% · amount"). No field is dropped
  // or renamed in the schema/data — see the `customer`/`tax_amount` cases
  // in the table body below, which render the combined content. This is
  // scoped to moduleId === 'invoice' only, so every other module's table
  // (and this module's own create/edit form, which still lists every
  // field via schema.fields directly) is untouched.
  // Sales column collapse: customer_phone folds into the existing
  // `customer` cell (same idea as invoice's customer_email/phone
  // fold above), and discount_amount folds into the existing
  // `unit_price` cell as "price (− discount)" — see
  // renderSalesCustomerCell/renderSalesPriceCell below. order_id and
  // source_batch_id are dropped from the table entirely, same
  // reasoning as source_order_id just above: real UUIDs meaningful to
  // the system's own cross-references (refund.rs, receipt.rs), not to
  // an owner glancing down a list of sales — still exported in full
  // via "Export to Excel" for anyone who does need them. No field is
  // dropped from the data or from this module's own create/edit form,
  // which still lists every field via schema.fields directly — this
  // is a records-table display choice only, the same scope the
  // invoice collapse above is limited to.
  const columns = useMemo(
    () =>
      schema?.fields
        .map((f) => f.name)
        .filter(
          (n) =>
            n !== 'source_order_id' &&
            !(moduleId === 'invoice' && (n === 'items_json' || n === 'customer_email' || n === 'customer_phone' || n === 'tax_rate')) &&
            !(moduleId === 'sales' && (n === 'customer_phone' || n === 'discount_amount' || n === 'order_id' || n === 'source_batch_id'))
        ) ?? [],
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
  // "update_batch_price" lives on inventory's own actions list — same
  // situation as "settle" above, checked directly off this page's own
  // schema since it's only ever relevant when moduleId is inventory
  // itself. Granted to both Owner and Manager; a Manager attempting to
  // ALSO edit a batch's cost (not just its price) is still caught
  // server-side by batches::update_batch_price's own Owner-only check
  // — the cost input isn't hidden from Manager here, since this page
  // has no cheap way to tell Owner and Manager apart yet, so the
  // server's rejection message is what actually enforces it.
  const canEditBatchPrice = moduleId === 'inventory' && !!schema?.my_permissions.includes('update_batch_price');

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
              {/* Sales has no generic "+ New" or "Import from Excel" —
                  every sale has to carry real cost and stock data, and
                  the only two paths that produce that are Checkout and
                  Service Sale (see the "Sell" tab), neither of which
                  is this page. This isn't just hidden here: the server
                  itself rejects a direct sales create or import (see
                  crud::create's and excel_import's own `if module_id
                  == "sales"` guards) — hiding the button is only the
                  UI half of that, not the actual boundary. */}
              {canCreate && moduleId !== 'invoice' && moduleId !== 'sales' && (
                <button className="btn btn-outline" onClick={() => { setShowExcelImport(true); setExcelResult(null); setExcelError(null); setExcelFile(null); setTemplateStatus(null); }}>
                  Import from Excel
                </button>
              )}
              {moduleId === 'invoice' ? (
                canCreate && <button className="btn btn-stamp" onClick={() => setShowInvoiceForm((v) => !v)}>{showInvoiceForm ? 'Cancel' : '+ New invoice'}</button>
              ) : moduleId === 'sales' ? null : (
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
              <div style={styles.formGrid}>
                {moduleId === 'purchasing' && (
                  <PurchaseItemSelector
                    items={inventoryItems}
                    value={formValues.inventory_record_id ?? ''}
                    required
                    onChange={(id, name) => setFormValues((p) => ({ ...p, inventory_record_id: id, item_name: name }))}
                  />
                )}
                {/* Not a `schema.fields` entry, deliberately, same as
                    PurchaseItemSelector just above: `expiry_date`
                    belongs to the BATCH this purchase creates (see
                    batches.rs), not to the purchasing row itself, so
                    it isn't in purchasing.json and the generic field
                    loop below never renders it on its own. handleSubmit
                    reads this one field out of formValues by name and
                    adds it to the payload separately, since the normal
                    payload-building loop only walks schema.fields. */}
                {moduleId === 'purchasing' && editingId === null && (
                  <div>
                    <label>expiry date</label>
                    <input
                      type="date"
                      value={formValues.expiry_date ?? ''}
                      onChange={(e) => setFormValues((p) => ({ ...p, expiry_date: e.target.value }))}
                    />
                  </div>
                )}
                {schema.fields.filter((f) => !isActionManagedField(moduleId, f.name)
                  && !(moduleId === 'purchasing' && (f.name === 'item_name' || f.name === 'inventory_record_id'))
                  // unit_cost/unit_price: hidden on CREATE only. A
                  // brand-new item is always forced to zero regardless
                  // of what's typed (crud::create's own rule — real
                  // price only ever enters through a purchase), so
                  // showing an editable input here would let someone
                  // type a price that's then silently discarded. Once
                  // the item EXISTS, these fields are the sanctioned way
                  // to correct a legacy item's price for as long as it
                  // has no real batch yet (see crud::update's
                  // inventory_has_batches check) — the backend itself
                  // enforces the cutoff the moment a batch exists, with
                  // a clear error, so the edit form doesn't need to know
                  // which state a given item is in.
                  && !(moduleId === 'inventory' && editingId === null && (f.name === 'unit_cost' || f.name === 'unit_price'))
                ).map((f) => (
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
            <table className="data-table" style={styles.table}>
              <thead>
                <tr>
                  {columns.map((c) => (
                    <th key={c} style={styles.th}>
                      {moduleId === 'invoice' && c === 'tax_amount' ? 'tax' : c.replace(/_/g, ' ')}
                    </th>
                  ))}
                  {(canUpdate || canDelete || moduleId === 'inventory' || canSettle) && moduleId !== 'invoice' && <th style={styles.th} />}
                </tr>
              </thead>
              <tbody>
                {records.length === 0 && (
                  <tr><td colSpan={columns.length + 1} style={{ ...styles.empty, display: 'block', textAlign: 'center' }}>No records yet — add the first one above.</td></tr>
                )}
                {records.map((r) => (
                  <tr key={r.id}>
                    {columns.map((c) => (
                      <td key={c} className={typeof r[c] === 'number' ? 'mono' : ''} style={styles.td} data-label={moduleId === 'invoice' && c === 'tax_amount' ? 'tax' : c.replace(/_/g, ' ')}>
                        {moduleId === 'invoice' && c === 'customer'
                          ? renderInvoiceCustomerCell(r)
                          : moduleId === 'invoice' && c === 'tax_amount'
                          ? renderInvoiceTaxCell(r, businessCurrency)
                          : moduleId === 'sales' && c === 'customer'
                          ? renderSalesCustomerCell(r)
                          : moduleId === 'sales' && c === 'unit_price'
                          ? renderSalesPriceCell(r, businessCurrency)
                          : formatCell(r[c], schema!.fields.find((f) => f.name === c)?.type, businessCurrency)}
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
                    {moduleId !== 'invoice' && (canUpdate || canDelete || moduleId === 'inventory' || canSettle) && (
                      <td style={styles.td}>
                        <div style={{ display: 'flex', gap: '0.4rem', flexWrap: 'wrap' }}>
                          {moduleId === 'inventory' && inventoryCanRepack && (
                            <button className="btn btn-outline" style={{ padding: '0.3em 0.7em', fontSize: '0.78rem' }} onClick={() => { setRepackSourceId(r.id); setRepackTargetId(''); setRepackTargetMode('existing'); setRepackNewTargetName(''); setRepackNewTargetPriceText(''); setRepackSourceQtyText('1'); setRepackTargetQtyText(''); setRepackNotes(''); setRepackError(null); }}>
                              Repack
                            </button>
                          )}
                          {moduleId === 'inventory' && (
                            <button className="btn btn-outline" style={{ padding: '0.3em 0.7em', fontSize: '0.78rem' }} onClick={() => openBatches(r.id)}>
                              Batches
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

      {viewingBatchesId && (
        <div style={styles.overlay} onClick={() => { setViewingBatchesId(null); setEditingBatchId(null); }}>
          <div className="card" style={{ ...styles.modal, maxWidth: '620px' }} onClick={(e) => e.stopPropagation()}>
            <h3 style={{ marginTop: 0 }}>Batches — {String(records.find((r) => r.id === viewingBatchesId)?.name ?? 'this item')}</h3>
            {batchesLoading && <div style={{ color: 'var(--ink-soft)' }}>Loading…</div>}
            {batchesError && <div style={styles.error}>{batchesError}</div>}
            {batchSummary && (
              <>
                <div style={{ fontSize: '0.82rem', color: 'var(--ink-soft)', marginBottom: '0.6rem' }}>
                  Sells in this order — soonest-expiring batches first, then undated batches, then legacy stock last.
                </div>
                {batchSummary.legacy_quantity > 0 && (
                  <div style={{ display: 'flex', justifyContent: 'space-between', padding: '0.4rem 0', borderBottom: '1px solid var(--paper-line)', fontSize: '0.86rem' }}>
                    <div>
                      <div>Legacy stock (pre-batch)</div>
                      <div style={{ fontSize: '0.76rem', color: 'var(--ink-soft)' }}>{batchSummary.legacy_quantity} units · no expiry · sells last</div>
                    </div>
                    <div style={{ textAlign: 'right', fontSize: '0.8rem', color: 'var(--ink-soft)' }}>
                      <div>cost {formatMoney(batchSummary.legacy_unit_cost, businessCurrency)}</div>
                      <div>price {formatMoney(batchSummary.legacy_unit_price, businessCurrency)}</div>
                    </div>
                  </div>
                )}
                {batchSummary.batches.length === 0 && batchSummary.legacy_quantity === 0 && (
                  <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>No stock on hand.</div>
                )}
                {batchSummary.batches.map((b) => (
                  <div key={b.id} style={{ padding: '0.5rem 0', borderBottom: '1px solid var(--paper-line)', fontSize: '0.86rem' }}>
                    {editingBatchId === b.id ? (
                      <div>
                        <div style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', marginBottom: '0.3rem' }}>
                          {b.quantity_remaining} units remaining{b.expiry_date ? ` · expires ${b.expiry_date}` : ' · no expiry'}
                        </div>
                        <label style={{ fontSize: '0.78rem' }}>Selling price</label>
                        <input type="text" inputMode="decimal" value={batchPriceText} onChange={(e) => setBatchPriceText(e.target.value)} style={{ width: '100%' }} />
                        <label style={{ fontSize: '0.78rem', marginTop: '0.4rem', display: 'block' }}>Cost (Owner only)</label>
                        <input type="text" inputMode="decimal" value={batchCostText} onChange={(e) => setBatchCostText(e.target.value)} style={{ width: '100%' }} />
                        {batchSaveError && <div style={styles.error}>{batchSaveError}</div>}
                        <div style={{ display: 'flex', gap: '0.4rem', marginTop: '0.5rem' }}>
                          <button className="btn btn-outline" style={{ padding: '0.25em 0.6em', fontSize: '0.78rem' }} onClick={() => setEditingBatchId(null)} disabled={batchSaving}>Cancel</button>
                          <button className="btn btn-stamp" style={{ padding: '0.25em 0.6em', fontSize: '0.78rem' }} onClick={submitBatchPrice} disabled={batchSaving}>
                            {batchSaving ? 'Saving…' : 'Save'}
                          </button>
                        </div>
                      </div>
                    ) : (
                      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', gap: '0.6rem' }}>
                        <div>
                          <div>{b.quantity_remaining} units{b.source_po_number ? ` · PO ${b.source_po_number}` : ''}</div>
                          <div style={{ fontSize: '0.76rem', color: 'var(--ink-soft)' }}>
                            {b.expiry_date ? `Expires ${b.expiry_date}` : 'No expiry'} · received {b.received_at.slice(0, 10)}
                          </div>
                        </div>
                        <div style={{ textAlign: 'right', flexShrink: 0 }}>
                          <div style={{ fontSize: '0.8rem', color: 'var(--ink-soft)' }}>cost {formatMoney(b.unit_cost, businessCurrency)}</div>
                          <div style={{ fontSize: '0.8rem', color: 'var(--ink-soft)' }}>price {formatMoney(b.unit_price, businessCurrency)}</div>
                          {canEditBatchPrice && (
                            <button className="btn btn-outline" style={{ padding: '0.2em 0.5em', fontSize: '0.74rem', marginTop: '0.2rem' }} onClick={() => startEditBatch(b)}>Edit price</button>
                          )}
                        </div>
                      </div>
                    )}
                  </div>
                ))}
              </>
            )}
            <div style={styles.modalActions}>
              <button className="btn btn-outline" onClick={() => { setViewingBatchesId(null); setEditingBatchId(null); }}>Close</button>
            </div>
          </div>
        </div>
      )}

      {repackSourceId && (
        <div style={styles.overlay} onClick={() => setRepackSourceId(null)}>
          <div className="card" style={styles.modal} onClick={(e) => e.stopPropagation()}>
            <h3 style={{ marginTop: 0 }}>Repack / break bulk</h3>
            <p style={{ fontSize: '0.85rem', color: 'var(--ink-soft)' }}>
              Converting stock from "{String(records.find((r) => r.id === repackSourceId)?.name ?? 'this item')}".
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
              return (
                <p style={{ fontSize: '0.85rem', color: 'var(--ink-soft)' }}>
                  Settles "{String(r?.party_name ?? 'this record')}" for{' '}
                  {formatMoney(Number(r?.amount ?? 0), businessCurrency)}. Can't be undone.
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
            {moduleId === 'purchasing' && (
              <p style={{ fontSize: '0.85rem', color: 'var(--ink-soft)' }}>
                Received orders can't be corrected by reimporting — use Repack or a stock take instead.
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

// Combines customer/email/phone into one cell for the invoice table.
function renderInvoiceCustomerCell(r: Record_) {
  const name = r.customer;
  const email = typeof r.customer_email === 'string' ? r.customer_email : '';
  const phone = typeof r.customer_phone === 'string' ? r.customer_phone : '';
  const contact = [email, phone].filter(Boolean).join(' · ');
  return (
    <div>
      <div>{name === null || name === undefined || name === '' ? <span style={{ color: 'var(--ink-faint)' }}>—</span> : String(name)}</div>
      {contact && <div style={{ fontSize: '0.76rem', color: 'var(--ink-soft)' }}>{contact}</div>}
    </div>
  );
}

// Same fold as renderInvoiceCustomerCell above, for the Sales table:
// customer_phone has no separate column of its own, it sits under the
// customer's name instead.
function renderSalesCustomerCell(r: Record_) {
  const name = r.customer;
  const phone = typeof r.customer_phone === 'string' ? r.customer_phone : '';
  return (
    <div>
      <div>{name === null || name === undefined || name === '' ? <span style={{ color: 'var(--ink-faint)' }}>—</span> : String(name)}</div>
      {phone && <div style={{ fontSize: '0.76rem', color: 'var(--ink-soft)' }}>{phone}</div>}
    </div>
  );
}

// Combines unit_price + discount_amount into one "price (− discount)"
// cell — same idea as renderInvoiceTaxCell below, just for Sales.
function renderSalesPriceCell(r: Record_, currency: string) {
  const price = typeof r.unit_price === 'number' ? r.unit_price : null;
  const discount = typeof r.discount_amount === 'number' ? r.discount_amount : 0;
  if (price === null) return <span style={{ color: 'var(--ink-faint)' }}>—</span>;
  return (
    <span>
      {formatMoney(price, currency)}
      {discount > 0 && <span style={{ fontSize: '0.76rem', color: 'var(--ink-soft)' }}> (− {formatMoney(discount, currency)})</span>}
    </span>
  );
}

// Combines tax_rate + tax_amount into one "rate% · amount" cell.
function renderInvoiceTaxCell(r: Record_, currency: string) {
  const rate = typeof r.tax_rate === 'number' ? r.tax_rate : null;
  const amount = typeof r.tax_amount === 'number' ? r.tax_amount : null;
  if (rate === null && amount === null) return <span style={{ color: 'var(--ink-faint)' }}>—</span>;
  const rateText = rate !== null ? `${rate}%` : null;
  const amountText = amount !== null ? formatMoney(amount, currency ?? 'USD') : null;
  return <span>{[rateText, amountText].filter(Boolean).join(' · ')}</span>;
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

// unit_price_text is raw typed text, converted to integer cents at
// submit time — keeps the input from reformatting mid-keystroke.
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
