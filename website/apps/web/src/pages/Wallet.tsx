import { A } from '@solidjs/router';
import { createSignal, For, Show } from 'solid-js';
import { theme } from '~/lib/theme';
import { Icon } from '~/components/Icon';
import { Callout, Feature, PageHero, SectionHead } from '~/components/ui';

const FEATURES: Array<[string, string, string]> = [
  ['node', 'My Full Node — one click', 'New in 1.1. Start My Full Node checks disk space, memory, the node program, the data folder and the RPC port first, then starts your own validating node. No open ports, no firewall change, no service; keeping it running after the wallet closes is your choice.'],
  ['fingerprint', 'Payment Proof 2.0', 'New in 1.1. Prove a payment with a code, PDF or JSON — and, for new payments, answer the recipient’s fresh challenge so they know you control the payment’s key. Nothing else about your wallet is disclosed. Anyone can also check a proof at genoryn.io/verify.'],
  ['shield', 'Privacy & Security', 'New in 1.1. Every check says Verified, Warning, Failed or Not tested, and what it is based on: Tor bootstrap, direct connections, fail-closed proxies, wallet encryption, storage hygiene and updates. No anonymity score.'],
  ['cpu', 'Shared mining', 'New in 1.1. Mine solo or with a pool of your choice. The pool checks every share with real RandomX and pays by verified work (PPLNS), so a slow computer earns in proportion to its speed.'],
  ['zap', 'Faster sync', 'New in 1.1. With your own node, sends and proofs fetch only new blocks (0.4 s instead of 11 s on the real chain); Quick mode validates long downloads in parallel. Every block is still fully validated.'],
  ['globe', 'Quick mode', 'No local node: blocks come from remote nodes over Tor and every one is validated on your computer. Keys, signing and balance stay local, and the verified chain is cached so later syncs download only new blocks.'],
  ['node', 'Full mode', 'Your own node validates the chain independently and shows its height, peers, difficulty and readable logs. Switching modes keeps keys, scan progress, pending payments and mining history.'],
  ['shield', 'Durable outbox', 'A payment is signed and its exact bytes are stored encrypted on your computer before sending, then retried with the same bytes. A node acknowledgement is not a confirmation — only a block is.'],
  ['layers', 'Remembered wallets', 'Up to eight wallets open at once. They come back at the next launch, and one password unlocks every remembered wallet that shares it. Only file paths are stored.'],
  ['search', 'Clear history', 'Transfers and mining rewards are separate, with search by transaction ID, memo, amount or block and filters for direction and confirmations.'],
  ['fingerprint', 'Payment proofs', 'Disclose a single output to someone you choose and verify proofs you receive — without revealing anything else.'],
  ['lock', 'Locked means locked', 'Nothing but the unlock screen is reachable until you unlock. The wallet locks after five minutes without input; Lock closes every open wallet.'],
  ['cpu', 'Mine with every core', 'Start and stop a CPU miner with one thread per logical CPU, up to 16. Rewards show as Just mined, Maturing, Mature or Replaced; replaced rewards stay in history but never in balances. Mining never starts by itself.'],
  ['user', 'On-chain @usernames', 'Register @you once and forever in Receive → Username, with live availability and price. Pay anyone with @name — resolved through your own node in Full mode.'],
  ['qr', 'Receive with QR', 'Show your receiving identity as a monochrome QR code or create a payment request with an exact amount.'],
  ['refresh', 'Updates you control', 'The wallet checks the official GitHub releases. On Windows, Download update fetches the matching installer from the repository — nothing installs without you.'],
  ['zap', 'Background, incremental sync', 'Scans run in their own lane and never block sending. With your own node the encrypted scan cache reads only new blocks — about 0.2 s instead of a full rescan.'],
  ['alert', 'Self-repairing miner', 'If mining stops, the wallet says why in plain words and, when a fix exists, restarts in portable RandomX mode or with fewer threads.'],
  ['sun', 'Dark and light', 'Dark by default with the typography of genoryn.io; the sun/moon button switches the appearance and remembers your choice.'],
  ['eyeOff', 'Tor built in', 'New installations start in Tor only: every peer connection, lookup and payment broadcast goes through the built-in Tor client. Nothing else to install.'],
  ['search', 'GENORYN Explorer link', 'The sidebar opens the GENORYN Explorer for public chain data. The explorer never sees your keys, addresses or balance.'],
  ['layers', 'Two editions', 'GENORYN with mining, or GENORYN Lite without a miner program at all. Both share your wallets and install side by side.'],
];

