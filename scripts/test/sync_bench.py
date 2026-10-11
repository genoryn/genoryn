#!/usr/bin/env python3
"""Wallet synchronization timings against a real, already synchronized node.

Measures one build (--bin-dir) the same way every time: a new wallet's first
Full-mode sync from genesis, an incremental sync with nothing new, and an
operation that needs the whole validated chain (a payment-proof check, which
is what sending and proofs require). Wallets are new and valueless; the node
is the operator's own (loopback admin RPC). Nothing is mined or sent.
"""
import argparse,json,os,pathlib,platform,subprocess,time,urllib.request

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir",required=True);parser.add_argument("--rpc",required=True);parser.add_argument("--token-file",required=True)
    parser.add_argument("--network",default="mainnet");parser.add_argument("--work-dir",required=True);parser.add_argument("--label",required=True)
    parser.add_argument("--report",required=True);parser.add_argument("--repeat",type=int,default=3)
    args=parser.parse_args()
    root=pathlib.Path(args.work_dir).resolve();root.mkdir(parents=True,exist_ok=True)
    exe=str(pathlib.Path(args.bin_dir).resolve()/("genoryn-wallet-cli"+(".exe" if os.name=="nt" else "")))
    with urllib.request.urlopen(args.rpc+"/v1/info",timeout=30) as r:info=json.load(r)
    def call(name,op,**fields):
        request=dict(op=op,network=args.network,wallet_path=str(root/f"{name}.wallet"),password="benchmark-only-password-1",rpc_url=args.rpc,token_file=args.token_file)
        request.update(fields)
        started=time.perf_counter()
        p=subprocess.run([exe,"--json-stdio"],input=json.dumps(request),capture_output=True,text=True,timeout=3600)
        seconds=time.perf_counter()-started
        return json.loads(p.stdout or "{}"),seconds
    results=[]
    for run in range(args.repeat):
        name=f"{args.label}-{run}-{int(time.time())}"
        created,_=call(name,"create")
        assert created.get("ok"),created
        first,first_s=call(name,"sync",birthday_height=0,full_rescan=True,rescan_height=0)
        assert first.get("ok"),first
        again,again_s=call(name,"sync")
        assert again.get("ok"),again
        proof={"version":1,"network":args.network.capitalize(),"txid":"00"*32,"action":0,"recipient":created["result"]["address"],"amount":1,"rho":[1]*32,"rseed":[2]*32,"cmx":[3]*32}
        checked,chain_s=call(name,"verify_proof",proof=proof)
        assert not checked.get("ok") and "not in a block" in checked.get("error",""),checked
        checked2,chain2_s=call(name,"verify_proof",proof=proof)
        results.append({"first_full_sync_seconds":first_s,"incremental_sync_seconds":again_s,"whole_chain_operation_seconds":chain_s,"whole_chain_operation_again_seconds":chain2_s,"height":first["result"]["height"]})
    def median(key):
        values=sorted(r[key] for r in results);return values[len(values)//2]
    report={"schema":1,"label":args.label,"node_height":info["height"],"network":args.network,
            "machine":{"os":platform.platform(),"cpu":platform.processor(),"logical_cpus":os.cpu_count()},
            "runs":results,"median":{k:median(k) for k in results[0] if k!="height"},
            "scope":"wallet CLI process times including Argon2id unlock and process start; the node is local and already synchronized"}
    pathlib.Path(args.report).write_text(json.dumps(report,indent=2)+"\n");print(json.dumps(report["median"]))

if __name__=="__main__":main()
