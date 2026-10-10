import { createResource, For, Show } from 'solid-js';
import { GENORYN_IO_ONION, GENORYN_IO_SEED, MAINNET_ONION_SEEDS_FILE, SEED1_ONION, MAINNET_SEED, MAINNET_SEEDS_FILE, NETWORKS, REPO_URL, formatNumber } from '@genoryn/shared';
import { Callout, CodeBlock, ExtLink, Feature, PageHero, SectionHead, StaleNote, Stat } from '~/components/ui';
import { api } from '~/lib/api';
import { rawStats, stats } from '~/lib/live';

const P2P = NETWORKS.mainnet.p2pPort;

export default function Nodes() {
  const [seed] = createResource(() => api<{ clearnet: string; onion: string | null; network_mode: string; light_wallet_source: string | null }>('/seed'));
  const [peers] = createResource(() => api<{ count: number; inbound: number; outbound: number; peers: Array<{ address: string; inbound: boolean; height: number }>; unavailable?: boolean }>('/peers'));
  const livePeers = () => (peers() && !peers()!.unavailable ? peers() : undefined);
  return (
    <>
      <PageHero eyebrow="nodes.genoryn.dev" title={<>Run a node. Be the network.</>} lead="Every GENORYN node validates everything independently. A node with a public P2P port also helps new nodes find the network." origin={`genoryn.io runs a public Mainnet 1.0 seed on port ${P2P}`} />
      <section class="section tight">
        <div class="container">
          <StaleNote class="mb-2" />
          <div class="stat-strip">
            <Stat label="genoryn.io node" live={!!rawStats() && !rawStats()!.stale} value={!rawStats() ? '—' : rawStats()!.stale ? 'unreachable' : 'online'} sub={rawStats()?.node_version} />
            <Stat label="Height" value={stats() ? formatNumber(stats()!.height) : '—'} />
            <Stat
              label="Peers"
              value={livePeers() ? String(livePeers()!.count) : stats() ? String(stats()!.peers) : '—'}
              sub={livePeers() ? `${livePeers()!.inbound} in · ${livePeers()!.outbound} out` : stats() ? `${stats()!.inbound} in · ${stats()!.outbound} out` : undefined}
            />
            <Stat label="P2P port" value={String(P2P)} sub="Mainnet 1.0, TCP" />
          </div>
        </div>
      </section>
      <section class="section">
        <div class="container">
          <SectionHead eyebrow="Why run one" title={<>Verify, don't trust.</>} />
          <div class="grid cols-3">
            <Feature icon="shield" title="Full validation">Your node checks proof of work, Halo 2 proofs, signatures, anchors and nullifier uniqueness for every block. No one can feed you a fake chain.</Feature>
            <Feature icon="eyeOff" title="Private scanning">In Full mode your wallet scans its notes against your own node. No third party learns which blocks you look at.</Feature>
            <Feature icon="globe" title="Help the network">A reachable node accepts inbound peers and relays blocks and transactions, making the network harder to partition.</Feature>
          </div>
        </div>
      </section>
      <section class="section">
        <div class="container">
          <div class="grid cols-2">
            <div>
              <SectionHead eyebrow="Setup" title={<>A public node in three commands</>} />
              <CodeBlock
                code={`# 1. Download and verify the 1.0.0 Linux release
$ sha256sum -c genoryn-1.0.0-mainnet-linux-x86_64.zip.sha256 && unzip genoryn-1.0.0-mainnet-linux-x86_64.zip
# 2. Open the P2P port
$ sudo ufw allow ${P2P}/tcp
# 3. Start the node (Mainnet 1.0 by default, listens on 0.0.0.0:${P2P})
$ ./genorynd --data-dir /var/lib/genoryn/mainnet-1.0
# 4. Optional: also serve Tor users as an onion seed and Quick-mode source
$ ./genorynd --network-mode hybrid --onion-service --public-rpc`}
              />
              <p class="muted mt-2" style={{ 'font-size': '14.5px' }}>Run it under systemd with <code>Restart=always</code> for an always-on seed. Nodes behind NAT or a VPN still take part through outbound connections. Nodes from releases before 1.0.0 belong to the retired network and cannot connect.</p>
            </div>
            <div>
              <SectionHead eyebrow="systemd" title={<>Keep it running</>} />
              <CodeBlock
                title="/etc/systemd/system/genorynd.service"
                lang="ini"
                code={`[Unit]
Description=GENORYN Mainnet 1.0 full node
After=network-online.target

[Service]
User=genoryn
ExecStart=/opt/genoryn/genorynd --network mainnet --data-dir /var/lib/genoryn/mainnet-1.0
Restart=always
NoNewPrivileges=true
ProtectSystem=strict
ReadWritePaths=/var/lib/genoryn/mainnet-1.0

[Install]
WantedBy=multi-user.target`}
              />
            </div>
          </div>
        </div>
      </section>
      <section class="section">
        <div class="container">
          <SectionHead eyebrow="genoryn.io seed" title={<>Our node, for your node.</>} lead="The genoryn.io server runs an always-on, non-mining Mainnet 1.0 full node in Hybrid mode. Its onion service, for Tor-only installations, is published at launch." />
          <div class="grid cols-2">
            <div class="card">
              <h3>Clearnet seed</h3>
              <p class="mono strong break" style={{ 'font-size': '14px' }}>{seed()?.clearnet ?? `${GENORYN_IO_SEED}:${P2P}`}</p>
              <p class="muted" style={{ 'font-size': '14px' }}>Listed in {MAINNET_SEEDS_FILE}. Explicit: <code>--peer {GENORYN_IO_SEED}:{P2P}</code></p>
            </div>
            <div class="card">
              <h3>Onion seed (Tor)</h3>
              <Show when={seed()?.onion} fallback={<p class="muted">Published at launch in {MAINNET_ONION_SEEDS_FILE}; the address appears here once it is published.</p>}>
                <p class="mono strong break" style={{ 'font-size': '13px' }}>{seed()!.onion}</p>
                <p class="muted" style={{ 'font-size': '14px' }}>
                  Listed in {MAINNET_ONION_SEEDS_FILE} and read automatically by Tor-only nodes.
                  <Show when={seed()!.light_wallet_source}> Quick-mode wallets can use <code class="break">{seed()!.light_wallet_source}</code> as a source.</Show>
                </p>
              </Show>
            </div>
          </div>
        </div>
      </section>

      <section class="section">
        <div class="container">
          <SectionHead eyebrow="Discovery" title={<>How nodes meet</>} lead="Seeds only introduce peers. They are never consensus authorities: every block and transaction is validated independently." />
          <div class="grid cols-2">
            <div class="table-wrap">
              <table class="data">
                <tbody>
                  <tr><td class="strong">Explicit peers</td><td class="muted"><code>--peer host:port</code> or <code>[p2p] peers</code> — tried first</td></tr>
                  <tr><td class="strong">Peer cache</td><td class="muted">Up to 128 validated peers persisted across restarts</td></tr>
                  <tr><td class="strong">Peer exchange</td><td class="muted">Connected nodes share public addresses they know</td></tr>
                  <tr><td class="strong">Built-in seeds</td><td class="muted"><code>apps/genoryn/src/seeds.rs</code></td></tr>
                  <tr><td class="strong">Published lists</td><td class="muted"><ExtLink href={`${REPO_URL}/blob/main/${MAINNET_SEEDS_FILE}`}>{MAINNET_SEEDS_FILE}</ExtLink> and <ExtLink href={`${REPO_URL}/blob/main/${MAINNET_ONION_SEEDS_FILE}`}>{MAINNET_ONION_SEEDS_FILE}</ExtLink>, fetched over HTTPS (5 s, 16 KiB, 16 names)</td></tr>
                  <tr><td class="strong">Operator DNS seeds</td><td class="muted"><code>--dns-seed</code>; <code>--no-default-seeds</code> opts out of built-ins</td></tr>
                  <tr><td class="strong">Limits</td><td class="muted">16 connections, 8 outbound, 4 inbound per IP, per-netgroup discovery caps, rate-limited GetBlocks/Ping/GetPeers</td></tr>
                  <tr><td class="strong">Seeds</td><td class="muted">{MAINNET_SEED} (seed 1, the project server — also the only known miner at launch) and {GENORYN_IO_SEED} (genoryn.io, non-mining full node), both on port {P2P}; onion seeds for Tor-only nodes: <span class="mono break">{SEED1_ONION}</span> (seed 1) and <span class="mono break">{GENORYN_IO_ONION}</span> (genoryn.io), P2P {P2P}, public RPC {NETWORKS.mainnet.publicRpcPort}. Both seeds are run by one operator</td></tr>
                  <tr><td class="strong">Tor</td><td class="muted">Hybrid and Tor-only modes with the built-in client; AddrV2 carries onion addresses between upgraded nodes</td></tr>
                </tbody>
              </table>
            </div>
            <div>
              <h3>Connected to genoryn.io</h3>
              <div class="table-wrap">
                <table class="data">
                  <thead><tr><th>Peer (masked)</th><th>Direction</th><th class="num">Height</th></tr></thead>
                  <tbody>
                    <For
                      each={livePeers()?.peers ?? []}
                      fallback={<tr><td colspan="3" class="center muted" style={{ padding: '24px' }}>{peers.loading ? 'Loading…' : peers()?.unavailable || peers.error ? 'Peer list unavailable · node unreachable' : 'No peers'}</td></tr>}
                    >
                      {(p) => (<tr><td class="mono">{p.address}</td><td><span class="badge">{p.inbound ? 'inbound' : 'outbound'}</span></td><td class="num mono">{formatNumber(p.height)}</td></tr>)}
                    </For>
                  </tbody>
                </table>
              </div>
              <Callout class="mt-2">Whoever controls a seed list can steer a new node's first contacts — which is why cached peers, peer exchange, explicit peers and full validation stay independent of any seed.</Callout>
            </div>
          </div>
        </div>
      </section>
    </>
  );
}
