import { A } from '@solidjs/router';
import { EMISSION_DOC_URL, REPO_URL } from '@genoryn/shared';
import { Toc } from '~/components/Toc';
import { Callout, CodeBlock, ExtLink, PageHero } from '~/components/ui';

const TOC: Array<[string, string]> = [
  ['quickstart', 'Quickstart'],
  ['windows', 'Windows'],
  ['linux', 'Linux'],
  ['backup', 'Backup & recovery'],
  ['cli-wallet', 'Command-line wallet'],
  ['cli-node', 'Node & CLI'],
  ['modes', 'Quick & Full mode'],
  ['tor', 'Tor'],
  ['config', 'Configuration'],
  ['ports', 'Ports & data'],
  ['json', 'JSON-stdio contract'],
  ['troubleshooting', 'Troubleshooting'],
];

export default function Docs() {
  return (
    <>
      <PageHero eyebrow="docs.genoryn.dev" title={<>Documentation</>} lead="Install, run and configure the GENORYN wallet, node, miner and tools." origin="Mirrors docs/quickstart.md and docs/wallet in the repository" />
      <section class="section tight">
        <div class="container">
          <div class="with-toc">
            <Toc items={TOC} />
            <article class="prose docs">
              <h2 id="quickstart">Quickstart</h2>
              <ol>
                <li><b>Download</b> <code>GENORYN-Setup-1.0.0.exe</code> (Windows) or <code>genoryn-1.0.0-mainnet-linux-x86_64.zip</code> from the <A href="/download">Download</A> page and verify its SHA-256 checksum.</li>
                <li><b>Open the wallet</b> and choose a mode: <b>Quick mode</b> (no local node; blocks come from remote nodes over Tor and are validated on your computer) or <b>Full mode</b> (the wallet starts its own node). Both join GENORYN Mainnet 1.0 (<code>genoryn-mainnet-1.0</code>). In Full mode Windows may ask once whether <code>genorynd</code> may accept connections; outbound works either way.</li>
                <li><b>Create a wallet</b> with a strong, unique password and write the 24-word recovery phrase down offline. Do not screenshot it.</li>
                <li><b>Receive</b>: copy your <code>gnr1…</code> address, or register an on-chain <code>@username</code> in Receive → Username.</li>
                <li><b>Mine</b> from the Mining page whenever you choose. Newly mined coins mature after 60 blocks (about an hour).</li>
              </ol>

              <h2 id="windows">Windows</h2>
              <CodeBlock
                title="PowerShell"
                code={`# Recommended: the installer (per-user, no admin rights)
$ .\\GENORYN-Setup-1.0.0.exe
# Portable alternative
$ Expand-Archive .\\genoryn-1.0.0-mainnet-windows-amd64.zip -DestinationPath .
$ .\\genoryn-1.0.0-mainnet-windows-amd64\\Start-Wallet.cmd
# Data lives in %USERPROFILE%\\.genoryn\\mainnet-1.0 — never touched by install, update or uninstall`}
              />
              <p>To update, run the newer installer: it closes the running wallet, replaces the program and keeps your settings. The wallet needs the Microsoft Visual C++ v14 x64 runtime; the installer offers to download it if missing. Packages are unsigned builds, so SmartScreen may warn on first run.</p>

              <h2 id="linux">Linux</h2>
              <CodeBlock
                code={`$ sha256sum -c genoryn-1.0.0-mainnet-linux-x86_64.zip.sha256
$ unzip genoryn-1.0.0-mainnet-linux-x86_64.zip
$ cd genoryn-1.0.0-mainnet-linux-x86_64 && sha256sum -c SHA256SUMS --quiet
$ ./start-wallet         # or ./genoryn-wallet
# Data lives in $HOME/.genoryn/mainnet-1.0 (override with GENORYN_DATA_DIR)`}
              />
              <p>The Linux package targets Ubuntu 24.04 x86-64 (glibc 2.39); older distributions should build from source. The GUI needs X11 or Wayland with OpenGL. The node, miner and CLI tools run headless on servers.</p>

              <h2 id="backup">Backup &amp; recovery</h2>
              <ul>
                <li>The <b>24-word recovery phrase</b> recovers your keys. Write it on paper; never photograph it or store it in a cloud.</li>
                <li><b>Restore</b> from the wallet's Restore tab with the phrase and a new password. Reconstruction scans the available chain.</li>
                <li>Username-registry metadata and local contact-owner pins are separate local data — back up the data directory if you rely on them.</li>
              </ul>
              <Callout warn>Never paste a seed or password into a command-line argument, a chat or a website. GENORYN tools always prompt with hidden input.</Callout>

              <h2 id="cli-wallet">Command-line wallet</h2>
              <CodeBlock
                code={`$ genoryn-wallet-cli --wallet alice.gnr create
$ genoryn-wallet-cli --wallet alice.gnr address
$ genoryn-wallet-cli --wallet alice.gnr balance
$ genoryn-wallet-cli --wallet alice.gnr estimate-fee
$ genoryn-wallet-cli --wallet alice.gnr send        # to gnr1… or @name
$ genoryn-wallet-cli --wallet alice.gnr proof
$ genoryn-wallet-cli verify-proof
# On-chain usernames (JSON-stdio ops): name_status, register_name, update_name, my_names`}
              />

              <h2 id="cli-node">Node &amp; CLI</h2>
              <CodeBlock
                code={`# Full node on Mainnet 1.0 (the default network)
$ genorynd --data-dir ~/.genoryn/mainnet-1.0
# Read-only public RPC (put it behind your own TLS proxy)
$ genorynd --public-rpc --public-rpc-listen 127.0.0.1:29712
# Queries
$ genoryn-cli get-network-info
$ genoryn-cli get-supply
# Audit the emission schedule at any height
$ genoryn-emission --height 2102901`}
              />
              <p>Monetary RPC values are decimal strings in gori, with 1 GNR = 1,000,000,000,000 gori. <code>/v1/supply</code> reports <code>early_issued</code>, <code>current_early_reward</code> and <code>early_blocks</code> (500) alongside the normal and treasury figures; see the <ExtLink href={EMISSION_DOC_URL}>Monetary Policy &amp; Emission Schedule</ExtLink>.</p>

              <h2 id="modes">Quick mode and Full mode</h2>
              <p>The 1.0 wallet is one program with two modes. You can switch at any time in Network; switching keeps your keys, scan progress, pending payments and mining history.</p>
              <ul>
                <li><b>Quick mode</b> — no local node. The wallet downloads blocks from remote nodes over Tor and validates every block on your computer; keys, signing and your balance never leave it. The verified chain is cached, so later syncs download only new blocks. When sources disagree, the wallet follows the chain with the most cumulative work as computed locally. Receiving, scanning and sending work in Quick mode; usernames and mining need Full mode.</li>
                <li><b>Full mode</b> — the wallet starts its own <code>genorynd</code>, which validates the chain independently and serves the wallet locally.</li>
                <li><b>Durable outbox</b> — a payment is signed, and its exact bytes are stored encrypted on your computer before it is sent. Retries resend the same bytes, so a payment is never signed twice. A node's acknowledgement is not a confirmation: the payment is confirmed only when it appears in a block.</li>
                <li><b>Mining rewards</b> appear as Just mined, Maturing, Mature or Replaced. A reward whose block was replaced by a reorganisation stays in the history but never counts toward a balance.</li>
              </ul>
              <p>From the command line, Quick mode is the wallet CLI request field <code>"light": true</code> with one to eight <code>"sources"</code>. Node operators can offer a source with <code>--public-rpc --onion-service</code>.</p>

              <h2 id="tor">Tor</h2>
              <p>New wallet installations start in <b>Tor only</b> with the built-in Tor client (Network → Network privacy). A standalone <code>genorynd</code> without a mode stays <code>direct</code>.</p>
              <CodeBlock
                code={`$ genorynd --network-mode tor-only                    # everything through Tor
$ genorynd --network-mode tor-only --onion-service    # + inbound via v3 onion service
$ genorynd --network-mode hybrid                      # clearnet + onion peers
$ genorynd --network-mode tor-only --peer <56-char>.onion:29710
$ genoryn-cli get-network-privacy --token-file ~/.genoryn/mainnet-1.0/admin.token
$ genoryn-cli reconnect-network --token-file ~/.genoryn/mainnet-1.0/admin.token`}
              />
              <p>Mainnet ports in Tor modes: 29710 P2P, 29711 loopback admin RPC, 29713 loopback onion-service target. See <A href="/privacy">Tor &amp; privacy</A>.</p>

              <h2 id="config">Configuration</h2>
              <CodeBlock
                title="genoryn.toml"
                lang="toml"
                code={`[network]
name = "mainnet"

[database]
data_dir = ".genoryn/mainnet-1.0"

[p2p]
# Loopback = isolated local lab. Omit or use "0.0.0.0:29710" to join the mainnet.
listen = "0.0.0.0:29710"
peers = []
dns_seeds = []

[rpc]
admin_listen = "127.0.0.1:29711"   # non-loopback admin binding is rejected

[public_rpc]
enabled = false
listen = "127.0.0.1:29712"         # read-only

[network_privacy]
mode = "tor-only"                  # direct | hybrid | tor-only

[mining]
enabled = false                    # must stay false: mining always needs an explicit action

[limits]
connections = 16
outbound_connections = 8
rpc_concurrency = 16
mempool_transactions = 256`}
              />
              <p>CLI arguments override matching values. Pass the file with <code>--config</code>.</p>

              <h2 id="ports">Ports &amp; data</h2>
              <div class="table-wrap">
                <table class="data">
                  <thead><tr><th>Network</th><th class="num">P2P</th><th class="num">Admin RPC</th><th class="num">Public RPC</th><th>Prefix</th></tr></thead>
                  <tbody>
                    <tr><td class="strong">mainnet (default)</td><td class="num mono">29710</td><td class="num mono">29711</td><td class="num mono">29712</td><td class="mono">gnr1…</td></tr>
                    <tr><td>testnet (labs)</td><td class="num mono">29510</td><td class="num mono">29511</td><td class="num mono">29512</td><td class="mono">gnrt1…</td></tr>
                    <tr><td>devnet (labs)</td><td class="num mono">29410</td><td class="num mono">29411</td><td class="num mono">29412</td><td class="mono">gnrd1…</td></tr>
                  </tbody>
                </table>
              </div>
              <p class="mt-2">Data directories: <code>~/.genoryn/mainnet-1.0</code> (Windows <code>%USERPROFILE%\.genoryn\mainnet-1.0</code>), <code>testnet-1.0</code> and <code>devnet-1.0</code>. Each contains <code>chain.sqlite</code> (+WAL), <code>usernames.sqlite</code>, <code>peers.json</code>, <code>peer-id</code>, <code>admin.token</code> and logs. Wallet files are separate and encrypted.</p>

              <h2 id="json">JSON-stdio contract</h2>
              <p>Integrations can drive the wallet backend with one JSON request on stdin and one JSON response on stdout:</p>
              <CodeBlock
                lang="json"
                title="genoryn-wallet-cli --json-stdio"
                code={`{"op":"sync","wallet_path":"alice.wallet","password":"…","rpc_url":"http://127.0.0.1:29711"}
# → {"ok":true,"result":{"address":"gnr1…","balance_atomic":"…","spendable_atomic":"…","height":123,"history":[…]}}`}
              />
              <p>Operations: <code>create</code>, <code>restore</code>, <code>open</code>, <code>address</code>, <code>derive_account</code>, <code>estimate_fee</code>, <code>sync</code>, <code>send</code>, <code>name_status</code>, <code>register_name</code>, <code>update_name</code>, <code>my_names</code>, <code>export_proof</code>, <code>verify_proof</code>. Atomic amounts are decimal strings.</p>

              <h2 id="troubleshooting">Troubleshooting</h2>
              <ul>
                <li><b>Node has no peers:</b> check outbound TCP to port 29710 and that the node is not bound to loopback. See <A href="/nodes">Nodes</A>.</li>
                <li><b>Miner stopped:</b> the Mining page names the reason (refused admin token — usually another node on the same port —, node not answering, wrong network, RandomX failed) and retries in portable mode or with fewer threads.</li>
                <li><b>Coming from the retired network or devnet:</b> coins of <code>genoryn-mainnet-v1</code> (releases 0.4–0.6) and of the devnet do not carry over, and restoring an old recovery phrase on Mainnet 1.0 does not restore a balance. Old data stays in <code>.genoryn/mainnet</code> and <code>.genoryn/devnet</code> until you delete it.</li>
                <li><b>Balance shows "unscanned":</b> the wallet has not scanned yet; press Scan chain or wait for the automatic scan.</li>
                <li><b>Payment pending:</b> it is in the mempool until a block includes it. Rescan before resending a cancelled payment.</li>
                <li><b>"database belongs to another network":</b> use a separate data directory per network.</li>
                <li>Full references: <ExtLink href={`${REPO_URL}/blob/main/docs/quickstart.md`}>quickstart</ExtLink>, <ExtLink href={`${REPO_URL}/blob/main/docs/wallet/gui.md`}>wallet GUI</ExtLink>, <ExtLink href={`${REPO_URL}/blob/main/docs/testing/devnet.md`}>three-node lab</ExtLink>.</li>
              </ul>
            </article>
          </div>
        </div>
      </section>
    </>
  );
}
