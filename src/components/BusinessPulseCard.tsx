import type { BusinessPulse } from '../api';
import { formatMoney } from '../lib/money';

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
      {pulse.low_stock_count > 0 && (
        <div style={styles.statRow}>
          <span>Low stock: <strong style={{ color: 'var(--stamp)' }}>{pulse.low_stock_count} item{pulse.low_stock_count === 1 ? '' : 's'}</strong></span>
        </div>
      )}
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
  cardCompact: {
    fontSize: '0.82rem',
  },
  label: { fontSize: '0.68rem', fontWeight: 700, color: 'var(--ink-faint)', textTransform: 'uppercase', letterSpacing: '0.04em', marginBottom: '0.3rem' },
  statRow: { display: 'flex', justifyContent: 'space-between', gap: '0.5rem', color: 'var(--ink)' },
  recommendation: { marginTop: '0.3rem', color: 'var(--ink-soft)', lineHeight: 1.4 },
  hint: { color: 'var(--ink-soft)' },
};
