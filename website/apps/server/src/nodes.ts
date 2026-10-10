import { createHash, randomBytes } from 'node:crypto';
import type { DB } from './db';

/**
 * Counts distinct nodes that connect to the genoryn.io node over time. Only salted hashes of
 * the IP (ports change behind NAT) or onion name are stored, never the address itself.
 */
export class NodeTracker {
  private salt: string;
  private upsert;
  private count;
  private online: { connected: number; known: number; perSource: Record<string, number> } = { connected: 0, known: 0, perSource: {} };

  constructor(private db: DB, private sources: Array<{ name: string; url: string }>) {
    db.exec(`CREATE TABLE IF NOT EXISTS node_seen (id TEXT PRIMARY KEY, first_seen INTEGER NOT NULL, last_seen INTEGER NOT NULL, onion INTEGER NOT NULL DEFAULT 0);
             CREATE INDEX IF NOT EXISTS node_seen_last ON node_seen(last_seen);`);
    const row = db.prepare("SELECT v FROM meta WHERE k = 'node_salt'").pluck().get() as string | undefined;
    this.salt = row ?? randomBytes(32).toString('hex');
    if (!row) db.prepare("INSERT INTO meta (k, v) VALUES ('node_salt', ?)").run(this.salt);
    this.upsert = db.prepare(`INSERT INTO node_seen (id, first_seen, last_seen, onion) VALUES (?, ?, ?, ?)
      ON CONFLICT(id) DO UPDATE SET last_seen = excluded.last_seen`);
    this.count = db.prepare('SELECT COUNT(*) FROM node_seen WHERE last_seen >= ?').pluck();
  }

  /** A node found by the network crawler (reachable, listening). */
  recordHost(host: string) {
    const t = Math.floor(Date.now() / 1000);
    this.upsert.run(this.id(host), t, t, 0);
  }

  private id(address: string): string {
    const host = address.replace(/^\[|\]?:\d+$/g, '').replace(/:\d+$/, '').toLowerCase();
    return createHash('sha256').update(this.salt).update(host).digest('hex').slice(0, 32);
  }

  async poll() {
    const t = Math.floor(Date.now() / 1000);
    const connectedNow = new Set<string>();
    const perSource: Record<string, number> = {};
    let known = 0;
    for (const src of this.sources) {
      try {
        const res = await fetch(`${src.url}/v1/peers`, { signal: AbortSignal.timeout(5000) });
        if (!res.ok) throw new Error(String(res.status));
        const peers = (await res.json()) as Array<{ address: string; network?: string; link?: string }>;
        perSource[src.name] = peers.length;
        const tx = this.db.transaction(() => {
          for (const p of peers) {
            // Onion-service inbound links all arrive from loopback and cannot be told apart.
            if (/^127\.|^\[?::1\]?/.test(p.address)) continue;
            const onion = p.network === 'onion' || /onion|tor/i.test(String(p.link ?? '')) ? 1 : 0;
            const id = this.id(p.address);
            connectedNow.add(id);
            this.upsert.run(id, t, t, onion);
          }
        });
        tx();
        // Loopback onion links are distinct peers too; count them per source without dedupe.
        const loop = peers.filter((p) => /^127\.|^\[?::1\]?/.test(p.address)).length;
        for (let i = 0; i < loop; i++) connectedNow.add(`${src.name}:onion:${i}`);
      } catch {
        perSource[src.name] = -1;
      }
      try {
        const res = await fetch(`${src.url}/v1/network`, { signal: AbortSignal.timeout(5000) });
        if (res.ok) known += ((await res.json()) as { known_addresses?: { total?: number } }).known_addresses?.total ?? 0;
      } catch {
        /* optional */
      }
    }
    this.online = { connected: connectedNow.size, known, perSource };
  }

  /** One-time import of distinct peers already logged by the node, minus those now connected. */
  backfill(entries: Array<{ tag: string; ts: number; onion: boolean }>, connectedNow: number) {
    if (this.db.prepare("SELECT v FROM meta WHERE k = 'node_backfill'").pluck().get()) return;
    const byTag = new Map<string, { first: number; last: number; onion: boolean }>();
    for (const e of entries) {
      const cur = byTag.get(e.tag);
      if (!cur) byTag.set(e.tag, { first: e.ts, last: e.ts, onion: e.onion });
      else cur.last = Math.max(cur.last, e.ts);
    }
    // Drop the most recently seen tags: those are the peers the live poll already counts.
    const sorted = [...byTag.entries()].sort((a, b) => a[1].last - b[1].last);
    const keep = sorted.slice(0, Math.max(0, sorted.length - connectedNow));
    const tx = this.db.transaction(() => {
      for (const [tag, v] of keep) this.upsert.run('log:' + createHash('sha256').update(this.salt).update(tag).digest('hex').slice(0, 32), v.first, v.last, v.onion ? 1 : 0);
      this.db.prepare("INSERT INTO meta (k, v) VALUES ('node_backfill', ?)").run(String(keep.length));
    });
    tx();
  }

  start() {
    void this.poll();
    setInterval(() => void this.poll(), 30_000).unref();
  }

  stats() {
    const now = Math.floor(Date.now() / 1000);
    return {
      nodes_connected: this.online.connected,
      nodes_24h: this.count.get(now - 86_400) as number,
      nodes_7d: this.count.get(now - 7 * 86_400) as number,
      nodes_all: this.count.get(0) as number,
      known_addresses: this.online.known,
      seeds_online: Object.values(this.online.perSource).filter((n) => n >= 0).length,
      per_seed: this.online.perSource,
    };
  }
}
