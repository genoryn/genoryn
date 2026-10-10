#!/usr/bin/env python3
"""Opt-in live Tor checks: real Tor network, real processes, real PoW and proofs.

Two tor-only devnet nodes reach each other only through v3 onion services.
Covers onion-to-onion peering, block sync, shielded transactions in both
directions and their inclusion, persistent onion identities across a hard
restart, and continuous OS-level sampling of every TCP endpoint the node
processes use. Optional on Linux as root: a logging resolver that must see no
node DNS query, and a simulated Tor outage (blackhole routes to every relay)
during which a transaction is submitted and must be delivered afterwards.
Needs Internet access to the Tor network; never part of default CI.
"""
import argparse, ipaddress, json, os, pathlib, platform, re, shlex, shutil, socket, socketserver, subprocess, sys, threading, time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from devnet import Lab, free_port  # noqa: E402


class LoggingResolver(socketserver.ThreadingUDPServer):
    """Answers nothing; records every DNS question name it receives."""
    allow_reuse_address = True

    def __init__(self, address):
        self.queries = []
        super().__init__(address, LoggingResolver.Handler)

    class Handler(socketserver.BaseRequestHandler):
        def handle(self):
            data = self.request[0]
            labels, position = [], 12
            while position < len(data) and data[position]:
                length = data[position]
                labels.append(data[position + 1:position + 1 + length].decode('ascii', 'replace'))
                position += 1 + length
            self.server.queries.append('.'.join(labels))


