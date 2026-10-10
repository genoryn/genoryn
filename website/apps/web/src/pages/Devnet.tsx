import { NetworkPage } from './NetworkParams';
import { Callout, CodeBlock } from '~/components/ui';

export default function Devnet() {
  return (
    <NetworkPage name="devnet" eyebrow="devnet.genoryn.dev" title={<>Devnet</>} lead="The development network, kept for development labs and automated tests. Its coins never had value and do not carry over.">
      <Callout class="mb-4">
        The real network is <a href="/mainnet" class="strong">Mainnet 1.0</a>. Releases before 1.0.0 use the earlier devnet and cannot connect to the 1.0 networks.
      </Callout>
      <div class="grid cols-2 mb-4">
        <CodeBlock title="Run a devnet lab node" code={`$ genorynd --network devnet --data-dir ~/.genoryn/devnet-1.0`} />
        <CodeBlock title="Isolated local lab" code={`$ genorynd --network devnet --p2p-listen 127.0.0.1:29410\n# loopback-only: never contacts any seeds`} />
      </div>
    </NetworkPage>
  );
}
