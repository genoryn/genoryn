/** Experimental payment URI v1, mirroring apps/genoryn-wallet/src/payment_uri.rs. */
import { validateAddress } from './bech32m';
import { goriToGnr, parseGnr } from './gori';
import { RESERVED_NAMES, type NetworkName } from './params';

export const MAX_MONEY_PER_TX = 9_223_372_036_854_775_807n; // i64::MAX

export interface PaymentRequest {
  address: string;
  amountGori: bigint | null;
}

export function parsePaymentUri(text: string, expected: NetworkName): PaymentRequest {
  // eslint-disable-next-line no-control-regex
  if (text.length > 2048 || !/^[\x00-\x7f]*$/.test(text) || /[\s#%]/.test(text)) throw new Error('malformed payment URI');
  if (!text.startsWith('genoryn:')) throw new Error('expected genoryn: scheme');
  const body = text.slice('genoryn:'.length);
  const q = body.indexOf('?');
  if (q === -1) throw new Error('payment URI version and network are required');
  const address = body.slice(0, q);
  const query = body.slice(q + 1);
  if (!address || address.includes('/') || address.includes('@')) throw new Error('payment URI requires a raw receiving identity');
  const check = validateAddress(address, expected);
  if (!check.valid) throw new Error('payment URI receiving identity is invalid or belongs to another network');
  const params = new Map<string, string>();
  for (const item of query.split('&')) {
    const eq = item.indexOf('=');
    if (eq === -1) throw new Error('malformed payment URI parameter');
    const key = item.slice(0, eq);
    const value = item.slice(eq + 1);
    if (!['v', 'network', 'amount'].includes(key) || !value) throw new Error('unsupported or empty payment URI parameter');
    if (params.has(key)) throw new Error('duplicate payment URI parameter');
    params.set(key, value);
  }
  if (params.get('v') !== '1') throw new Error('unsupported payment URI version');
  if (params.get('network') !== expected) throw new Error('payment URI belongs to another network');
  let amountGori: bigint | null = null;
  const raw = params.get('amount');
  if (raw !== undefined) {
    const amount = parseGnr(raw);
    if (amount <= 0n || amount > MAX_MONEY_PER_TX) throw new Error('payment URI amount is outside the supported transaction range');
    if (goriToGnr(amount) !== raw) throw new Error('payment URI amount must use canonical decimal notation');
    amountGori = amount;
  }
  return { address, amountGori };
}

export function buildPaymentUri(network: NetworkName, address: string, amount?: string): string {
  const check = validateAddress(address, network);
  if (!check.valid) throw new Error(check.error ?? 'invalid address');
  let uri = `genoryn:${address}?v=1&network=${network}`;
  if (amount && amount.trim()) {
    const atomic = parseGnr(amount);
    if (atomic <= 0n || atomic > MAX_MONEY_PER_TX) throw new Error('request amount outside transaction range');
    uri += `&amount=${goriToGnr(atomic)}`;
  }
  parsePaymentUri(uri, network);
  return uri;
}

/** On-chain username rules from docs/protocol/usernames.md (v0.4.0). */
export function canonicalUsername(input: string): { ok: true; name: string } | { ok: false; error: string } {
  const name = input.trim().replace(/^@/, '').toLowerCase();
  if (name.length < 3 || name.length > 32) return { ok: false, error: 'names have 3–32 characters' };
  if (!/^[a-z]/.test(name)) return { ok: false, error: 'names must begin with a letter' };
  if (!/^[a-z0-9_]+$/.test(name)) return { ok: false, error: 'only lowercase ASCII letters, digits and underscores' };
  if ((RESERVED_NAMES as readonly string[]).includes(name)) return { ok: false, error: 'this username is reserved' };
  return { ok: true, name };
}
