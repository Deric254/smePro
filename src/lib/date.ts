export function parseBackendTimestamp(value: string): Date {
  const trimmed = value.trim();
  if (/^\d{4}-\d{2}-\d{2}$/.test(trimmed)) return new Date(trimmed);
  if (trimmed.endsWith('Z') || /[+-]\d{2}:?\d{2}$/.test(trimmed)) return new Date(trimmed);
  return new Date(`${trimmed.replace(' ', 'T')}Z`);
}

export function formatBackendDateTime(value: string): string {
  return parseBackendTimestamp(value).toLocaleString();
}
export function formatBackendDate(value: string): string {
  return parseBackendTimestamp(value).toLocaleDateString();
}
