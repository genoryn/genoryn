/** Exact atomic-unit arithmetic. 1 GNR = 10^12 gori. Never binary floating point. */

export const GORI = 1_000_000_000_000n;
export const MAX_SUPPLY = 21_000_000n * GORI;
export const DECIMALS = 12;

export type GoriLike = bigint | string | number;

export function toBig(v: GoriLike): bigint {
  if (typeof v === 'bigint') return v;
  if (typeof v === 'number') {
    if (!Number.isSafeInteger(v)) throw new RangeError('unsafe integer gori value');
    return BigInt(v);
  }
  if (!/^-?\d+$/.test(v)) throw new RangeError(`invalid gori string: ${v}`);
  return BigInt(v);
}

/** Exact decimal GNR string, trailing zeroes trimmed (e.g. "7.92056697108"). */
export function goriToGnr(v: GoriLike, opts: { minFraction?: number; maxFraction?: number } = {}): string {
  const n = toBig(v);
  const neg = n < 0n;
  const abs = neg ? -n : n;
  const whole = abs / GORI;
  let frac = (abs % GORI).toString().padStart(DECIMALS, '0');
  const max = opts.maxFraction ?? DECIMALS;
  if (max < DECIMALS) frac = frac.slice(0, max); // truncation, never rounding up value
  frac = frac.replace(/0+$/, '');
  const min = opts.minFraction ?? 0;
  if (frac.length < min) frac = frac.padEnd(min, '0');
  return `${neg ? '-' : ''}${whole.toString()}${frac ? '.' + frac : ''}`;
}

/** Group thousands of the integer part. */
export function groupDigits(s: string): string {
  const [i = '', f] = s.split('.');
  const neg = i.startsWith('-');
  const digits = neg ? i.slice(1) : i;
  const grouped = digits.replace(/\B(?=(\d{3})+(?!\d))/g, ',');
  return `${neg ? '-' : ''}${grouped}${f !== undefined ? '.' + f : ''}`;
}

export function formatGnr(v: GoriLike, maxFraction = 4, minFraction = 0): string {
  return groupDigits(goriToGnr(v, { maxFraction, minFraction }));
}

/** Parse an exact decimal GNR amount (max 12 fractional digits) into gori. */
export function parseGnr(text: string): bigint {
  const t = text.trim();
  if (!/^\d+(\.\d{1,12})?$/.test(t)) throw new RangeError('amount must be a nonnegative decimal with at most 12 fractional digits');
  const [w = '0', f = ''] = t.split('.');
  const amount = BigInt(w) * GORI + BigInt(f.padEnd(DECIMALS, '0') || '0');
  if (amount > MAX_SUPPLY) throw new RangeError('amount exceeds maximum supply');
  return amount;
}

export function percent(part: bigint, total: bigint, digits = 4): string {
  if (total === 0n) return '0';
  const scale = 10n ** BigInt(digits + 2);
  const v = (part * scale) / total; // percent * 10^digits
  const s = v.toString().padStart(digits + 1, '0');
  const out = `${s.slice(0, -digits)}.${s.slice(-digits)}`.replace(/\.?0+$/, '');
  return out || '0';
}