class TorLab(Lab):
    def __init__(self, args):
        super().__init__(args)
        self.ports = {n: (free_port(), free_port()) for n in 'ab'}
        self.onion_listen = {n: free_port() for n in 'ab'}
        self.public_rpc = {n: free_port() for n in 'ab'}
        self.wrap = shlex.split(args.wrap) if args.wrap else []
        self.endpoints = {}
        self.listening = {}
        self.sampling = threading.Event()
        self.blackholed = []

    def node_args(self, n, peers):
        args = [self.exe('genorynd'), '--network', 'devnet', '--data-dir', str(self.root / n),
                '--rpc-listen', f'127.0.0.1:{self.ports[n][1]}', '--network-mode', 'tor-only',
                '--onion-service', '--no-default-seeds']
        if self.args.tor_socks:
            args += ['--tor-socks', self.args.tor_socks, '--tor-control', self.args.tor_control,
                     '--onion-listen', f'127.0.0.1:{self.onion_listen[n]}']
            if self.args.tor_cookie:
                args += ['--tor-cookie-file', self.args.tor_cookie]
        else:
            # Light wallets reach the public RPC through the built-in onion service.
            args += ['--public-rpc', '--public-rpc-listen', f'127.0.0.1:{self.public_rpc[n]}']
        for peer in peers:
            args += ['--peer', peer]
        return self.wrap + args

    def start(self, n, peers=()):
        directory = self.root / n
        directory.mkdir(exist_ok=True)
        log = open(self.root / f'{n}.log', 'ab')
        self.logs[n] = log
        self.nodes[n] = subprocess.Popen(self.node_args(n, peers), stdout=log, stderr=log, env=self.env)
        self.wait(lambda: self.get(n, 'info'), timeout=180, description=f'node {n} startup')

    def network(self, n):
        return self.get(n, 'network')

    def pids(self):
        return {process.pid: name for name, process in self.nodes.items()}

    # --- OS-level endpoint sampling -------------------------------------------------
    def sample(self):
        pids = self.pids()
        if os.name == 'nt':
            output = subprocess.run(['netstat', '-ano'], capture_output=True, text=True).stdout
            for line in output.splitlines():
                parts = line.split()
                if len(parts) >= 4 and parts[0] in ('TCP', 'UDP') and parts[-1].isdigit() and int(parts[-1]) in pids:
                    pid = int(parts[-1])
                    if parts[0] == 'TCP' and len(parts) == 5:
                        self.record_endpoint(pid, 'tcp', parts[1], parts[2], parts[3])
                    elif parts[0] == 'UDP':
                        self.record_endpoint(pid, 'udp', parts[1], parts[2], '')
        else:
            output = subprocess.run(['ss', '-tunapH'], capture_output=True, text=True).stdout
            for line in output.splitlines():
                parts = line.split()
                found = re.findall(r'pid=(\d+)', line)
                for pid in {int(p) for p in found} & set(pids):
                    self.record_endpoint(pid, parts[0], parts[4], parts[5], parts[1])

    def record_endpoint(self, pid, protocol, local, remote, state):
        if protocol == 'tcp' and state.upper() in ('LISTEN', 'LISTENING'):
            self.listening.setdefault(pid, set()).add(local.rsplit(':', 1)[1])
            return
        self.endpoints.setdefault(pid, set()).add((protocol, local, remote, state.upper()))

    def sampler(self):
        while not self.sampling.is_set():
            try:
                self.sample()
            except Exception:
                pass
            self.sampling.wait(0.5)

    def fallback_directories(self):
        """Arti's compiled-in fallback directory relays, from the locked tor-dircommon."""
        if self.args.fallback_dirs:
            source = pathlib.Path(self.args.fallback_dirs)
        else:
            metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--format-version', '1'],
                                                          cwd=pathlib.Path(__file__).resolve().parents[2]))
            package = next(p for p in metadata['packages'] if p['name'] == 'tor-dircommon')
            source = pathlib.Path(package['manifest_path']).parent / 'data' / 'fallback_dirs.rs'
        found = set(re.findall(r'"(\d+\.\d+\.\d+\.\d+:\d+|\[[0-9a-fA-F:]+\]:\d+)"', source.read_text()))
        assert len(found) > 100, f'unexpected fallback list at {source}'
        return found

    def relays(self):
        """Every relay IP:ORPort in the consensus each built-in client used."""
        relays = set()
        for n in 'ab':
            for blob in (self.root / n / 'tor' / 'cache' / 'dir_blobs').glob('con_*'):
                for line in blob.read_text(errors='replace').splitlines():
                    fields = line.split()
                    if fields[:1] == ['r'] and len(fields) >= 8:
                        relays.add(f'{fields[-3]}:{fields[-2]}')
                    elif fields[:1] == ['a'] and len(fields) == 2:
                        relays.add(fields[1])
        return relays

    @staticmethod
    def host(endpoint):
        host = endpoint.rsplit(':', 1)[0].strip('[]')
        return ipaddress.ip_address(host.split('%')[0])

    def check_endpoints(self):
        consensus = self.relays() if not self.args.tor_socks else set()
        fallbacks = self.fallback_directories() if not self.args.tor_socks else set()
        relays = consensus | fallbacks
        tor_ports = {address.rsplit(':', 1)[1] for address in (self.args.tor_socks, self.args.tor_control) if address}
        violations, relay_endpoints, udp = [], set(), []
        for pid, endpoints in self.endpoints.items():
            own = self.listening.get(pid, set())
            for protocol, local, remote, state in endpoints:
                if protocol == 'udp':
                    if remote not in ('*:*', '0.0.0.0:*', '[::]:*', '*'):
                        udp.append((pid, local, remote))
                    continue
                try:
                    peer = self.host(remote)
                except ValueError:
                    continue
                if peer.is_unspecified:
                    continue
                if peer.is_loopback:
                    remote_port, local_port = remote.rsplit(':', 1)[1], local.rsplit(':', 1)[1]
                    if local_port in own or remote_port in own or remote_port in tor_ports:
                        continue
                    violations.append(dict(pid=pid, reason='unexpected loopback connection', local=local, remote=remote))
                elif self.args.tor_socks:
                    violations.append(dict(pid=pid, reason='external-Tor node opened a non-loopback socket', remote=remote))
                elif remote.replace('[', '').replace(']', '') in {r.replace('[', '').replace(']', '') for r in relays}:
                    relay_endpoints.add(remote)
                else:
                    violations.append(dict(pid=pid, reason='destination is neither a consensus relay ORPort nor an Arti fallback directory', remote=remote))
        assert not udp, f'node processes used UDP: {udp}'
        normalized = {r.replace('[', '').replace(']', '') for r in fallbacks}
        self.fallback_used = sorted(r for r in relay_endpoints if r.replace('[', '').replace(']', '') in normalized)
        return violations, relay_endpoints

    # --- Tor outage simulation (Linux, root) ---------------------------------------
    def outage(self):
        if self.args.outage_cmd:
            subprocess.run(self.args.outage_cmd, shell=True, check=True)
            return
        relays = sorted({str(self.host(r)) for r in self.relays() | self.fallback_directories()})
        self.blackholed = relays
        lines = '\n'.join(f"route add blackhole {ip}/{128 if ':' in ip else 32}" for ip in relays) + '\n'
        subprocess.run(['ip', '-batch', '-'], input=lines, text=True, check=True)

    def restore(self):
        if self.args.restore_cmd:
            subprocess.run(self.args.restore_cmd, shell=True, check=True)
            return
        lines = '\n'.join(f"route del blackhole {ip}/{128 if ':' in ip else 32}" for ip in self.blackholed) + '\n'
        subprocess.run(['ip', '-batch', '-'], input=lines, text=True, check=False)
        self.blackholed = []

    # --- Scenario -------------------------------------------------------------------
    def run(self):
        sampler = threading.Thread(target=self.sampler, daemon=True)
        sampler.start()
        engine = 'external' if self.args.tor_socks else 'built-in'
        started = time.perf_counter()
        self.start('a')
        ready = self.wait(lambda: (lambda s: s if s['tor']['ready'] and s['tor']['onion']['address'] else None)(self.network('a')),
                          timeout=600, description='node a Tor ready with an onion address')
        onion_a = ready['tor']['onion']['address']
        self.onion_a = onion_a
        self.measurements.append(dict(operation='tor_ready_and_onion_identity', engine=engine, seconds=time.perf_counter() - started))
        assert ready['tor']['backend'] == engine, ready['tor']
        self.start('b', [onion_a])
        onion_b = self.wait(lambda: self.network('b')['tor']['onion']['address'], timeout=600, description='node b onion address')
        self.record('two_tor_only_nodes_with_v3_onion_services', engine=engine,
                    onion_a=onion_a.split('.')[0][:8] + '…', onion_b=onion_b.split('.')[0][:8] + '…')
        peering = time.perf_counter()
        self.wait(lambda: all(self.get(n, 'info')['peer_count'] > 0 for n in 'ab'), timeout=1200, description='onion-to-onion peering')
        self.measurements.append(dict(operation='onion_to_onion_peering_after_start', engine=engine, seconds=time.perf_counter() - peering))
        a, b = self.network('a'), self.network('b')
        assert a['connections']['onion_inbound'] == 1 and b['connections']['tor_outbound'] == 1, (a['connections'], b['connections'])
        for status in (a, b):
            assert status['connections']['clearnet_inbound'] == status['connections']['clearnet_outbound'] == 0
            assert status['transport']['direct_attempts'] == 0
            assert status['clearnet_listener'] is None
        self.record('onion_to_onion_p2p_without_clearnet')

        alice = self.wallet('alice', 'create')
        bob = self.wallet('bob', 'create', node='b')
        synced = time.perf_counter()
        self.mine('a', alice['address'], 4)
        mined = time.perf_counter()
        self.sync('ab', 4)
        self.measurements.append(dict(operation='four_block_sync_over_tor_after_mining', engine=engine, seconds=time.perf_counter() - mined))
        self.record('block_sync_over_tor', height=4, mining_seconds=round(mined - synced, 1))

        self.wallet('alice', 'sync')
        relay = time.perf_counter()
        self.wallet('alice', 'send', to=bob['address'], amount_atomic='1000000000000', fee_atomic='10000000')
        self.wait(lambda: self.get('b', 'info')['mempool_count'] == 1, timeout=600, description='shielded transaction a->b over Tor')
        self.measurements.append(dict(operation='shielded_tx_relay_over_tor', engine=engine, seconds=time.perf_counter() - relay))
        assert self.pending('a') == self.pending('b') != []
        self.mine('a', alice['address'], 1)
        self.sync('ab', 5)
        assert self.get('a', 'info')['mempool_count'] == self.get('b', 'info')['mempool_count'] == 0
        assert int(self.wallet('bob', 'sync', node='b')['balance_atomic']) == 10**12
        self.record('shielded_tx_a_to_b_relayed_and_mined', height=5)

        relay = time.perf_counter()
        self.wallet('bob', 'send', node='b', to=alice['address'], amount_atomic='300000000000', fee_atomic='10000000')
        self.wait(lambda: self.get('a', 'info')['mempool_count'] == 1, timeout=600, description='shielded transaction b->a over Tor')
        self.measurements.append(dict(operation='shielded_tx_relay_over_tor', engine=engine, seconds=time.perf_counter() - relay))
        assert self.pending('a') == self.pending('b') != []
        self.mine('a', alice['address'], 1)
        self.sync('ab', 6)
        self.bob_expected = 10**12 - 300000000000 - 10000000
        assert int(self.wallet('bob', 'sync', node='b')['balance_atomic']) == self.bob_expected, 'b->a spend not mined'
        self.record('shielded_tx_b_to_a_relayed_and_mined', height=6)

        if not self.args.tor_socks:
            self.light_wallet_scenario(alice)

        height_before_restart = self.get('a', 'info')['height']
        for n in 'ab':
            self.stop(n)
        self.start('a')
        self.start('b', [onion_a])
        assert self.wait(lambda: self.network('a')['tor']['onion']['address'], timeout=600, description='a onion after restart') == onion_a
        assert self.wait(lambda: self.network('b')['tor']['onion']['address'], timeout=600, description='b onion after restart') == onion_b
        reconnect = time.perf_counter()
        self.wait(lambda: all(self.get(n, 'info')['peer_count'] > 0 for n in 'ab'), timeout=1200, description='peering after restart')
        self.measurements.append(dict(operation='onion_peering_after_hard_restart', engine=engine, seconds=time.perf_counter() - reconnect))
        assert all(self.get(n, 'info')['height'] == height_before_restart for n in 'ab')
        self.record('onion_identities_and_chain_persist_across_hard_restart')

        if self.args.outage or self.args.outage_cmd:
            self.outage_scenario(alice)

        violations, relay_endpoints = self.check_endpoints()
        assert not violations, violations
        self.record('os_level_endpoints_only_loopback_or_tor_relays', engine=engine,
                    distinct_relay_endpoints=len(relay_endpoints),
                    fallback_directories_used=len(getattr(self, 'fallback_used', [])),
                    samples_scope='netstat/ss every 0.5 s for each node process; short-lived sockets between samples can be missed')
        if self.resolver is not None:
            self.check_resolver()

    def light_wallet_scenario(self, alice):
        """A wallet without a node: own built-in Tor, both nodes' onion RPCs as sources."""
        sources = []
        for n in 'ab':
            rpc = self.wait(lambda: self.network(n)['tor']['onion'].get('rpc_address'), timeout=120,
                            description=f'{n} onion RPC address')
            sources.append(f'http://{rpc}')
        light = dict(light=True, sources=sources, tor_dir=str(self.root / 'carol-tor'))
        carol = self.wallet('carol', 'create')
        amount = 500000000000
        self.wallet('alice', 'sync')
        self.wallet('alice', 'send', to=carol['address'], amount_atomic=str(amount), fee_atomic='10000000')
        self.wait(lambda: self.get('b', 'info')['mempool_count'] == 1, timeout=600, description='payment to the light wallet')
        height = self.get('a', 'info')['height']
        self.mine('a', alice['address'], 1)
        self.sync('ab', height + 1)
        started = time.perf_counter()
        synced = self.wallet('carol', 'sync', **light)
        self.measurements.append(dict(operation='light_wallet_first_sync_over_tor', seconds=time.perf_counter() - started,
                                      scope='includes bootstrapping its own built-in Tor in an empty directory and full validation'))
        assert int(synced['balance_atomic']) == amount, synced
        statuses = sorted(source['status'] for source in synced['light_sources'])
        assert statuses == ['agrees', 'used'], statuses
        self.record('light_wallet_synced_and_validated_through_two_onion_rpcs', sources=statuses)
        started = time.perf_counter()
        sent = self.wallet('carol', 'send', to=alice['address'], amount_atomic='100000000000', fee_atomic='10000000', **light)
        self.measurements.append(dict(operation='light_wallet_send_over_tor', seconds=time.perf_counter() - started))
        assert sent['relayed_via'] in sources
        self.wait(lambda: self.get('a', 'info')['mempool_count'] == 1 and self.get('b', 'info')['mempool_count'] == 1,
                  timeout=600, description='light wallet payment relayed to both nodes')
        height = self.get('a', 'info')['height']
        self.mine('a', alice['address'], 1)
        self.sync('ab', height + 1)
        after = self.wallet('carol', 'sync', **light)
        assert int(after['balance_atomic']) == amount - 100000000000 - 10000000, after
        self.record('light_wallet_payment_relayed_through_a_remote_node_and_mined', height=height + 1)

    def outage_scenario(self, alice):
        self.outage()
        try:
            outage = time.perf_counter()
            self.wallet('bob', 'send', node='b', to=alice['address'], amount_atomic='100000000000', fee_atomic='10000000')
            time.sleep(30)
            # A hard restart mid-outage: the pending transaction must come back from disk.
            self.stop('b')
            self.start('b', [self.onion_a])
            restored = self.wait(lambda: self.get('b', 'info')['mempool_count'] == 1 and self.network('b')['local_broadcasts_pending'] == 1,
                                 timeout=120, description='pending local transaction restored after restart')
            self.record('pending_local_tx_restored_after_hard_restart_during_outage', restored=bool(restored))
            time.sleep(max(0, self.args.outage_seconds - (time.perf_counter() - outage)))
            b = self.network('b')
            assert b['transport']['direct_attempts'] == 0
            assert self.get('a', 'info')['mempool_count'] == 0, 'transaction crossed a total Tor outage'
            self.record('tx_submitted_during_tor_outage_stays_local', outage_seconds=self.args.outage_seconds,
                        local_broadcasts_pending=b.get('local_broadcasts_pending'))
        finally:
            self.restore()
        recovered = time.perf_counter()
        self.wait(lambda: self.get('a', 'info')['mempool_count'] == 1, timeout=1800, description='transaction delivered after the outage')
        self.measurements.append(dict(operation='tx_delivery_after_tor_outage_restored', seconds=time.perf_counter() - recovered,
                                      outage_seconds=round(recovered - outage, 1)))
        assert self.pending('a') == self.pending('b') != []
        height = self.get('a', 'info')['height']
        self.mine('a', alice['address'], 1)
        self.sync('ab', height + 1)
        expected = self.bob_expected - 100000000000 - 10000000
        assert int(self.wallet('bob', 'sync', node='b')['balance_atomic']) == expected, 'outage spend not mined'
        assert self.network('b')['transport']['direct_attempts'] == 0
        self.record('tx_from_outage_delivered_after_recovery_and_mined', height=height + 1)

    def pending(self, n):
        return sorted(self.get(n, 'mempool/nullifiers')['nullifiers'])

    def check_resolver(self):
        control = 'positive-control.genoryn.invalid'
        try:
            socket.getaddrinfo(control, 80)
        except OSError:
            pass
        time.sleep(1)
        queries = list(self.resolver.queries)
        assert control in queries, f'resolver interception is not effective: {queries}'
        leaked = [name for name in queries if name != control]
        assert not leaked, f'DNS queries left the node processes: {leaked}'
        self.record('no_dns_queries_from_node_processes', positive_control_seen=True)

    def finish(self):
        self.sampling.set()
        if self.blackholed:
            self.restore()
        for n in list(self.nodes):
            self.stop(n)
        report = dict(schema=1, kind='live-tor', platform=platform.platform(), engine='external' if self.args.tor_socks else 'built-in',
                      real_tor_network=True, real_processes=True, real_pow=True, real_zk_proofs=True,
                      checks=self.checks, measurements=self.measurements,
                      measurement_scope='Single same-host run over the public Tor network; timings depend on circuits and relays chosen and are not guarantees.',
                      passed=bool(self.checks) and all(c['passed'] for c in self.checks))
        pathlib.Path(self.args.report).parent.mkdir(parents=True, exist_ok=True)
        pathlib.Path(self.args.report).write_text(json.dumps(report, indent=2) + '\n')


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--bin-dir', default='target/debug')
    p.add_argument('--work-dir', required=True)
    p.add_argument('--report', required=True)
    p.add_argument('--tor-socks', help='external Tor SOCKS address; default is the built-in client')
    p.add_argument('--tor-control')
    p.add_argument('--tor-cookie')
    p.add_argument('--wrap', help='command prefix for node processes')
    p.add_argument('--resolver', help='IP to run a logging DNS resolver on (Linux, root; resolv.conf must point to it)')
    p.add_argument('--outage', choices=['blackhole'], help='simulate a total Tor outage (Linux, root)')
    p.add_argument('--outage-cmd')
    p.add_argument('--restore-cmd')
    p.add_argument('--outage-seconds', type=int, default=300)
    p.add_argument('--fallback-dirs', help="path to the locked tor-dircommon's data/fallback_dirs.rs (default: from cargo metadata)")
    p.add_argument('--keep', action='store_true')
    args = p.parse_args()
    if args.tor_socks and not args.tor_control:
        raise SystemExit('--tor-socks needs --tor-control for the onion services')
    root = pathlib.Path(args.work_dir)
    if root.exists() and any(root.iterdir()):
        raise SystemExit('Use a fresh empty --work-dir; existing wallets are never overwritten.')
    lab = TorLab(args)
    lab.resolver = None
    if args.resolver:
        lab.resolver = LoggingResolver((args.resolver, 53))
        threading.Thread(target=lab.resolver.serve_forever, daemon=True).start()
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
