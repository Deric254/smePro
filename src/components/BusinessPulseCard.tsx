import type { BusinessPulse } from '../api';
import { formatMoney } from '../lib/money';

/**
 * The "how is my business doing" readout — see business_pulse.rs's own
 * doc comment on why every number here is real, computed arithmetic
 * from actual sales/inventory history, never something narrated from
 * memory. Originally only rendered under an AI chat answer; now also
 * used standalone on the Dashboard (via GET /ai/pulse) so the owner
 * sees it without having to open the AI panel and ask a question
 * first. Deliberately one component with one set of styles for both
 * call sites — the numbers are identical, so the presentation should
 * never accidentally diverge between them.
 */
export default function BusinessPulseCard({ pulse, compact = false }: { pulse: BusinessPulse; compact?: boolean }) {
  if (!pulse.has_data) {
    return (
      <div style={compact ? styles.cardCompact : styles.card}>
        <div style={styles.label}>Business pulse</div>
        <div style={styles.hint}>{pulse.recommendations[0]}</div>
      </div>
    );
  }

  const trendArrow = pulse.pct_change === null ? '' : pulse.pct_change >= 0 ? '↑' : '↓';
  const trendColor = pulse.pct_change === null ? 'var(--ink-soft)' : pulse.pct_change >= 0 ? 'var(--ok, #2a7a3b)' : 'var(--stamp)';

  return (
    <div style={compact ? styles.cardCompact : styles.card}>
      <div style={styles.label}>Business pulse</div>
      <div style={styles.statRow}>
        <span>This month: <strong>{formatMoney(pulse.revenue_this_period_cents, pulse.currency)}</strong></span>
        {pulse.pct_change !== null && (
          <span style={{ color: trendColor, fontWeight: 600 }}>
            {trendArrow} {Math.abs(pulse.pct_change).toFixed(0)}%
          </span>
        )}
      </div>
      <div style={styles.statRow}>
        <span>Next month (estimate): <strong>{formatMoney(pulse.forecast_next_period_cents, pulse.currency)}</strong></span>
      </div>
      {pulse.recommendations.map((r, i) => (
        <div key={i} style={styles.recommendation}>• {r}</div>
      ))}
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  card: {
    marginTop: '0.35rem', padding: '0.55rem 0.7rem', background: 'var(--paper)',
    border: '1px solid var(--paper-line)', borderRadius: 6, fontSize: '0.78rem', maxWidth: '85%',
  },
  // Same visual language as the chat-panel card, sized for sitting in
  // its own full-width "card" wrapper on the Dashboard (see
  // Dashboard.tsx) instead of a narrow chat bubble — no maxWidth
  // clamp, and the outer .card class already supplies the border,
  // background and padding there, so this stays unpadded/unbordered
  // to avoid a double frame.
  cardCompact: {
    fontSize: '0.82rem',
  },
  label: { fontSize: '0.68rem', fontWeight: 700, color: 'var(--ink-faint)', textTransform: 'uppercase', letterSpacing: '0.04em', marginBottom: '0.3rem' },
  statRow: { display: 'flex', justifyContent: 'space-between', gap: '0.5rem', color: 'var(--ink)' },
  recommendation: { marginTop: '0.3rem', color: 'var(--ink-soft)', lineHeight: 1.4 },
  hint: { color: 'var(--ink-soft)' },
};
