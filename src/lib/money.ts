
const ZERO_DECIMAL = new Set([
  'JPY', 'KRW', 'VND', 'UGX', 'RWF', 'XOF', 'XAF', 'BIF', 'DJF', 'GNF',
  'KMF', 'MGA', 'PYG', 'VUV', 'CLP',
]);
const THREE_DECIMAL = new Set(['BHD', 'IQD', 'JOD', 'KWD', 'OMR', 'TND']);

export function decimalPlacesFor(currencyCode: string): number {
  const code = (currencyCode || 'USD').toUpperCase();
  if (ZERO_DECIMAL.has(code)) return 0;
  if (THREE_DECIMAL.has(code)) return 3;
  return 2;
}

export function formatMoney(cents: number | null | undefined, currencyCode: string = 'USD'): string {
  if (cents === null || cents === undefined || Number.isNaN(cents)) return '';
  const places = decimalPlacesFor(currencyCode);
  const scale = Math.pow(10, places);
  const value = cents / scale;
  return value.toLocaleString(undefined, {
    minimumFractionDigits: places,
    maximumFractionDigits: places,
  });
}

export function parseMoneyInput(input: string, currencyCode: string = 'USD'): number | null {
  const trimmed = (input ?? '').trim().replace(/,/g, '');
  if (!trimmed) return null;

  const negative = trimmed.startsWith('-');
  const unsigned = negative ? trimmed.slice(1) : trimmed;

  const places = decimalPlacesFor(currencyCode);
  const parts = unsigned.split('.');
  if (parts.length > 2) return null;
  const [whole, frac = ''] = parts;

  if (frac.length > places) return null; // more precision than this currency supports
  if (!/^\d*$/.test(whole) || !/^\d*$/.test(frac)) return null;
  if (whole === '' && frac === '') return null;

  const scale = Math.pow(10, places);
  const wholeVal = whole === '' ? 0 : parseInt(whole, 10);
  const fracVal = places === 0 ? 0 : parseInt(frac.padEnd(places, '0'), 10);

  const cents = wholeVal * scale + fracVal;
  if (!Number.isSafeInteger(cents)) return null;
  return negative ? -cents : cents;
}

export function multiplyMoney(cents: number, quantity: number): number {
  return cents * quantity;
}

export function sumMoney(amounts: number[]): number {
  return amounts.reduce((total, c) => total + c, 0);
}
