import React, { useEffect, useState } from 'react';
import { getToken, getLogoUrl, markInvoiceSent, markInvoicePaid, cancelInvoice, ApiError, API_BASE as API } from '../api';
import { formatMoney } from '../lib/money';
import '../styles/invoice-print.css';

// unit_price, subtotal, tax_amount, total below are all integer minor
// units (cents) — see src/lib/money.ts.
interface InvoiceItem {
  description: string;
  quantity: number;
  unit_price: number;
}

interface InvoiceRecord {
  id: string;
  invoice_number: string;
  customer: string;
  customer_email?: string;
  customer_phone?: string;
  issue_date: string;
  due_date: string;
  status: 'draft' | 'sent' | 'paid' | 'overdue' | 'cancelled';
  items_json: string;
  subtotal: number;
  tax_rate: number;
  tax_amount: number;
  total: number;
  notes?: string;
}



export default function InvoiceView({ invoiceId, onClose, onStatusChanged }: { invoiceId: string; onClose: () => void; onStatusChanged?: () => void }) {
  const [invoice, setInvoice] = useState<InvoiceRecord | null>(null);
  const [business, setBusiness] = useState<{name:string; slogan?:string; logo_path?:string; currency:string} | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [actionLoading, setActionLoading] = useState(false);
  const [actionError, setActionError] = useState('');

  function reload() {
    return fetch(`${API}/modules/invoice/records`, {
      cache: 'no-store', headers: { Authorization: `Bearer ${getToken()}` } })
      .then(r => r.json())
      .then((recordsData) => {
        const rec = recordsData.records?.find((r: any) => r.id === invoiceId);
        if (rec) setInvoice(rec);
      });
  }

  async function handleAction(action: () => Promise<unknown>) {
    setActionError('');
    setActionLoading(true);
    try {
      await action();
      await reload();
      onStatusChanged?.();
    } catch (err) {
      setActionError(err instanceof ApiError ? err.message : 'Could not update this invoice');
    } finally {
      setActionLoading(false);
    }
  }

  useEffect(() => {
    Promise.all([
      fetch(`${API}/modules/invoice/records`, {
      cache: 'no-store', headers: { Authorization: `Bearer ${getToken()}` } })
        .then(r => r.json()),
      fetch(`${API}/business`, {
      cache: 'no-store', headers: { Authorization: `Bearer ${getToken()}` } })
        .then(r => r.json())
    ]).then(([recordsData, bizData]) => {
      const rec = recordsData.records?.find((r: any) => r.id === invoiceId);
      if (!rec) throw new Error('Invoice not found');
      setInvoice(rec);
      setBusiness(bizData);
    }).catch(e => setError(e.message)).finally(() => setLoading(false));
  }, [invoiceId]);

  const items: InvoiceItem[] = invoice ? JSON.parse(invoice.items_json || '[]') : [];

  const handlePrint = () => window.print();

  const statusColor = (s: string) => {
    switch(s) {
      case 'paid': return '#27ae60';
      case 'sent': return '#2980b9';
      case 'overdue': return '#c0392b';
      case 'cancelled': return '#7f8c8d';
      default: return '#f39c12';
    }
  };

  if (loading) return <div style={overlay}><div style={modal}>Loading…</div></div>;
  if (error) return <div style={overlay}><div style={modal}><p style={{color:'#c0392b'}}>{error}</p><button onClick={onClose} style={secondaryBtn}>Close</button></div></div>;
  if (!invoice || !business) return null;

  return (
    <div style={overlay} onClick={onClose}>
      <div style={modal} onClick={e => e.stopPropagation()} className="invoice-print-area">
        <div style={header}>
          {getLogoUrl(business.logo_path) && (
            <img src={getLogoUrl(business.logo_path) || ''} alt="" style={logo} />
          )}
          <h2 style={bizName}>{business.name}</h2>
          {business.slogan && <p style={slogan}>{business.slogan}</p>}
        </div>

        <div style={{display:'flex', justifyContent:'space-between', marginBottom:16, fontSize:13}}>
          <div>
            <div><strong>Invoice #:</strong> {invoice.invoice_number}</div>
            <div><strong>Status:</strong> <span style={{color:statusColor(invoice.status), fontWeight:600, textTransform:'uppercase'}}>{invoice.status}</span></div>
          </div>
          <div style={{textAlign:'right'}}>
            <div><strong>Issue Date:</strong> {new Date(invoice.issue_date).toLocaleDateString()}</div>
            <div><strong>Due Date:</strong> {new Date(invoice.due_date).toLocaleDateString()}</div>
          </div>
        </div>

        <div style={{marginBottom:16, padding:12, background:'#f8f8f8', borderRadius:6, fontSize:13}}>
          <strong>Bill To:</strong><br/>
          {invoice.customer}<br/>
          {invoice.customer_email && <>{invoice.customer_email}<br/></>}
          {invoice.customer_phone && <>{invoice.customer_phone}</>}
        </div>

        <table style={table}>
          <thead>
            <tr style={thRow}>
              <th style={{...th, textAlign:'left'}}>Description</th>
              <th style={th}>Qty</th>
              <th style={th}>Unit Price</th>
              <th style={th}>Amount</th>
            </tr>
          </thead>
          <tbody>
            {items.map((item, i) => (
              <tr key={i} style={tr}>
                <td style={{...td, textAlign:'left'}}>{item.description}</td>
                <td style={td}>{item.quantity}</td>
                <td style={td}>{business.currency} {formatMoney(item.unit_price, business.currency)}</td>
                <td style={td}>{business.currency} {formatMoney(item.quantity * item.unit_price, business.currency)}</td>
              </tr>
            ))}
          </tbody>
        </table>

        <div style={totals}>
          <div style={row}><span>Subtotal:</span><span>{business.currency} {formatMoney(invoice.subtotal, business.currency)}</span></div>
          {invoice.tax_rate > 0 && (
            <div style={row}><span>Tax ({invoice.tax_rate}%):</span><span>{business.currency} {formatMoney(invoice.tax_amount, business.currency)}</span></div>
          )}
          <div style={{...row, ...totalRow}}>
            <span>Total:</span>
            <span>{business.currency} {formatMoney(invoice.total, business.currency)}</span>
          </div>
        </div>

        {invoice.notes && (
          <div style={{marginTop:14, fontSize:12, color:'#555', fontStyle:'italic'}}>
            Notes: {invoice.notes}
          </div>
        )}

        <div style={footer}>
          <p>Thank you for your business!</p>
        </div>

        {actionError && <div style={{ color: '#c0392b', fontSize: 13, marginTop: 10 }} className="no-print">{actionError}</div>}

        <div style={actions} className="no-print">
          {invoice.status === 'draft' && (
            <button onClick={() => handleAction(() => markInvoiceSent(invoice.id))} disabled={actionLoading} style={primaryBtn}>
              {actionLoading ? 'Working…' : 'Mark as sent'}
            </button>
          )}
          {(invoice.status === 'sent' || invoice.status === 'overdue') && (
            <button onClick={() => handleAction(() => markInvoicePaid(invoice.id))} disabled={actionLoading} style={primaryBtn}>
              {actionLoading ? 'Working…' : 'Mark as paid'}
            </button>
          )}
          {(invoice.status === 'draft' || invoice.status === 'sent') && (
            <button
              onClick={() => { if (confirm('Cancel this invoice? This cannot be undone.')) handleAction(() => cancelInvoice(invoice.id)); }}
              disabled={actionLoading}
              style={secondaryBtn}
            >
              Cancel invoice
            </button>
          )}
          <button onClick={handlePrint} style={primaryBtn}>🖨️ Print Invoice</button>
          <button onClick={onClose} style={secondaryBtn}>Close</button>
        </div>
      </div>
    </div>
  );
}

