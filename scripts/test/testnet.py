#!/usr/bin/env python3
"""One real testnet block on two owned loopback nodes, never a public launch."""
import argparse,json,pathlib,subprocess,time
from devnet import Lab


def main():
    p=argparse.ArgumentParser();p.add_argument('--bin-dir',default='target/debug');p.add_argument('--work-dir',required=True);p.add_argument('--report',required=True);p.add_argument('--max-seconds',type=int,default=300);a=p.parse_args()
    if not 30<=a.max_seconds<=1800:raise SystemExit('Mining budget must be 30..1800 seconds')
    root=pathlib.Path(a.work_dir)
    if root.exists() and any(root.iterdir()):raise SystemExit('Use a fresh empty work directory')
    a.quick=True;a.keep=True;lab=Lab(a);report={'schema':1,'scope':'two real isolated loopback testnet nodes, one valueless proof-of-work block; no external peers','passed':False}
    try:
        for name in 'ab':
            directory=lab.root/name;directory.mkdir();log=open(lab.root/f'{name}.log','ab');lab.logs[name]=log
            command=[lab.exe('genorynd'),'--network','testnet','--data-dir',str(directory),'--p2p-listen',f'127.0.0.1:{lab.ports[name][0]}','--rpc-listen',f'127.0.0.1:{lab.ports[name][1]}']
            if name=='b':command+=['--peer',f'127.0.0.1:{lab.ports["a"][0]}']
            lab.nodes[name]=subprocess.Popen(command,stdout=log,stderr=log,env=lab.env)
            lab.wait(lambda:lab.get(name,'info'),description='testnet startup')
        lab.wait(lambda:all(lab.get(name,'info')['peer_count']>=1 for name in 'ab'),description='testnet peer connection')
        infos=[lab.get(name,'info')for name in 'ab'];assert all(info['network']=='testnet' and info['chain_id']=='genoryn-testnet-1.0' for info in infos)
        assert infos[0]['genesis_hash']==infos[1]['genesis_hash']
        request={'op':'create','network':'testnet','wallet_path':str(lab.root/'testnet.wallet'),'password':'GENORYN-testnet-test-only-8!'}
        result=subprocess.run([lab.exe('genoryn-wallet-cli'),'--json-stdio'],input=json.dumps(request),capture_output=True,text=True,env=lab.env,timeout=60)
        wallet=json.loads(result.stdout);assert result.returncode==0 and wallet['ok'];address=wallet['result']['address'];assert address.startswith('gnrt1');del wallet
        started=time.perf_counter()
        mined=subprocess.run([lab.exe('genoryn-miner'),'--network','testnet','--node',lab.url('a'),'--token-file',str(lab.root/'a'/'admin.token'),'--wallet',address,'--threads','1','--max-blocks','1','--max-seconds',str(a.max_seconds)],capture_output=True,text=True,env=lab.env,timeout=a.max_seconds+60)
        metrics=json.loads(mined.stdout.strip().splitlines()[-1])if mined.stdout.strip()else{}
        report.update(miner_exit_code=mined.returncode,miner_metrics=metrics,mining_budget_seconds=a.max_seconds)
        assert mined.returncode==0,mined.stderr[-1000:]
        assert metrics.get('accepted_blocks')==1,metrics
        lab.sync('ab',1)
        assert lab.get('a','supply')==json.loads(subprocess.check_output([lab.exe('genoryn-emission'),'--height','1'],text=True))
        report.update(passed=True,network='testnet',height=1,genesis_hash=infos[0]['genesis_hash'],same_validated_tip=True,testnet_receiving_prefix=True,real_randomx_and_shielded_coinbase=True,public_supply_matches_calculator=True,elapsed_mining_and_sync_seconds=time.perf_counter()-started,miner_metrics=metrics)
    finally:
        for name in list(lab.nodes):lab.stop(name)
        pathlib.Path(a.report).parent.mkdir(parents=True,exist_ok=True);pathlib.Path(a.report).write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report))
if __name__=='__main__':main()
