import { useState, useRef, useEffect } from 'react';
import type { FormEvent } from 'react';
import {
  askAiInSession, getAiContext, ApiError,
  listAiSessions, createAiSession, getAiSessionMessages, clearAiSession, deleteAiSession, exportAiChatHistory,
} from '../api';
import type { AiChatSession, AiChatMessage, BusinessPulse } from '../api';
import { decimalPlacesFor, formatMoney } from '../lib/money';
import { MarkdownLite } from '../lib/markdownLite';

export default function AiFloatingButton({ open, onClose }: { open: boolean; onClose: () => void }) {
  const [question, setQuestion] = useState('');
  const [messages, setMessages] = useState<AiChatMessage[]>([]);
  const [loading, setLoading] = useState(false);
  const bodyRef = useRef<HTMLDivElement>(null);

  // The active conversation. Null means "no session yet" — the panel
  // creates one lazily on the first question asked, rather than
  // writing an empty session row every time someone just opens the
  // panel to look, then closes it without asking anything.
  const [sessionId, setSessionId] = useState<string | null>(null);

  // History sidebar — past sessions, shown on demand rather than
  // always visible, since most opens are "ask one thing," not
  // "browse my chat history."
  const [showHistory, setShowHistory] = useState(false);
  const [sessions, setSessions] = useState<AiChatSession[]>([]);
  const [sessionsLoading, setSessionsLoading] = useState(false);
  const [exporting, setExporting] = useState(false);

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

  async function refreshSessions() {
    setSessionsLoading(true);
    try {
      const r = await listAiSessions();
      setSessions(r.sessions);
    } catch {
      // A failed history fetch shouldn't block the chat itself —
      // the sidebar just stays empty/stale until reopened.
    } finally {
      setSessionsLoading(false);
    }
  }

  function toggleHistory() {
    if (showHistory) { setShowHistory(false); return; }
    setShowHistory(true);
    refreshSessions();
  }

  async function openSession(id: string) {
    setShowHistory(false);
    setSessionId(id);
    setMessages([]);
    try {
      const r = await getAiSessionMessages(id);
      setMessages(r.messages);
    } catch {
      // Session may have been deleted from another tab/device between
      // the list fetch and this open — fall back to a fresh, empty
      // conversation rather than showing a broken panel.
      setSessionId(null);
    }
  }

  function startNewChat() {
    setShowHistory(false);
    setSessionId(null);
    setMessages([]);
  }

  async function handleClearChat() {
    setMessages([]);
    if (!sessionId) return; // nothing persisted yet — clearing the empty view is enough
    try {
      await clearAiSession(sessionId);
    } catch {
      // The visible chat is already cleared either way; a failed
      // server-side clear just means it'll reappear next time this
      // session is reopened from history, not a broken state now.
    }
  }

  async function handleDeleteSession(id: string, e: React.MouseEvent) {
    e.stopPropagation(); // don't also trigger the row's openSession click
    try {
      await deleteAiSession(id);
      setSessions((s) => s.filter((sess) => sess.id !== id));
      if (sessionId === id) startNewChat();
    } catch {
      // Leave the row in the list — the person can retry the delete
      // rather than silently losing track of whether it worked.
    }
  }

  async function handleExport() {
    setExporting(true);
    try {
      await exportAiChatHistory();
    } catch (err) {
      const msg = err instanceof ApiError ? err.message : 'Could not export chat history';
      setMessages((m) => [...m, { role: 'ai', content: msg, created_at: new Date().toISOString() }]);
    } finally {
      setExporting(false);
    }
  }

  async function handleAsk(e: FormEvent) {
    e.preventDefault();
    if (!question.trim()) return;
    const q = question;
    const now = new Date().toISOString();
    setMessages((m) => [...m, { role: 'user', content: q, created_at: now }]);
    setQuestion('');
    setLoading(true);
    try {
      // A session is created lazily on the very first real question —
      // this is the one place a new row gets written, so opening the
      // panel to just look never creates an empty conversation.
      let activeSession = sessionId;
      if (!activeSession) {
        const created = await createAiSession();
        activeSession = created.session_id;
        setSessionId(activeSession);
      }
      const res = await askAiInSession(activeSession, q);
      setMessages((m) => [...m, { role: 'ai', content: res.answer, created_at: new Date().toISOString(), business_pulse: res.business_pulse }]);
    } catch (err) {
      const msg = err instanceof ApiError ? err.message : 'Could not reach the assistant';
      setMessages((m) => [...m, { role: 'ai', content: msg, created_at: new Date().toISOString() }]);
    } finally {
      setLoading(false);
    }
  }

  return (
    <>
      {open && (
        <div style={styles.panel} className="card ai-panel">
          <div style={styles.panelHeader}>
            <div style={{ display: 'flex', alignItems: 'center', gap: '0.6rem' }}>
              <span className="stamp-badge" style={{ width: '1.7rem', height: '1.7rem', fontSize: '0.65rem', color: 'var(--stamp)' }}>AI</span>
              <strong style={{ fontSize: '0.9rem' }}>Ask about your business</strong>
            </div>
            <div style={{ display: 'flex', alignItems: 'center', gap: '0.2rem' }}>
              <button onClick={startNewChat} style={styles.iconBtn} aria-label="New chat" title="New chat">+</button>
              <button onClick={toggleHistory} style={styles.iconBtn} aria-label="Chat history" title="Chat history">🕘</button>
              <button onClick={handleExport} disabled={exporting} style={styles.iconBtn} aria-label="Export chat history to Excel" title="Export all chat history to Excel">⇩</button>
              <button onClick={toggleContext} style={styles.iconBtn} aria-label="What can the AI see?" title="What can the AI see?">ⓘ</button>
              <button onClick={onClose} style={styles.closeBtn} aria-label="Close">×</button>
            </div>
          </div>

          {showHistory && (
            <div style={styles.historyPanel}>
              {sessionsLoading && <div style={styles.hint}>Loading…</div>}
              {!sessionsLoading && sessions.length === 0 && (
                <div style={styles.hint}>No past conversations yet — ask something to start one.</div>
              )}
              {sessions.map((s) => (
                <div key={s.id} onClick={() => openSession(s.id)} style={styles.historyRow}>
                  <div style={{ flex: 1, minWidth: 0 }}>
                    <div style={{ fontSize: '0.82rem', fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{s.title}</div>
                    {s.last_message && (
                      <div style={{ fontSize: '0.74rem', color: 'var(--ink-soft)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{s.last_message}</div>
                    )}
                  </div>
                  <button onClick={(e) => handleDeleteSession(s.id, e)} style={styles.deleteBtn} aria-label="Delete this chat" title="Delete this chat">🗑</button>
                </div>
              ))}
            </div>
          )}

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
              <div key={i}>
                <div style={m.role === 'user' ? styles.bubbleUser : styles.bubbleAi}>
                  <MarkdownLite text={m.content} />
                </div>
                {m.role === 'ai' && m.business_pulse && <BusinessPulseCard pulse={m.business_pulse} />}
              </div>
            ))}
            {loading && <div style={styles.bubbleAi}>Thinking…</div>}
          </div>

          <div style={styles.actionRow}>
            <button onClick={handleClearChat} style={styles.clearBtn} disabled={messages.length === 0}>Clear chat</button>
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
    </>
  );
}

/**
 * The "how is my business doing" readout attached under every AI
 * answer — see business_pulse.rs's own doc comment on why every
 * number here is real, computed arithmetic from actual sales history,
 * never something the AI model narrated from memory. This card is
 * deliberately plain and compact: it's a standing feature that
 * appears after EVERY message (even "good morning"), so it can't be
 * visually loud or it would drown out the actual answer above it.
 */
function BusinessPulseCard({ pulse }: { pulse: BusinessPulse }) {
  if (!pulse.has_data) {
    return (
      <div style={pulseStyles.card}>
        <div style={pulseStyles.label}>Business pulse</div>
        <div style={pulseStyles.hint}>{pulse.recommendations[0]}</div>
      </div>
    );
  }

  const trendArrow = pulse.pct_change === null ? '' : pulse.pct_change >= 0 ? '↑' : '↓';
  const trendColor = pulse.pct_change === null ? 'var(--ink-soft)' : pulse.pct_change >= 0 ? 'var(--ok, #2a7a3b)' : 'var(--stamp)';

  return (
    <div style={pulseStyles.card}>
      <div style={pulseStyles.label}>Business pulse</div>
      <div style={pulseStyles.statRow}>
        <span>This month: <strong>{formatMoney(pulse.revenue_this_period_cents, pulse.currency)}</strong></span>
        {pulse.pct_change !== null && (
          <span style={{ color: trendColor, fontWeight: 600 }}>
            {trendArrow} {Math.abs(pulse.pct_change).toFixed(0)}%
          </span>
        )}
      </div>
      <div style={pulseStyles.statRow}>
        <span>Next month (estimate): <strong>{formatMoney(pulse.forecast_next_period_cents, pulse.currency)}</strong></span>
      </div>
      {pulse.recommendations.map((r, i) => (
        <div key={i} style={pulseStyles.recommendation}>• {r}</div>
      ))}
    </div>
  );
}

const pulseStyles: Record<string, React.CSSProperties> = {
  card: {
    marginTop: '0.35rem', padding: '0.55rem 0.7rem', background: 'var(--paper)',
    border: '1px solid var(--paper-line)', borderRadius: 6, fontSize: '0.78rem', maxWidth: '85%',
  },
  label: { fontSize: '0.68rem', fontWeight: 700, color: 'var(--ink-faint)', textTransform: 'uppercase', letterSpacing: '0.04em', marginBottom: '0.3rem' },
  statRow: { display: 'flex', justifyContent: 'space-between', gap: '0.5rem', color: 'var(--ink)' },
  recommendation: { marginTop: '0.3rem', color: 'var(--ink-soft)', lineHeight: 1.4 },
  hint: { color: 'var(--ink-soft)' },
};

const styles: Record<string, React.CSSProperties> = {
  panel: {
    // Anchored bottom-right on desktop — a small card near where it
    // was opened from (the sidebar), not an unrelated floating
    // widget. On phones (see .ai-panel in mobile.css) this becomes a
    // full-height right-side drawer instead, using the exact same
    // slide-in pattern as .app-sidebar itself (see index.css), so
    // opening the assistant reads as "part of this app's navigation"
    // rather than a chat-widget bolted on from outside.
    position: 'fixed', bottom: 'calc(1.6rem + env(safe-area-inset-bottom))', right: 'calc(1.6rem + env(safe-area-inset-right))', width: 340, maxWidth: 'calc(100vw - 2.5rem)',
    height: 460, display: 'flex', flexDirection: 'column', padding: 0, zIndex: 40,
    boxShadow: '0 10px 30px rgba(32,20,15,0.2)',
  },
  panelHeader: { display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '0.8rem 0.9rem', borderBottom: '1px solid var(--paper-line)' },
  iconBtn: { background: 'none', border: 'none', fontSize: '0.95rem', color: 'var(--ink-soft)', lineHeight: 1, cursor: 'pointer', padding: '0.2rem 0.35rem' },
  closeBtn: { background: 'none', border: 'none', fontSize: '1.3rem', color: 'var(--ink-soft)', lineHeight: 1, cursor: 'pointer' },
  historyPanel: { padding: '0.5rem 0.5rem', borderBottom: '1px solid var(--paper-line)', maxHeight: 200, overflowY: 'auto', background: 'var(--paper)' },
  historyRow: { display: 'flex', alignItems: 'center', gap: '0.4rem', padding: '0.5rem 0.5rem', borderRadius: 6, cursor: 'pointer' },
  deleteBtn: { background: 'none', border: 'none', fontSize: '0.85rem', color: 'var(--ink-soft)', cursor: 'pointer', flexShrink: 0 },
  contextPanel: { padding: '0.7rem 0.9rem', borderBottom: '1px solid var(--paper-line)', maxHeight: 160, overflowY: 'auto', background: 'var(--paper)' },
  body: { flex: 1, overflowY: 'auto', padding: '0.8rem 0.9rem', display: 'flex', flexDirection: 'column', gap: '0.5rem' },
  hint: { fontSize: '0.8rem', color: 'var(--ink-soft)', lineHeight: 1.5 },
  bubbleUser: { alignSelf: 'flex-end', background: 'var(--ink)', color: '#fff', padding: '0.5em 0.75em', borderRadius: '10px 10px 2px 10px', fontSize: '0.85rem', maxWidth: '85%' },
  bubbleAi: { alignSelf: 'flex-start', background: 'var(--paper)', border: '1px solid var(--paper-line)', padding: '0.5em 0.75em', borderRadius: '10px 10px 10px 2px', fontSize: '0.85rem', maxWidth: '85%', lineHeight: 1.5 },
  actionRow: { display: 'flex', justifyContent: 'flex-end', padding: '0.3rem 0.9rem 0' },
  clearBtn: { background: 'none', border: 'none', fontSize: '0.72rem', color: 'var(--ink-soft)', cursor: 'pointer', textDecoration: 'underline', padding: '0.1rem 0' },
  inputRow: { display: 'flex', gap: '0.5rem', padding: '0.7rem 0.9rem', borderTop: '1px solid var(--paper-line)' },
};
