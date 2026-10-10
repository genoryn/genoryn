import { NetworkPage } from './NetworkParams';
import { Callout } from '~/components/ui';

export default function Testnet() {
  return (
    <NetworkPage name="testnet" eyebrow="testnet.genoryn.dev" title={<>Testnet</>} lead="Testnet uses production-like maturity (60 blocks) and initial difficulty. Its parameters and genesis are defined; public seeds have not been published yet.">
      <Callout class="mb-4">
        The isolated testnet has been exercised in the project's verification runs, but no public testnet seeds (<code>seeds/testnet-1.0.txt</code>) are published yet. Until seeds are announced, run testnet nodes with explicit <code>--peer</code> addresses. The real network is <a href="/mainnet" class="strong">Mainnet 1.0</a>.
      </Callout>
    </NetworkPage>
  );
}
