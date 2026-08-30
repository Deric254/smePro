// Money as integer minor units (cents), not floating point — the
// frontend counterpart to src-tauri/src/money.rs. Every "money"-typed
// field, everywhere in the app, is an integer number of cents on the
// wire and in memory. This file is the ONLY place a decimal string
// gets parsed into cents or cents get formatted back to a decimal
// string — no component does its own `.toFixed(2)` or `parseFloat`
// on a money value anymore.

// Minor-unit decimal places per ISO 4217 currency code. Mirrors
// money::decimal_places_for in the Rust backend exactly — if this
// list ever needs to change, change it in both places.
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

/**
 * Formats integer minor units (cents) into a display string, e.g.
 * formatMoney(1250, 'USD') -> "12.50". Currency-aware: a 0-decimal
 * currency like JPY never gets a fractional part appended.
 */
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

/**
 * Parses a human-typed decimal string into integer minor units.
 * Mirrors money::parse_money_input in the Rust backend: rejects more
 * precision than the currency supports (e.g. "12.505" for a 2dp
 * currency) rather than silently rounding it away. Returns null for
 * anything invalid — callers decide how to surface that.
 *
 * THE BUG THIS FIXES: formatMoney() above renders amounts >= 1000
 * with thousands separators via toLocaleString ("50,000.00") — which
 * is exactly what startEdit() in ModuleView.tsx seeds a money field
 * with when editing an existing record, and exactly what this same
 * function's own onBlur handler re-formats a field into as soon as a
 * person tabs away from it. Every one of those comma-formatted
 * strings then had nowhere to go: this parser rejected any non-digit
 * character, including the very commas formatMoney had just inserted,
 * so simply opening an HR salary (almost always >= 1,000) for edit
 * and saving it straight back — without changing a thing — failed
 * with "'50,000.00' is not a valid amount". Thousands separators are
 * also just how people normally type larger figures by hand. Commas
 * are stripped before every other check runs, so "50,000.00",
 * "50,000", and "50000" all parse identically — this never loosens
 * what counts as a valid decimal point or digit, it only ignores the
 * grouping punctuation a human (or this same file) would put in.
 */
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

/**
 * Multiplies an integer cents amount by a quantity — exact, since
 * both are integers. Prefer this over inline `price * qty` so every
 * money calculation in the app is visibly going through one place.
 */
export function multiplyMoney(cents: number, quantity: number): number {
  return cents * quantity;
}

/**
 * Sums an array of integer cents amounts — exact, since JS numbers
 * represent integers up to 2^53 without any floating-point error.
 */
export function sumMoney(amounts: number[]): number {
  return amounts.reduce((total, c) => total + c, 0);
}
