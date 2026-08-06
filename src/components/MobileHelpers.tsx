import React from 'react';

/**
 * useMediaQuery — React hook for responsive breakpoints
 * Usage: const isMobile = useMediaQuery('(max-width: 768px)');
 */
export function useMediaQuery(query: string): boolean {
  const [matches, setMatches] = React.useState(() => {
    if (typeof window === 'undefined') return false;
    return window.matchMedia(query).matches;
  });

  React.useEffect(() => {
    const mql = window.matchMedia(query);
    const handler = (e: MediaQueryListEvent) => setMatches(e.matches);
    mql.addEventListener('change', handler);
    return () => mql.removeEventListener('change', handler);
  }, [query]);

  return matches;
}

/**
 * MobileTable — renders table data as cards on mobile
 * Usage: <MobileTable columns={cols} data={rows} />
 */
interface Column {
  key: string;
  label: string;
  render?: (value: any, row: any) => React.ReactNode;
}

export function MobileTable({ columns, data }: { columns: Column[]; data: any[] }) {
  const isMobile = useMediaQuery('(max-width: 768px)');

  if (!isMobile) {
    return (
      <table className="data-table" style={{ width: '100%', borderCollapse: 'collapse', fontSize: 13 }}>
        <thead>
          <tr style={{ borderBottom: '2px solid #333' }}>
            {columns.map(c => (
              <th key={c.key} style={{ padding: '8px 6px', textAlign: 'left', fontSize: 12, textTransform: 'uppercase', letterSpacing: 0.5 }}>{c.label}</th>
            ))}
          </tr>
        </thead>
        <tbody>
          {data.map((row, i) => (
            <tr key={i} style={{ borderBottom: '1px solid #eee' }}>
              {columns.map(c => (
                <td key={c.key} style={{ padding: '8px 6px' }}>
                  {c.render ? c.render(row[c.key], row) : row[c.key]}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    );
  }

  return (
    <div>
      {data.map((row, i) => (
        <div key={i} style={{ marginBottom: 12, padding: 14, border: '1px solid #e5e5e5', borderRadius: 8, background: '#fff' }}>
          {columns.map(c => (
            <div key={c.key} style={{ display: 'flex', justifyContent: 'space-between', padding: '6px 0', borderBottom: '1px solid #f5f5f5' }}>
              <span style={{ fontSize: 11, fontWeight: 600, color: '#666', textTransform: 'uppercase' }}>{c.label}</span>
              <span style={{ fontSize: 13 }}>{c.render ? c.render(row[c.key], row) : row[c.key]}</span>
            </div>
          ))}
        </div>
      ))}
    </div>
  );
}
