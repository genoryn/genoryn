/** Route catalogue: navigation, SEO metadata, sitemap and prerendering all read from here. */

export type Section = 'org' | 'dev' | 'meta';

export interface RouteMeta {
  path: string;
  title: string;
  nav: string;
  description: string;
  section: Section;
  /** The original subdomain this page stands in for. */
  origin?: string;
  priority: number;
  changefreq: 'always' | 'hourly' | 'daily' | 'weekly' | 'monthly';
}

export const ROUTES: RouteMeta[] = [
  { path: '/', nav: 'Home', title: "GENORYN — Privacy is not a crime. It's a right.", description: 'GENORYN (GNR) is private, finite, peer-to-peer money: shielded Halo 2 notes, a hard cap of 21,000,000 GNR and CPU-fair RandomX proof of work. Open source. Verify, don\'t trust.', section: 'meta', priority: 1, changefreq: 'hourly', origin: 'genoryn.org' },
  { path: '/wallet', nav: 'Wallet', title: 'GENORYN Wallet — native full-node wallet', description: 'The GENORYN 1.0 desktop wallet: Quick mode over Tor or Full mode with its own node, keys encrypted with Argon2id and XChaCha20-Poly1305, every block validated locally, mining on demand.', section: 'org', origin: 'wallet.genoryn.org', priority: 0.9, changefreq: 'weekly' },
  { path: '/download', nav: 'Download', title: 'Download GENORYN — latest release for Windows and Linux', description: 'Download the latest GENORYN mainnet wallet: Windows installer, Windows and Linux packages, SHA-256 checksums and step-by-step verification.', section: 'org', origin: 'download.genoryn.org', priority: 0.95, changefreq: 'daily' },
  { path: '/name', nav: 'Names', title: 'GENORYN Names — on-chain @usernames', description: 'Check any GENORYN @username on the blockchain: availability, owner, receiver and burned price. Names are first-come, permanent and globally unique.', section: 'org', origin: 'name.genoryn.org', priority: 0.6, changefreq: 'weekly' },
  { path: '/pay', nav: 'Pay', title: 'GENORYN Pay — payment requests and QR codes', description: 'Create a GENORYN payment request URI and a monochrome QR code in your browser, validate receiving identities offline and learn how shielded payments work.', section: 'org', origin: 'pay.genoryn.org', priority: 0.7, changefreq: 'monthly' },
  { path: '/verify', nav: 'Verify payment', title: 'Verify a GENORYN payment proof — Payment Proof 2.0', description: 'Check a GENORYN payment proof against the Mainnet 1.0 chain: the exact amount, the receiving address and confirmations, then ask the sender to prove it was them with a fresh challenge.', section: 'org', origin: 'genoryn.io/verify', priority: 0.85, changefreq: 'monthly' },
  { path: '/support', nav: 'Support', title: 'GENORYN Support — FAQ and help', description: 'Answers about wallets, recovery phrases, mining, privacy, fees and the devnet. Where to report bugs and security issues.', section: 'org', origin: 'support.genoryn.org', priority: 0.6, changefreq: 'monthly' },
  { path: '/learn', nav: 'Learn', title: 'Learn GENORYN — how private money works', description: 'A clear guide to shielded notes, nullifiers, zero-knowledge proofs, proof of work, keys, issuance and what privacy can and cannot protect.', section: 'org', origin: 'learn.genoryn.org', priority: 0.85, changefreq: 'monthly' },
  { path: '/status', nav: 'Status', title: 'GENORYN Status — live network health', description: 'Live status of the genoryn.io mainnet full node, indexer, miner, peers and the GENORYN network.', section: 'org', origin: 'status.genoryn.org', priority: 0.6, changefreq: 'always' },
  { path: '/docs', nav: 'Docs', title: 'GENORYN Docs — quickstart, CLI and configuration', description: 'Install, run and configure the GENORYN wallet, node, miner and command-line tools on Windows and Linux.', section: 'dev', origin: 'docs.genoryn.dev', priority: 0.85, changefreq: 'weekly' },
  { path: '/explorer', nav: 'Explorer', title: 'GENORYN Explorer — live blocks and shielded transactions', description: 'Explore the live GENORYN mainnet: blocks, shielded transactions, nullifiers, note commitments, usernames, difficulty, issuance and fees.', section: 'dev', origin: 'explorer.genoryn.dev', priority: 1, changefreq: 'always' },
  { path: '/api', nav: 'API', title: 'GENORYN API — public explorer and node RPC reference', description: 'Free read-only JSON API for GENORYN chain data, live Server-Sent Events, CSV exports and the node RPC v1 reference.', section: 'dev', origin: 'api.genoryn.dev', priority: 0.8, changefreq: 'weekly' },
  { path: '/nodes', nav: 'Nodes', title: 'GENORYN Nodes — run a full node and seed', description: 'Run a GENORYN full node: ports, seeds, peer discovery, public RPC and the live peer summary of the genoryn.io node.', section: 'dev', origin: 'nodes.genoryn.dev', priority: 0.75, changefreq: 'daily' },
  { path: '/protocol', nav: 'Protocol', title: 'GENORYN Protocol — consensus, transactions and privacy', description: 'The GENORYN v1 protocol: heaviest-work consensus, block and header layout, Halo 2 shielded transactions, on-chain usernames, encoding, networking and upgrades.', section: 'dev', origin: 'protocol.genoryn.dev', priority: 0.85, changefreq: 'monthly' },
  { path: '/mining', nav: 'Mining', title: 'GENORYN Mining — RandomX CPU mining, rewards and calculator', description: 'How to mine GNR on mainnet with an ordinary CPU: RandomX, block rewards, halvings, difficulty retargeting, 60-block maturity and a live mining calculator.', section: 'dev', origin: 'mining.genoryn.dev', priority: 0.9, changefreq: 'daily' },
  { path: '/mainnet', nav: 'Mainnet', title: 'GENORYN Mainnet 1.0 — genoryn-mainnet-1.0', description: 'The live GENORYN Mainnet 1.0: genesis hash, chain ID, ports, seeds, early-mining disclosure, burned treasury, current height, difficulty and supply.', section: 'dev', origin: 'genoryn.io/mainnet', priority: 0.9, changefreq: 'hourly' },
  { path: '/testnet', nav: 'Testnet', title: 'GENORYN Testnet — parameters', description: 'GENORYN testnet parameters for development labs: ports, prefixes, maturity and genesis.', section: 'dev', origin: 'testnet.genoryn.dev', priority: 0.4, changefreq: 'monthly' },
  { path: '/devnet', nav: 'Devnet', title: 'GENORYN Devnet — development lab network', description: 'The GENORYN devnet parameters, kept for development labs and automated tests. The real network is mainnet.', section: 'dev', origin: 'devnet.genoryn.dev', priority: 0.3, changefreq: 'monthly' },
  { path: '/releases', nav: 'Releases', title: 'GENORYN Releases — version history', description: 'Every GENORYN release with notes, platforms, downloads and SHA-256 checksums, straight from GitHub.', section: 'dev', origin: 'releases.genoryn.dev', priority: 0.7, changefreq: 'daily' },
  { path: '/privacy', nav: 'Tor & privacy', title: 'GENORYN Network Privacy — built-in Tor, onion services, Quick mode', description: 'How GENORYN hides your IP address: built-in Tor (Arti), Direct, Hybrid and Tor-only modes, onion services, private payment broadcast, the Quick-mode wallet over Tor and the limits of network privacy.', section: 'dev', origin: 'genoryn.io/privacy', priority: 0.85, changefreq: 'weekly' },
  { path: '/security', nav: 'Security', title: 'GENORYN Security — threat model and disclosure', description: 'What GENORYN protects, what it does not, wallet encryption, node validation, release verification and how to report vulnerabilities.', section: 'dev', origin: 'security.genoryn.dev', priority: 0.75, changefreq: 'monthly' },
  { path: '/about', nav: 'About', title: 'About GENORYN — origin, principles and license', description: 'GENORYN is an independent, open-source proof-of-work monetary network. Its creator is unknown. Read the principles, genesis message and license.', section: 'meta', priority: 0.6, changefreq: 'monthly' },
  { path: '/brand', nav: 'Brand', title: 'GENORYN Brand — logos, colors and usage', description: 'Download the GENORYN sign, mark and wordmark in SVG and PNG, with the monochrome palette and usage rules.', section: 'meta', priority: 0.4, changefreq: 'monthly' },
];

export const routeMeta = (path: string): RouteMeta | undefined => ROUTES.find((r) => r.path === path);
