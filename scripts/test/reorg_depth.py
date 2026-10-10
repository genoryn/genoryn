#!/usr/bin/env python3
"""Genuine-work deeper reorg and wallet rollback lab, using valueless prior fixtures."""
import argparse,json,pathlib,shutil,sqlite3,subprocess
from contextlib import closing
from devnet import Lab

def snapshot(source,destination):
    destination.parent.mkdir(parents=True,exist_ok=True)
    with closing(sqlite3.connect(f'file:{source}?mode=ro',uri=True))as src,closing(sqlite3.connect(destination))as dst:src.backup(dst)

def main():
    p=argparse.ArgumentParser();p.add_argument('--source-lab',required=True);p.add_argument('--work-dir',required=True);p.add_argument('--bin-dir',default='target/debug');p.add_argument('--report',required=True);p.add_argument('--depth',type=int,default=5);a=p.parse_args()
    assert 1<=a.depth<=20,'choose a bounded depth from 1 to 20'
    source=pathlib.Path(a.source_lab).resolve();root=pathlib.Path(a.work_dir).resolve()
    if root.exists()and any(root.iterdir()):raise SystemExit('Use a fresh empty work directory')
    a.quick=False;a.keep=True;lab=Lab(a)
    for name in ['alice','bob']:
        shutil.copy2(source/f'{name}.wallet',root/f'{name}.wallet')
    for node in ['a','b']:snapshot(source/'a/chain.sqlite',root/node/'chain.sqlite')
    try:
        lab.start('a');lab.start('b')
        base=lab.get('a','info')['height'];assert lab.get('b','info')['height']==base
        alice=lab.wallet('alice','open')['address'];bob=lab.wallet('bob','open')['address']
        baseline=lab.wallet('bob','sync')['balance_atomic']
        payment=lab.wallet('alice','send',to=bob,amount_atomic='250000000000',fee_atomic='10000000')
        lab.mine('a',alice,a.depth)
        confirmed=lab.wallet('bob','sync')['balance_atomic'];assert int(confirmed)==int(baseline)+250000000000
        lab.mine('b',alice,a.depth+2)
        winning_tip=lab.get('b','info')['best_hash']
        lab.stop('b');lab.start('b',['a'])
        lab.wait(lambda:lab.get('a','info')['best_hash']==winning_tip,timeout=480,description='deeper heavier-work reorg')
        rolled_back=lab.wallet('bob','sync')['balance_atomic'];assert rolled_back==baseline
        assert lab.get('a','info')['mempool_count']>=1,'orphaned private payment should be reconsidered'
        entries=[]
        for line in (root/'a.log').read_text(errors='replace').splitlines():
            try:entries.append(json.loads(line))
            except ValueError:pass
        depths=[e.get('fields',{}).get('reorg_depth',0)for e in entries]
        assert max(depths)>=a.depth,(depths,a.depth)
        lab.record('private_payment_rollback_and_mempool_reconsideration',depth=a.depth,txid=payment['txid'],height=base+a.depth+2)
        lab.mine('a',alice,1);lab.sync('ab',base+a.depth+3)
        reapplied=lab.wallet('bob','sync')['balance_atomic'];assert int(reapplied)==int(baseline)+250000000000
        lab.record('orphaned_private_payment_remined_once',depth=a.depth)
    except Exception as e:
        lab.checks.append(dict(check='deep_reorg_completion',passed=False,error=str(e)));raise
    finally:lab.finish()
if __name__=='__main__':main()
