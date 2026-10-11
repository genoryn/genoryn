/**
 * Payment proofs as the GENORYN wallet shares them: the proof JSON, the one-line
 * code (`GNRPROOF1-` + base64url of the compact JSON) or a wallet-made PDF that
 * carries the code. Same rules as the wallet's Verify payment page.
 *
 * Amounts in a proof are whole gori and may exceed 2^53, so the proof is kept
 * as its original JSON text and never re-serialized from a JavaScript object.
 */

export const PROOF_CODE_PREFIX = 'GNRPROOF1-';
export const CHALLENGE_PREFIX = 'GNRCHAL1-';
export const RESPONSE_PREFIX = 'GNRRESP1-';
/** Proof files may be PDFs, which are larger than the JSON they carry. */
export const MAX_PROOF_FILE_BYTES = 256 * 1024;
/** A proof's JSON is far smaller; anything bigger is not a proof. */
export const MAX_PROOF_JSON_BYTES = 8 * 1024;
/** Symbols after the prefix: base64url of 127 + 4 and 97 + 4 bytes. */
const CODE_SYMBOLS: Record<string, number> = { [CHALLENGE_PREFIX]: 175, [RESPONSE_PREFIX]: 135 };

const ALPHABET = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_';

function base64urlDecode(text: string): Uint8Array | null {
  if (text.length % 4 === 1) return null;
  const out: number[] = [];
  for (let i = 0; i < text.length; i += 4) {
    const chunk = text.slice(i, i + 4);
    let n = 0;
    for (let j = 0; j < chunk.length; j++) {
      const v = ALPHABET.indexOf(chunk[j]!);
      if (v < 0) return null;
      n |= v << (18 - 6 * j);
    }
    out.push((n >>> 16) & 255);
    if (chunk.length > 2) out.push((n >>> 8) & 255);
    if (chunk.length > 3) out.push(n & 255);
  }
  return Uint8Array.from(out);
}

function utf8(bytes: Uint8Array): string | null {
  try {
    return new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  } catch {
    return null;
  }
}

function isJsonObject(text: string): boolean {
  try {
    const value: unknown = JSON.parse(text);
    return typeof value === 'object' && value !== null && !Array.isArray(value);
  } catch {
    return false;
  }
}

const isCodeSymbol = (c: string) => /^[A-Za-z0-9_-]$/.test(c);

/**
 * The proof JSON text inside anything a person may paste or load: the JSON
 * itself, the text code (line breaks and spaces inside it are ignored), or the
 * text of a PDF made by the wallet.
 */
export function extractProofJson(input: string): string {
  const trimmed = input.replace(/^\uFEFF/, '').trim();
  if (trimmed.startsWith('{')) {
    if (!isJsonObject(trimmed)) throw new Error('This looks like JSON but is not a valid payment proof.');
    return trimmed;
  }
  let best: string | null = null;
  for (let start = input.indexOf(PROOF_CODE_PREFIX); start >= 0; start = input.indexOf(PROOF_CODE_PREFIX, start + 1)) {
    // Runs of code symbols separated by line breaks or spaces; words typed
    // after the code are dropped by trying the longest valid prefix.
    const runs = [''];
    for (const c of input.slice(start + PROOF_CODE_PREFIX.length)) {
      if (isCodeSymbol(c)) runs[runs.length - 1] += c;
      else if (/\s/.test(c)) {
        if (runs[runs.length - 1]) runs.push('');
      } else break;
    }
    for (let count = runs.length; count >= 1; count--) {
      const bytes = base64urlDecode(runs.slice(0, count).join(''));
      const json = bytes && utf8(bytes);
      if (json && json.startsWith('{') && isJsonObject(json)) {
        if (!best || json.length > best.length) best = json;
        break;
      }
    }
  }
  if (!best) throw new Error('No payment proof found. Paste the proof code (GNRPROOF1-…), the proof JSON, or load the .json or .pdf file.');
  return best;
}

/** What a proof says about itself, read from its text without losing precision. */
export interface ProofClaims {
  network: string;
  txid: string;
  action: number;
  recipient: string;
  amountAtomic: string;
  fromSender: boolean;
}

export function proofClaims(json: string): ProofClaims {
  if (json.length > MAX_PROOF_JSON_BYTES) throw new Error('This is too large to be a payment proof.');
  const value = JSON.parse(json) as Record<string, unknown>;
  const amount = /"amount"\s*:\s*(\d{1,20})\s*[,}]/.exec(json)?.[1];
  const txid = typeof value.txid === 'string' ? value.txid : '';
  if (value.version !== 1 || typeof value.network !== 'string' || !/^[0-9a-f]{64}$/.test(txid) || !Number.isInteger(value.action) || typeof value.recipient !== 'string' || !amount) {
    throw new Error('This is not a GENORYN payment proof (version 1).');
  }
  return {
    network: value.network,
    txid,
    action: value.action as number,
    recipient: value.recipient,
    amountAtomic: amount,
    fromSender: value.sender_key !== undefined && value.sender_key !== null,
  };
}

/**
 * Challenge and response codes are pasted from messages, sometimes broken
 * across lines: collect exactly the code's own symbols after its prefix.
 */
export function cleanCode(text: string, prefix: typeof CHALLENGE_PREFIX | typeof RESPONSE_PREFIX): string | null {
  const at = text.indexOf(prefix);
  if (at < 0) return null;
  const wanted = CODE_SYMBOLS[prefix]!;
  let code = '';
  for (const c of text.slice(at + prefix.length)) {
    if (code.length === wanted) break;
    if (/\s/.test(c)) continue;
    if (!isCodeSymbol(c)) break;
    code += c;
  }
  return code.length === wanted ? prefix + code : null;
}

/** The node's answer about a proof (fields of the wallet's `verify_proof`). */
export interface ProofCheck {
  outcome: 'payment_verified' | 'not_on_chain' | 'reorganized';
  txid: string;
  action: number;
  chain_tip: number;
  recipient?: string;
  amount_atomic?: string;
  memo?: string;
  created_by_sender?: boolean;
  sender_auth_key?: string | null;
  sender_evidence?: 'fresh_authentication_available' | 'origin_disclosure_only' | 'none';
  block_height?: number;
  confirmations?: number;
  reorg_risk?: string;
  recipient_matches?: boolean;
  amount_matches?: boolean;
}

export interface ChallengeIssued {
  challenge: string;
  expires: number;
}

export interface SenderCheck {
  outcome: 'sender_verified' | 'legacy_sender_evidence_only';
  txid: string;
  action: number;
  amount_atomic: string;
  recipient: string;
  block_height: number;
  confirmations: number;
  reorg_risk: string;
  sender_auth_key?: string;
  meaning?: string;
  note?: string;
}
