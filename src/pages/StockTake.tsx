import { useEffect, useState } from 'react';
import {
  initiateStockTake, getOpenStockTake, recordStockTakeCount, closeStockTake, getStockTakeHistory,
  ApiError,
} from '../api';
import type { StockTake, StockTakeSummary, StockTakeCloseResult } from '../api';
import { formatBackendDateTime } from '../lib/date';

export default function StockTakePage() {
  const [open, setOpen] = useState<StockTake | null>(null);
  const [history, setHistory] = useState<StockTakeSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);
  const [closing, setClosing] = useState(false);
  const [closeResult, setCloseResult] = useState<StockTakeCloseResult | null>(null);
  // Local text per item, so someone can clear a field and retype
  // without an in-flight save fighting the input mid-keystroke.
  const [countText, setCountText] = useState<Record<string, string>>({});
  const [savingItemId, setSavingItemId] = useState<string | null>(null);

  function loadOpenAndHistory() {
    setLoading(true);
    Promise.all([getOpenStockTake(), getStockTakeHistory()])
      .then(([o, h]) => {
        setOpen(o.open);
        setHistory(h.stock_takes.filter((s) => s.status === 'closed'));
        if (o.open) {
          const initial: Record<string, string> = {};
          for (const item of o.open.items) {
            if (item.counted_qty !== null) initial[item.id] = String(item.counted_qty);
          }
          setCountText(initial);
        }
      })
      .catch(() => setError('Could not load stock take status.'))
      .finally(() => setLoading(false));
  }

  useEffect(() => { loadOpenAndHistory(); }, []);

  async function handleStart() {
    setStarting(true);
    setError(null);
    try {
      const st = await initiateStockTake();
      setOpen(st);
      setCloseResult(null);
      setCountText({});
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not start a stock take.');
    } finally {
      setStarting(false);
    }
  }

  async function handleSaveCount(itemId: string) {
    if (!open) return;
    const text = countText[itemId] ?? '';
    const qty = parseInt(text, 10);
    if (text.trim() === '' || Number.isNaN(qty) || qty < 0) {
      setError('Enter a whole number of 0 or more before saving a count.');
      return;
    }
    setSavingItemId(itemId);
    setError(null);
    try {
      await recordStockTakeCount(open.id, itemId, qty);
      setOpen((prev) => prev
        ? { ...prev, items: prev.items.map((i) => (i.id === itemId ? { ...i, counted_qty: qty } : i)) }
        : prev);
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not save that count.');
    } finally {
      setSavingItemId(null);
    }
  }

  async function handleClose() {
    if (!open) return;
    setClosing(true);
    setError(null);
    try {
      const result = await closeStockTake(open.id);
      setCloseResult(result);
      setOpen(null);
      setCountText({});
      loadOpenAndHistory();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not close this stock take.');
    } finally {
      setClosing(false);
    }
  }

  const countedCount = open ? open.items.filter((i) => i.counted_qty !== null).length : 0;

  return (
    <div>
      <h1>Stock Take</h1>

      {error && (
        <div className="card" style={{ borderColor: 'var(--stamp)', color: 'var(--stamp)', marginBottom: '1rem' }}>{error}</div>
      )}

      {loading ? (
        <div style={{ color: 'var(--ink-soft)' }}>Loading…</div>
      ) : !open ? (
        <>
          <div className="card" style={{ marginBottom: '1rem' }}>
            <p style={{ marginTop: 0, color: 'var(--ink-soft)', fontSize: '0.9rem' }}>
              Start a count to reconcile physical stock against what the system has on record. You can count as
              many or as few items as you have time for — anything left uncounted is untouched when you close.
            </p>
            <button className="btn btn-stamp" onClick={handleStart} disabled={starting}>
              {starting ? 'Starting…' : 'Start Stock Take'}
            </button>
          </div>

          {closeResult && (
            <CloseSummary result={closeResult} onDismiss={() => setCloseResult(null)} />
          )}

          {history.length > 0 && (
            <div className="card">
              <div style={{ fontSize: '0.85rem', fontWeight: 600, marginBottom: '0.5rem' }}>Past stock takes</div>
              <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: '0.85rem' }}>
                <thead>
                  <tr style={{ textAlign: 'left', color: 'var(--ink-soft)' }}>
                    <th style={{ padding: '0.3rem 0.5rem' }}>Closed</th>
                    <th style={{ padding: '0.3rem 0.5rem' }}>Items counted</th>
                  </tr>
                </thead>
                <tbody>
                  {history.map((h) => (
                    <tr key={h.id} style={{ borderTop: '1px solid var(--paper-line)' }}>
                      <td style={{ padding: '0.3rem 0.5rem' }}>{h.closed_at ? formatBackendDateTime(h.closed_at) : '—'}</td>
                      <td style={{ padding: '0.3rem 0.5rem' }}>{h.counted_count} of {h.item_count}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </>
      ) : (
        <>
          <div className="card" style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: '1rem' }}>
            <div style={{ fontSize: '0.85rem', color: 'var(--ink-soft)' }}>
              {countedCount} of {open.items.length} items counted · started {formatBackendDateTime(open.created_at)}
            </div>
            <button className="btn btn-stamp" onClick={handleClose} disabled={closing}>
              {closing ? 'Closing…' : 'Close Stock Take'}
            </button>
          </div>

          <div className="card">
            <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: '0.9rem' }}>
              <thead>
                <tr style={{ textAlign: 'left', color: 'var(--ink-soft)' }}>
                  <th style={{ padding: '0.4rem 0.5rem' }}>Item</th>
                  <th style={{ padding: '0.4rem 0.5rem', textAlign: 'right' }}>Expected</th>
                  <th style={{ padding: '0.4rem 0.5rem', textAlign: 'right' }}>Counted</th>
                  <th style={{ padding: '0.4rem 0.5rem' }} />
                </tr>
              </thead>
              <tbody>
                {open.items.map((item) => (
                  <tr key={item.id} style={{ borderTop: '1px solid var(--paper-line)' }}>
                    <td style={{ padding: '0.4rem 0.5rem' }}>{item.item_name}</td>
                    <td style={{ padding: '0.4rem 0.5rem', textAlign: 'right', color: 'var(--ink-soft)' }}>{item.expected_qty}</td>
                    <td style={{ padding: '0.4rem 0.5rem', textAlign: 'right' }}>
                      <input
                        type="number"
                        min={0}
                        step={1}
                        value={countText[item.id] ?? ''}
                        onChange={(e) => setCountText((prev) => ({ ...prev, [item.id]: e.target.value }))}
                        style={{ width: '5rem', textAlign: 'right' }}
                      />
                    </td>
                    <td style={{ padding: '0.4rem 0.5rem' }}>
                      <button
                        className="btn btn-outline"
                        onClick={() => handleSaveCount(item.id)}
                        disabled={savingItemId === item.id}
                        style={{ fontSize: '0.78rem', padding: '0.2rem 0.6rem' }}
                      >
                        {item.counted_qty !== null ? 'Update' : 'Save'}
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      )}
    </div>
  );
}

function CloseSummary({ result, onDismiss }: { result: StockTakeCloseResult; onDismiss: () => void }) {
  const changed = result.adjustments.filter((a) => a.variance !== 0);
  return (
    <div className="card" style={{ marginBottom: '1rem' }}>
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '0.5rem' }}>
        <div style={{ fontWeight: 600, fontSize: '0.9rem' }}>Stock take closed</div>
        <button className="btn btn-outline" onClick={onDismiss} style={{ fontSize: '0.78rem', padding: '0.2rem 0.6rem' }}>Dismiss</button>
      </div>
      <div style={{ fontSize: '0.85rem', color: 'var(--ink-soft)', marginBottom: '0.6rem' }}>
        {result.items_counted} counted, {result.items_skipped} skipped ·
        {' '}net change: {result.total_variance_units > 0 ? '+' : ''}{result.total_variance_units} units
      </div>
      {changed.length === 0 ? (
        <div style={{ fontSize: '0.85rem', color: 'var(--ink-soft)' }}>No discrepancies found.</div>
      ) : (
        <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: '0.85rem' }}>
          <thead>
            <tr style={{ textAlign: 'left', color: 'var(--ink-soft)' }}>
              <th style={{ padding: '0.3rem 0.5rem' }}>Item</th>
              <th style={{ padding: '0.3rem 0.5rem', textAlign: 'right' }}>Expected</th>
              <th style={{ padding: '0.3rem 0.5rem', textAlign: 'right' }}>Counted</th>
              <th style={{ padding: '0.3rem 0.5rem', textAlign: 'right' }}>Variance</th>
            </tr>
          </thead>
          <tbody>
            {changed.map((a) => (
              <tr key={a.inventory_record_id} style={{ borderTop: '1px solid var(--paper-line)' }}>
                <td style={{ padding: '0.3rem 0.5rem' }}>{a.item_name}</td>
                <td style={{ padding: '0.3rem 0.5rem', textAlign: 'right' }}>{a.expected_qty}</td>
                <td style={{ padding: '0.3rem 0.5rem', textAlign: 'right' }}>{a.counted_qty}</td>
                <td style={{ padding: '0.3rem 0.5rem', textAlign: 'right', color: a.variance < 0 ? 'var(--stamp)' : 'inherit' }}>
                  {a.variance > 0 ? '+' : ''}{a.variance}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
