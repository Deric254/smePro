import React, { useEffect, useState } from 'react';
import { getToken } from '../api';
import '../styles/receipt-print.css';

interface ReceiptLine {
  item_name: string;
  quantity: number;
  unit_price: number;
  line_total: number;
}

interface ReceiptData {
  order_id: string;
  business_name: string;
  business_slogan?: string;
  business_logo_path?: string;
  business_currency: string;
  customer?: string;
  date: string;
  items: ReceiptLine[];
  subtotal: number;
  tax_rate: number;
  tax_amount: number;
  total: number;
  payment_method?: string;
  cashier_name: string;
}

const API = 'http://127.0.0.1:8080';

export default function ReceiptView({ orderId, onClose }: { orderId: string; onClose: () => void }) {
  const [receipt, setReceipt] = useState<ReceiptData | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');

  useEffect(() => {
    fetch(`${API}/pos/receipt/${orderId}`, {
      headers: { Authorization: `Bearer ${getToken() || ''}` }
    })
      .then(r => r.json())
      .then(data => {
        if (data.error) throw new Error(data.error);
        setReceipt(data);
      })
      .catch(e => setError(e.message))
      .finally(() => setLoading(false));
  }, [orderId]);

  const handlePrint = () => window.print();

  function receiptText(): string {
    if (!receipt) return '';
    const lines = receipt.items.map(
      (i) => `${i.item_name} x${i.quantity} — ${receipt.business_currency} ${i.line_total.toFixed(2)}`
    );
    return [
      receipt.business_name,
      receipt.business_slogan || '',
      '',
      `Receipt #${receipt.order_id.slice(0, 8).toUpperCase()}`,
      new Date(receipt.date).toLocaleString(),
      '',
      ...lines,
      '',
      `Total: ${receipt.business_currency} ${receipt.total.toFixed(2)}`,
      receipt.payment_method ? `Paid via ${receipt.payment_method}` : '',
      '',
      'Thank you for your business!',
    ].filter(Boolean).join('\n');
  }

  function shareWhatsApp() {
    const text = encodeURIComponent(receiptText());
    // wa.me works identically whether WhatsApp is installed (opens the
    // app directly) or not (falls back to WhatsApp Web) — no phone
    // number needed here since the person sharing picks the recipient
    // themselves in WhatsApp's own share sheet, same as sharing any
    // link or text from a phone normally works.
    window.open(`https://wa.me/?text=${text}`, '_blank');
  }

  function shareEmail() {
    const subject = encodeURIComponent(`Receipt from ${receipt?.business_name || 'your purchase'}`);
    const body = encodeURIComponent(receiptText());
    window.location.href = `mailto:?subject=${subject}&body=${body}`;
  }

  if (loading) return (
    <div style={overlay}>
      <div style={modal}>Loading receipt…</div>
    </div>
  );
  if (error) return (
    <div style={overlay}>
      <div style={modal}>
        <p style={{color:'#c0392b'}}>Error: {error}</p>
        <button onClick={onClose} style={secondaryBtn}>Close</button>
      </div>
    </div>
  );
  if (!receipt) return null;

  const shortOrder = receipt.order_id.slice(0, 8).toUpperCase();

  return (
    <div style={overlay} onClick={onClose}>
      <div style={modal} onClick={e => e.stopPropagation()} className="receipt-print-area">
        <div style={header}>
          {receipt.business_logo_path && (
            <img
              src={`${API}/uploads/${receipt.business_logo_path.split('/').pop() || ''}`}
              alt=""
              style={logo}
              onError={(e) => { (e.target as HTMLImageElement).style.display = 'none'; }}
            />
          )}
          <h2 style={bizName}>{receipt.business_name}</h2>
          {receipt.business_slogan && <p style={slogan}>{receipt.business_slogan}</p>}
        </div>

        <div style={meta}>
          <div><strong>Receipt #:</strong> {shortOrder}</div>
          <div><strong>Date:</strong> {new Date(receipt.date).toLocaleString()}</div>
          {receipt.customer && <div><strong>Customer:</strong> {receipt.customer}</div>}
          <div><strong>Cashier:</strong> {receipt.cashier_name}</div>
        </div>

        <table style={table}>
          <thead>
            <tr style={thRow}>
              <th style={{...th, textAlign:'left'}}>Item</th>
              <th style={th}>Qty</th>
              <th style={th}>Unit Price</th>
              <th style={th}>Total</th>
            </tr>
          </thead>
          <tbody>
            {receipt.items.map((item, i) => (
              <tr key={i} style={tr}>
                <td style={{...td, textAlign:'left'}}>{item.item_name}</td>
                <td style={td}>{item.quantity}</td>
                <td style={td}>{receipt.business_currency} {item.unit_price.toFixed(2)}</td>
                <td style={td}>{receipt.business_currency} {item.line_total.toFixed(2)}</td>
              </tr>
            ))}
          </tbody>
        </table>

        <div style={totals}>
          <div style={row}><span>Subtotal:</span><span>{receipt.business_currency} {receipt.subtotal.toFixed(2)}</span></div>
          {receipt.tax_rate > 0 && (
            <div style={row}>
              <span>Tax ({receipt.tax_rate}%):</span>
              <span>{receipt.business_currency} {receipt.tax_amount.toFixed(2)}</span>
            </div>
          )}
          <div style={{...row, ...totalRow}}>
            <span>Total:</span>
            <span>{receipt.business_currency} {receipt.total.toFixed(2)}</span>
          </div>
          {receipt.payment_method && (
            <div style={row}><span>Paid via:</span><span>{receipt.payment_method}</span></div>
          )}
        </div>

        <div style={footer}>
          <p>Thank you for your business!</p>
        </div>

        <div style={actions} className="no-print">
          <button onClick={handlePrint} style={primaryBtn}>🖨️ Print</button>
          <button onClick={shareWhatsApp} style={secondaryBtn}>💬 WhatsApp</button>
          <button onClick={shareEmail} style={secondaryBtn}>✉️ Email</button>
          <button onClick={onClose} style={secondaryBtn}>Close</button>
        </div>
      </div>
    </div>
  );
}

