#!/usr/bin/env python3
"""Real-process devnet release checks. Never uses a mock consensus/proof backend."""
import argparse, datetime, json, os, pathlib, socket, subprocess, sys, threading, time, urllib.request, urllib.error, shutil
from contextlib import closing

def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0)); return s.getsockname()[1]

QUICK_ONION = "genorynquickmodetestsourcexxxxxxxxxxxxxxxxxxxxxxxxxxxx.onion"

class OnionProxy:
    """Stand-in for Tor's SOCKS port: forwards one onion name to a local node and
    refuses everything else, so Quick mode is exercised without the Tor network."""
    def __init__(self, target_port):
        self.target_port = target_port; self.requests = []; self.closed = False
        self.server = socket.socket(); self.server.bind(("127.0.0.1", 0)); self.server.listen(32)
        self.address = "127.0.0.1:%d" % self.server.getsockname()[1]
        self.acceptor = threading.Thread(target=self.accept, daemon=True); self.acceptor.start()
    def accept(self):
        while True:
            try: client, _ = self.server.accept()
            except OSError: return
            if self.closed: client.close(); return
            threading.Thread(target=self.serve, args=(client,), daemon=True).start()
    def close(self):
        """Stops listening for real. On Linux, close() alone does not stop a socket
        that another thread is blocked accepting on: the listener stays open until
        accept() returns. shutdown() wakes it first."""
        self.closed = True
        try: self.server.shutdown(socket.SHUT_RDWR)
        except OSError: pass
        self.server.close(); self.acceptor.join(timeout=10)
        assert not self.acceptor.is_alive(), "the SOCKS stand-in kept listening"
        probe = socket.socket()
        try: refused = probe.connect_ex(("127.0.0.1", int(self.address.rsplit(":", 1)[1]))) != 0
        finally: probe.close()
        assert refused, "the SOCKS stand-in still accepts connections"
    @staticmethod
    def read(sock, n):
        data = b""
        while len(data) < n:
            chunk = sock.recv(n - len(data))
            if not chunk: raise OSError("closed")
            data += chunk
        return data
    def serve(self, client):
        try:
            version, count = self.read(client, 2); methods = self.read(client, count)
            if 2 in methods:
                client.sendall(b"\x05\x02"); _, ulen = self.read(client, 2); self.read(client, ulen)
                plen = self.read(client, 1)[0]; self.read(client, plen); client.sendall(b"\x01\x00")
            else:
                client.sendall(b"\x05\x00")
            _, command, _, atyp = self.read(client, 4)
            if atyp != 3: client.sendall(b"\x05\x08\x00\x01" + b"\x00" * 6); return
            host = self.read(client, self.read(client, 1)[0]).decode(); self.read(client, 2)
            self.requests.append(host)
            if host != QUICK_ONION: client.sendall(b"\x05\x04\x00\x01" + b"\x00" * 6); return
            upstream = socket.create_connection(("127.0.0.1", self.target_port))
            client.sendall(b"\x05\x00\x00\x01" + b"\x00" * 6)
            def pump(a, b):
                try:
                    while True:
                        data = a.recv(65536)
                        if not data: break
                        b.sendall(data)
                except OSError: pass
                finally:
                    try: b.shutdown(socket.SHUT_WR)
                    except OSError: pass
            threading.Thread(target=pump, args=(upstream, client), daemon=True).start()
            pump(client, upstream)
        except OSError:
            pass

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
        request=dict(op=op,network="devnet",wallet_path=str(self.root/f"{name}.wallet"),password="GENORYN-local-test-only-7!",rpc_url=self.url(node),token_file=str(self.root/node/"admin.token"))
        request.update(fields)
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
        # Blocks 1-4 pay the early subsidy of exactly 1,000 GNR each.
        assert summary["balance_atomic"]==str(4*1000*10**12),summary["balance_atomic"]
        mined={r["height"]:r["status"] for r in summary["mined"]}
        assert mined=={1:"Mature",2:"Mature",3:"JustMined",4:"JustMined"},mined
        self.record("early_subsidy_and_reward_maturity_statuses",mined=mined)
        supply=self.get("a","supply");calculator=json.loads(subprocess.check_output([self.exe("genoryn-emission"),"--height","4"],env=self.env))
        assert supply==calculator
        self.record("real_randomx_mining_and_supply",height=4,issued_gori=supply["issued_supply"])
        sent=self.wallet("alice","send",to=bob_payment_address,amount_atomic="1000000000000",fee_atomic="10000000",memo="private devnet invoice")
        assert sent["acknowledged"] is True and sent["status"]=="Submitted",sent
        pending=self.wallet("alice","sync")
        assert [e["status"] for e in pending["outbox"]]==["Submitted"] and int(pending["reserved_atomic"])>0,pending["outbox"]
        self.wait(lambda:self.get("b","info")["mempool_count"]==1,description="transaction gossip")
        self.mine("a",alice["address"],1);self.sync("abc",5)
        settled=self.wallet("alice","sync")
        assert settled["outbox"][0]["status"]=="Confirmed" and settled["outbox"][0]["confirmed_height"]==5,settled["outbox"]
        assert settled["reserved_atomic"]=="0"
        self.record("durable_outbox_confirms_and_releases_reservation",txid=sent["txid"])
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
        # One-step proof: the payment's own output is found automatically and the
        # sender's outgoing cipher key shows the paying wallet made it.
        auto=self.wallet("alice","export_proof",txid=sent["txid"])
        assert auto["recipient"]==bob_payment_address and auto.get("sender_key") and auto["mined_at"]==5,auto
        auto.pop("mined_at")
        checked=self.wallet("bob","verify_proof",node="b",proof=auto,expected_recipient=bob_payment_address)
        assert checked["created_by_sender"] is True and checked["recipient_matches"] is True and checked["belongs_to_wallet"] is True,checked
        assert checked["memo"]=="private devnet invoice" and checked["block_height"]==5 and checked["confirmations"]>=1,checked
        other=self.wallet("carol","verify_proof",node="c",proof=auto,expected_recipient=carol["address"])
        assert other["recipient_matches"] is False and other["belongs_to_wallet"] is False,other
        forged=dict(auto);forged["sender_key"]=[7]*32
        try:
            self.wallet("carol","verify_proof",node="c",proof=forged);raise RuntimeError("forged sender key accepted")
        except AssertionError as error:assert "sender part" in str(error),error
        received=self.wallet("bob","export_proof",node="b",txid=sent["txid"])
        assert received["recipient"]==bob_payment_address and "sender_key" not in received,received
        received.pop("mined_at")
        assert self.wallet("carol","verify_proof",node="c",proof=received)["created_by_sender"] is False
        self.record("one_step_sender_proof_address_check_and_forgery_rejected")
        self.proofs={"sender":auto,"received":received}
        self.sender_challenge(auto,sent,bob_payment_address)
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
        self.lost_acknowledgement(alice,carol)
        self.quick_mode(alice,carol)
        self.batch_sync()
        self.combine_notes(alice)
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
            # The devnet test payment's proofs, for checking verifiers such as genoryn.io/verify.
            for kind,proof in self.proofs.items():(self.root/f"proof-{kind}.json").write_text(json.dumps(proof))
    def sender_challenge(self,proof,sent,bob_address):
        """Payment Proof 2.0 through the real CLI: Bob checks the payment, sends a
        fresh challenge, and only Alice's wallet can answer it. Bob's and Carol's
        wallets cannot, and a copied answer does not satisfy a new challenge."""
        amount="1000000000000"
        checked=self.wallet("bob","verify_proof",node="b",proof=proof,expected_recipient=bob_address,expected_amount_atomic=amount,structured_outcomes=True)
        assert checked["outcome"]=="payment_verified" and checked["sender_evidence"]=="fresh_authentication_available",checked
        assert checked["amount_matches"] is True and checked["recipient_matches"] is True,checked
        ask=lambda:self.wallet("bob","create_payment_challenge",txid=sent["txid"],action=proof["action"],recipient=bob_address,amount_atomic=amount)["challenge"]
        challenge=ask();assert challenge.startswith("GNRCHAL1-"),challenge
        for impostor,node in [("bob","b"),("carol","c")]:
            try:self.wallet(impostor,"answer_payment_challenge",node=node,challenge=challenge);raise RuntimeError(impostor+" answered for the sender")
            except AssertionError as error:assert "did not make this payment" in str(error),error
        answer=self.wallet("alice","answer_payment_challenge",challenge=challenge)
        assert answer["response"].startswith("GNRRESP1-") and answer["txid"]==sent["txid"] and answer["amount_atomic"]==amount,answer
        with_proof=self.wallet("bob","verify_payment_challenge",node="b",challenge=challenge,response=answer["response"],proof=proof)
        assert with_proof["outcome"]=="sender_verified" and with_proof["confirmations"]>=1,with_proof
        from_own_note=self.wallet("bob","verify_payment_challenge",node="b",challenge=challenge,response=answer["response"])
        assert from_own_note["outcome"]=="sender_verified",from_own_note
        try:self.wallet("bob","verify_payment_challenge",node="b",challenge=ask(),response=answer["response"],proof=proof);raise RuntimeError("a copied answer was accepted")
        except AssertionError as error:assert "different challenge" in str(error),error
        missing=dict(proof);missing["txid"]="00"*32
        absent=self.wallet("bob","verify_proof",node="b",proof=missing,structured_outcomes=True)
        assert absent["outcome"]=="not_on_chain",absent
        self.record("payment_proof_2_fresh_sender_challenge_impostors_and_replay_rejected")
    def lost_acknowledgement(self,alice,carol):
        """The node refuses the first relay (as if its answer were lost); the
        payment stays stored and reserved, and the same signed bytes are relayed
        again later. It confirms once, with no second spend."""
        wrong=self.root/"wrong.token";wrong.write_text("0"*64)
        height=self.get("a","info")["height"]
        before=int(self.wallet("carol","sync")["balance_atomic"])
        first=self.wallet("alice","send",to=carol["address"],amount_atomic="300000000000",fee_atomic="10000000",token_file=str(wrong))
        assert first["status"]=="Pending" and first["acknowledged"] is False,first
        assert self.get("a","info")["mempool_count"]==0,"nothing reached the node"
        held=self.wallet("alice","sync",token_file=str(wrong))
        assert held["outbox"][0]["txid"]==first["txid"] and held["outbox"][0]["status"]=="Pending"
        assert int(held["reserved_atomic"])>0,"its coins stay reserved"
        time.sleep(16)
        retried=self.wallet("alice","sync")
        entry=[e for e in retried["outbox"] if e["txid"]==first["txid"]][0]
        assert entry["status"]=="Submitted" and entry["attempts"]>=2,entry
        self.mine("a",alice["address"],1);self.sync("abc",height+1)
        done=self.wallet("alice","sync")
        entry=[e for e in done["outbox"] if e["txid"]==first["txid"]][0]
        assert entry["status"]=="Confirmed",entry
        after=int(self.wallet("carol","sync")["balance_atomic"])
        assert after-before==300000000000,(before,after)
        self.record("lost_acknowledgement_retries_same_bytes_without_double_spend",txid=first["txid"])
    def quick_mode(self,alice,carol):
        """Quick mode over a SOCKS stand-in for Tor: the chain is downloaded from a
        remote node, validated here, kept, and later extended incrementally."""
        proxy=OnionProxy(self.ports["b"][1])
        quick=dict(light=True,sources=[f"http://{QUICK_ONION}:{self.ports['b'][1]}"],tor_socks=proxy.address)
        full=self.wallet("alice","sync")
        # A separate restored copy of Alice's wallet, scanned only through Quick mode.
        self.wallet("alice-quick","restore",seed_phrase=alice["seed_phrase"])
        first=self.wallet("alice-quick","sync",**quick)
        assert first["mode"]=="quick" and first["stale"] is False,first.get("light_sources")
        assert first["balance_atomic"]==full["balance_atomic"] and first["height"]==full["height"]
        assert first["light_sources"][0]["status"]=="used",first["light_sources"]
        assert all(host==QUICK_ONION for host in proxy.requests),"only the onion source was contacted"
        height=self.get("a","info")["height"]
        before=int(self.wallet("carol","sync")["balance_atomic"])
        sent=self.wallet("alice-quick","send",to=carol["address"],amount_atomic="50000000000",fee_atomic="10000000",**quick)
        assert sent["acknowledged"] is True,sent
        self.wait(lambda:self.get("a","info")["mempool_count"]==1,description="quick payment gossip to the miner")
        self.mine("a",alice["address"],1);self.sync("abc",height+1)
        again=self.wallet("alice-quick","sync",**quick)
        stats=again["sync_stats"]
        assert stats["replayed_local_blocks"]==height and stats["fork_height"]==height,stats
        assert [e["status"] for e in again["outbox"] if e["txid"]==sent["txid"]]==["Confirmed"]
        assert int(self.wallet("carol","sync")["balance_atomic"])-before==50000000000
        switched=self.wallet("alice-quick","sync")
        assert switched["balance_atomic"]==again["balance_atomic"],"Full and Quick agree after switching"
        assert switched["balance_atomic"]==self.wallet("alice","sync")["balance_atomic"]
        proxy.close()
        dead=self.wallet("alice-quick","sync",**quick)
        assert dead["stale"] is True and dead["balance_atomic"]==again["balance_atomic"],"stale, never zero"
        self.record("quick_mode_validates_locally_sends_and_syncs_incrementally",replay_ms=stats["replay_ms"],total_ms=stats["total_ms"])
    def combine_notes(self,alice):
        """Self-transfers merge up to 16 notes each; only the fee is spent. A
        proof can be made before mining from the stored payment. A recovery
        phrase that is already open is never written to a second file."""
        before=self.wallet("alice","sync");notes=before["spendable_notes"]
        assert notes>=2 and int(before["max_single_payment_atomic"])>0,before
        combined=self.wallet("alice","combine_notes")
        assert combined["transactions"]>=1 and combined["notes_combined"]>=2,combined
        pending=self.wallet("alice","export_proof",txid=combined["txids"][0])
        assert pending["mined_at"] is None and pending.get("sender_key"),"proof from the outbox before mining"
        height=self.get("a","info")["height"]
        self.wait(lambda:self.get("a","info")["mempool_count"]==combined["transactions"],description="combine transfers reach the miner")
        self.mine("a",alice["address"],1);self.sync("abc",height+1)
        after=self.wallet("alice","sync")
        # Alice mined the block, so the fees come back in its reward: exactly +1,000 GNR.
        assert int(after["balance_atomic"])==int(before["balance_atomic"])+1000*10**12,(before["balance_atomic"],after["balance_atomic"])
        assert after["spendable_notes"]<=notes-combined["notes_combined"]+combined["transactions"]+2,(notes,after["spendable_notes"])
        assert all(e["status"]=="Confirmed" for e in after["outbox"] if e["txid"] in combined["txids"])
        try:
            self.wallet("alice-twice","restore",seed_phrase=alice["seed_phrase"],refuse_addresses=[alice["address"]]);raise RuntimeError("duplicate wallet written")
        except AssertionError as error:assert "already open" in str(error),error
        assert not (self.root/"alice-twice.wallet").exists()
        self.record("combine_notes_and_duplicate_wallets_refused",notes_before=notes,notes_after=after["spendable_notes"])
    def batch_sync(self):
        """One sync_batch call scans several wallets from the node's blocks once;
        each result equals that wallet's own scan, and a wrong password or a
        missing file fails only that entry."""
        names=["alice","bob","carol","recovered"]
        singles={n:self.wallet(n,"sync") for n in names}
        password="GENORYN-local-test-only-7!"
        entries=[dict(wallet_path=str(self.root/f"{n}.wallet"),password=password) for n in names]
        entries+=[dict(wallet_path=str(self.root/"alice.wallet"),password="not-the-password-42"),dict(wallet_path=str(self.root/"absent.wallet"),password=password)]
        batch=self.wallet("alice","sync_batch",wallets=entries)
        rows=batch["wallets"];assert batch["stale"] is False and batch["mode"]=="full",batch
        for name,row in zip(names,rows):
            single=singles[name]
            assert row["ok"] and row["wallet_path"].endswith(f"{name}.wallet"),row
            for key in ["balance_atomic","spendable_atomic","height","address"]:assert row[key]==single[key],(name,key,row[key],single[key])
        assert rows[4]["ok"] is False and rows[5]["ok"] is False,rows[4:]
        assert all("history" not in row and "mined" not in row for row in rows),"compact summaries"
        self.record("batch_sync_matches_single_wallet_scans",wallets=len(names))
    def reorg(self,address):
        # Snapshot a stopped peer, mine different genuine-work branches, reconnect.
        import sqlite3
        self.stop("c")
        (self.root/"d").mkdir(exist_ok=True)
        with closing(sqlite3.connect(self.root/"c"/"chain.sqlite"))as source,closing(sqlite3.connect(self.root/"d"/"chain.sqlite"))as target:source.backup(target)
        self.start("d")
        base=self.get("a","info")["height"]
        self.mine("a",address,1);self.sync("ab",base+1)
        seen=self.wallet("alice","sync")
        replaced_hash=[r for r in seen["mined"] if r["height"]==base+1][0]["block_hash"]
        self.mine("d",address,3);assert self.get("d","info")["height"]==base+3
        branch_tip=self.get("d","info")["best_hash"]
        self.stop("d");self.start("d",["a"])
        self.wait(lambda:self.get("a","info")["best_hash"]==branch_tip,timeout=360,description="heavier fork reorganization")
        self.start("c",["a"]);self.sync("abc",base+3)
        # The batch meets the reorganization first: it rolls Alice back exactly
        # as her own scan would.
        batched=self.wallet("alice","sync_batch",wallets=[dict(wallet_path=str(self.root/"alice.wallet"),password="GENORYN-local-test-only-7!")])["wallets"][0]
        after=self.wallet("alice","sync")
        assert batched["ok"] and batched["height"]==after["height"]==base+3,batched
        assert batched["balance_atomic"]==after["balance_atomic"],(batched["balance_atomic"],after["balance_atomic"])
        at=[r for r in after["mined"] if r["height"]==base+1]
        statuses={r["block_hash"]:r["status"] for r in at}
        assert statuses.get(replaced_hash)=="Replaced",statuses
        assert len(at)==2,"the replaced reward is kept beside the new canonical one"
        assert sum(1 for r in after["mined"] if r["status"]=="Replaced")==1
        self.record("real_competing_chain_reorganization_keeps_replaced_reward",height=base+3)
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
