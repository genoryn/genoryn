/** Bech32m (BIP-350) decoding for GENORYN receiving identities (gnrd / gnrt / gnr). */
import { NETWORKS, type NetworkName } from './params';

const CHARSET = 'qpzry9x8gf2tvdw0s3jn54khce6mua7l';
const BECH32M_CONST = 0x2bc830a3;
const GEN = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3];

function polymod(values: number[]): number {
  let chk = 1;
  for (const v of values) {
    const top = chk >>> 25;
    chk = ((chk & 0x1ffffff) << 5) ^ v;
    for (let i = 0; i < 5; i++) if ((top >>> i) & 1) chk ^= GEN[i]!;
  }
  return chk >>> 0;
}

function hrpExpand(hrp: string): number[] {
  const out: number[] = [];
  for (let i = 0; i < hrp.length; i++) out.push(hrp.charCodeAt(i) >> 5);
  out.push(0);
  for (let i = 0; i < hrp.length; i++) out.push(hrp.charCodeAt(i) & 31);
  return out;
}

function convertBits(data: number[], from: number, to: number, pad: boolean): number[] | null {
  let acc = 0;
  let bits = 0;
  const out: number[] = [];
  const maxv = (1 << to) - 1;
  for (const v of data) {
    if (v < 0 || v >> from) return null;
    acc = (acc << from) | v;
    bits += from;
    while (bits >= to) {
      bits -= to;
      out.push((acc >> bits) & maxv);
    }
  }
  if (pad) {
    if (bits) out.push((acc << (to - bits)) & maxv);
  } else if (bits >= from || (acc << (to - bits)) & maxv) {
    return null;
  }
  return out;
}

export interface DecodedAddress {
  hrp: string;
  network: NetworkName | null;
  bytes: Uint8Array;
}

export function bech32mDecode(input: string, maxLength = 1023): DecodedAddress {
  const s = input.trim();
  if (s.length > maxLength) throw new Error('address is too long');
  if (s !== s.toLowerCase() && s !== s.toUpperCase()) throw new Error('mixed-case address');
  const lower = s.toLowerCase();
  const pos = lower.lastIndexOf('1');
  if (pos < 1 || pos + 7 > lower.length) throw new Error('missing separator or checksum');
  const hrp = lower.slice(0, pos);
  for (const c of hrp) {
    const code = c.charCodeAt(0);
    if (code < 33 || code > 126) throw new Error('invalid prefix character');
  }
  const data: number[] = [];
  for (const c of lower.slice(pos + 1)) {
    const d = CHARSET.indexOf(c);
    if (d === -1) throw new Error(`invalid character "${c}"`);
    data.push(d);
  }
  if (polymod([...hrpExpand(hrp), ...data]) !== BECH32M_CONST) throw new Error('checksum mismatch (not a valid Bech32m string)');
  const bytes = convertBits(data.slice(0, -6), 5, 8, false);
  if (!bytes) throw new Error('invalid padding');
  const network = (Object.values(NETWORKS).find((n) => n.hrp === hrp)?.name ?? null) as NetworkName | null;
  return { hrp, network, bytes: Uint8Array.from(bytes) };
}

export function bech32mEncode(hrp: string, bytes: Uint8Array): string {
  const data = convertBits(Array.from(bytes), 8, 5, true)!;
  const values = [...hrpExpand(hrp), ...data, 0, 0, 0, 0, 0, 0];
  const mod = polymod(values) ^ BECH32M_CONST;
  const checksum = Array.from({ length: 6 }, (_, i) => (mod >> (5 * (5 - i))) & 31);
  return hrp + '1' + [...data, ...checksum].map((d) => CHARSET[d]).join('');
}

/** Raw Orchard-style receiver: 11-byte diversifier + 32-byte pk_d. */
export const RECEIVER_BYTES = 43;

export interface AddressCheck {
  valid: boolean;
  network: NetworkName | null;
  hrp: string | null;
  byteLength: number | null;
  error: string | null;
}

export function validateAddress(input: string, expected?: NetworkName): AddressCheck {
  try {
    const d = bech32mDecode(input);
    if (!d.network) return { valid: false, network: null, hrp: d.hrp, byteLength: d.bytes.length, error: `unknown prefix "${d.hrp}" (expected gnrd, gnrt or gnr)` };
    if (expected && d.network !== expected) return { valid: false, network: d.network, hrp: d.hrp, byteLength: d.bytes.length, error: `address belongs to ${d.network}, not ${expected}` };
    if (d.bytes.length !== RECEIVER_BYTES) return { valid: false, network: d.network, hrp: d.hrp, byteLength: d.bytes.length, error: `receiver must be ${RECEIVER_BYTES} bytes, got ${d.bytes.length}` };
    return { valid: true, network: d.network, hrp: d.hrp, byteLength: d.bytes.length, error: null };
  } catch (e) {
    return { valid: false, network: null, hrp: null, byteLength: null, error: (e as Error).message };
  }
}
