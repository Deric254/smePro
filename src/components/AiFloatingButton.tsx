import { useState, useRef, useEffect } from 'react';
import type { FormEvent } from 'react';
import { askAi, getAiContext, ApiError } from '../api';
import { decimalPlacesFor } from '../lib/money';

interface Message { role: 'user' | 'ai'; text: string }

export default function AiFloatingButton() {
  const [open, setOpen] = useState(false);
  const [question, setQuestion] = useState('');
  const [messages, setMessages] = useState<Message[]>([]);
  const [loading, setLoading] = useState(false);
  const bodyRef = useRef<HTMLDivElement>(null);
  // "What can the AI see?" — a plain transparency view of the same
  // business snapshot the assistant actually reasons from (see
  // ai_context.rs::build_snapshot), so a business owner can check
  // exactly what data it has access to rather than taking that on
  // faith. Fetched on demand, not preloaded — most people opening the
  // chat just want to ask a question.
  const [showContext, setShowContext] = useState(false);
  const [contextData, setContextData] = useState<any>(null);
  const [contextLoading, setContextLoading] = useState(false);
  const [contextError, setContextError] = useState<string | null>(null);

  useEffect(() => {
    bodyRef.current?.scrollTo({ top: bodyRef.current.scrollHeight, behavior: 'smooth' });
  }, [messages, open]);

  function toggleContext() {
    if (showContext) { setShowContext(false); return; }
    setShowContext(true);
    if (contextData) return; // already fetched this session
    setContextLoading(true);
    setContextError(null);
    getAiContext()
      .then(setContextData)
      .catch((err) => setContextError(err instanceof ApiError ? err.message : 'Could not load this'))
      .finally(() => setContextLoading(false));
  }

  async function handleAsk(e: FormEvent) {
    e.preventDefault();
    if (!question.trim()) return;
    const q = question;
    setMessages((m) => [...m, { role: 'user', text: q }]);
    setQuestion('');
    setLoading(true);
    try {
      const res = await askAi(q);
      setMessages((m) => [...m, { role: 'ai', text: res.answer }]);
    } catch (err) {
      const msg = err instanceof ApiError ? err.message : 'Could not reach the assistant';
      setMessages((m) => [...m, { role: 'ai', text: msg }]);
    } finally {
      setLoading(false);
    }
  }

  return (
    <>
      {open && (
        <div style={styles.panel} className="card">
          <div style={styles.panelHeader}>
            <div style={{ display: 'flex', alignItems: 'center', gap: '0.6rem' }}>
              <span className="stamp-badge" style={{ width: '1.7rem', height: '1.7rem', fontSize: '0.65rem', color: 'var(--stamp)' }}>AI</span>
              <strong style={{ fontSize: '0.9rem' }}>Ask about your business</strong>
            </div>
            <div style={{ display: 'flex', alignItems: 'center', gap: '0.3rem' }}>
              <button onClick={toggleContext} style={styles.infoBtn} aria-label="What can the AI see?" title="What can the AI see?">ⓘ</button>
              <button onClick={() => setOpen(false)} style={styles.closeBtn} aria-label="Close">×</button>
            </div>
          </div>

          {showContext && (
            <div style={styles.contextPanel}>
              {contextLoading && <div style={styles.hint}>Loading…</div>}
              {contextError && <div style={{ ...styles.hint, color: 'var(--stamp)' }}>{contextError}</div>}
              {contextData && (
                <>
                  <div style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', marginBottom: '0.5rem' }}>
                    This is exactly what the assistant sees about {contextData.business_name} — nothing more.
                  </div>
                  {Object.entries(contextData.modules ?? {}).map(([id, data]: [string, any]) => (
                    <div key={id} style={{ marginBottom: '0.5rem' }}>
                      <strong style={{ fontSize: '0.82rem' }}>{data.display_name}</strong>
                      <div style={{ fontSize: '0.78rem', color: 'var(--ink-soft)' }}>
                        {data.record_count} record{data.record_count === 1 ? '' : 's'}
                        {Object.entries(data.totals ?? {}).map(([field, value]: [string, any]) => (
                          <span key={field}> · {field.replace(/_/g, ' ')}: {
                            typeof value === 'number' && field.match(/price|cost|revenue|amount|total|salary/i)
                              // ai_context.rs already converts "money" totals to decimal
                              // currency (not cents) before sending — see build_snapshot's
                              // comment on why. Formatted here with the currency's own
                              // decimal-place count, straight from the value as given, with
                              // no cents round-trip to introduce avoidable float noise.
                              ? value.toLocaleString(undefined, { minimumFractionDigits: decimalPlacesFor(contextData.currency), maximumFractionDigits: decimalPlacesFor(contextData.currency) })
                              : String(value)
                          }</span>
                        ))}
                        {data.low_stock_alerts?.length > 0 && <span> · {data.low_stock_alerts.length} item(s) low on stock</span>}
                      </div>
                    </div>
                  ))}
                </>
              )}
            </div>
          )}

          <div ref={bodyRef} style={styles.body}>
            {messages.length === 0 && (
              <div style={styles.hint}>
                Ask things like "what's low on stock?" or "how were sales this month?" — answers are grounded in your actual data.
              </div>
            )}
            {messages.map((m, i) => (
              <div key={i} style={m.role === 'user' ? styles.bubbleUser : styles.bubbleAi}>
                {m.text}
              </div>
            ))}
            {loading && <div style={styles.bubbleAi}>Thinking…</div>}
          </div>

          <form onSubmit={handleAsk} style={styles.inputRow}>
            <input
              value={question}
              onChange={(e) => setQuestion(e.target.value)}
              placeholder="Ask a question…"
              style={{ flex: 1 }}
            />
            <button className="btn btn-stamp" type="submit" disabled={loading}>Send</button>
          </form>
        </div>
      )}

      <button
        onClick={() => setOpen((v) => !v)}
        style={styles.fab}
        aria-label="Open AI assistant"
      >
        {open ? '×' : 'AI'}
      </button>
    </>
  );
}