const overlay: React.CSSProperties = {
  position:'fixed', inset:0, background:'rgba(0,0,0,0.45)',
  display:'flex', alignItems:'center', justifyContent:'center', zIndex:1000,
};
const modal: React.CSSProperties = {
  background:'#fff', borderRadius:10, padding:28, width:520, maxWidth:'94vw',
  maxHeight:'90vh', overflowY:'auto', boxShadow:'0 8px 32px rgba(0,0,0,0.18)',
  fontFamily:"'IBM Plex Sans', system-ui, sans-serif",
};
const header: React.CSSProperties = { textAlign:'center', marginBottom:16, borderBottom:'1px dashed #ccc', paddingBottom:12 };
const logo: React.CSSProperties = { maxHeight:56, marginBottom:8, objectFit:'contain' };
const bizName: React.CSSProperties = { margin:'4px 0', fontSize:20, fontWeight:700, fontFamily:"Newsreader, Georgia, serif" };
const slogan: React.CSSProperties = { margin:0, fontSize:12, color:'#666', fontStyle:'italic' };
const table: React.CSSProperties = { width:'100%', borderCollapse:'collapse', fontSize:13, marginBottom:14 };
const thRow: React.CSSProperties = { borderBottom:'2px solid #333' };
const th: React.CSSProperties = { padding:'6px 4px', fontWeight:600, textAlign:'right', fontSize:12, textTransform:'uppercase' };
const tr: React.CSSProperties = { borderBottom:'1px solid #eee' };
const td: React.CSSProperties = { padding:'6px 4px', textAlign:'right' };
const totals: React.CSSProperties = { borderTop:'2px solid #333', paddingTop:10, fontSize:13 };
const row: React.CSSProperties = { display:'flex', justifyContent:'space-between', padding:'3px 0' };
const totalRow: React.CSSProperties = { fontWeight:700, fontSize:15, borderTop:'1px solid #ddd', marginTop:6, paddingTop:6 };
const footer: React.CSSProperties = { textAlign:'center', marginTop:18, fontSize:12, color:'#666' };
const actions: React.CSSProperties = { display:'flex', gap:10, marginTop:20, justifyContent:'center' };
const primaryBtn: React.CSSProperties = { padding:'10px 18px', borderRadius:8, border:'none', background:'#1a1a1a', color:'#fff', fontSize:13, fontWeight:600, cursor:'pointer' };
const secondaryBtn: React.CSSProperties = { padding:'10px 18px', borderRadius:8, border:'1px solid #ccc', background:'#fff', color:'#333', fontSize:13, cursor:'pointer' };
