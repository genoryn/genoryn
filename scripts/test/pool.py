#!/usr/bin/env python3
"""Shared mining on a real devnet: real nodes, real RandomX, real Halo 2 payouts.

A weak (1 thread) and a strong (3 threads) miner mine through genoryn-pool.
Checks: hostile shares are refused, every pool block is credited only after
maturity, both miners earn in proportion to verified work, the ledger balances
exactly, a payout is an ordinary private transfer both miners receive, a
repeated payout run pays nothing twice, and a pool restart keeps every number.
"""
import argparse,json,os,pathlib,socket,subprocess,time,urllib.request

PASSWORD="GENORYN-pool-test-only-7!"

def free_port():
    with socket.socket() as s:s.bind(("127.0.0.1",0));return s.getsockname()[1]

class Lab:
    def __init__(self,args):
        self.root=pathlib.Path(args.work_dir).resolve();self.root.mkdir(parents=True,exist_ok=True)
        self.bin=pathlib.Path(args.bin_dir).resolve();self.suffix=".exe" if os.name=="nt" else ""
        self.env=dict(os.environ,RAYON_NUM_THREADS="2",RUST_LOG="warn")
        self.p2p,self.rpc,self.pool_port,self.api_port=free_port(),free_port(),free_port(),free_port()
        self.processes={};self.checks=[];self.report=pathlib.Path(args.report)
    def exe(self,name):return str(self.bin/(name+self.suffix))
    def record(self,name,**details):
        self.checks.append(dict(check=name,passed=True,**details));print(json.dumps(self.checks[-1]),flush=True)
    def wait(self,fn,timeout=300,description="condition"):
        until=time.monotonic()+timeout;last=None
        while time.monotonic()<until:
            try:
                value=fn()
                if value:return value
            except Exception as error:last=repr(error)[:200]
            time.sleep(0.5)
        raise AssertionError(f"Timed out: {description} ({last})")
    def get(self,path):
        with urllib.request.urlopen(f"http://127.0.0.1:{self.rpc}/v1/{path}",timeout=60) as r:return json.load(r)
    def api(self,path):
        with urllib.request.urlopen(f"http://127.0.0.1:{self.api_port}/v1/pool/{path}",timeout=60) as r:return json.load(r)
    def spawn(self,name,args):
        log=open(self.root/f"{name}.log","ab")
        self.processes[name]=(subprocess.Popen(args,stdout=log,stderr=log,env=self.env),log)
    def stop(self,name):
        process,log=self.processes.pop(name)
        process.kill();process.wait(timeout=30);log.close()
    def wallet(self,name,op,**fields):
        request=dict(op=op,network="devnet",wallet_path=str(self.root/f"{name}.wallet"),password=PASSWORD,rpc_url=f"http://127.0.0.1:{self.rpc}",token_file=str(self.root/"node"/"admin.token"))
        request.update(fields)
        p=subprocess.run([self.exe("genoryn-wallet-cli"),"--json-stdio"],input=json.dumps(request),capture_output=True,text=True,env=self.env,timeout=900)
        result=json.loads(p.stdout)
        if not result.get("ok"):raise AssertionError(f"wallet {op}: {result.get('error')}")
        return result["result"]
    def solo(self,address,blocks,threads=4):
        p=subprocess.run([self.exe("genoryn-miner"),"--network","devnet","--node",f"http://127.0.0.1:{self.rpc}","--token-file",str(self.root/"node"/"admin.token"),"--wallet",address,"--threads",str(threads),"--randomx-mode","light","--max-blocks",str(blocks),"--max-seconds","1200"],capture_output=True,text=True,env=self.env,timeout=1300)
        assert p.returncode==0,p.stderr[-1500:]
    def pool_args(self,command):
        args=[self.exe("genoryn-pool"),command,"--network","devnet","--ledger",str(self.root/"pool.sqlite"),"--node",f"http://127.0.0.1:{self.rpc}","--token-file",str(self.root/"node"/"admin.token"),
              "--fee-bps","100","--payout-threshold","100000000","--initial-share-difficulty","200","--min-share-difficulty","20","--share-seconds","4"]
        return args
    def serve(self):
        self.spawn("pool",self.pool_args("serve")+["--pool-address",self.pool_address,"--listen",f"127.0.0.1:{self.pool_port}","--api",f"127.0.0.1:{self.api_port}","--verify-threads","2"])
        self.wait(lambda:self.api("status")["open"],120,"pool open")
    def raw_client(self):
        """A hand-written client sending hostile shares."""
        sock=socket.create_connection(("127.0.0.1",self.pool_port),timeout=30);reader=sock.makefile("r")
        def send(obj):sock.sendall((json.dumps(obj)+"\n").encode())
        def until(predicate):
            while True:
                message=json.loads(reader.readline())
                if predicate(message):return message
        send({"id":1,"method":"login","params":{"address":"gnrd1notanaddress"}})
        assert until(lambda m:m.get("id")==1)["error"]["code"]=="invalid_address"
        send({"id":2,"method":"submit","params":{"job_id":"x","nonce":"0000000000000000"}})
        assert until(lambda m:m.get("id")==2)["error"]["code"]=="not_logged_in"
        send({"id":3,"method":"login","params":{"address":self.attacker}})
        job=until(lambda m:m.get("method")=="job")["params"]
        start=int.from_bytes(bytes.fromhex(job["nonce_start"]),"little")
        codes={}
        for request_id,(job_id,nonce) in enumerate([("nope",start),(job["job_id"],start+(1<<41)),(job["job_id"],start+5),(job["job_id"],start+5)],10):
            send({"id":request_id,"method":"submit","params":{"job_id":job_id,"nonce":nonce.to_bytes(8,"little").hex()}})
            reply=until(lambda m:m.get("id")==request_id)
            codes[request_id]=(reply.get("error") or {}).get("code","accepted")
        sock.sendall(b"{not json\n")
        assert reader.readline() and reader.readline()=="", "a malformed line closes the session"
        sock.close()
        assert codes[10]=="unknown_job" and codes[11]=="out_of_range",codes
        # The same nonce twice: the second is a duplicate whether or not the first met the difficulty.
        assert codes[13]=="duplicate" and codes[12] in ("low_difficulty","accepted"),codes
        return codes
    def run(self):
        (self.root/"node").mkdir(exist_ok=True)
        self.spawn("node",[self.exe("genorynd"),"--network","devnet","--data-dir",str(self.root/"node"),"--p2p-listen",f"127.0.0.1:{self.p2p}","--rpc-listen",f"127.0.0.1:{self.rpc}"])
        self.wait(lambda:self.get("info"),180,"node")
        names=["pool","weak","strong","solo","attacker"]
        addresses={name:self.wallet(name,"create")["address"] for name in names}
        self.pool_address,self.attacker=addresses["pool"],addresses["attacker"]
        # Raise the network difficulty well above the share difficulty first.
        self.solo(addresses["solo"],42)
        difficulty=int(self.get("info")["difficulty"]);assert difficulty>=1000,difficulty
        self.record("network_difficulty_raised_by_real_solo_mining",difficulty=difficulty,height=self.get("info")["height"])
        self.serve()
        self.record("hostile_shares_refused",codes=self.raw_client())
        common=["--network","devnet","--pool",f"tcp://127.0.0.1:{self.pool_port}","--randomx-mode","light","--max-seconds",str(self.args_seconds)]
        self.spawn("weak",[self.exe("genoryn-miner"),*common,"--wallet",addresses["weak"],"--threads","1","--worker","weak"])
        self.spawn("strong",[self.exe("genoryn-miner"),*common,"--wallet",addresses["strong"],"--threads","3","--worker","strong"])
        found=self.wait(lambda:self.api("status")["stats"]["blocks_found"]>=3 and self.api("status")["stats"]["blocks_found"],self.args_seconds+60,"three pool blocks")
        for name in ["weak","strong"]:
            self.processes[name][0].wait(timeout=self.args_seconds+120);self.processes[name][1].close();self.processes.pop(name)
        status=self.api("status");stats=status["stats"]
        assert stats["shares_accepted"]>0 and stats["busy_rejections"]==0,stats
        self.record("weak_and_strong_miners_submit_verified_shares",blocks_found=found,stats=stats)
        # Two more solo blocks mature every pool block (devnet maturity is two).
        self.solo(addresses["solo"],3)
        def matured():
            blocks=self.api("status")["recent_blocks"]
            pool_blocks=[b for b in blocks if b["status"] in ("mature","immature","submitted")]
            return pool_blocks and all(b["status"]=="mature" for b in pool_blocks) and blocks
        blocks=self.wait(matured,120,"pool blocks mature")
        audit=self.api("status")["audit"];assert audit["balanced"],audit
        view={name:self.api("miner?address="+addresses[name]) for name in ["weak","strong"]}
        weak_credit=int(view["weak"]["matured_fixed"]);strong_credit=int(view["strong"]["matured_fixed"])
        assert weak_credit>0 and strong_credit>weak_credit,view
        mature=[b for b in blocks if b["status"]=="mature"]
        assert all(b["reward"]>0 for b in mature)
        self.record("both_miners_credited_after_maturity_and_ledger_balances",mature_blocks=len(mature),weak_fixed=str(weak_credit),strong_fixed=str(strong_credit),
                    ratio=round(strong_credit/weak_credit,3))
        # Restart recovery: the same ledger, the same numbers.
        self.stop("pool");self.serve()
        again={name:self.api("miner?address="+addresses[name]) for name in ["weak","strong"]}
        assert again["weak"]["matured_fixed"]==view["weak"]["matured_fixed"] and again["strong"]["matured_fixed"]==view["strong"]["matured_fixed"],again
        assert self.api("status")["audit"]["balanced"]
        self.record("pool_restart_keeps_the_ledger")
        password=self.root/"pool.password";password.write_text(PASSWORD)
        payout=self.pool_args("payout")+["--wallet",str(self.root/"pool.wallet"),"--password-file",str(password),"--bin-dir",str(self.bin),"--once"]
        def payout_once():
            p=subprocess.run(payout,capture_output=True,text=True,env=self.env,timeout=900)
            assert p.returncode==0,p.stderr[-1500:]
            return json.loads(p.stdout.strip().splitlines()[-1])
        first=payout_once();assert first["sent"] and "txid" in first["sent"][0],first
        txid=first["sent"][0]["txid"]
        second=payout_once();assert all("txid" not in s or s["txid"]==txid for s in second["sent"]),second
        self.record("payout_planned_once_and_not_repeated",txid=txid,recipients=first["sent"][0]["recipients"])
        height=self.get("info")["height"]
        self.wait(lambda:self.get("info")["mempool_count"]>=1,60,"payout reaches the node")
        self.solo(addresses["solo"],1)
        confirmed=payout_once();assert confirmed["settled"] and confirmed["settled"][0].get("confirmed_height")==height+1,confirmed
        paid={name:int(self.api("miner?address="+addresses[name])["paid_gori"]) for name in ["weak","strong"]}
        received={name:int(self.wallet(name,"sync")["balance_atomic"]) for name in ["weak","strong"]}
        assert paid==received and all(v>0 for v in paid.values()),(paid,received)
        assert self.api("status")["audit"]["balanced"]
        self.record("private_payout_received_exactly_as_ledger_says",paid={k:str(v) for k,v in paid.items()})
    def close(self,passed):
        for name in list(self.processes):
            try:self.stop(name)
            except Exception:pass
        self.report.parent.mkdir(parents=True,exist_ok=True)
        self.report.write_text(json.dumps({"schema":1,"scope":"one devnet node, genoryn-pool on SQLite, two real light-mode miners; payouts are real Halo 2 transfers","checks":self.checks,"passed":passed},indent=2)+"\n")

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir",default="target/debug");parser.add_argument("--work-dir",required=True);parser.add_argument("--report",required=True)
    parser.add_argument("--mining-seconds",type=int,default=240)
    args=parser.parse_args();lab=Lab(args);lab.args_seconds=args.mining_seconds;passed=False
    try:lab.run();passed=True
    finally:lab.close(passed)

if __name__=="__main__":main()