// ── Styles ──
const overlay: React.CSSProperties = {
  position: 'fixed', inset: 0, background: 'rgba(0,0,0,0.45)',
  display: 'flex', alignItems: 'center', justifyContent: 'center', zIndex: 1000,
};
const modal: React.CSSProperties = {
  background: '#fff', borderRadius: 10, padding: 28, width: 420, maxWidth: '92vw',
  maxHeight: '90vh', overflowY: 'auto', boxShadow: '0 8px 32px rgba(0,0,0,0.18)',
  fontFamily: "'IBM Plex Sans', system-ui, sans-serif",
};
const header: React.CSSProperties = { textAlign: 'center', marginBottom: 16, borderBottom: '1px dashed #ccc', paddingBottom: 12 };
const logo: React.CSSProperties = { maxHeight: 56, marginBottom: 8, objectFit: 'contain' };
const bizName: React.CSSProperties = { margin: '4px 0', fontSize: 20, fontWeight: 700, fontFamily: "Newsreader, Georgia, serif" };
const slogan: React.CSSProperties = { margin: 0, fontSize: 12, color: '#666', fontStyle: 'italic' };
const meta: React.CSSProperties = { fontSize: 12, color: '#444', lineHeight: 1.6, marginBottom: 14 };
const table: React.CSSProperties = { width: '100%', borderCollapse: 'collapse', fontSize: 13, marginBottom: 14 };
const thRow: React.CSSProperties = { borderBottom: '2px solid #333' };
const th: React.CSSProperties = { padding: '6px 4px', fontWeight: 600, textAlign: 'right', fontSize: 12, textTransform: 'uppercase', letterSpacing: 0.5 };
const tr: React.CSSProperties = { borderBottom: '1px solid #eee' };
const td: React.CSSProperties = { padding: '6px 4px', textAlign: 'right' };
const totals: React.CSSProperties = { borderTop: '2px solid #333', paddingTop: 10, fontSize: 13 };
const row: React.CSSProperties = { display: 'flex', justifyContent: 'space-between', padding: '3px 0' };
const totalRow: React.CSSProperties = { fontWeight: 700, fontSize: 15, borderTop: '1px solid #ddd', marginTop: 6, paddingTop: 6 };
const footer: React.CSSProperties = { textAlign: 'center', marginTop: 18, fontSize: 12, color: '#666' };
const actions: React.CSSProperties = { display: 'flex', gap: 10, marginTop: 20, justifyContent: 'center', flexWrap: 'wrap' };
const primaryBtn: React.CSSProperties = {
  padding: '10px 18px', borderRadius: 8, border: 'none', background: '#1a1a1a', color: '#fff',
  fontSize: 13, fontWeight: 600, cursor: 'pointer',
};
const secondaryBtn: React.CSSProperties = {
  padding: '10px 18px', borderRadius: 8, border: '1px solid #ccc', background: '#fff', color: '#333',
  fontSize: 13, cursor: 'pointer',
};
