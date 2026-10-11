import { spawn } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import type { FastifyInstance, FastifyReply } from 'fastify';
import {
  CHALLENGE_PREFIX,
  MAX_PROOF_JSON_BYTES,
  RESPONSE_PREFIX,
  cleanCode,
  isHex64,
  parseGnr,
  proofClaims,
  validateAddress,
} from '@genoryn/shared';
import { config } from './config';

/**
 * Payment-proof checks with the wallet's own code (`genoryn-wallet-cli
 * --json-stdio`) against this server's node. The CLI keeps the validated
 * chain next to a valueless verifier wallet, so a check only fetches new
 * blocks. Proofs are never stored or logged.
 */
const MAX_QUEUE = 16;
const TIMEOUT_MS = 90_000;
const MAX_OUTPUT_BYTES = 1024 * 1024;

type CliAnswer = { ok: true; result: Record<string, unknown> } | { ok: false; error: string };

class Unavailable extends Error {}

export class ProofVerifier {
  private queue: Promise<unknown> = Promise.resolve();
  private waiting = 0;
  private password: string | null = null;

  get enabled(): boolean {
    return !!config.verifierCli && existsSync(config.verifierCli) && existsSync(config.verifierWallet) && existsSync(config.verifierPasswordFile);
  }

  private secret(): string {
    this.password ??= readFileSync(config.verifierPasswordFile, 'utf8').trim();
    return this.password;
  }

  /** Runs one CLI request; requests run one at a time because they share the stored chain. */
  run(op: string, fields: Record<string, unknown>, rawProof?: string): Promise<CliAnswer> {
    if (!this.enabled) return Promise.reject(new Unavailable('payment-proof verification is not set up on this server'));
    if (this.waiting >= MAX_QUEUE) return Promise.reject(new Unavailable('the verifier is busy; try again in a minute'));
    this.waiting++;
    const job = this.queue.then(() => this.spawnCli(op, fields, rawProof));
    this.queue = job.catch(() => undefined).finally(() => this.waiting--);
    return job;
  }

  private spawnCli(op: string, fields: Record<string, unknown>, rawProof?: string): Promise<CliAnswer> {
    const request = JSON.stringify({
      ...fields,
      op,
      network: config.network,
      wallet_path: config.verifierWallet,
      password: this.secret(),
      // Loopback reads without the public RPC's rate limit; no token is given, so nothing can be changed.
      rpc_url: config.adminRpc,
    });
    // The proof text is spliced in unchanged so amounts above 2^53 gori stay exact.
    const body = rawProof === undefined ? request : `${request.slice(0, -1)},"proof":${rawProof}}`;
    return new Promise((resolve, reject) => {
      const child = spawn(config.verifierCli!, ['--json-stdio'], { stdio: ['pipe', 'pipe', 'ignore'], timeout: TIMEOUT_MS });
      const chunks: Buffer[] = [];
      let size = 0;
      child.stdout.on('data', (chunk: Buffer) => {
        size += chunk.length;
        if (size > MAX_OUTPUT_BYTES) child.kill();
        else chunks.push(chunk);
      });
      child.on('error', () => reject(new Unavailable('the verifier could not be started')));
      child.on('close', () => {
        const line = Buffer.concat(chunks).toString('utf8').trim().split('\n').pop() ?? '';
        try {
          resolve(JSON.parse(line) as CliAnswer);
        } catch {
          reject(new Unavailable('the verifier did not answer in time'));
        }
      });
      child.stdin.on('error', () => undefined);
      child.stdin.end(body);
    });
  }
}

/** Only what the page shows; the verifier wallet's own view is never returned. */
const PUBLIC_FIELDS = [
  'outcome', 'txid', 'action', 'chain_tip', 'recipient', 'amount_atomic', 'memo', 'created_by_sender', 'sender_auth_key',
  'sender_evidence', 'block_height', 'confirmations', 'reorg_risk', 'recipient_matches', 'amount_matches',
  'previously_confirmed_at', 'expires', 'challenge', 'meaning', 'note',
] as const;
const publicView = (result: Record<string, unknown>) => Object.fromEntries(PUBLIC_FIELDS.filter((k) => k in result).map((k) => [k, result[k]]));

/** The proof JSON from a request body, checked to be one proof-shaped object. */
function proofText(body: unknown): string {
  const text = (body as { proof?: unknown })?.proof;
  if (typeof text !== 'string' || !text.trim()) throw new Error('paste a payment proof');
  if (text.length > MAX_PROOF_JSON_BYTES) throw new Error('this is too large to be a payment proof');
  const claims = proofClaims(text.trim());
  if (claims.network.toLowerCase() !== config.network) throw new Error(`this payment proof is for another network (${claims.network})`);
  return text.trim();
}

export function registerProofApi(app: FastifyInstance, verifier: ProofVerifier) {
  const limited = { config: { rateLimit: { max: 15, timeWindow: '1 minute' } } };

  const answer = async (reply: FastifyReply, work: () => Promise<CliAnswer>) => {
    try {
      const out = await work();
      if (!out.ok) return reply.code(422).send({ error: out.error });
      return reply.code(200).send(publicView(out.result));
    } catch (e) {
      if (e instanceof Unavailable) return reply.code(503).send({ error: e.message });
      return reply.code(400).send({ error: (e as Error).message });
    }
  };

  app.get('/api/v1/proof/status', async () => ({ enabled: verifier.enabled, network: config.network }));

  app.post('/api/v1/proof/verify', limited, async (req, reply) =>
    answer(reply, async () => {
      const raw = proofText(req.body);
      const body = req.body as { expected_recipient?: unknown; expected_amount?: unknown };
      const fields: Record<string, unknown> = { structured_outcomes: true };
      if (typeof body.expected_recipient === 'string' && body.expected_recipient.trim()) {
        const check = validateAddress(body.expected_recipient.trim(), config.network);
        if (!check.valid) throw new Error(`the address to check is not valid: ${check.error}`);
        fields.expected_recipient = body.expected_recipient.trim();
      }
      if (typeof body.expected_amount === 'string' && body.expected_amount.trim()) {
        fields.expected_amount_atomic = parseGnr(body.expected_amount).toString();
      }
      return verifier.run('verify_proof', fields, raw);
    }),
  );

  app.post('/api/v1/proof/challenge', limited, async (req, reply) =>
    answer(reply, async () => {
      const claims = proofClaims(proofText(req.body));
      if (!isHex64(claims.txid)) throw new Error('invalid transaction id');
      return verifier.run('create_payment_challenge', {
        txid: claims.txid,
        action: claims.action,
        recipient: claims.recipient,
        amount_atomic: claims.amountAtomic,
      });
    }),
  );

  app.post('/api/v1/proof/sender', limited, async (req, reply) =>
    answer(reply, async () => {
      const raw = proofText(req.body);
      const body = req.body as { challenge?: unknown; response?: unknown };
      const challenge = typeof body.challenge === 'string' ? cleanCode(body.challenge, CHALLENGE_PREFIX) : null;
      const response = typeof body.response === 'string' ? cleanCode(body.response, RESPONSE_PREFIX) : null;
      if (!challenge) throw new Error('the challenge code is missing or incomplete');
      if (!response) throw new Error(`paste the sender's whole response code (${RESPONSE_PREFIX}…)`);
      return verifier.run('verify_payment_challenge', { challenge, response }, raw);
    }),
  );
}
