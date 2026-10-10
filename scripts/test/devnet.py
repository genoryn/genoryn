#!/usr/bin/env python3
"""Real-process devnet release checks. Never uses a mock consensus/proof backend."""
import argparse, datetime, json, os, pathlib, socket, subprocess, sys, time, urllib.request, urllib.error, shutil
from contextlib import closing

def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0)); return s.getsockname()[1]

class Lab:
    def __init__(self, args):
        self.args=args; self.root=pathlib.Path(args.work_dir).resolve(); self.root.mkdir(parents=True,exist_ok=True)
        self.bin=pathlib.Path(args.bin_dir).resolve(); self.suffix=".exe" if os.name=="nt" else ""
        self.nodes={}; self.logs={}; self.ports={n:(free_port(),free_port()) for n in "abcd"}; self.checks=[]; self.measurements=[]
        self.env=dict(os.environ,RAYON_NUM_THREADS="2",RUST_LOG="info")
    def exe(self,name): return str(self.bin/(name+self.suffix))
    def record(self,name,**details):
        self.checks.append(dict(check=name,passed=True,**details)); print(json.dumps(self.checks[-1]),flush=True)
    def url(self,n):return f"http://127.0.0.1:{self.ports[n][1]}"
    def start(self,n,peers=()):
        started=time.perf_counter()
        directory=self.root/n;directory.mkdir(exist_ok=True)
        log=open(self.root/f"{n}.log","ab");self.logs[n]=log
        args=[self.exe("genorynd"),"--network","devnet","--data-dir",str(directory),"--p2p-listen",f"127.0.0.1:{self.ports[n][0]}","--rpc-listen",f"127.0.0.1:{self.ports[n][1]}"]
        for p in peers:args.extend(["--peer",f"127.0.0.1:{self.ports[p][0]}"])
        self.nodes[n]=subprocess.Popen(args,stdout=log,stderr=log,env=self.env)
        self.wait(lambda:self.get(n,"info"),timeout=180,description=f"node {n} startup")
        self.measurements.append(dict(operation='node_startup_including_database_replay',seconds=time.perf_counter()-started))
    def stop(self,n):
        process=self.nodes.pop(n,None)
        if process is not None:
            process.kill();process.wait(timeout=20)
        log=self.logs.pop(n,None)
        if log:log.close()
    def get(self,n,path):
        with urllib.request.urlopen(self.url(n)+"/v1/"+path,timeout=600) as r:return json.load(r)
    def post(self,n,path,obj,authenticated=True):
        headers={"Content-Type":"application/json"}
        if authenticated:headers["Authorization"]="Bearer "+(self.root/n/"admin.token").read_text().strip()
        request=urllib.request.Request(self.url(n)+"/v1/"+path,data=json.dumps(obj).encode(),headers=headers,method="POST")
        try:
            with urllib.request.urlopen(request,timeout=600)as r:return r.status,json.load(r)
        except urllib.error.HTTPError as e:return e.code,json.loads(e.read())
    def wait(self,fn,timeout=240,description="condition"):
        until=time.monotonic()+timeout;last=None
        while time.monotonic()<until:
            try:
                result=fn()
                if result:return result
            except (OSError,ValueError,urllib.error.URLError) as e:last=type(e).__name__
            time.sleep(0.5)
        raise AssertionError(f"Timed out: {description} ({last})")
    def wallet(self,name,op,node="a",**fields):
        request=dict(op=op,network="devnet",wallet_path=str(self.root/f"{name}.wallet"),password="GENORYN-local-test-only-7!",rpc_url=self.url(node),token_file=str(self.root/node/"admin.token"),**fields)
        started=time.perf_counter()
        p=subprocess.run([self.exe("genoryn-wallet-cli"),"--json-stdio"],input=json.dumps(request),capture_output=True,text=True,env=self.env,timeout=600)
        if p.returncode:raise AssertionError(f"wallet {op} process failed: {p.stderr[-1000:]}")
        result=json.loads(p.stdout)
        if not result.get("ok"):raise AssertionError(f"wallet {op} rejected: {result.get('error')}")
        self.measurements.append(dict(operation='wallet_'+op,seconds=time.perf_counter()-started,scope='includes CLI process, password KDF, local RPC and operation; cold process cryptographic setup'))
        return result["result"]
    def mine(self,node,address,count):
        args=[self.exe("genoryn-miner"),"--network","devnet","--node",self.url(node),"--token-file",str(self.root/node/"admin.token"),"--wallet",address,"--threads","1","--max-blocks",str(count),"--max-seconds","900"]
        p=subprocess.run(args,capture_output=True,text=True,env=self.env,timeout=1000)
        assert p.returncode==0,f"miner failed: {p.stderr[-1000:]}"
        stats=json.loads(p.stdout.strip().splitlines()[-1]);assert stats["accepted_blocks"]==count,stats
        self.measurements.append(dict(operation='actual_mining',**{key:stats[key] for key in ['accepted_blocks','hashes','elapsed_seconds','template_seconds','hashing_seconds','submission_seconds'] if key in stats}))
        return stats
    def sync(self,names,height):
        self.wait(lambda:all(self.get(n,"info")["height"]>=height for n in names),description=f"nodes {names} height {height}")
        tips=[self.get(n,"info")["best_hash"]for n in names];assert len(set(tips))==1,tips
    def run(self):
        self.start("a");self.start("b",["a"]);self.start("c",["b"])
        self.wait(lambda:all(self.get(n,"info")["peer_count"]>0 for n in "abc"),description="three real TCP peers")
        genesis={self.get(n,"info")["genesis_hash"]for n in "abc"};assert len(genesis)==1
        self.record("three_real_peers_same_genesis",genesis=next(iter(genesis)))
        alice=self.wallet("alice","create");bob=self.wallet("bob","create");carol=self.wallet("carol","create")
        bob_payment_address=self.wallet('bob','address',address_index=7)['address']
        assert bob_payment_address!=bob['address']
        account=self.wallet('alice','derive_account',account=7,output=str(self.root/'alice-account.wallet'))
        recovered_account=self.wallet('recovered-account','restore',seed_phrase=alice['seed_phrase'],account=7)
        assert account['account']==recovered_account['account']==7 and account['address']==recovered_account['address'] and account['address']!=alice['address']
        self.record("three_encrypted_wallets_created")
        self.record('derived_account_and_receiving_index_recover')
        self.mine("a",alice["address"],4);self.sync("abc",4)
        summary=self.wallet("alice","sync");assert int(summary["spendable_atomic"])>10**12
        supply=self.get("a","supply");calculator=json.loads(subprocess.check_output([self.exe("genoryn-emission"),"--height","4"],env=self.env))
        assert supply==calculator
        self.record("real_randomx_mining_and_supply",height=4,issued_gori=supply["issued_supply"])
        sent=self.wallet("alice","send",to=bob_payment_address,amount_atomic="1000000000000",fee_atomic="10000000",memo="private devnet invoice")
        self.wait(lambda:self.get("b","info")["mempool_count"]==1,description="transaction gossip")
        self.mine("a",alice["address"],1);self.sync("abc",5)
        bobscan=self.wallet("bob","sync",node="b");carolscan=self.wallet("carol","sync",node="c")
        assert int(bobscan["balance_atomic"])==10**12;assert int(carolscan["balance_atomic"])==0
        blocks=self.get("a","blocks?from=5&limit=1");payment=blocks[0]["body"]["transactions"][0]
        raw=json.dumps(payment);assert bob_payment_address not in raw and "private devnet invoice"not in raw
        assert "amount"not in payment["bundle"] and "recipient"not in payment["bundle"]
        status,_=self.post("a","transactions",payment);assert status>=400
        altered=json.loads(json.dumps(payment));altered["bundle"]["proof"][0]^=1;status,_=self.post("a","transactions",altered);assert status>=400
        status,_=self.post("a","transactions",payment,authenticated=False);assert status>=400
        self.record("private_payment_gossip_scan_and_double_spend_rejection",txid=sent["txid"])
        proof=None
        for action in range(len(payment["bundle"]["actions"])):
            try:
                candidate=self.wallet("alice","export_proof",txid=sent["txid"],action=action)
                if candidate["recipient"]==bob_payment_address:proof=candidate;break
            except AssertionError:pass
        assert proof is not None
        verified=self.wallet("alice","verify_proof",proof=proof);assert verified["amount_atomic"]=="1000000000000"
        self.record("voluntary_payment_proof_verified")
        self.wallet("bob","send",to=carol["address"],amount_atomic="200000000000",fee_atomic="10000000")
        self.wallet('alice','send',to=alice['address'],amount_atomic='10000000000',fee_atomic='50000000')
        self.mine("a",alice["address"],1);self.sync("abc",6)
        fee_order=[tx['bundle']['value_balance'] for tx in self.get('a','blocks?from=6&limit=1')[0]['body']['transactions']]
        assert fee_order==[50000000,10000000],fee_order
        self.record('real_fee_priority_template_order',public_fees_gori=fee_order)
        assert self.wallet("carol","sync")["balance_atomic"]=="200000000000"
        self.record("recipient_spends_private_received_note")
        self.wallet("bob","register_username",username="bob_devnet")
        resolved=self.wallet("alice","resolve_username",username="bob_devnet");assert resolved["address"]==bob["address"]
        self.wallet("alice","send",to="@bob_devnet",amount_atomic="100000000000",fee_atomic="10000000")
        self.mine("a",alice["address"],1);self.sync("abc",7)
        assert "bob_devnet"not in json.dumps(self.get("a","blocks?from=7&limit=1")[0]["body"]["transactions"])
        self.record("signed_username_payment_without_plaintext_linkage")
        restored=self.wallet("recovered","restore",seed_phrase=alice["seed_phrase"]);assert restored["address"]==alice["address"]
        old=self.wallet("alice","sync");new=self.wallet("recovered","sync");assert old["balance_atomic"]==new["balance_atomic"]
        self.record("seed_recovery_restores_private_balance")
        self.stop("c");self.mine("a",alice["address"],2);self.sync("ab",9);self.start("c",["a"]);self.sync("abc",9)
        self.record("offline_node_catches_up_after_restart")
        if not self.args.quick:self.reorg(alice["address"])
        height=self.get("a","info")["height"];tip=self.get("a","info")["best_hash"]
        for n in "abc":self.stop(n)
        for n in "abc":self.start(n,["a"]if n!="a"else[])
        self.sync("abc",height);assert self.get("a","info")["best_hash"]==tip
        for n in "abc":
            text=(self.root/f"{n}.log").read_text(errors="replace");assert alice["seed_phrase"]not in text and "GENORYN-local-test-only-7!"not in text and "private devnet invoice"not in text
        self.record("abrupt_three_node_restart_and_no_secrets_in_logs",height=height)
        if self.args.keep:
            # Test-only public receiver metadata, no seed/password. Useful for native UI QA.
            (self.root/"lab-public.json").write_text(json.dumps({"ports":self.ports,"alice_address":alice["address"],"height":height},indent=2))
    def reorg(self,address):
        # Snapshot a stopped peer, mine different genuine-work branches, reconnect.
        import sqlite3
        self.stop("c")
        (self.root/"d").mkdir(exist_ok=True)
        with closing(sqlite3.connect(self.root/"c"/"chain.sqlite"))as source,closing(sqlite3.connect(self.root/"d"/"chain.sqlite"))as target:source.backup(target)
        self.start("d")
        self.mine("a",address,1);self.sync("ab",10)
        self.mine("d",address,3);assert self.get("d","info")["height"]==12
        branch_tip=self.get("d","info")["best_hash"]
        self.stop("d");self.start("d",["a"])
        self.wait(lambda:self.get("a","info")["best_hash"]==branch_tip,timeout=360,description="heavier fork reorganization")
        self.start("c",["a"]);self.sync("abc",12)
        self.record("real_competing_chain_reorganization",height=12)
    def finish(self):
        for n in list(self.nodes):self.stop(n)
        propagation=[]
        observations={}
        for name in 'abc':
            log=self.root/f'{name}.log'
            if not log.exists():continue
            for line in log.read_text(errors='replace').splitlines():
                try:
                    record=json.loads(line);fields=record.get('fields',{});height=fields.get('height')
                    if fields.get('message')=='accepted block' and isinstance(height,int) and 1<=height<=7:
                        stamp=datetime.datetime.fromisoformat(record['timestamp'].replace('Z','+00:00')).timestamp()
                        observations.setdefault(height,{}).setdefault(name,stamp)
                except (ValueError,KeyError,TypeError):pass
        for height,times in sorted(observations.items()):
            if len(times)==3:propagation.append(dict(height=height,three_node_acceptance_span_ms=(max(times.values())-min(times.values()))*1000))
        report={"schema":1,"real_processes":True,"real_pow":True,"real_zk_proofs":True,"checks":self.checks,"measurements":self.measurements,"propagation":propagation,"measurement_scope":"Single same-host development run. Acceptance span includes network relay, block validation and scheduling; not pure wire latency or a production throughput guarantee.","passed":all(c["passed"]for c in self.checks)}
        pathlib.Path(self.args.report).parent.mkdir(parents=True,exist_ok=True);pathlib.Path(self.args.report).write_text(json.dumps(report,indent=2)+"\n")

def main():
    p=argparse.ArgumentParser();p.add_argument("--bin-dir",default="target/debug");p.add_argument("--work-dir",default=".devnet/integration");p.add_argument("--report",default=".devnet/integration-report.json");p.add_argument("--quick",action="store_true");p.add_argument("--keep",action="store_true");args=p.parse_args()
    root=pathlib.Path(args.work_dir)
    if root.exists()and any(root.iterdir()):raise SystemExit("Use a fresh empty --work-dir; existing wallets are never overwritten.")
    lab=Lab(args)
    try:lab.run()
    except Exception as e:lab.checks.append(dict(check="scenario_completion",passed=False,error=str(e)));raise
    finally:lab.finish()
    if not args.keep:shutil.rmtree(lab.root)
if __name__=="__main__":main()
