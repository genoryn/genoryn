import { A } from '@solidjs/router';
import { MAINNET_ONION_SEEDS_FILE, REPO_URL } from '@genoryn/shared';
import { Toc } from '~/components/Toc';
import { Callout, CodeBlock, ExtLink, PageHero } from '~/components/ui';

const TOC: Array<[string, string]> = [
  ['layers', 'Two privacy layers'],
  ['modes', 'Network modes'],
  ['builtin', 'Tor is built in'],
  ['onion', 'Onion services'],
  ['broadcast', 'Payment broadcast'],
  ['light', 'Quick mode'],
  ['leaks', 'Leak protections'],
  ['limits', 'What Tor cannot do'],
  ['operators', 'For node operators'],
];

export default function Privacy() {
  return (
    <div class="editorial">
      <PageHero
        eyebrow="Network privacy · Tor"
        title={<>Hide your coins. <em>Hide your IP.</em></>}
        lead="Shielded notes keep amounts, senders and receivers off the chain. A Tor client is built into every GENORYN node and wallet, so peers do not need to learn your IP address either — and new installations use Tor from the first start."
        origin="Built-in Tor (Arti) · Direct, Hybrid and Tor-only modes · onion services · Quick mode"
      />
      <section class="section tight">
        <div class="container">
          <div class="with-toc">
            <Toc items={TOC} />
            <article class="prose">
              <h2 id="layers">Two independent layers</h2>
              <div class="table-wrap">
                <table class="data">
                  <thead><tr><th>Layer</th><th>Protects</th><th>Mechanism</th></tr></thead>
                  <tbody>
                    <tr><td class="strong">On-chain privacy</td><td>Amounts, senders and receivers of every transfer</td><td>Orchard / Halo 2 shielded notes — in every network mode</td></tr>
                    <tr><td class="strong">Network privacy</td><td>This computer's IP address, as seen by peers</td><td>Tor routing in Hybrid or Tor-only mode</td></tr>
                  </tbody>
                </table>
              </div>
              <p class="mt-2">Neither layer makes anyone completely anonymous, and the wallet never shows a "100% anonymous" indicator.</p>

              <h2 id="modes">Network modes</h2>
              <p>
                Choose the mode on the wallet's <b>Network → Network privacy</b> page, with <code>--network-mode</code>, or <code>[network_privacy] mode</code> in the node TOML. <b>New wallet installations start in Tor only.</b> Existing installations keep their mode and are never switched silently; a standalone <code>genorynd</code> without a mode stays Direct.
              </p>
              <div class="table-wrap">
                <table class="data">
                  <thead><tr><th /><th>Direct</th><th>Hybrid</th><th>Tor only</th></tr></thead>
                  <tbody>
                    <tr><td class="strong">Clearnet peers</td><td>Direct TCP</td><td>Direct TCP</td><td>Through Tor exits</td></tr>
                    <tr><td class="strong">Onion peers</td><td>Refused</td><td>Through Tor</td><td>Through Tor</td></tr>
                    <tr><td class="strong">Inbound peers</td><td>Clearnet listener</td><td>Clearnet + optional onion service</td><td>Onion service only</td></tr>
                    <tr><td class="strong">Peer DNS lookups</td><td>System resolver</td><td>System resolver (never for .onion)</td><td>None — names resolved inside Tor</td></tr>
                    <tr><td class="strong">Your payments</td><td>One relay peer first</td><td>Prefers Tor-routed peers</td><td>Tor-routed peers only</td></tr>
                    <tr><td class="strong">If Tor fails</td><td>—</td><td>Onion peers pause</td><td>All networking pauses — never falls back to direct</td></tr>
                    <tr><td class="strong">Peers see your IP</td><td>Yes</td><td>Yes (clearnet peers)</td><td>No</td></tr>
                  </tbody>
                </table>
              </div>

              <h2 id="builtin">Tor is built in</h2>
              <p>
                Hybrid and Tor-only modes use a Tor client inside GENORYN (<ExtLink href="https://gitlab.torproject.org/tpo/core/arti">Arti</ExtLink>). There is nothing to download or keep open. The first start fetches about 10 MB of Tor directory data, so the first connection takes a little longer. A Tor daemon or Tor Browser you already run still works ("My own Tor", <code>--tor-socks</code>).
              </p>
              <CodeBlock
                code={`# Everything through Tor, no inbound peers
$ genorynd --network-mode tor-only
# Also accept inbound peers through a persistent v3 onion service
$ genorynd --network-mode tor-only --onion-service
# Clearnet plus onion peers
$ genorynd --network-mode hybrid
# Use an existing Tor daemon instead of the built-in client
$ genorynd --network-mode tor-only --tor-socks 127.0.0.1:9050 --tor-control 127.0.0.1:9051 --onion-service`}
              />

              <h2 id="onion">Onion services</h2>
              <p>
                With <code>--onion-service</code> the node publishes a v3 onion address, so other nodes can connect to yours without either side learning the other's IP. The key lives under <code>&lt;data-dir&gt;/tor/state</code>, so the address survives restarts. Onion addresses spread only between upgraded Hybrid and Tor-only nodes, are announced only over Tor links, and are validated (version, SHA3-256 checksum, ed25519 key) before they enter the peer cache.
              </p>
              <p>
                Tor-only nodes find their first peers through the onion seed list <ExtLink href={`${REPO_URL}/blob/main/${MAINNET_ONION_SEEDS_FILE}`}>{MAINNET_ONION_SEEDS_FILE}</ExtLink>, read automatically at startup — no new release needed when a seed is added. Onion seed addresses for Mainnet 1.0, including the genoryn.io node, are published there at launch (see <A href="/nodes">Nodes</A>).
              </p>

              <h2 id="broadcast">Payment broadcast</h2>
              <ul>
                <li>A payment created on your computer is first sent to <b>one</b> relay peer (preferring outbound Tor links) — a one-hop "stem" in the spirit of Dandelion++ — and only then broadcast more widely after a random 20–45 s embargo.</li>
                <li>Relays to each peer use independent random delays, so opening many connections gives an observer no earlier copy.</li>
                <li>A peer that silently drops your payment can no longer stop it from spreading.</li>
                <li>A payment created while Tor or the network is down is kept on disk and goes out when a peer is reachable again — in Tor-only mode never any other way.</li>
              </ul>
              <p>This makes it harder to tell which computer created a payment. It is <b>not</b> anonymity: the chosen relay peer learns that your node sent it first, and timing analysis remains possible.</p>

              <h2 id="light">Quick mode over Tor</h2>
              <p>
                In <b>Quick mode</b> the wallet runs without a node of its own. It reaches the public RPC of remote nodes <b>only through Tor</b>, each on its own circuit, and <b>fully validates every block locally</b> — RandomX work, Halo 2 proofs, state roots — before scanning. When sources disagree, it follows the chain with the most cumulative work as computed on your computer, never a source's claim. The verified chain is cached, so later syncs download only new blocks. Payments go out through <code>POST /v1/relay</code> of a different source than the one it synced from.
              </p>
              <p>Keys, signing and your balance stay on your computer. A payment's exact signed bytes are stored encrypted before sending and retried unchanged; a node's acknowledgement is not a confirmation. Remote nodes never see your addresses, viewing keys or balance; they can see when a wallet syncs and can withhold new blocks, so use two or more independent sources. Usernames and mining need Full mode, where your own node validates the chain.</p>

              <h2 id="leaks">Leak protections</h2>
              <ul>
                <li><b>One dialing path, no fallback.</b> Every outbound connection goes through one policy check; in Tor-only mode a direct route cannot be produced, and SOCKS failures never lead to a direct attempt.</li>
                <li><b>No local DNS for onion names, ever.</b> In Tor-only mode host names are resolved inside Tor (SOCKS5 remote resolution).</li>
                <li><b>Unlinkable identities.</b> The handshake identity is random per run; Tor links use a second identity and advertise no listening address.</li>
                <li><b>Stream isolation.</b> Each Tor peer connection gets its own circuit.</li>
                <li><b>Separate peer caches</b> per mode, so direct peers never shape Tor connections.</li>
                <li><b>Log redaction</b> of addresses in Tor modes. Public RPC never reveals the mode, Tor status or onion address.</li>
                <li>The seed list and the wallet's update check go through Tor in Tor-only mode, or not at all.</li>
              </ul>

              <h2 id="limits">What Tor cannot do</h2>
              <Callout warn>
                Application-level checks do not replace operating-system isolation. For strong guarantees run GENORYN where all traffic is forced through Tor (Whonix, Tails) or block direct traffic with firewall rules. Tor cannot help against a compromised computer or a malicious Tor binary, an adversary who watches both ends of a circuit, malicious exits censoring clearnet peers (altered blocks still fail validation), traffic of other applications, or the timing and size of your node's traffic.
              </Callout>

              <h2 id="operators">For node operators</h2>
              <CodeBlock
                code={`# An onion seed / Quick-mode source, always on
$ genorynd --network-mode hybrid --onion-service --public-rpc
# Show the onion address to publish in ${MAINNET_ONION_SEEDS_FILE}
$ genoryn-cli get-network-privacy --token-file ~/.genoryn/mainnet-1.0/admin.token`}
              />
              <p class="mt-2">
                Mainnet 1.0 ports: 29710 P2P, 29711 loopback admin RPC, 29712 public RPC, 29713 loopback onion-service target. Releases before 1.0.0 belong to the retired pre-1.0 network and cannot connect to Mainnet 1.0. Full reference: <ExtLink href={`${REPO_URL}/blob/main/docs/network-privacy.md`}>network privacy and Tor</ExtLink>.
              </p>
            </article>
          </div>
        </div>
      </section>
    </div>
  );
}
