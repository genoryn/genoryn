import Fastify, { type FastifyReply, type FastifyRequest } from 'fastify';
import compress from '@fastify/compress';
import cors from '@fastify/cors';
import rateLimit from '@fastify/rate-limit';
import fastifyStatic from '@fastify/static';
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { config } from './config';
import { openDb, queries } from './db';
import { LiveHub } from './live';
import { Indexer } from './indexer';
import { MinerWatcher } from './miner';
import { NodeTracker } from './nodes';
import { Crawler } from './crawler';
import { registerApi } from './api';
import { ProofVerifier, registerProofApi } from './proofs';
import { rpc } from './rpc';
import { headTags, resolveMeta, robots, sitemap } from './seo';

const db = openDb(config.dbPath);
const q = queries(db);
const hub = new LiveHub();
const indexer = new Indexer(db, q, hub);
const miner = new MinerWatcher(
  config.minerUnit,
  config.minerAddress,
  hub,
  config.minerSsh ? { target: config.minerSsh, key: config.minerSshKey, knownHosts: config.minerSshKnownHosts } : null,
);
const nodes = new NodeTracker(db, config.peerSources);
const crawler = new Crawler(db, (host) => nodes.recordHost(host), ['187.7.72.137', '2.24.162.163']);
indexer.nodeStats = () => {
  const c = crawler.current();
  return { ...nodes.stats(), reachable_now: c.reachable_now, reachable_24h: c.reachable_24h, crawl_addresses: c.addresses_known, last_crawl: c.last_crawl };
};

const app = Fastify({
  logger: { level: process.env.LOG_LEVEL ?? 'warn' },
  trustProxy: true,
  bodyLimit: 16 * 1024,
});

const clientIp = (req: { headers: Record<string, unknown>; ip: string }) => String(req.headers['cf-connecting-ip'] ?? req.ip);

await app.register(compress, { global: true, threshold: 1024, encodings: ['br', 'gzip'] });
await app.register(rateLimit, {
  global: true,
  max: 600,
  timeWindow: '1 minute',
  keyGenerator: clientIp,
  allowList: (req) => req.url.startsWith('/assets/') || req.url.startsWith('/fonts/'),
});
await app.register(cors, { origin: (origin, cb) => cb(null, true), methods: ['GET', 'HEAD'] });

const CSP = [
  "default-src 'self'",
  "script-src 'self'",
  "style-src 'self' 'unsafe-inline'",
  "img-src 'self' data: blob:",
  "font-src 'self' data:",
  "connect-src 'self'",
  "worker-src 'self' blob:",
  "object-src 'none'",
  "base-uri 'self'",
  "form-action 'self'",
  "frame-ancestors 'none'",
].join('; ');

app.addHook('onRequest', async (req, reply) => {
  const [path = '/', query] = req.url.split('?');
  if (path.length > 1 && path.endsWith('/') && !path.startsWith('/api/')) {
    return reply.redirect(path.replace(/\/+$/, '') + (query ? `?${query}` : ''), 301);
  }
});

app.addHook('onSend', async (req, reply, payload) => {
  reply.header('x-content-type-options', 'nosniff');
  reply.header('referrer-policy', 'strict-origin-when-cross-origin');
  reply.header('permissions-policy', 'camera=(), microphone=(), geolocation=(), interest-cohort=()');
  const type = String(reply.getHeader('content-type') ?? '');
  if (type.includes('text/html')) {
    reply.header('content-security-policy', CSP);
    reply.header('x-frame-options', 'DENY');
    reply.header('cache-control', 'no-cache');
  } else if (req.url.startsWith('/assets/') || req.url.startsWith('/fonts/')) {
    reply.header('cache-control', 'public, max-age=31536000, immutable');
  } else if (req.url.startsWith('/api/')) {
    if (!reply.getHeader('cache-control')) reply.header('cache-control', 'no-store');
  } else if (!reply.getHeader('cache-control')) {
    reply.header('cache-control', 'public, max-age=3600');
  }
  return payload;
});

registerApi(app, { q, indexer, hub, miner });
registerProofApi(app, new ProofVerifier());

app.get('/sitemap.xml', async (_req, reply) => reply.type('application/xml; charset=utf-8').send(sitemap(q)));
app.get('/robots.txt', async (_req, reply) => reply.type('text/plain; charset=utf-8').send(robots()));

