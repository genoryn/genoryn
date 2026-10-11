import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = fileURLToPath(new URL('.', import.meta.url));
const env = process.env;

export const config = {
  host: env.HOST ?? '127.0.0.1',
  port: Number(env.PORT ?? 3000),
  siteUrl: (env.SITE_URL ?? 'https://genoryn.io').replace(/\/$/, ''),
  network: (env.GENORYN_NETWORK ?? 'mainnet') as 'mainnet' | 'testnet' | 'devnet',
  publicRpc: env.PUBLIC_RPC_URL ?? 'http://127.0.0.1:29712',
  adminRpc: env.ADMIN_RPC_URL ?? 'http://127.0.0.1:29711',
  dbPath: resolve(env.DB_PATH ?? resolve(here, '../../../data/index.sqlite')),
  webDist: resolve(env.WEB_DIST ?? resolve(here, '../../web/dist/client')),
  pollMs: Number(env.POLL_MS ?? 3000),
  minerUnit: env.MINER_UNIT ?? 'genoryn-miner',
  minerAddress: env.MINER_ADDRESS ?? null,
  // user@host whose forced-command SSH key streams that host's miner journal (read-only).
  minerSsh: env.MINER_SSH?.trim() || null,
  minerSshKey: env.MINER_SSH_KEY ?? '/etc/genoryn/minerlog_key',
  minerSshKnownHosts: env.MINER_SSH_KNOWN_HOSTS ?? '/etc/genoryn/seed1_known_hosts',
  githubToken: env.GITHUB_TOKEN ?? null,
  nodeVersion: env.NODE_VERSION_LABEL ?? 'genorynd 1.0.0',
  seedClearnet: env.SEED_CLEARNET ?? '2.24.162.163:29710',
  // Empty or unset means no onion service is published yet.
  seedOnion: env.SEED_ONION?.trim() || null,
  networkMode: env.NETWORK_MODE ?? 'hybrid',
  // Payment-proof checks: the wallet CLI and a valueless verifier wallet that keeps the validated chain.
  verifierCli: env.VERIFIER_CLI?.trim() || null,
  verifierWallet: env.VERIFIER_WALLET ?? '/var/lib/genoryn-io/verifier/verifier.wallet',
  verifierPasswordFile: env.VERIFIER_PASSWORD_FILE ?? '/etc/genoryn/verifier.pass',
  // name=url pairs of admin RPCs whose peers are counted (local node plus tunnelled seeds).
  peerSources: (env.PEER_SOURCES ?? 'genoryn.io=http://127.0.0.1:29711')
    .split(',')
    .map((x) => x.trim())
    .filter(Boolean)
    .map((x) => ({ name: x.split('=')[0]!, url: x.slice(x.indexOf('=') + 1) })),
};
