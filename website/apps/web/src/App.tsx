import { Route, Router } from '@solidjs/router';
import Layout from './components/Layout';
import Home from './pages/Home';
import Wallet from './pages/Wallet';
import Download from './pages/Download';
import Names from './pages/Names';
import Pay from './pages/Pay';
import Support from './pages/Support';
import Learn from './pages/Learn';
import Status from './pages/Status';
import Docs from './pages/Docs';
import Explorer from './pages/explorer/Explorer';
import Blocks from './pages/explorer/Blocks';
import BlockPage from './pages/explorer/Block';
import TxPage from './pages/explorer/Tx';
import NullifierPage from './pages/explorer/Nullifier';
import CommitmentPage from './pages/explorer/Commitment';
import SearchPage from './pages/explorer/Search';
import Api from './pages/Api';
import Nodes from './pages/Nodes';
import Protocol from './pages/Protocol';
import Mining from './pages/Mining';
import Testnet from './pages/Testnet';
import Devnet from './pages/Devnet';
import Mainnet from './pages/Mainnet';
import Privacy from './pages/Privacy';
import Releases from './pages/Releases';
import Security from './pages/Security';
import About from './pages/About';
import Brand from './pages/Brand';
import NotFound from './pages/NotFound';

export default function App(props: { url?: string }) {
  return (
    <Router url={props.url} root={Layout}>
      <Route path="/" component={Home} />
      <Route path="/wallet" component={Wallet} />
      <Route path="/download" component={Download} />
      <Route path="/name" component={Names} />
      <Route path="/pay" component={Pay} />
      <Route path="/support" component={Support} />
      <Route path="/learn" component={Learn} />
      <Route path="/status" component={Status} />
      <Route path="/docs" component={Docs} />
      <Route path="/explorer" component={Explorer} />
      <Route path="/explorer/blocks" component={Blocks} />
      <Route path="/explorer/block/:id" component={BlockPage} />
      <Route path="/explorer/tx/:id" component={TxPage} />
      <Route path="/explorer/nullifier/:id" component={NullifierPage} />
      <Route path="/explorer/commitment/:id" component={CommitmentPage} />
      <Route path="/explorer/search" component={SearchPage} />
      <Route path="/api" component={Api} />
      <Route path="/nodes" component={Nodes} />
      <Route path="/protocol" component={Protocol} />
      <Route path="/mining" component={Mining} />
      <Route path="/testnet" component={Testnet} />
      <Route path="/mainnet" component={Mainnet} />
      <Route path="/privacy" component={Privacy} />
      <Route path="/devnet" component={Devnet} />
      <Route path="/releases" component={Releases} />
      <Route path="/security" component={Security} />
      <Route path="/about" component={About} />
      <Route path="/brand" component={Brand} />
      <Route path="*404" component={NotFound} />
    </Router>
  );
}
