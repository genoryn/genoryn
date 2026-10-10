import { NetworkPage } from './NetworkParams';
import { CodeBlock, ExtLink, Gnr, Stat } from '~/components/ui';
import { EMISSION_DOC_URL, MAINNET_GENESIS_HASH, MAX_CIRCULATING, NETWORKS, RETIRED_MAINNET_GENESIS, formatGnr } from '@genoryn/shared';
import { stats } from '~/lib/live';
import { EarlyMinerDisclosure } from '~/components/EarlyMining';

export default function Mainnet() {
  const n = NETWORKS.mainnet;
  return (
    <NetworkPage name="mainnet" eyebrow="genoryn-mainnet-1.0" title={<>Mainnet 1.0</>} lead="GENORYN Mainnet 1.0 is a new network launched on 10 October 2026 with release 1.0.0, from block 0: no premine, no founder-only minting privilege, and the treasury share burned.">
      <div class="stat-strip mb-4">
        <Stat label="Max circulating" value={`${formatGnr(MAX_CIRCULATING, 5)} GNR`} sub="lifetime issuance minus the burned treasury" />
        <Stat label="Treasury burned" value={<Gnr gori={stats()?.treasury_burned} digits={0} />} sub="1 GNR per block, heights 501–210,500" />
        <Stat label="Coinbase maturity" value="60 blocks" sub="about one hour" />
        <Stat label="Genesis" value={<span style={{ 'font-size': '15px' }}>{MAINNET_GENESIS_HASH.slice(0, 10)}…{MAINNET_GENESIS_HASH.slice(-6)}</span>} sub={n.genesisTag} />
      </div>
      <div class="card mb-4">
        <h3>Early mining and launch history</h3>
        <EarlyMinerDisclosure />
        <p class="muted mt-2" style={{ 'font-size': '14.5px' }}>
          The earlier network <code>genoryn-mainnet-v1</code> (genesis <code>{RETIRED_MAINNET_GENESIS.slice(0, 8)}…{RETIRED_MAINNET_GENESIS.slice(-4)}</code>, releases 0.4–0.6) is retired: its coins do not carry over, and restoring its recovery phrase on Mainnet 1.0 does not bring a balance with it. Releases before 1.0.0 cannot connect to Mainnet 1.0.{' '}
          <ExtLink href={EMISSION_DOC_URL} class="strong">
            Monetary Policy &amp; Emission Schedule
          </ExtLink>
        </p>
      </div>
      <div class="grid cols-2 mb-4">
        <CodeBlock title="Join the mainnet" code={`# Every 1.0 program defaults to Mainnet 1.0\n$ genorynd\n# data in ~/.genoryn/mainnet-1.0\n# listens on 0.0.0.0:${n.p2pPort} and contacts the seeds`} />
        <CodeBlock title="Verify the genesis" code={`$ genoryn-cli get-network-info\n# chain_id must be ${n.chainId}\n# genesis_hash must be\n# ${MAINNET_GENESIS_HASH}`} />
      </div>
    </NetworkPage>
  );
}
