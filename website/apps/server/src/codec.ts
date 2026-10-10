/**
 * Canonical consensus encoding (bincode 1, fixint, little endian) and GENORYN domain hashing,
 * re-implemented from genoryn-core codec.rs / block.rs / privacy.rs so the explorer can derive
 * block hashes and transaction IDs that the RPC JSON does not carry.
 */
import { createHash } from 'node:crypto';

export type Bytes = number[];
export type NetworkTag = 'Devnet' | 'Testnet' | 'Mainnet';
const NETWORK_INDEX: Record<NetworkTag, number> = { Devnet: 0, Testnet: 1, Mainnet: 2 };

export interface Header {
  version: number;
  network: NetworkTag;
  height: number;
  previous: Bytes;
  timestamp: number;
  difficulty: number;
  pow_seed: Bytes;
  nonce: string; // u64 kept as decimal string to avoid precision loss
  body_hash: Bytes;
  notes_root: Bytes;
  nullifiers_root: Bytes;
}
export interface TxContext {
  protocol: number;
  network: NetworkTag;
  kind: number;
  height: number;
  expires: number;
  nonce: Bytes;
}
export interface ActionWire {
  nullifier: Bytes;
  rk: Bytes;
  cmx: Bytes;
  ephemeral_key: Bytes;
  encrypted_note: Bytes;
  outgoing_ciphertext: Bytes;
  value_commitment: Bytes;
  signature: Bytes;
}
export interface BundleWire {
  actions: ActionWire[];
  flags: number;
  value_balance: string; // i64 as decimal string
  anchor: Bytes;
  proof: Bytes;
  binding_signature: Bytes;
}
export interface ShieldedTx {
  context: TxContext;
  bundle: BundleWire;
}
export interface Body {
  miner: ShieldedTx;
  treasury: ShieldedTx | null;
  transactions: ShieldedTx[];
}
export interface Block {
  header: Header;
  body: Body;
}

/** Parse RPC JSON while keeping 64-bit integers that may exceed 2^53 exact. */
export function parseBlocksJson(text: string): Block[] {
  const safe = text.replace(/"(nonce|value_balance)":(-?\d+)/g, '"$1":"$2"');
  return JSON.parse(safe) as Block[];
}

class Writer {
  private chunks: Buffer[] = [];
  length = 0;
  private push(b: Buffer) {
    this.chunks.push(b);
    this.length += b.length;
  }
  u8(v: number) {
    const b = Buffer.alloc(1);
    b.writeUInt8(v);
    this.push(b);
  }
  u16(v: number) {
    const b = Buffer.alloc(2);
    b.writeUInt16LE(v);
    this.push(b);
  }
  u32(v: number) {
    const b = Buffer.alloc(4);
    b.writeUInt32LE(v);
    this.push(b);
  }
  u64(v: number | bigint | string) {
    const b = Buffer.alloc(8);
    b.writeBigUInt64LE(BigInt(v));
    this.push(b);
  }
  i64(v: number | bigint | string) {
    const b = Buffer.alloc(8);
    b.writeBigInt64LE(BigInt(v));
    this.push(b);
  }
  fixed(bytes: Bytes, len: number) {
    if (bytes.length !== len) throw new Error(`expected ${len} bytes, got ${bytes.length}`);
    this.push(Buffer.from(bytes));
  }
  vec(bytes: Bytes) {
    this.u64(bytes.length);
    this.push(Buffer.from(bytes));
  }
  out(): Buffer {
    return Buffer.concat(this.chunks, this.length);
  }
}

function network(w: Writer, n: NetworkTag) {
  const idx = NETWORK_INDEX[n];
  if (idx === undefined) throw new Error(`unknown network ${n}`);
  w.u32(idx);
}

export function encodeHeader(h: Header): Buffer {
  const w = new Writer();
  w.u16(h.version);
  network(w, h.network);
  w.u64(h.height);
  w.fixed(h.previous, 32);
  w.u64(h.timestamp);
  w.u64(h.difficulty);
  w.fixed(h.pow_seed, 32);
  w.u64(h.nonce);
  w.fixed(h.body_hash, 32);
  w.fixed(h.notes_root, 32);
  w.fixed(h.nullifiers_root, 32);
  return w.out();
}

function writeTx(w: Writer, tx: ShieldedTx) {
  const c = tx.context;
  w.u16(c.protocol);
  network(w, c.network);
  w.u8(c.kind);
  w.u64(c.height);
  w.u64(c.expires);
  w.fixed(c.nonce, 32);
  const b = tx.bundle;
  w.u64(b.actions.length);
  for (const a of b.actions) {
    w.fixed(a.nullifier, 32);
    w.fixed(a.rk, 32);
    w.fixed(a.cmx, 32);
    w.fixed(a.ephemeral_key, 32);
    w.vec(a.encrypted_note);
    w.vec(a.outgoing_ciphertext);
    w.fixed(a.value_commitment, 32);
    w.vec(a.signature);
  }
  w.u8(b.flags);
  w.i64(b.value_balance);
  w.fixed(b.anchor, 32);
  w.vec(b.proof);
  w.vec(b.binding_signature);
}

export function encodeTx(tx: ShieldedTx): Buffer {
  const w = new Writer();
  writeTx(w, tx);
  return w.out();
}

export function encodeBody(body: Body): Buffer {
  const w = new Writer();
  writeTx(w, body.miner);
  if (body.treasury === null) w.u8(0);
  else {
    w.u8(1);
    writeTx(w, body.treasury);
  }
  w.u64(body.transactions.length);
  for (const t of body.transactions) writeTx(w, t);
  return w.out();
}

export function domainHash(domain: string, bytes: Buffer): Buffer {
  const d = Buffer.from(domain, 'ascii');
  const len = (n: number) => {
    const b = Buffer.alloc(8);
    b.writeBigUInt64LE(BigInt(n));
    return b;
  };
  return createHash('sha256').update(len(d.length)).update(d).update(len(bytes.length)).update(bytes).digest();
}

export const hex = (b: Bytes | Buffer) => Buffer.from(b).toString('hex');
export const blockHash = (h: Header) => domainHash('GENORYN/block/v1', encodeHeader(h)).toString('hex');
export const bodyHash = (b: Body) => domainHash('GENORYN/body/v1', encodeBody(b)).toString('hex');
export const txId = (tx: ShieldedTx) => domainHash('GENORYN/transaction/v1', encodeTx(tx)).toString('hex');
export const blockSize = (b: Block) => encodeHeader(b.header).length + encodeBody(b.body).length;
