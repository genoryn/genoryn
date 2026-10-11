import { describe, expect, it } from 'vitest';
import { CHALLENGE_PREFIX, PROOF_CODE_PREFIX, RESPONSE_PREFIX, cleanCode, extractProofJson, proofClaims } from '../src/proof';

const txid = 'ab'.repeat(32);
// 20,000 GNR in gori: above 2^53, so a JavaScript number would round it.
const json = `{"version":1,"network":"Mainnet","txid":"${txid}","action":1,"recipient":"gnr1example","amount":20000000000000000,"rho":"00","rseed":"00","cmx":"00","sender_key":"11"}`;
const code = PROOF_CODE_PREFIX + Buffer.from(json).toString('base64url');

describe('payment proof formats', () => {
  it('accepts the JSON itself, unchanged', () => {
    expect(extractProofJson(`\uFEFF  ${json}\n`)).toBe(json);
  });

  it('decodes the one-line code, broken across lines and followed by words', () => {
    const broken = `Here is my proof:\n${code.slice(0, 50)}\n${code.slice(50, 120)} ${code.slice(120)}\nthanks!`;
    expect(extractProofJson(broken)).toBe(json);
  });

  it('finds the code inside PDF text', () => {
    expect(extractProofJson(`%PDF-1.4\n1 0 obj << /Subject (${code}) >>\nendobj`)).toBe(json);
  });

  it('refuses text without a proof', () => {
    expect(() => extractProofJson('GNRPROOF1-@@@')).toThrow(/No payment proof/);
    expect(() => extractProofJson('{not json')).toThrow();
  });

  it('reads the amount exactly, beyond 2^53', () => {
    const claims = proofClaims(json);
    expect(claims.amountAtomic).toBe('20000000000000000');
    expect(claims).toMatchObject({ txid, action: 1, recipient: 'gnr1example', fromSender: true });
  });

  it('rejects proofs of another version or with a malformed txid', () => {
    expect(() => proofClaims(json.replace('"version":1', '"version":2'))).toThrow(/version 1/);
    expect(() => proofClaims(json.replace(txid, 'zz'))).toThrow();
  });

  it('collects exactly a challenge or response code', () => {
    const body = 'A'.repeat(175);
    expect(cleanCode(`Please answer: ${CHALLENGE_PREFIX}${body.slice(0, 80)}\n${body.slice(80)} — thanks`, CHALLENGE_PREFIX)).toBe(CHALLENGE_PREFIX + body);
    expect(cleanCode(`${RESPONSE_PREFIX}${'B'.repeat(134)}`, RESPONSE_PREFIX)).toBeNull();
    expect(cleanCode(`${RESPONSE_PREFIX}${'B'.repeat(140)}`, RESPONSE_PREFIX)).toBe(RESPONSE_PREFIX + 'B'.repeat(135));
  });
});
