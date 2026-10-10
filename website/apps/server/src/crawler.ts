/**
 * GENORYN network crawler (peer protocol v1). For each reachable node it performs the normal
 * handshake (Hello ↔ Hello), asks once for its known addresses (GetPeers → Peers) and closes.
 * Nodes behind NAT cannot be reached by any crawler; they are counted from inbound links instead.
 */
import { createHash, randomBytes } from 'node:crypto';
import net from 'node:net';
import { MAINNET_GENESIS_HASH, NETWORKS } from '@genoryn/shared';
import type { DB } from './db';
import { rpc } from './rpc';

// Mainnet 1.0: 0x47 0x4e 0x52 (0x10 + network id 3).
export const MAGIC_MAINNET = Buffer.from([0x47, 0x4e, 0x52, 0x10 + NETWORKS.mainnet.id]);
const NETWORK_MAINNET = 2; // bincode variant index of Network::Mainnet
export const GENESIS = Buffer.from(MAINNET_GENESIS_HASH, 'hex');
const P2P_PORT = NETWORKS.mainnet.p2pPort;
const MAX_FRAME = 2_100_000;
const CONCURRENCY = 24;
const TIMEOUT_MS = 8000;
const REVISIT_MS = 30 * 60 * 1000;
const MSG = { Hello: 0, GetPeers: 5, Peers: 6 } as const;

interface HelloInfo {
  version: number;
  height: number;
  listenPort: number;
}

function u32(n: number) {
  const b = Buffer.alloc(4);
  b.writeUInt32LE(n);
  return b;
}
function u64(n: number | bigint) {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(BigInt(n));
  return b;
}
function frame(payload: Buffer) {
  return Buffer.concat([MAGIC_MAINNET, u32(payload.length), payload]);
}

function helloFrame(height: number, tip: Buffer) {
  const listen = Buffer.concat([u32(0), Buffer.from([0, 0, 0, 0]), Buffer.from([0, 0])]); // V4 0.0.0.0:0 — advertise nothing
  const hello = Buffer.concat([
    u32(MSG.Hello),
    Buffer.from([1, 0]), // version u16 = 1
    u32(NETWORK_MAINNET),
    GENESIS,
    randomBytes(32), // fresh identity per connection
    listen,
    u64(height),
    tip,
  ]);
  return frame(hello);
}

function readSocketAddr(b: Buffer, o: number): { host: string; port: number; next: number } {
  const variant = b.readUInt32LE(o);
  o += 4;
  if (variant === 0) {
    const host = `${b[o]}.${b[o + 1]}.${b[o + 2]}.${b[o + 3]}`;
    const port = b.readUInt16LE(o + 4);
    return { host, port, next: o + 6 };
  }
  const parts: string[] = [];
  for (let i = 0; i < 16; i += 2) parts.push(b.readUInt16BE(o + i).toString(16));
  const port = b.readUInt16LE(o + 16);
  return { host: parts.join(':'), port, next: o + 18 };
}

function parseHello(p: Buffer): HelloInfo | null {
  if (p.readUInt32LE(0) !== MSG.Hello) return null;
  let o = 4;
  const version = p.readUInt16LE(o);
  o += 2;
  const network = p.readUInt32LE(o);
  o += 4;
  const genesis = p.subarray(o, o + 32);
  o += 32 + 32; // genesis + id
  if (network !== NETWORK_MAINNET || !genesis.equals(GENESIS)) return null;
  const listen = readSocketAddr(p, o);
  o = listen.next;
  const height = Number(p.readBigUInt64LE(o));
  return { version, height, listenPort: listen.port };
}

function parsePeers(p: Buffer): Array<{ host: string; port: number }> {
  let o = 4;
  const n = Number(p.readBigUInt64LE(o));
  o += 8;
  const out = [];
  for (let i = 0; i < Math.min(n, 1000); i++) {
    const a = readSocketAddr(p, o);
    o = a.next;
    out.push({ host: a.host, port: a.port });
  }
  return out;
}

function isPublicV4(host: string) {
  const m = host.match(/^(\d+)\.(\d+)\.(\d+)\.(\d+)$/);
  if (!m) return false;
  const [a, b] = [Number(m[1]), Number(m[2])];
  if (a === 10 || a === 127 || a === 0 || a >= 224) return false;
  if (a === 172 && b >= 16 && b <= 31) return false;
  if (a === 192 && b === 168) return false;
  if (a === 169 && b === 254) return false;
  if (a === 100 && b >= 64 && b <= 127) return false;
  return true;
}

/** One polite visit: handshake, one GetPeers, close. */
function visit(host: string, port: number, height: number, tip: Buffer): Promise<{ hello: HelloInfo; peers: Array<{ host: string; port: number }> } | null> {
  return new Promise((resolve) => {
    const sock = net.connect({ host, port });
    let buf = Buffer.alloc(0);
    let hello: HelloInfo | null = null;
    let done = false;
    const finish = (r: { hello: HelloInfo; peers: Array<{ host: string; port: number }> } | null) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      sock.destroy();
      resolve(r);
    };
    const timer = setTimeout(() => finish(hello ? { hello, peers: [] } : null), TIMEOUT_MS);
    sock.on('connect', () => sock.write(helloFrame(height, tip)));
    sock.on('error', () => finish(hello ? { hello, peers: [] } : null));
    sock.on('close', () => finish(hello ? { hello, peers: [] } : null));
    sock.on('data', (chunk: Buffer) => {
      buf = Buffer.concat([buf, chunk]);
      while (buf.length >= 8) {
        if (!buf.subarray(0, 4).equals(MAGIC_MAINNET)) return finish(null);
        const len = buf.readUInt32LE(4);
        if (len > MAX_FRAME) return finish(null);
        if (buf.length < 8 + len) break;
        const payload = buf.subarray(8, 8 + len);
        buf = buf.subarray(8 + len);
        try {
          const kind = payload.readUInt32LE(0);
          if (!hello) {
            hello = parseHello(payload);
            if (!hello) return finish(null);
            sock.write(frame(u32(MSG.GetPeers)));
          } else if (kind === MSG.Peers) {
            return finish({ hello, peers: parsePeers(payload) });
          }
        } catch {
          return finish(hello ? { hello, peers: [] } : null);
        }
      }
    });
  });
}

