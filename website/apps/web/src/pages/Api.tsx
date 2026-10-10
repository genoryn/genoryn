import { createSignal, For, Show } from 'solid-js';
import { Icon } from '~/components/Icon';
import { Callout, CodeBlock, PageHero, SectionHead } from '~/components/ui';
import { stats } from '~/lib/live';

interface Endpoint {
  path: string;
  desc: string;
  example?: () => string;
  stream?: boolean;
  download?: boolean;
}

const ENDPOINTS: Endpoint[] = [
  { path: '/api/v1/stats', desc: 'Chain summary: height, difficulty, hashrate, supply, peers, mempool, totals; stale, last_updated and error tell whether the node answered' },
  { path: '/api/v1/health', desc: 'Server and indexer health: node_online, stale, last_updated (unix seconds), error, indexed height' },
  { path: '/api/v1/blocks?limit=5', desc: 'Latest blocks (limit ≤ 100, paginate with before=height)' },
  { path: '/api/v1/block/{height|hash}', desc: 'Block detail with header fields and transactions', example: () => `/api/v1/block/${Math.max(1, (stats()?.height ?? 2) - 1)}` },
  { path: '/api/v1/block/{id}/raw', desc: 'Full raw block JSON exactly as served by the node', download: true, example: () => `/api/v1/block/${Math.max(1, (stats()?.height ?? 2) - 1)}/raw` },
  { path: '/api/v1/txs?limit=10&kind=transfer', desc: 'Latest transactions; kind=transfer for shielded transfers only' },
  { path: '/api/v1/tx/{txid}', desc: 'Transaction detail with every action (nullifier, cmx, rk, ciphertext sizes)' },
  { path: '/api/v1/tx/{txid}/raw', desc: 'Raw transaction JSON', download: true },
  { path: '/api/v1/nullifier/{hex}', desc: 'Has this nullifier been revealed on chain? In which transaction?' },
  { path: '/api/v1/commitment/{hex}', desc: 'Where was this note commitment created, and how many confirmations?' },
  { path: '/api/v1/search?q={query}', desc: 'Classify a height, hash, txid, nullifier, commitment, address or @name', example: () => `/api/v1/search?q=${stats()?.height ?? 1}` },
  { path: '/api/v1/supply', desc: 'Live issuance and reward fields from the node, including early_issued and current_early_reward (decimal strings in gori)' },
  { path: '/api/v1/emission/{height}', desc: 'Exact consensus supply schedule at any height (computed)', example: () => '/api/v1/emission/2102901' },
  { path: '/api/v1/emission-curve', desc: 'Issued supply by protocol year, 0–40' },
  { path: '/api/v1/charts/{metric}?limit=240', desc: 'Series for difficulty, blocktime, txs, fees, size or actions', example: () => '/api/v1/charts/difficulty?limit=20' },
  { path: '/api/v1/genesis', desc: 'Deterministic genesis record, canonical preimage and hash' },
  { path: '/api/v1/peers', desc: 'Peer summary of the genoryn.io node (addresses masked)' },
  { path: '/api/v1/mining', desc: 'Live stats of the genoryn.io CPU miner plus network summary' },
  { path: '/api/v1/names/{name}', desc: 'On-chain username: registered record, pending registration, availability and burned price', example: () => '/api/v1/names/satoshi' },
  { path: '/api/v1/names/owner/{owner}', desc: 'Names registered by one Ed25519 owner key (64 hex)' },
  { path: '/api/v1/address/{address}', desc: 'Offline Bech32m validation. Never returns a balance (there is none to return)' },
  { path: '/api/v1/seed', desc: 'The genoryn.io seed: clearnet address and seed lists; onion address and Quick-mode source once published (null until then)' },
  { path: '/api/v1/releases', desc: 'GitHub releases with assets and SHA-256 checksums (cached 10 min)' },
  { path: '/api/v1/export/blocks.csv?from=1&to=1000', desc: 'CSV export of block headers and totals (≤ 10,000 blocks)', download: true },
  { path: '/api/v1/export/txs.csv?from=1&to=1000', desc: 'CSV export of transactions (≤ 10,000 blocks)', download: true },
  { path: '/api/v1/live', desc: 'Server-Sent Events: stats, block, miner, reorg', stream: true },
];