// ---- HTML pages: prerendered content + per-request SEO head --------------------------------
const htmlCache = new Map<string, string | null>();
function prerendered(path: string): string | null {
  if (htmlCache.has(path)) return htmlCache.get(path)!;
  const file = path === '/' ? join(config.webDist, 'index.html') : join(config.webDist, path.slice(1), 'index.html');
  const html = !path.includes('..') && existsSync(file) ? readFileSync(file, 'utf8') : null;
  htmlCache.set(path, html);
  return html;
}
const shell = () => prerendered('/_shell') ?? '<!doctype html><html><head><!--app-head--></head><body><div id="app"></div></body></html>';

const renderPage = async (req: FastifyRequest, reply: FastifyReply) => {
  if (req.method !== 'GET' && req.method !== 'HEAD') return reply.code(405).send({ error: 'method not allowed' });
  let path: string;
  try {
    path = decodeURIComponent(req.url.split('?')[0] ?? '/');
  } catch {
    return reply.code(400).type('text/plain').send('bad request');
  }
  if (path.startsWith('/api/')) return reply.code(404).send({ error: 'unknown API route' });
  if (/\.[a-z0-9]{2,5}$/i.test(path)) return reply.code(404).type('text/plain').send('not found');
  const meta = resolveMeta(path, q);
  const page = (meta.status === 200 && prerendered(meta.path)) || shell();
  const html = page.replace('<!--app-head-->', headTags(meta));
  return reply.code(meta.status).type('text/html; charset=utf-8').send(html);
};

app.get('/', renderPage);

if (existsSync(config.webDist)) {
  await app.register(fastifyStatic, { root: config.webDist, prefix: '/', index: false, redirect: false, wildcard: true, serveDotFiles: false, list: false });
} else {
  app.log.warn(`web build not found at ${config.webDist}`);
}

app.setNotFoundHandler(renderPage);

app.setErrorHandler((error, _req, reply) => {
  const err = error as { statusCode?: number; message: string };
  const status = err.statusCode ?? 500;
  if (status >= 500) app.log.error(err);
  reply.code(status).send({ error: status >= 500 ? 'internal error' : err.message });
});

indexer.start();
miner.start();
nodes.start();
crawler.start();
void (async () => {
  try {
    const { execFile } = await import('node:child_process');
    // Peer tags are salted per node run, so only the current run's log is comparable.
    const pid: string = await new Promise((res) => execFile('systemctl', ['show', 'genorynd', '-p', 'MainPID', '--value'], (_e, so) => res(String(so ?? '').trim())));
    if (!/^[1-9]\d*$/.test(pid)) return;
    const out: string = await new Promise((res) => execFile('journalctl', [`_PID=${pid}`, '--no-pager', '-o', 'json', '--output-fields=MESSAGE,__REALTIME_TIMESTAMP'], { maxBuffer: 256 * 1024 * 1024 }, (_e, so) => res(so ?? '')));
    const entries: Array<{ tag: string; ts: number; onion: boolean }> = [];
    for (const line of out.split('\n')) {
      if (!line.includes('peer connected')) continue;
      try {
        const j = JSON.parse(line) as { MESSAGE: string; __REALTIME_TIMESTAMP: string };
        const f = (JSON.parse(j.MESSAGE) as { fields?: { peer?: string; link?: string } }).fields;
        if (f?.peer) entries.push({ tag: f.peer, ts: Math.floor(Number(j.__REALTIME_TIMESTAMP) / 1e6), onion: /onion|tor/i.test(f.link ?? '') });
      } catch {
        /* not a JSON log line */
      }
    }
    const connected = (await rpc.peers().catch(() => [])).length;
    nodes.backfill(entries, connected);
  } catch {
    /* journal unavailable */
  }
})();

await app.listen({ host: config.host, port: config.port });
console.log(JSON.stringify({ t: new Date().toISOString(), src: 'server', msg: 'listening', host: config.host, port: config.port, db: config.dbPath, web: config.webDist }));

const shutdown = async () => {
  indexer.stop();
  miner.stop();
  await app.close();
  db.close();
  process.exit(0);
};
process.on('SIGTERM', shutdown);
process.on('SIGINT', shutdown);
