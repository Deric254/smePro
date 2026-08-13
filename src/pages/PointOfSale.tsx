import { useEffect, useState } from 'react';
import { listRecords, checkout, getOrder, processRefund, getBusinessInfo, ApiError } from '../api';
import ReceiptView from '../components/ReceiptView';
import CustomerPicker from '../components/CustomerPicker';
import type { Record_ } from '../types';
import { formatMoney, parseMoneyInput, sumMoney } from '../lib/money';

// unit_price, revenue, line_total, subtotal below are all integer
// minor units (cents) — see src/lib/money.ts. Never do float math on
// them directly; go through formatMoney/parseMoneyInput/sumMoney.
interface CartLine {
  inventory_record_id: string;
  name: string;
  sku: string;
  unit_price: number;
  available: number;
  quantity: number;
}

interface OrderLookupItem {
  sale_id: string;
  item_name: string;
  quantity: number;
  revenue: number;
  unit_price?: number;
  customer?: string;
  payment_method?: string;
  created_at: string;
}

export default function PointOfSale() {
  const [mode, setMode] = useState<'sell' | 'refund'>('sell');
  const [products, setProducts] = useState<Record_[]>([]);
  const [search, setSearch] = useState('');
  const [cart, setCart] = useState<CartLine[]>([]);
  const [paymentMethod, setPaymentMethod] = useState('cash');
  const [customer, setCustomer] = useState('');
  const [customerPhone, setCustomerPhone] = useState('');
  const [onCredit, setOnCredit] = useState(false);
  const [dueDate, setDueDate] = useState('');
  const [allowOversell, setAllowOversell] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Integer cents everywhere below — see src/lib/money.ts. Fetched
  // once so every formatMoney/parseMoneyInput call in this screen
  // uses the business's actual currency (decimal places, not just the
  // symbol) instead of assuming USD's 2dp.
  const [currency, setCurrency] = useState('USD');
  const [receipt, setReceipt] = useState<{
    order_id: string; subtotal: number; customer?: string; payment_method?: string; on_credit?: boolean;
    items: { name: string; sku: string; quantity: number; unit_price: number; line_total: number; remaining_stock: number }[];
  } | null>(null);
  const [showReceipt, setShowReceipt] = useState(false);

  // ---- Refund flow state ----
  const [orderIdInput, setOrderIdInput] = useState('');
  const [orderLookup, setOrderLookup] = useState<{ order_id: string; subtotal: number; items: OrderLookupItem[] } | null>(null);
  const [lookupError, setLookupError] = useState<string | null>(null);
  const [lookupLoading, setLookupLoading] = useState(false);
  const [refundingSaleId, setRefundingSaleId] = useState<string | null>(null);
  const [refundQty, setRefundQty] = useState(1);
  const [refundAmountText, setRefundAmountText] = useState('0.00');
  const [refundReason, setRefundReason] = useState('');
  const [refundRestock, setRefundRestock] = useState(true);
  const [refundError, setRefundError] = useState<string | null>(null);
  const [refundSubmitting, setRefundSubmitting] = useState(false);
  const [refundSuccess, setRefundSuccess] = useState<string | null>(null);

  async function lookupOrder() {
    if (!orderIdInput.trim()) return;
    setLookupLoading(true);
    setLookupError(null);
    setOrderLookup(null);
    setRefundSuccess(null);
    try {
      const result = await getOrder(orderIdInput.trim());
      setOrderLookup(result as { order_id: string; subtotal: number; items: OrderLookupItem[] });
    } catch (err) {
      setLookupError(err instanceof ApiError ? err.message : 'Could not find that order.');
    } finally {
      setLookupLoading(false);
    }
  }

  function startRefund(item: OrderLookupItem) {
    setRefundingSaleId(item.sale_id);
    setRefundQty(item.quantity);
    // A sensible default -- the full line's original value -- but
    // always editable, since a real refund isn't always full price
    // back (a restocking fee, a partial goodwill adjustment).
    setRefundAmountText(formatMoney(item.revenue, currency));
    setRefundReason('');
    setRefundRestock(true);
    setRefundError(null);
  }

  async function submitRefund() {
    if (!refundingSaleId) return;
    const refundAmountCents = parseMoneyInput(refundAmountText, currency);
    if (refundAmountCents === null || refundAmountCents < 0) {
      setRefundError('Enter a valid refund amount.');
      return;
    }
    setRefundSubmitting(true);
    setRefundError(null);
    try {
      await processRefund({
        sale_id: refundingSaleId,
        quantity: refundQty,
        refund_amount: refundAmountCents,
        reason: refundReason || undefined,
        restock: refundRestock,
      });
      setRefundSuccess(`Refunded ${refundQty} unit(s), ${formatMoney(refundAmountCents, currency)} returned.`);
      setRefundingSaleId(null);
      // Re-look-up the order so the screen reflects what's now
      // actually left refundable, rather than showing stale numbers.
      const refreshed = await getOrder(orderIdInput.trim());
      setOrderLookup(refreshed as { order_id: string; subtotal: number; items: OrderLookupItem[] });
    } catch (err) {
      setRefundError(err instanceof ApiError ? err.message : 'Refund failed.');
    } finally {
      setRefundSubmitting(false);
    }
  }

  useEffect(() => {
    getBusinessInfo()
      .then((b: any) => { if (b?.currency) setCurrency(b.currency); })
      .catch(() => {}); // default 'USD' stands if this fails — never blocks the POS screen
  }, []);

  useEffect(() => {
    let cancelled = false;
    const timer = setTimeout(() => {
      listRecords('inventory', search || undefined)
        .then((r) => {
          if (cancelled) return;
          // Highest stock first by default — the products a cashier is
          // most likely to be selling right now, front and center,
          // without having to search for them. A search term still
          // takes over the ordering the backend itself returns for
          // that search, this sort only applies to the "browse
          // everything" no-search-term view.
          const sorted = search
            ? r.records
            : [...r.records].sort((a, b) => Number(b.quantity ?? 0) - Number(a.quantity ?? 0));
          setProducts(sorted);
        })
        .catch(() => {});
    }, search ? 250 : 0); // instant on initial load / cleared search, debounced while typing
    return () => { cancelled = true; clearTimeout(timer); };
  }, [search]);

  function addToCart(p: Record_) {
    const existing = cart.find((c) => c.inventory_record_id === p.id);
    const available = Number(p.quantity ?? 0);
    if (existing) {
      setCart(cart.map((c) => c.inventory_record_id === p.id ? { ...c, quantity: c.quantity + 1 } : c));
    } else {
      setCart([...cart, {
        inventory_record_id: p.id,
        name: String(p.name ?? ''),
        sku: String(p.sku ?? ''),
        unit_price: Number(p.unit_price ?? 0),
        available,
        quantity: 1,
      }]);
    }
  }

  function updateQuantity(id: string, quantity: number) {
    if (quantity <= 0) {
      setCart(cart.filter((c) => c.inventory_record_id !== id));
    } else {
      setCart(cart.map((c) => c.inventory_record_id === id ? { ...c, quantity } : c));
    }
  }

  const subtotal = sumMoney(cart.map((c) => c.unit_price * c.quantity));

  // Enter finalizes the sale; Enter again (once the receipt is
  // showing) starts the next one — the actual, honest version of "one
  // key does the next thing," without pretending this can also fire a
  // printer silently with no dialog, which isn't something a webview
  // can do on any platform.
  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      if (e.key !== 'Enter') return;
      if (mode !== 'sell') return;
      const target = e.target as HTMLElement;
      // Typing in the product search box: Enter shouldn't hijack that
      // into finalizing a sale mid-search.
      if (target?.tagName === 'INPUT' && target.getAttribute('placeholder') === 'Search products…') return;

      if (receipt) {
        e.preventDefault();
        newSale();
      } else if (cart.length > 0 && !loading) {
        e.preventDefault();
        handleCheckout();
      }
    }
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode, cart, receipt, loading]);

  async function handleCheckout() {
    if (cart.length === 0) return;
    setError(null);
    setLoading(true);
    try {
      const result = await checkout({
        items: cart.map((c) => ({ inventory_record_id: c.inventory_record_id, quantity: c.quantity })),
        payment_method: onCredit ? undefined : paymentMethod,
        customer: customer || undefined,
        customer_phone: customerPhone || undefined,
        allow_oversell: allowOversell,
        on_credit: onCredit,
        due_date: onCredit ? (dueDate || undefined) : undefined,
      });
      setReceipt(result);
      setCart([]);
      setCustomer('');
      setCustomerPhone('');
      setDueDate('');
      setOnCredit(false);
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Checkout failed');
    } finally {
      setLoading(false);
    }
  }

  function newSale() {
    setReceipt(null);
    setShowReceipt(false);
    setError(null);
  }

  if (mode === 'sell' && receipt) {
    return (
      <div>
        <h1>Sale complete</h1>
        <div className="card mono" style={{ maxWidth: 420 }}>
          <div style={{ fontSize: '0.75rem', color: 'var(--ink-soft)', marginBottom: '0.8rem' }}>
            Order {receipt.order_id.slice(0, 8)}
          </div>
          {receipt.items.map((item, i) => (
            <div key={i} style={{ display: 'flex', justifyContent: 'space-between', fontSize: '0.88rem', padding: '0.3rem 0', borderBottom: '1px solid var(--paper-line)' }}>
              <span>{item.name} × {item.quantity}</span>
              <span>{formatMoney(item.line_total, currency)}</span>
            </div>
          ))}
          <div style={{ display: 'flex', justifyContent: 'space-between', fontWeight: 700, fontSize: '1.05rem', marginTop: '0.8rem', paddingTop: '0.6rem', borderTop: '2px solid var(--ink)' }}>
            <span>Total</span>
            <span>{formatMoney(receipt.subtotal, currency)}</span>
          </div>
          {receipt.customer && <div style={{ fontSize: '0.8rem', color: 'var(--ink-soft)', marginTop: '0.6rem' }}>Customer: {receipt.customer}</div>}
          {receipt.on_credit && <div style={{ fontSize: '0.8rem', color: 'var(--stamp)', marginTop: '0.2rem' }}>Sold on credit — added to Debt &amp; Credit</div>}
          {!receipt.on_credit && receipt.payment_method && <div style={{ fontSize: '0.8rem', color: 'var(--ink-soft)', marginTop: '0.2rem' }}>Paid via {receipt.payment_method}</div>}
        </div>
        <div style={{ display: 'flex', gap: '0.6rem', marginTop: '1.2rem' }}>
          <button className="btn btn-stamp" onClick={newSale}>New sale</button>
          <button className="btn btn-outline" onClick={() => setShowReceipt(true)}>Print receipt</button>
        </div>
        {showReceipt && <ReceiptView orderId={receipt.order_id} onClose={() => setShowReceipt(false)} />}
      </div>
    );
  }

  return (
    <div>
      <h1>Point of Sale</h1>
      <div style={{ display: 'flex', gap: '0.5rem', marginBottom: '1rem' }}>
        <button className={mode === 'sell' ? 'btn' : 'btn btn-outline'} onClick={() => setMode('sell')}>Sell</button>
        <button className={mode === 'refund' ? 'btn' : 'btn btn-outline'} onClick={() => setMode('refund')}>Refund</button>
      </div>

      {mode === 'refund' && (
        <div style={{ maxWidth: 560 }}>
          <div className="card">
            <label>Order ID</label>
            <div style={{ display: 'flex', gap: '0.5rem' }}>
              <input
                value={orderIdInput}
                onChange={(e) => setOrderIdInput(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && lookupOrder()}
                placeholder="Paste or type the order ID from the receipt…"
                style={{ flex: 1 }}
              />
              <button className="btn btn-outline" onClick={lookupOrder} disabled={lookupLoading || !orderIdInput.trim()}>
                {lookupLoading ? 'Looking up…' : 'Find order'}
              </button>
            </div>
            {lookupError && (
              <div style={{ background: 'var(--stamp-wash)', color: 'var(--stamp)', padding: '0.5em 0.7em', borderRadius: 3, fontSize: '0.85rem', marginTop: '0.7rem' }}>
                {lookupError}
              </div>
            )}
          </div>

          {refundSuccess && (
            <div className="card" style={{ marginTop: '0.8rem', color: 'var(--stamp)', fontWeight: 600 }}>
              {refundSuccess}
            </div>
          )}

          {orderLookup && (
            <div className="card" style={{ marginTop: '0.8rem' }}>
              <div style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', marginBottom: '0.6rem' }}>
                Order {orderLookup.order_id.slice(0, 8)} · subtotal {formatMoney(orderLookup.subtotal, currency)}
              </div>
              {orderLookup.items.map((item) => (
                <div key={item.sale_id} style={{ borderBottom: '1px solid var(--paper-line)', padding: '0.6rem 0' }}>
                  <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                    <div>
                      <div style={{ fontWeight: 600, fontSize: '0.9rem' }}>{item.item_name}</div>
                      <div style={{ fontSize: '0.76rem', color: 'var(--ink-soft)' }}>
                        {item.quantity} sold · {formatMoney(item.revenue, currency)} total
                      </div>
                    </div>
                    {refundingSaleId !== item.sale_id && (
                      <button className="btn btn-outline" style={{ padding: '0.3em 0.7em', fontSize: '0.78rem' }} onClick={() => startRefund(item)}>
                        Refund
                      </button>
                    )}
                  </div>

                  {refundingSaleId === item.sale_id && (
                    <div style={{ marginTop: '0.7rem', paddingTop: '0.7rem', borderTop: '1px dashed var(--paper-line)' }}>
                      <div style={{ display: 'flex', gap: '0.6rem' }}>
                        <div style={{ flex: 1 }}>
                          <label>Quantity to refund</label>
                          <input
                            type="number"
                            min={1}
                            max={item.quantity}
                            value={refundQty}
                            onChange={(e) => setRefundQty(parseInt(e.target.value, 10) || 0)}
                          />
                        </div>
                        <div style={{ flex: 1 }}>
                          <label>Amount to return</label>
                          <input
                            type="text"
                            inputMode="decimal"
                            value={refundAmountText}
                            onChange={(e) => setRefundAmountText(e.target.value)}
                            onBlur={() => {
                              const parsed = parseMoneyInput(refundAmountText, currency);
                              if (parsed !== null) setRefundAmountText(formatMoney(parsed, currency));
                            }}
                          />
                        </div>
                      </div>
                      <label style={{ marginTop: '0.5rem', display: 'block' }}>Reason (optional)</label>
                      <input value={refundReason} onChange={(e) => setRefundReason(e.target.value)} style={{ width: '100%' }} />
                      <label style={{ display: 'flex', alignItems: 'center', gap: '0.5rem', textTransform: 'none', fontSize: '0.82rem', marginTop: '0.6rem', cursor: 'pointer' }}>
                        <input type="checkbox" checked={refundRestock} onChange={(e) => setRefundRestock(e.target.checked)} />
                        Put this quantity back into sellable stock
                      </label>
                      {!refundRestock && (
                        <div style={{ fontSize: '0.76rem', color: 'var(--ink-soft)', marginTop: '0.2rem' }}>
                          Leave unchecked for damaged, expired, or otherwise unsellable returns.
                        </div>
                      )}
                      {refundError && (
                        <div style={{ background: 'var(--stamp-wash)', color: 'var(--stamp)', padding: '0.5em 0.7em', borderRadius: 3, fontSize: '0.85rem', marginTop: '0.6rem' }}>
                          {refundError}
                        </div>
                      )}
                      <div style={{ display: 'flex', gap: '0.5rem', marginTop: '0.7rem' }}>
                        <button className="btn btn-stamp" onClick={submitRefund} disabled={refundSubmitting || refundQty <= 0}>
                          {refundSubmitting ? 'Processing…' : 'Confirm refund'}
                        </button>
                        <button className="btn btn-outline" onClick={() => setRefundingSaleId(null)}>Cancel</button>
                      </div>
                    </div>
                  )}
                </div>
              ))}
            </div>
          )}
        </div>
      )}

      {mode === 'sell' && (
      <div className="pos-layout">
        <div>
          <input
            placeholder="Search products…"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            style={{ width: '100%', marginBottom: '0.8rem' }}
          />
          <div style={styles.productGrid}>
            {products.map((p) => {
              const qty = Number(p.quantity ?? 0);
              const outOfStock = qty <= 0;
              return (
                <button
                  key={p.id}
                  className="card"
                  style={{ ...styles.productTile, opacity: outOfStock ? 0.5 : 1 }}
                  onClick={() => !outOfStock && addToCart(p)}
                  disabled={outOfStock}
                >
                  <div style={{ fontWeight: 600, fontSize: '0.9rem' }}>{String(p.name ?? '')}</div>
                  <div style={{ fontSize: '0.76rem', color: 'var(--ink-soft)', marginTop: '0.2rem' }}>
                    {outOfStock ? 'Out of stock' : `${qty} in stock`} · {formatMoney(Number(p.unit_price ?? 0), currency)}
                  </div>
                </button>
              );
            })}
            {products.length === 0 && <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>No products found.</div>}
          </div>
        </div>

        <div className="card" style={styles.cartPanel}>
          <h3 style={{ marginTop: 0 }}>Cart</h3>
          {cart.length === 0 ? (
            <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem' }}>Tap a product to add it.</div>
          ) : (
            cart.map((c) => (
              <div key={c.inventory_record_id} style={styles.cartLine}>
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div style={{ fontSize: '0.85rem', fontWeight: 600 }}>{c.name}</div>
                  <div style={{ fontSize: '0.75rem', color: 'var(--ink-soft)' }}>{formatMoney(c.unit_price, currency)} each</div>
                </div>
                <input
                  type="number"
                  min={0}
                  value={c.quantity}
                  onChange={(e) => updateQuantity(c.inventory_record_id, parseInt(e.target.value, 10) || 0)}
                  style={{ width: '4rem' }}
                />
              </div>
            ))
          )}

          <div style={{ display: 'flex', justifyContent: 'space-between', fontWeight: 700, marginTop: '0.9rem', paddingTop: '0.7rem', borderTop: '1px solid var(--paper-line)' }}>
            <span>Subtotal</span>
            <span>{formatMoney(subtotal, currency)}</span>
          </div>

          <div style={{ marginTop: '1rem' }}>
            <CustomerPicker
              name={customer}
              phone={customerPhone}
              onChangeName={setCustomer}
              onChangePhone={setCustomerPhone}
            />
          </div>
          {(customer.trim() || customerPhone.trim()) && (
            <div style={{ fontSize: '0.75rem', color: 'var(--ink-soft)', marginTop: '0.3rem' }}>
              Saved to your customer list — see their full purchase history under Admin → Customers.
              {!customerPhone.trim() && ' (Matched by name only, since no phone was given — less reliable than phone if another customer shares this name.)'}
            </div>
          )}

          <label style={{ display: 'flex', alignItems: 'center', gap: '0.5rem', textTransform: 'none', fontSize: '0.85rem', marginTop: '0.8rem', cursor: 'pointer' }}>
            <input type="checkbox" checked={onCredit} onChange={(e) => setOnCredit(e.target.checked)} />
            Sell on credit (adds to Debt &amp; Credit)
          </label>

          {onCredit ? (
            <div style={{ marginTop: '0.6rem' }}>
              <label>Due date</label>
              <input type="date" value={dueDate} onChange={(e) => setDueDate(e.target.value)} style={{ width: '100%' }} />
              {!customer && <div style={{ fontSize: '0.76rem', color: 'var(--stamp)', marginTop: '0.3rem' }}>A customer name is required for a credit sale.</div>}
            </div>
          ) : (
            <div style={{ marginTop: '0.6rem' }}>
              <label>Payment method</label>
              <select value={paymentMethod} onChange={(e) => setPaymentMethod(e.target.value)} style={{ width: '100%' }}>
                <option value="cash">Cash</option>
                <option value="mobile_money">Mobile money</option>
                <option value="card">Card</option>
              </select>
            </div>
          )}

          <label style={{ display: 'flex', alignItems: 'center', gap: '0.5rem', textTransform: 'none', fontSize: '0.8rem', marginTop: '0.6rem', cursor: 'pointer', color: 'var(--ink-soft)' }}>
            <input type="checkbox" checked={allowOversell} onChange={(e) => setAllowOversell(e.target.checked)} />
            Allow selling more than what's in stock
          </label>

          {error && <div style={{ background: 'var(--stamp-wash)', color: 'var(--stamp)', padding: '0.5em 0.7em', borderRadius: 3, fontSize: '0.85rem', marginTop: '0.8rem' }}>{error}</div>}

          <button
            className="btn btn-stamp"
            style={{ width: '100%', justifyContent: 'center', marginTop: '1rem' }}
            disabled={cart.length === 0 || loading || (onCredit && !customer)}
            onClick={handleCheckout}
          >
            {loading ? 'Processing…' : `Checkout — ${formatMoney(subtotal, currency)}`}
          </button>
        </div>
      </div>
      )}
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  productGrid: { display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(160px, 1fr))', gap: '0.7rem' },
  productTile: { textAlign: 'left', cursor: 'pointer' },
  cartPanel: { position: 'sticky', top: '1rem' },
  cartLine: { display: 'flex', alignItems: 'center', gap: '0.6rem', padding: '0.5rem 0', borderBottom: '1px solid var(--paper-line)' },
};
