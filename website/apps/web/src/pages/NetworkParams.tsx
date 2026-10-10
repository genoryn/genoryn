import { createResource, Show, type JSX } from 'solid-js';
import { GENESIS_MESSAGE, MAINNET_GENESIS_HASH, MAINNET_ONION_P2P_PORT, NETWORKS, formatNumber, genesisPreimage, type NetworkName } from '@genoryn/shared';
import { Callout, Gnr, Hash, Kv, PageHero, StaleNote, Stat } from '~/components/ui';
import { api } from '~/lib/api';
import { stats } from '~/lib/live';

export function NetworkPage(props: { name: NetworkName; eyebrow: string; title: JSX.Element; lead: string; children?: JSX.Element }) {
  const n = NETWORKS[props.name];
  const [genesis] = createResource(() => (n.live ? true : null), () => api<Record<string, unknown>>('/genesis'));
  return (
    <>
      <PageHero eyebrow={props.eyebrow} title={props.title} lead={props.lead} origin={n.live ? 'Live' : 'Not yet public'} />
      <section class="section tight">
        <div class="container">
          <Show when={n.live}>
            <StaleNote class="mb-2" />
            <div class="stat-strip mb-4">
              <Stat label="Height" live={!!stats()?.node_online} value={stats() ? formatNumber(stats()!.height) : '—'} />
              <Stat label="Difficulty" value={stats() ? formatNumber(stats()!.difficulty) : '—'} />
              <Stat label="Issued" value={<Gnr gori={stats()?.issued_supply} digits={0} />} />
              <Stat label="Peers" value={stats() ? String(stats()!.peers) : '—'} />
            </div>
          </Show>
          {props.children}
          <Kv
            rows={[
              ['Network', props.name],
              ['Chain ID', n.chainId],
              ['Status', n.live ? 'live · public · finalized genesis' : 'development lab network'],
              ['Address prefix (HRP)', `${n.hrp}1…`],
              ['Network magic', n.magic],
              ['P2P port', String(n.p2pPort)],
              ['Admin RPC (loopback)', String(n.adminRpcPort)],
              ['Public RPC (read-only, opt-in)', String(n.publicRpcPort)],
              ...(props.name === 'mainnet' ? ([['Onion service target', String(MAINNET_ONION_P2P_PORT)]] as Array<[JSX.Element, JSX.Element]>) : []),
              ['Data directory', <code class="mono">~/.genoryn/{props.name}-1.0</code>],
              ['Coinbase maturity', `${n.maturity} blocks`],
              ['Initial difficulty', String(n.initialDifficulty)],
              ['Genesis tag', n.genesisTag],
              ['Genesis message', GENESIS_MESSAGE],
              ['Genesis preimage', <span class="break">{genesisPreimage(n)}</span>],
              ['Genesis hash', n.live ? <Hash value={genesis() ? String(genesis()!.hash) : MAINNET_GENESIS_HASH} short={false} /> : <span class="muted">H("GENORYN/genesis/v1", preimage)</span>],
              ['Treasury receiver', props.name === 'mainnet' ? 'burn address — provably unspendable' : 'public test key'],
            ]}
          />
          <Show when={n.live}>
            <Callout class="mt-3">Genesis creates no currency (height 0 issues nothing). On mainnet the treasury share is paid to a provably unspendable burn address; test networks use public test keys and their coins have no value.</Callout>
          </Show>
        </div>
      </section>
    </>
  );
}
