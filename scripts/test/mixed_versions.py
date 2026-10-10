#!/usr/bin/env python3
"""Mixed-version devnet: released v0.4.0 and v0.5.0 nodes, wallets and miners
next to the current build in direct and hybrid mode. Real processes, RandomX
PoW and Halo 2 proofs on loopback; nothing else is contacted except, in hybrid
mode, the Tor network by the built-in client.

Checks blocks and shielded transactions crossing versions in both directions,
v1 peer stability, and that the current build opens legacy chain databases and
wallet files. Legacy binaries must come from separate build directories.
"""
import argparse, json, pathlib, shutil, subprocess, sys, time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from devnet import Lab, free_port  # noqa: E402

AMOUNT = 1_000_000_000_000
FEE = 10_000_000


class MixedLab(Lab):
    def __init__(self, args):
        super().__init__(args)
        self.bins = {'v0.4.0': pathlib.Path(args.v040).resolve(), 'v0.5.0': pathlib.Path(args.v050).resolve(),
                     'current': pathlib.Path(args.bin_dir).resolve()}
        self.version = {}
        self.ports = {n: (free_port(), free_port()) for n in ('l4', 'l5', 'n', 'h', 'u4', 'u5')}

    def exe_of(self, version, name):
        return str(self.bins[version] / (name + self.suffix))

    def start_version(self, n, version, peers=(), extra=(), data=None):
        directory = self.root / (data or n)
        directory.mkdir(exist_ok=True)
        log = open(self.root / f'{n}.log', 'ab')
        self.logs[n] = log
        args = [self.exe_of(version, 'genorynd'), '--network', 'devnet', '--data-dir', str(directory),
                '--p2p-listen', f'127.0.0.1:{self.ports[n][0]}', '--rpc-listen', f'127.0.0.1:{self.ports[n][1]}',
                '--no-default-seeds', *extra]
        for p in peers:
            args += ['--peer', f'127.0.0.1:{self.ports[p][0]}']
        self.nodes[n] = subprocess.Popen(args, stdout=log, stderr=log, env=self.env)
        self.version[n] = version
        self.wait(lambda: self.get(n, 'info'), timeout=180, description=f'{version} node {n} startup')

    def wallet_v(self, version, name, op, node, **fields):
        request = dict(op=op, network='devnet', wallet_path=str(self.root / f'{name}.wallet'),
                       password='GENORYN-local-test-only-7!', rpc_url=self.url(node),
                       token_file=str(self.root / node / 'admin.token'), **fields)
        p = subprocess.run([self.exe_of(version, 'genoryn-wallet-cli'), '--json-stdio'], input=json.dumps(request),
                           capture_output=True, text=True, env=self.env, timeout=600)
        if p.returncode:
            raise AssertionError(f'{version} wallet {op} process failed: {p.stderr[-1000:]}')
        result = json.loads(p.stdout)
        if not result.get('ok'):
            raise AssertionError(f'{version} wallet {op} rejected: {result.get("error")}')
        return result['result']

    def mine_v(self, version, node, address, count):
        p = subprocess.run([self.exe_of(version, 'genoryn-miner'), '--network', 'devnet', '--node', self.url(node),
                            '--token-file', str(self.root / node / 'admin.token'), '--wallet', address,
                            '--threads', '1', '--max-blocks', str(count), '--max-seconds', '900'],
                           capture_output=True, text=True, env=self.env, timeout=1000)
        assert p.returncode == 0, f'{version} miner failed: {p.stderr[-1000:]}'
        stats = json.loads(p.stdout.strip().splitlines()[-1])
        assert stats['accepted_blocks'] == count, stats

    def connected(self, names):
        return all(self.get(n, 'info')['peer_count'] > 0 for n in names)

    def run(self):
        self.start_version('l4', 'v0.4.0')
        self.start_version('l5', 'v0.5.0', ['l4'])
        self.start_version('n', 'current', ['l5'])
        self.start_version('h', 'current', ['n', 'l4'], ['--network-mode', 'hybrid'])
        all_nodes = ('l4', 'l5', 'n', 'h')
        self.wait(lambda: self.connected(all_nodes), description='mixed-version peers')
        genesis = {self.get(n, 'info')['genesis_hash'] for n in all_nodes}
        assert len(genesis) == 1
        self.record('v040_v050_current_direct_and_hybrid_peer', genesis=next(iter(genesis)))

        alice = self.wallet_v('v0.4.0', 'alice-v040', 'create', 'l4')
        bob = self.wallet_v('current', 'bob-current', 'create', 'n')
        carol = self.wallet_v('v0.5.0', 'carol-v050', 'create', 'l5')
        self.mine_v('v0.4.0', 'l4', alice['address'], 4)
        self.sync(all_nodes, 4)
        self.record('blocks_mined_by_v040_accepted_by_all', height=4)

        self.wallet_v('v0.4.0', 'alice-v040', 'sync', 'l4')
        self.wallet_v('v0.4.0', 'alice-v040', 'send', 'l4', to=bob['address'], amount_atomic=str(AMOUNT), fee_atomic=str(FEE))
        self.wait(lambda: all(self.get(n, 'info')['mempool_count'] == 1 for n in all_nodes), description='v0.4.0 transaction relayed to every version')
        self.mine_v('current', 'n', bob['address'], 1)
        self.sync(all_nodes, 5)
        assert all(self.get(n, 'info')['mempool_count'] == 0 for n in all_nodes)
        assert int(self.wallet_v('current', 'bob-current', 'sync', 'n')['balance_atomic']) >= AMOUNT
        self.record('v040_wallet_tx_mined_by_current_miner_and_accepted_by_legacy_nodes', height=5)

        self.wallet_v('current', 'bob-current', 'send', 'n', to=carol['address'], amount_atomic=str(AMOUNT // 4), fee_atomic=str(FEE))
        self.wait(lambda: all(self.get(n, 'info')['mempool_count'] == 1 for n in all_nodes), timeout=300,
                  description='current-version local transaction relayed to every version (stem and embargo)')
        self.mine_v('v0.4.0', 'l4', alice['address'], 1)
        self.sync(all_nodes, 6)
        assert int(self.wallet_v('v0.5.0', 'carol-v050', 'sync', 'l5')['balance_atomic']) == AMOUNT // 4
        self.record('current_wallet_tx_relayed_to_and_mined_by_v040_seen_by_v050_wallet', height=6)

        time.sleep(15)
        assert self.connected(all_nodes), 'a peer dropped a connection'
        for n in ('l4', 'l5'):
            text = (self.root / f'{n}.log').read_text(errors='replace').lower()
            for marker in ('unknown variant', 'invalid value', 'undecodable', 'misbehavior threshold'):
                assert marker not in text, f'{n} log contains {marker!r}'
        self.record('legacy_nodes_stay_connected_and_log_no_decode_errors')

        for legacy, upgraded, version in (('l5', 'u5', 'v0.5.0'), ('l4', 'u4', 'v0.4.0')):
            self.stop(legacy)
            shutil.copytree(self.root / legacy, self.root / upgraded)
            self.start_version(upgraded, 'current', ['n'])
            info = self.get(upgraded, 'info')
            assert info['height'] == 6 and info['best_hash'] == self.get('n', 'info')['best_hash'], info
            self.record(f'{version.replace(".", "")}_chain_database_opened_by_current_build', height=6)
        legacy_view = self.wallet_v('v0.5.0', 'carol-v050', 'sync', 'u5')
        current_view = self.wallet_v('current', 'carol-v050', 'sync', 'u5')
        assert current_view['balance_atomic'] == legacy_view['balance_atomic'] == str(AMOUNT // 4)
        alice_current = self.wallet_v('current', 'alice-v040', 'sync', 'u4')
        assert int(alice_current['balance_atomic']) > 0
        self.record('v040_and_v050_wallet_files_opened_by_current_wallet_cli')

    def finish(self):
        for n in list(self.nodes):
            self.stop(n)
        report = dict(schema=1, kind='mixed-versions', real_processes=True, real_pow=True, real_zk_proofs=True,
                      versions={v: str(p) for v, p in self.bins.items()}, checks=self.checks,
                      passed=bool(self.checks) and all(c['passed'] for c in self.checks))
        pathlib.Path(self.args.report).parent.mkdir(parents=True, exist_ok=True)
        pathlib.Path(self.args.report).write_text(json.dumps(report, indent=2) + '\n')


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--bin-dir', default='target/debug')
    p.add_argument('--v040', required=True, help='directory with v0.4.0 genorynd, genoryn-miner, genoryn-wallet-cli')
    p.add_argument('--v050', required=True, help='directory with v0.5.0 binaries')
    p.add_argument('--work-dir', required=True)
    p.add_argument('--report', required=True)
    p.add_argument('--keep', action='store_true')
    args = p.parse_args()
    root = pathlib.Path(args.work_dir)
    if root.exists() and any(root.iterdir()):
        raise SystemExit('Use a fresh empty --work-dir; existing wallets are never overwritten.')
    lab = MixedLab(args)
    try:
        lab.run()
    except Exception as e:
        lab.checks.append(dict(check='scenario_completion', passed=False, error=str(e)))
        raise
    finally:
        lab.finish()
    if not args.keep:
        shutil.rmtree(lab.root, ignore_errors=True)


if __name__ == '__main__':
    main()
