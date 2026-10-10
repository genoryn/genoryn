#!/usr/bin/env python3
"""Real localhost regression: a full stale cache must not hide an explicit peer."""
import argparse,json,os,pathlib,socket,subprocess,time,urllib.request

def main():
 parser=argparse.ArgumentParser();parser.add_argument('--bin-dir',default='target/debug');parser.add_argument('--work-dir',required=True);parser.add_argument('--report',required=True);args=parser.parse_args()
 root=pathlib.Path(args.work_dir).resolve();root.mkdir(parents=True,exist_ok=False)
 binary=pathlib.Path(args.bin_dir).resolve()/('genorynd.exe'if os.name=='nt'else'genorynd')
 owned=[];processes=[];logs=[];opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
 def reserve():
  sock=socket.socket();sock.bind(('127.0.0.1',0));owned.append(sock);return sock
 def info(port):
  with opener.open(f'http://127.0.0.1:{port}/v1/info',timeout=2)as response:return json.load(response)
 def start(name,p2p_socket=None,peer=None,cache=None):
  directory=root/name;directory.mkdir()
  if cache is not None:(directory/'peers.json').write_text(json.dumps(cache),encoding='utf-8')
  listener=p2p_socket or reserve();rpc_socket=reserve();p2p=listener.getsockname()[1];rpc=rpc_socket.getsockname()[1];listener.close();rpc_socket.close()
  log=open(root/f'{name}.log','wb');logs.append(log)
  command=[str(binary),'--network','devnet','--data-dir',str(directory),'--p2p-listen',f'127.0.0.1:{p2p}','--rpc-listen',f'127.0.0.1:{rpc}']
  if peer:command+=['--peer',peer]
  process=subprocess.Popen(command,stdout=log,stderr=log,env=dict(os.environ,RUST_LOG='info',RAYON_NUM_THREADS='2'));processes.append(process)
  until=time.monotonic()+30
  while time.monotonic()<until:
   assert process.poll()is None,'node exited during startup'
   try:info(rpc);return rpc
   except OSError:time.sleep(.1)
  raise AssertionError('node did not start within 30 seconds')
 report={'schema':1,'scope':'owned loopback sockets only','real_tcp_peers':2,'mining':False,'payments':False,'passed':False};started=time.monotonic()
 try:
  sockets=sorted([reserve()for _ in range(129)],key=lambda sock:sock.getsockname()[1]);stale=[f'127.0.0.1:{sock.getsockname()[1]}'for sock in sockets[:-1]];live=sockets[-1];address=f'127.0.0.1:{live.getsockname()[1]}'
  target=start('target',p2p_socket=live);focal=start('focal',peer=address,cache=stale)
  until=time.monotonic()+15
  while info(focal)['peer_count']==0 and time.monotonic()<until:time.sleep(.1)
  assert info(focal)['peer_count']>0,'full stale cache excluded the explicit live peer'
  # The first dial tick persists the bounded cache before connecting.
  persisted=json.loads((root/'focal/peers.json').read_text(encoding='utf-8'))
  assert len(persisted)<=128,'persistent cache exceeded its resource bound'
  assert address in persisted,'explicit peer was lost from persistent discovery'
  assert info(focal)['genesis_hash']==info(target)['genesis_hash']
  report.update(passed=True,stale_cached_peers=128,explicit_peer_connected=True,explicit_peer_preserved=True,persisted_count=len(persisted))
 except Exception as error:report['error']=str(error);raise
 finally:
  for process in processes:
   if process.poll()is None:process.terminate()
  for process in processes:
   try:process.wait(timeout=10)
   except subprocess.TimeoutExpired:process.kill();process.wait()
  for log in logs:log.close()
  for sock in owned:sock.close()
  report['elapsed_seconds']=round(time.monotonic()-started,3);output=pathlib.Path(args.report);output.parent.mkdir(parents=True,exist_ok=True);output.write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8');print(json.dumps(report),flush=True)
if __name__=='__main__':main()