export interface CrawlStats {
  reachable_now: number;
  reachable_24h: number;
  addresses_known: number;
  last_crawl: number | null;
  crawl_seconds: number | null;
  versions: Record<string, number>;
  heights: { max: number; median: number };
}

export class Crawler {
  private stats: CrawlStats = { reachable_now: 0, reachable_24h: 0, addresses_known: 0, last_crawl: null, crawl_seconds: null, versions: {}, heights: { max: 0, median: 0 } };
  private running = false;

  constructor(private db: DB, private recordNode: (host: string, source: 'crawl') => void, private extraSeeds: string[]) {
    db.exec(`CREATE TABLE IF NOT EXISTS crawl_addr (addr TEXT PRIMARY KEY, last_try INTEGER NOT NULL DEFAULT 0, last_ok INTEGER, height INTEGER, version INTEGER, fails INTEGER NOT NULL DEFAULT 0)`);
  }

  current() {
    return this.stats;
  }

  private addAddr = (host: string, port: number) => {
    if (!isPublicV4(host) || port <= 0) return;
    this.db.prepare('INSERT OR IGNORE INTO crawl_addr (addr) VALUES (?)').run(`${host}:${port}`);
  };

  async crawl() {
    if (this.running) return;
    this.running = true;
    const started = Date.now();
    try {
      const info = await rpc.info();
      const tip = Buffer.from(info.best_hash, 'hex');
      for (const s of this.extraSeeds) this.addAddr(s, P2P_PORT);
      try {
        for (const p of await rpc.peers()) {
          const host = p.address.replace(/:\d+$/, '');
          this.addAddr(host, P2P_PORT);
        }
      } catch {
        /* node busy */
      }
      const due = () =>
        this.db
          .prepare('SELECT addr FROM crawl_addr WHERE last_try < ? AND fails < 6 ORDER BY last_ok IS NULL, last_try LIMIT 500')
          .pluck()
          .all(Date.now() - REVISIT_MS) as string[];
      const seen = new Set<string>();
      let queue = due();
      while (queue.length) {
        const batch = queue.filter((a) => !seen.has(a));
        batch.forEach((a) => seen.add(a));
        for (let i = 0; i < batch.length; i += CONCURRENCY) {
          await Promise.all(
            batch.slice(i, i + CONCURRENCY).map(async (addr) => {
              const [host, portStr] = [addr.slice(0, addr.lastIndexOf(':')), addr.slice(addr.lastIndexOf(':') + 1)];
              const r = await visit(host, Number(portStr), info.height, tip);
              const now = Date.now();
              if (r) {
                this.db.prepare('UPDATE crawl_addr SET last_try = ?, last_ok = ?, height = ?, version = ?, fails = 0 WHERE addr = ?').run(now, now, r.hello.height, r.hello.version, addr);
                this.recordNode(host, 'crawl');
                for (const p of r.peers) this.addAddr(p.host, p.port);
              } else {
                this.db.prepare('UPDATE crawl_addr SET last_try = ?, fails = fails + 1 WHERE addr = ?').run(now, addr);
              }
            }),
          );
        }
        queue = due().filter((a) => !seen.has(a));
      }
      const day = Date.now() - 86_400_000;
      const rows = this.db.prepare('SELECT height, version FROM crawl_addr WHERE last_ok >= ?').all(Date.now() - REVISIT_MS - 5 * 60_000) as Array<{ height: number; version: number }>;
      const heights = rows.map((r) => r.height).sort((a, b) => a - b);
      const versions: Record<string, number> = {};
      for (const r of rows) versions[`v${r.version}`] = (versions[`v${r.version}`] ?? 0) + 1;
      this.stats = {
        reachable_now: rows.length,
        reachable_24h: this.db.prepare('SELECT COUNT(*) FROM crawl_addr WHERE last_ok >= ?').pluck().get(day) as number,
        addresses_known: this.db.prepare('SELECT COUNT(*) FROM crawl_addr').pluck().get() as number,
        last_crawl: Math.floor(Date.now() / 1000),
        crawl_seconds: Math.round((Date.now() - started) / 1000),
        versions,
        heights: { max: heights.at(-1) ?? 0, median: heights[Math.floor(heights.length / 2)] ?? 0 },
      };
    } catch {
      /* node unavailable; retry next round */
    } finally {
      this.running = false;
    }
  }

  start() {
    setTimeout(() => void this.crawl(), 15_000).unref();
    setInterval(() => void this.crawl(), 10 * 60 * 1000).unref();
  }
}

export const hashHost = (salt: string, host: string) => createHash('sha256').update(salt).update(host.toLowerCase()).digest('hex').slice(0, 32);