const SHOTS = [
  { id: 'overview', label: 'Overview', caption: 'Overview — balance from your own local scan, your @username, and immature mining rewards shown separately.', fresh: false },
  { id: 'node', label: 'My Full Node', caption: 'My Full Node — one-click start after real checks of this computer, honest status, logs and diagnostics.', fresh: true },
  { id: 'verify', label: 'Verify payment', caption: 'Verify payment — check a proof against your own validated chain, the address and the exact amount, then challenge the sender.', fresh: true },
  { id: 'security', label: 'Privacy & Security', caption: 'Privacy & Security — what this computer can actually observe, check by check, with what each result is based on.', fresh: true },
  { id: 'mining', label: 'Mining', caption: 'Mining — solo or shared (PPLNS) mining, threads up to the logical CPU count and the five stages of a pool reward.', fresh: true },
  { id: 'send', label: 'Send', caption: 'Send — to a gnr1 address or @username; disabled until the matured balance covers amount plus fee.', fresh: false },
  { id: 'receive', label: 'Receive & @name', caption: 'Receive — address, monochrome QR, payment requests and on-chain username registration.', fresh: false },
  { id: 'transactions', label: 'Transactions', caption: 'Transactions — transfers and mined rewards, search, filters and payment proofs.', fresh: false },
  { id: 'network', label: 'Network', caption: 'Network — pinned chain identity, public supply, and network privacy: Direct, Hybrid or Tor only.', fresh: true },
  { id: 'lite', label: 'Lite edition', caption: 'GENORYN Lite — the same wallet, Quick and Full mode, without the Mining page.', fresh: false },
] as const;

function WalletShowcase() {
  const [active, setActive] = createSignal<(typeof SHOTS)[number]['id']>('overview');
  const shot = () => SHOTS.find((x) => x.id === active())!;
  const src = () => (shot().id === 'overview' && theme() === 'light' ? '/images/wallet/light.png' : `/images/wallet/${shot().id}.png`);
  return (
    <div class="reveal">
      <div class="tabs mb-3" role="tablist" aria-label="Wallet screens">
        <For each={SHOTS}>
          {(x) => (
            <button type="button" role="tab" aria-selected={active() === x.id} class={active() === x.id ? 'active' : ''} onClick={() => setActive(x.id)}>
              {x.label}
            </button>
          )}
        </For>
      </div>
      <figure class="shot" style={{ margin: 0 }}>
        <img src={src()} alt={shot().caption} width="1180" height="820" loading="eager" />
        <figcaption>
          <Show when={true}>{shot().caption}</Show>{' '}
          <span class="faint mono" style={{ 'font-size': '11.5px' }}>
            {shot().fresh
              ? '· real screenshot of GENORYN 1.1 on Mainnet 1.0, with a new empty wallet'
              : '· real screenshot from the official repository, taken with a pre-1.0 wallet on the retired network'}
          </span>
        </figcaption>
      </figure>
    </div>
  );
}

export default function Wallet() {
  return (
    <>
      <PageHero
        eyebrow="wallet.genoryn.org"
        title={<>Your wallet. Your keys. Your validation.</>}
        lead="A native Rust desktop application — no embedded website, no remote UI, no telemetry. It uses only black, white and gray, and it never fabricates a number it does not actually have."
        origin="GENORYN and GENORYN Lite · Windows installers · Windows and Linux packages"
        actions={
          <>
            <A href="/download" class="btn primary">
              <Icon name="download" size={16} /> Download wallet
            </A>
            <A href="/docs" class="btn">
              Quickstart
            </A>
          </>
        }
      />
      <section class="section tight">
        <div class="container">
          <WalletShowcase />
        </div>
      </section>
      <section class="section">
        <div class="container">
          <SectionHead eyebrow="Features" title={<>Everything you need. Nothing you don't.</>} />
          <div class="grid cols-3">
            <For each={FEATURES}>{([icon, title, body]) => <Feature icon={icon} title={title}>{body}</Feature>}</For>
          </div>
        </div>
      </section>
      <section class="section">
        <div class="container">
          <div class="grid cols-2">
            <div>
              <SectionHead eyebrow="Security" title={<>Built to fail safe.</>} />
              <ul class="muted" style={{ 'line-height': 1.9, 'padding-left': '20px' }}>
                <li>Wallet files encrypted with Argon2id + XChaCha20-Poly1305.</li>
                <li>Recovery phrase from OS entropy, shown once, requires an offline-backup acknowledgement.</li>
                <li>Passwords and seeds are never command-line arguments and never echoed.</li>
                <li>Existing files are never overwritten; payment-proof files use mode 0600 on Unix.</li>
                <li>Exact 12-decimal amount arithmetic — never binary floating point.</li>
                <li>A separate confirmation window shows the fixed payment intent before signing.</li>
              </ul>
            </div>
            <div>
              <SectionHead eyebrow="Honesty" title={<>No made-up numbers.</>} />
              <p class="muted">
                The wallet never fabricates balances, history, hashrate, peers, prices, sync percentages or supply. A missing response is shown as unavailable. "Unscanned" is distinct from zero. An optional USD reference value is entered by you, computed with checked integer arithmetic and kept separate from payments and consensus.
              </p>
              <Callout warn class="mt-3">
                The software has not been independently audited: mine or hold only what you can afford to lose. Use a trusted computer and keep your 24 words offline.
              </Callout>
            </div>
          </div>
        </div>
      </section>
    </>
  );
}
