import { useState, useEffect } from 'react';
import { createRecord, getBusinessInfo, ApiError } from '../api';
import ReceiptView from '../components/ReceiptView';
import { formatMoney, parseMoneyInput, sumMoney } from '../lib/money';

// Integer cents throughout — see src/lib/money.ts.
interface ServiceLine {
  description: string;
  priceText: string;
  quantity: number;
}

/**
 * "Log a sale" for businesses that don't carry stock — services,
 * consulting, anything without an Inventory module enabled. The main
 * Point of Sale screen fundamentally can't work here: its checkout
 * requires every line to reference a real inventory record, and a
 * service business has none. This writes directly to the same
 * "sales" module every checkout writes to (via the plain, already
 * tested generic create endpoint — not pos.rs's checkout logic at
 * all, so this can never affect or risk the goods-based checkout
 * flow), sharing one order_id across the lines so the existing,
 * unmodified receipt system (receipt.rs just queries sales by
 * order_id — it doesn't care how those rows were created) works
 * exactly the same as it does for a goods sale.
 */
export default function ServiceSale() {
  const [lines, setLines] = useState<ServiceLine[]>([{ description: '', priceText: '0.00', quantity: 1 }]);
  const [customer, setCustomer] = useState('');
  const [customerPhone, setCustomerPhone] = useState('');
  const [paymentMethod, setPaymentMethod] = useState('cash');
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [completedOrderId, setCompletedOrderId] = useState<string | null>(null);
  const [currency, setCurrency] = useState('USD');

  useEffect(() => {
    getBusinessInfo().then((b: any) => { if (b?.currency) setCurrency(b.currency); }).catch(() => {});
  }, []);

  function updateLine(i: number, patch: Partial<ServiceLine>) {
    setLines((prev) => prev.map((l, idx) => (idx === i ? { ...l, ...patch } : l)));
  }
  function addLine() {
    setLines((prev) => [...prev, { description: '', priceText: '0.00', quantity: 1 }]);
  }
  function removeLine(i: number) {
    setLines((prev) => prev.filter((_, idx) => idx !== i));
  }

  const total = sumMoney(
    lines.map((l) => (parseMoneyInput(l.priceText, currency) ?? 0) * (l.quantity || 0))
  );

  async function handleSubmit() {
    setError(null);
    const parsed: { description: string; cents: number; quantity: number }[] = [];
    for (const l of lines) {
      if (!l.description.trim()) continue;
      const cents = parseMoneyInput(l.priceText, currency);
      if (cents === null || cents < 0 || !(l.quantity > 0)) {
        setError(`"${l.description || 'A line'}" has an invalid price or quantity.`);
        return;
      }
      parsed.push({ description: l.description.trim(), cents, quantity: l.quantity });
    }
    if (parsed.length === 0) {
      setError('Add at least one line with a description.');
      return;
    }

    setSubmitting(true);
    // A real order_id, generated client-side, shared by every line in
    // this sale — the same grouping key checkout() would generate
    // server-side, just produced here instead since this path
    // doesn't go through checkout() at all.
    const orderId = crypto.randomUUID();
    try {
      for (const item of parsed) {
        await createRecord('sales', {
          item_name: item.description,
          quantity: item.quantity,
          revenue: item.cents * item.quantity,
          unit_price: item.cents,
          customer: customer || undefined,
          customer_phone: customerPhone || undefined,
          payment_method: paymentMethod,
          order_id: orderId,
        });
      }
      setCompletedOrderId(orderId);
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not record this sale');
    } finally {
      setSubmitting(false);
    }
  }

  function startNewSale() {
    setCompletedOrderId(null);
    setLines([{ description: '', priceText: '0.00', quantity: 1 }]);
    setCustomer('');
    setCustomerPhone('');
    setPaymentMethod('cash');
  }

  if (completedOrderId) {
    return <ReceiptView orderId={completedOrderId} onClose={startNewSale} />;
  }

  return (
    <div style={{ maxWidth: 640 }}>
      <h2 style={{ marginTop: 0 }}>Log a sale</h2>
      <p style={{ fontSize: '0.85rem', color: 'var(--ink-soft)' }}>
        For services and anything else that isn't tracked as stock — describe what was sold,
        the price, and how many. No inventory item needed.
      </p>

      {lines.map((l, i) => (
        <div key={i} style={{ display: 'flex', gap: '0.5rem', marginBottom: '0.6rem', alignItems: 'flex-end' }}>
          <div style={{ flex: 1 }}>
            <label>Description</label>
            <input
              value={l.description}
              onChange={(e) => updateLine(i, { description: e.target.value })}
              placeholder="e.g. Haircut, Consulting session"
              style={{ width: '100%' }}
            />
          </div>
          <div style={{ width: 110 }}>
            <label>Price</label>
            <input
              type="text"
              inputMode="decimal"
              value={l.priceText}
              onChange={(e) => updateLine(i, { priceText: e.target.value })}
              onBlur={() => {
                const parsed = parseMoneyInput(l.priceText, currency);
                if (parsed !== null) updateLine(i, { priceText: formatMoney(parsed, currency) });
              }}
              style={{ width: '100%' }}
            />
          </div>
          <div style={{ width: 70 }}>
            <label>Qty</label>
            <input
              type="number"
              min={1}
              value={l.quantity}
              onChange={(e) => updateLine(i, { quantity: parseInt(e.target.value, 10) || 1 })}
              style={{ width: '100%' }}
            />
          </div>
          {lines.length > 1 && (
            <button className="btn btn-outline" type="button" onClick={() => removeLine(i)} style={{ padding: '0.5em 0.7em' }}>
              ✕
            </button>
          )}
        </div>
      ))}

      <button className="btn btn-outline" type="button" onClick={addLine} style={{ marginBottom: '1rem' }}>
        + Add line
      </button>

      <div style={{ display: 'flex', gap: '0.6rem', marginBottom: '0.8rem' }}>
        <div style={{ flex: 1 }}>
          <label>Customer (optional)</label>
          <input value={customer} onChange={(e) => setCustomer(e.target.value)} style={{ width: '100%' }} />
        </div>
        <div style={{ flex: 1 }}>
          <label>Phone (optional)</label>
          <input value={customerPhone} onChange={(e) => setCustomerPhone(e.target.value)} style={{ width: '100%' }} />
        </div>
        <div style={{ width: 140 }}>
          <label>Payment</label>
          <select value={paymentMethod} onChange={(e) => setPaymentMethod(e.target.value)} style={{ width: '100%' }}>
            <option value="cash">Cash</option>
            <option value="mpesa">M-Pesa</option>
            <option value="card">Card</option>
            <option value="bank">Bank transfer</option>
          </select>
        </div>
      </div>

      <div style={{ display: 'flex', justifyContent: 'space-between', fontSize: '1.1rem', fontWeight: 600, marginBottom: '1rem' }}>
        <span>Total</span>
        <span className="mono">{formatMoney(total, currency)}</span>
      </div>

      {error && <div style={{ color: 'var(--stamp)', fontSize: '0.85rem', marginBottom: '0.8rem' }}>{error}</div>}

      <button className="btn btn-stamp" onClick={handleSubmit} disabled={submitting}>
        {submitting ? 'Recording…' : 'Complete sale'}
      </button>
    </div>
  );
}
