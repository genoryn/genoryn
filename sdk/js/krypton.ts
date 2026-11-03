/**
 * @genoryn/krypton: TypeScript client for the Genoryn Krypton node API.
 * Works in browsers, Node.js 18+, Deno and Bun (uses fetch). Zero dependencies.
 *
 *   const k = new KryptonClient("https://genoryn.io");
 *   const info = await k.info();
 *   const blocks = await k.blocks(10);
 */

export const COIN = 100_000_000n;

export interface NodeInfo {
  network: "mainnet" | "testnet" | "devnet";
  chain_id: string;
  version: string;
  height: number;
  tip: string;
  genesis: string;
  difficulty: string;
  supply: number;
  max_supply: number;
  next_subsidy: number;
  peers: number;
  mempool: number;
  hashrate: number;
}

export interface BlockSummary { height: number; hash: string; timestamp: number; difficulty: string; txs: number; size: number; reward: number }
export interface TxStatus { txid: string; status: "pending" | "confirmed"; height?: number; confirmations?: number; fee?: number }
export type ChainEvent = { type: "block"; height: number; hash: string; txs: number; reorg_depth: number } | { type: "tx"; txid: string };

export class KryptonError extends Error {
  constructor(public status: number, message: string) { super(message); }
}

export class KryptonClient {
  constructor(public base: string, private timeoutMs = 20_000) { this.base = base.replace(/\/$/, ""); }

  private async req<T>(path: string, init?: RequestInit): Promise<T> {
    const ctl = new AbortController();
    const t = setTimeout(() => ctl.abort(), this.timeoutMs);
    try {
      const r = await fetch(this.base + path, { ...init, signal: ctl.signal });
      const body = await r.json().catch(() => ({}));
      if (!r.ok) throw new KryptonError(r.status, (body as { error?: string }).error ?? r.statusText);
      return body as T;
    } finally { clearTimeout(t); }
  }

  info() { return this.req<NodeInfo>("/v1/info"); }
  supply() { return this.req<Record<string, number | string>>("/v1/supply"); }
  blocks(count = 20, from?: number) { return this.req<BlockSummary[]>(`/v1/blocks?count=${count}${from != null ? `&from=${from}` : ""}`); }
  block(id: number | string) { return this.req<Record<string, unknown>>(`/v1/block/${id}`); }
  tx(txid: string) { return this.req<TxStatus>(`/v1/tx/${txid}`); }
  nullifier(hex: string) { return this.req<{ spent_height: number | null }>(`/v1/nullifier/${hex}`); }
  validateAddress(addr: string) { return this.req<{ valid: boolean; network?: string; short?: string; error?: string }>(`/v1/validate/${encodeURIComponent(addr)}`); }
  resolveShort(short: string) { return this.req<{ address: string }>(`/v1/address/${short}`); }

  /** Submits a serialized transaction (built by a wallet; proofs are made client-side). */
  async submit(raw: Uint8Array) {
    return this.req<{ txid: string }>("/v1/tx", { method: "POST", body: raw as unknown as BodyInit, headers: { "content-type": "application/octet-stream" } });
  }

  /** JSON-RPC 2.0 call (getblockcount, getblock, sendrawtransaction, ...). */
  async rpc<T = unknown>(method: string, params: unknown[] = []): Promise<T> {
    const r = await this.req<{ result?: T; error?: { message: string } }>("/rpc", {
      method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
    });
    if (r.error) throw new KryptonError(400, r.error.message);
    return r.result as T;
  }

  /** Live events (new blocks, new pool transactions). Returns a function that closes the socket. */
  subscribe(onEvent: (e: ChainEvent) => void): () => void {
    const ws = new WebSocket(this.base.replace(/^http/, "ws") + "/v1/ws");
    ws.onmessage = (m) => { try { const e = JSON.parse(String(m.data)); if (e.type) onEvent(e); } catch { /* info frame */ } };
    return () => ws.close();
  }
}

/** "12.5" -> 1250000000n (exact, no floating point). */
export function parseAmount(s: string): bigint {
  const m = /^\s*(\d*)(?:\.(\d{0,8}))?\s*$/.exec(s);
  if (!m || (m[1] === "" && !m[2])) throw new Error("invalid amount");
  return BigInt(m[1] || "0") * COIN + BigInt((m[2] || "").padEnd(8, "0") || "0");
}

export function formatAmount(gori: bigint | number): string {
  const v = BigInt(gori);
  return `${v / COIN}.${(v % COIN).toString().padStart(8, "0")}`;
}

/** Bitcoin-style schedule: 5 GNR, halving every 2,100,000 blocks. */
export function subsidy(height: number): bigint {
  if (height === 0) return 0n;
  const halvings = Math.floor((height - 1) / 2_100_000);
  return halvings >= 64 ? 0n : (5n * COIN) >> BigInt(halvings);
}