function EndpointRow(props: { e: Endpoint }) {
  const [out, setOut] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  const url = () => (props.e.example ? props.e.example() : props.e.path);
  const runnable = () => !props.e.stream && !props.e.download && !url().includes('{');
  const run = async () => {
    setBusy(true);
    try {
      const r = await fetch(url());
      const text = await r.text();
      try {
        setOut(JSON.stringify(JSON.parse(text), null, 2).slice(0, 12_000));
      } catch {
        setOut(text.slice(0, 4000));
      }
    } catch (e) {
      setOut(String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <details class="endpoint">
      <summary>
        <span class="method">GET</span>
        <span class="path">{props.e.path}</span>
        <span class="desc">{props.e.desc}</span>
      </summary>
      <div class="body">
        <p class="muted mt-2" style={{ 'font-size': '14px' }}>{props.e.desc}</p>
        <div class="row">
          <code class="break">{url()}</code>
          <Show when={runnable()}>
            <button class="btn sm" type="button" onClick={run} disabled={busy()}>
              {busy() ? 'Running…' : 'Try it'}
            </button>
          </Show>
          <Show when={props.e.download && !url().includes('{')}>
            <a class="btn sm" href={url()} download="">
              <Icon name="download" size={14} /> Download
            </a>
          </Show>
        </div>
        <Show when={out()}>
          <div class="code mt-2">
            <div class="code-head"><span>response</span></div>
            <pre>{out()}</pre>
          </div>
        </Show>
      </div>
    </details>
  );
}

export default function Api() {
  return (
    <>
      <PageHero eyebrow="api.genoryn.dev" title={<>Public API</>} lead="Free, read-only JSON for everything public on the GENORYN chain — plus a live event stream and CSV exports. No key, no signup, CORS enabled." origin="Base URL: https://genoryn.io/api/v1" />
      <section class="section tight">
        <div class="container">
          <div class="grid cols-3">
            <div class="card flat"><h3>Exact numbers</h3><p class="muted">All monetary values are decimal strings in gori (1 GNR = 10¹² gori). Parse them with a big-integer type, never a float.</p></div>
            <div class="card flat"><h3>Fair use</h3><p class="muted">600 requests per minute per IP. For heavy use, run your own node with <code>--public-rpc</code> — it's the same data.</p></div>
            <div class="card flat"><h3>Privacy by design</h3><p class="muted">There is no balance, history or rich-list endpoint, and there never will be. Those don't exist on chain.</p></div>
          </div>
          <div class="mt-4">
            <For each={ENDPOINTS}>{(e) => <EndpointRow e={e} />}</For>
          </div>
        </div>
      </section>
      <section class="section">
        <div class="container">
          <SectionHead eyebrow="Examples" title={<>Use it anywhere</>} />
          <div class="grid cols-2">
            <CodeBlock title="curl" code={`$ curl -s https://genoryn.io/api/v1/stats | jq .height\n$ curl -s "https://genoryn.io/api/v1/blocks?limit=3" | jq '.blocks[].hash'\n$ curl -N https://genoryn.io/api/v1/live   # live events`} />
            <CodeBlock
              title="JavaScript"
              lang="js"
              code={`const es = new EventSource('https://genoryn.io/api/v1/live');
es.addEventListener('block', (e) => {
  const b = JSON.parse(e.data);
  console.log('new block', b.height, b.hash);
});
// gori -> GNR, exactly
const gnr = (g) => (BigInt(g) / 10n ** 12n) + '.' + (BigInt(g) % 10n ** 12n).toString().padStart(12, '0');`}
            />
          </div>
        </div>
      </section>
      <section class="section">
        <div class="container">
          <SectionHead eyebrow="Node RPC v1" title={<>Talk to a node directly</>} lead="Every genorynd exposes versioned HTTP RPC. Default Mainnet 1.0 ports: 29711 admin (loopback only, HTTP 401 without the token), 29712 optional public read-only." />
          <div class="table-wrap">
            <table class="data">
              <thead><tr><th>Endpoint</th><th>Access</th><th>Description</th></tr></thead>
              <tbody>
                <tr><td class="mono">GET /v1/info</td><td>public</td><td class="muted">Network, chain, genesis, height, best hash, difficulty, target-implied hashrate, peers, mempool, database size</td></tr>
                <tr><td class="mono">GET /v1/genesis</td><td>public</td><td class="muted">Height-zero record, canonical preimage and hash</td></tr>
                <tr><td class="mono">GET /v1/supply</td><td>public</td><td class="muted">Integer issuance and reward fields as decimal strings (normal, early and treasury, plus early_blocks)</td></tr>
                <tr><td class="mono">GET /v1/blocks?from=H&amp;limit=1</td><td>public</td><td class="muted">One full encrypted block starting at height H</td></tr>
                <tr><td class="mono">GET /v1/names?name=</td><td>public</td><td class="muted">On-chain username status: registered (receiver, owner, sequence, heights, txid, burned), pending, available, price</td></tr>
                <tr><td class="mono">GET /v1/names/owner?owner=</td><td>public</td><td class="muted">Names of one owner key plus pending registrations</td></tr>
                <tr><td class="mono">GET /v1/usernames?username=</td><td>public</td><td class="muted">Legacy local signed receiver bundle (compatibility only)</td></tr>
                <tr><td class="mono">GET /v1/mempool/nullifiers</td><td>public</td><td class="muted">Nullifiers spent by transactions waiting in the mempool (lets wallets avoid double-spending their own pending coins)</td></tr>
                <tr><td class="mono">POST /v1/relay</td><td>public</td><td class="muted">Hand over a signed shielded transaction for relay — used by Quick-mode wallets over Tor</td></tr>
                <tr><td class="mono">GET /v1/peers</td><td>admin</td><td class="muted">Connected peers</td></tr>
                <tr><td class="mono">GET /v1/network</td><td>admin</td><td class="muted">Network mode, Tor status and onion address (never exposed on public RPC)</td></tr>
                <tr><td class="mono">POST /v1/mine/template</td><td>admin + token</td><td class="muted">Genuine reward proof and block template for a receiving address</td></tr>
                <tr><td class="mono">POST /v1/mine/submit</td><td>admin + token</td><td class="muted">Submit a full candidate block</td></tr>
                <tr><td class="mono">POST /v1/transactions</td><td>admin + token</td><td class="muted">Submit a signed private transaction (success = mempool only)</td></tr>
                <tr><td class="mono">POST /v1/usernames</td><td>admin + token</td><td class="muted">Register a signed ownership bundle</td></tr>
              </tbody>
            </table>
          </div>
          <Callout class="mt-3">Admin routes require <code>Authorization: Bearer &lt;admin.token&gt;</code>, bind to loopback only and are omitted entirely in public mode. Private keys and passwords never belong in RPC requests.</Callout>
        </div>
      </section>
    </>
  );
}