const styles: Record<string, React.CSSProperties> = {
  fab: {
    position: 'fixed', bottom: '1.6rem', right: '1.6rem', width: '3.4rem', height: '3.4rem',
    borderRadius: '999px', background: 'var(--stamp)', color: '#fff', border: '2px solid var(--stamp)',
    fontFamily: 'var(--font-display)', fontWeight: 600, fontSize: '0.95rem',
    boxShadow: '0 4px 14px rgba(32,20,15,0.25)', zIndex: 40,
  },
  panel: {
    position: 'fixed', bottom: '5.4rem', right: '1.6rem', width: 320, maxWidth: 'calc(100vw - 2.5rem)',
    height: 420, display: 'flex', flexDirection: 'column', padding: 0, zIndex: 40,
    boxShadow: '0 10px 30px rgba(32,20,15,0.2)',
  },
  panelHeader: { display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '0.8rem 0.9rem', borderBottom: '1px solid var(--paper-line)' },
  infoBtn: { background: 'none', border: 'none', fontSize: '1rem', color: 'var(--ink-soft)', lineHeight: 1, cursor: 'pointer', padding: '0.1rem 0.3rem' },
  contextPanel: { padding: '0.7rem 0.9rem', borderBottom: '1px solid var(--paper-line)', maxHeight: 160, overflowY: 'auto', background: 'var(--paper)' },
  closeBtn: { background: 'none', border: 'none', fontSize: '1.3rem', color: 'var(--ink-soft)', lineHeight: 1 },
  body: { flex: 1, overflowY: 'auto', padding: '0.8rem 0.9rem', display: 'flex', flexDirection: 'column', gap: '0.5rem' },
  hint: { fontSize: '0.8rem', color: 'var(--ink-soft)', lineHeight: 1.5 },
  bubbleUser: { alignSelf: 'flex-end', background: 'var(--ink)', color: '#fff', padding: '0.5em 0.75em', borderRadius: '10px 10px 2px 10px', fontSize: '0.85rem', maxWidth: '85%' },
  bubbleAi: { alignSelf: 'flex-start', background: 'var(--paper)', border: '1px solid var(--paper-line)', padding: '0.5em 0.75em', borderRadius: '10px 10px 10px 2px', fontSize: '0.85rem', maxWidth: '85%', whiteSpace: 'pre-wrap' },
  inputRow: { display: 'flex', gap: '0.5rem', padding: '0.7rem 0.9rem', borderTop: '1px solid var(--paper-line)' },
};
