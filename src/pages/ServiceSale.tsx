import { useState, useEffect } from 'react';
import { createServiceSale, getBusinessInfo, ApiError } from '../api';
import ReceiptView from '../components/ReceiptView';
import CustomerPicker from '../components/CustomerPicker';
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
 * service business has none. This goes through `pos::create_service_sale`
 * instead of pos.rs's goods checkout — same atomicity and customer/
 * lifetime-value tracking guarantees, minus the inventory dependency —
 * so a service business's repeat customers show up in the Customers
 * list exactly like a goods business's do, and a receipt works the
 * same way (receipt.rs just queries sales by order_id, doesn't care
 * how those rows were created).
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
    try {
      // One atomic call — every line commits together or none do, and
      // a customer phone here now actually creates/updates a real
      // customer record with correct lifetime value, exactly like the
      // goods-based checkout already did. order_id comes back from the
      // server now (see pos::create_service_sale) instead of being
      // generated client-side and hoped to match.
      const result = await createServiceSale({
        lines: parsed.map((item) => ({ description: item.description, unit_price: item.cents, quantity: item.quantity })),
        payment_method: paymentMethod,
        customer: customer || undefined,
        customer_phone: customerPhone || undefined,
      });
      setCompletedOrderId(result.order_id);
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

      <div style={{ display: 'flex', gap: '0.6rem', marginBottom: '0.8rem', alignItems: 'flex-end' }}>
        <div style={{ flex: 2 }}>
          <CustomerPicker
            name={customer}
            phone={customerPhone}
            onChangeName={setCustomer}
            onChangePhone={setCustomerPhone}
          />
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
      {(customer.trim() || customerPhone.trim()) && (
        <div style={{ fontSize: '0.75rem', color: 'var(--ink-soft)', marginBottom: '1rem' }}>
          Saved to your customer list — see their full purchase history under Admin → Customers.
          {!customerPhone.trim() && ' (Matched by name only, since no phone was given — less reliable than phone if another customer shares this name.)'}
        </div>
      )}

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
