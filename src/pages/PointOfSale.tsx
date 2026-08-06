import { useEffect, useState } from 'react';
import { listRecords, checkout, ApiError } from '../api';
import ReceiptView from '../components/ReceiptView';
import type { Record_ } from '../types';

interface CartLine {
  inventory_record_id: string;
  name: string;
  sku: string;
  unit_price: number;
  available: number;
  quantity: number;
}

export default function PointOfSale() {
  const [products, setProducts] = useState<Record_[]>([]);
  const [search, setSearch] = useState('');
  const [cart, setCart] = useState<CartLine[]>([]);
  const [paymentMethod, setPaymentMethod] = useState('cash');
  const [customer, setCustomer] = useState('');
  const [onCredit, setOnCredit] = useState(false);
  const [dueDate, setDueDate] = useState('');
  const [allowOversell, setAllowOversell] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [receipt, setReceipt] = useState<{
    order_id: string; subtotal: number; customer?: string; payment_method?: string; on_credit?: boolean;
    items: { name: string; sku: string; quantity: number; unit_price: number; line_total: number; remaining_stock: number }[];
  } | null>(null);
  const [showReceipt, setShowReceipt] = useState(false);

  useEffect(() => {
    let cancelled = false;
    listRecords('inventory', search || undefined)
      .then((r) => { if (!cancelled) setProducts(r.records); })
      .catch(() => {});
    return () => { cancelled = true; };
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

  const subtotal = cart.reduce((sum, c) => sum + c.unit_price * c.quantity, 0);

  async function handleCheckout() {
    if (cart.length === 0) return;
    setError(null);
    setLoading(true);
    try {
      const result = await checkout({
        items: cart.map((c) => ({ inventory_record_id: c.inventory_record_id, quantity: c.quantity })),
        payment_method: onCredit ? undefined : paymentMethod,
        customer: customer || undefined,
        allow_oversell: allowOversell,
        on_credit: onCredit,
        due_date: onCredit ? (dueDate || undefined) : undefined,
      });
      setReceipt(result);
      setCart([]);
      setCustomer('');
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

  if (receipt) {
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
              <span>{item.line_total.toFixed(2)}</span>
            </div>
          ))}
          <div style={{ display: 'flex', justifyContent: 'space-between', fontWeight: 700, fontSize: '1.05rem', marginTop: '0.8rem', paddingTop: '0.6rem', borderTop: '2px solid var(--ink)' }}>
            <span>Total</span>
            <span>{receipt.subtotal.toFixed(2)}</span>
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
                    {outOfStock ? 'Out of stock' : `${qty} in stock`} · {Number(p.unit_price ?? 0).toFixed(2)}
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
                  <div style={{ fontSize: '0.75rem', color: 'var(--ink-soft)' }}>{c.unit_price.toFixed(2)} each</div>
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
            <span>{subtotal.toFixed(2)}</span>
          </div>

          <div style={{ marginTop: '1rem' }}>
            <label>Customer (optional)</label>
            <input value={customer} onChange={(e) => setCustomer(e.target.value)} style={{ width: '100%' }} />
          </div>

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
            {loading ? 'Processing…' : `Checkout — ${subtotal.toFixed(2)}`}
          </button>
        </div>
      </div>
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  productGrid: { display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(160px, 1fr))', gap: '0.7rem' },
  productTile: { textAlign: 'left', cursor: 'pointer' },
  cartPanel: { position: 'sticky', top: '1rem' },
  cartLine: { display: 'flex', alignItems: 'center', gap: '0.6rem', padding: '0.5rem 0', borderBottom: '1px solid var(--paper-line)' },
};
