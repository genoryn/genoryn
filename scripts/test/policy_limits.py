#!/usr/bin/env python3
"""Exercise configured peer limits with three genuine owned loopback nodes.
No mining, payments, fake RPC service or external connection is involved.
"""
import argparse,json,pathlib,subprocess,time
from devnet import Lab


def main():
    p=argparse.ArgumentParser();p.add_argument('--bin-dir',default='target/debug');p.add_argument('--work-dir',required=True);p.add_argument('--report',required=True);a=p.parse_args()
    root=pathlib.Path(a.work_dir)
    if root.exists() and any(root.iterdir()):raise SystemExit('Use a fresh empty work directory')
    a.quick=True;a.keep=True;lab=Lab(a)
    report={'schema':1,'scope':'three actual owned loopback nodes; configured admission, permit release and diagnostics only','passed':False}
    try:
        directory=lab.root/'a';directory.mkdir()
        config=lab.root/'limits.toml';config.write_text('[limits]\nconnections=1\ninbound_connections=1\noutbound_connections=0\nrpc_concurrency=2\nmempool_transactions=16\nmempool_bytes=1000000\n')
        log=open(lab.root/'a.log','ab');lab.logs['a']=log
        lab.nodes['a']=subprocess.Popen([lab.exe('genorynd'),'--network','devnet','--config',str(config),'--data-dir',str(directory),'--p2p-listen',f'127.0.0.1:{lab.ports["a"][0]}','--rpc-listen',f'127.0.0.1:{lab.ports["a"][1]}'],stdout=log,stderr=log,env=lab.env)
        lab.wait(lambda:lab.get('a','info'),description='limited node startup')
        lab.start('b',['a']);lab.wait(lambda:lab.get('a','info')['peer_count']==1,description='first allowed real inbound connection')
        lab.start('c',['a'])
        samples=[]
        for _ in range(10):
            info=lab.get('a','info');samples.append(info['peer_count'])
            assert info['peer_count']==info['inbound_peers']==1 and info['outbound_peers']==0,info
            assert isinstance(info['database_bytes'],int) and info['database_bytes']>0
            assert info['estimated_hashrate_hs']>=0 and '60-second' in info['hashrate_estimate_basis']
            time.sleep(.5)
        lab.stop('b')
        lab.wait(lambda:lab.get('c','info')['peer_count']>=1 and lab.get('a','info')['peer_count']==1,timeout=45,description='permit reuse after first peer stops')
        for name in ('a','c'):assert lab.get(name,'info')['genesis_hash']==info['genesis_hash']
        report.update(passed=True,peer_count_samples=samples,connection_permit_reused=True,database_size_reported=True,nominal_hashrate_basis_reported=True)
    finally:
        for name in list(lab.nodes):lab.stop(name)
        pathlib.Path(a.report).parent.mkdir(parents=True,exist_ok=True);pathlib.Path(a.report).write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report))
if __name__=='__main__':main()
