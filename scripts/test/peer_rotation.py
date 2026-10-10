#!/usr/bin/env python3
"""Bounded real TCP rotation test. No mining, wallet creation, or payments."""
import argparse,json,pathlib,time
from devnet import Lab

def events(root,name):
 result=[]
 for line in (root/f'{name}.log').read_text(encoding='utf-8',errors='replace').splitlines():
  try:result.append(json.loads(line).get('fields',{}).get('message',''))
  except ValueError:pass
 return result

def main():
 parser=argparse.ArgumentParser();parser.add_argument('--bin-dir',default='target/debug');parser.add_argument('--work-dir',required=True);parser.add_argument('--report',required=True);parser.add_argument('--max-wait',type=int,default=1900);args=parser.parse_args()
 if not 900<=args.max_wait<=2100:raise SystemExit('Choose a bounded observation window between 900 and 2100 seconds')
 root=pathlib.Path(args.work_dir)
 if root.exists()and any(root.iterdir()):raise SystemExit('Use a fresh empty work directory')
 args.keep=True;lab=Lab(args);start=time.monotonic();report={'schema':1,'real_tcp_peers':4,'mining':False,'payments':False,'passed':False}
 try:
  for index,name in enumerate('abcd'):lab.start(name,list('abcd'[:index]))
  lab.wait(lambda:all(lab.get(n,'info')['peer_count']>=2 for n in 'abcd'),timeout=90,description='redundant real TCP peers')
  report['initial_peer_counts']={n:lab.get(n,'info')['peer_count']for n in 'abcd'}
  genesis={lab.get(n,'info')['genesis_hash']for n in 'abcd'};assert len(genesis)==1
  print(json.dumps({'phase':'observing_default_15_to_30_minute_rotation','initial_peer_counts':report['initial_peer_counts']}),flush=True)
  until=time.monotonic()+args.max_wait;rotated=[]
  while time.monotonic()<until:
   assert all(process.poll()is None for process in lab.nodes.values()),'node exited while observing rotation'
   rotated=[n for n in 'abcd'if 'rotating outbound peer connection'in events(lab.root,n)]
   if rotated:break
   time.sleep(5)
  assert rotated,'no default-policy rotation observed within the bounded window'
  def reconnected():
   for name in rotated:
    log=events(lab.root,name);index=log.index('rotating outbound peer connection')
    if 'peer connected'not in log[index+1:]:return False
   return all(lab.get(n,'info')['peer_count']>=2 for n in 'abcd')
  lab.wait(reconnected,timeout=90,description='rotated peers reconnect with redundant connectivity')
  report.update(passed=True,rotated_nodes=rotated,final_peer_counts={n:lab.get(n,'info')['peer_count']for n in 'abcd'},same_genesis=True)
 except Exception as error:
  report['error']=str(error);raise
 finally:
  for name in list(lab.nodes):lab.stop(name)
  report['elapsed_seconds']=round(time.monotonic()-start,3)
  output=pathlib.Path(args.report);output.parent.mkdir(parents=True,exist_ok=True);output.write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8');print(json.dumps(report),flush=True)
if __name__=='__main__':main()
