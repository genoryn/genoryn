#!/usr/bin/env python3
"""Bounded read-only RPC capacity measurement against an isolated owned node.

No remote target is accepted. These measurements are not chain TPS or a claim
that ten thousand active wallets/nodes have been validated.
"""
import argparse,concurrent.futures,http.client,json,os,pathlib,statistics,subprocess,time,sqlite3
from contextlib import closing
from devnet import Lab

def percentile(values,p):
    values=sorted(values)
    return values[min(len(values)-1,int((len(values)-1)*p))] if values else None

def main():
    p=argparse.ArgumentParser();p.add_argument('--bin-dir',default='target/debug');p.add_argument('--work-dir',required=True);p.add_argument('--report',required=True);p.add_argument('--requests',type=int,default=1000);p.add_argument('--registry-fixture');p.add_argument('--registered-count',type=int,default=0);a=p.parse_args()
    assert 1<=a.requests<=5000,'bounded local measurement: 1-5000 requests per level'
    root=pathlib.Path(a.work_dir)
    if root.exists()and any(root.iterdir()):raise SystemExit('Use a fresh empty work directory')
    a.quick=True;a.keep=True;lab=Lab(a);report={'schema':1,'scope':'owned loopback read-only RPC, genesis state; not wallet/TPS/network scale certification','registered_records':a.registered_count,'levels':[]}
    if a.registry_fixture:
        assert 1<=a.registered_count<=100000
        destination=lab.root/'a'/'usernames.sqlite';destination.parent.mkdir(parents=True,exist_ok=True)
        with closing(sqlite3.connect(f'file:{pathlib.Path(a.registry_fixture).resolve()}?mode=ro',uri=True))as source,closing(sqlite3.connect(destination))as target:source.backup(target)
    try:
        lab.start('a');port=lab.ports['a'][1];info=lab.get('a','info');identity=info['genesis_hash']
        if a.registry_fixture:assert info['registered_usernames']==a.registered_count
        def request(index):
            start=time.perf_counter();connection=http.client.HTTPConnection('127.0.0.1',port,timeout=10)
            try:
                name=f'capacity_{index*a.registered_count//a.requests:06}'if a.registry_fixture and index%2 else None
                connection.request('GET',f'/v1/usernames?username={name}'if name else '/v1/info');response=connection.getresponse();body=response.read(65536)
                value=json.loads(body)if response.status==200 else None
                valid=response.status!=200 or (value['username']==name if name else value['genesis_hash']==identity)
                return response.status,(time.perf_counter()-start)*1000,valid
            except (OSError,ValueError,http.client.HTTPException):return 0,(time.perf_counter()-start)*1000,False
            finally:connection.close()
        for concurrency in [1,4,16,64,128]:
            start=time.perf_counter()
            with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency)as pool:results=list(pool.map(request,range(a.requests)))
            elapsed=time.perf_counter()-start;codes={str(code):sum(r[0]==code for r in results)for code in set(r[0]for r in results)}
            latency=[r[1]for r in results if r[0]==200]
            level={'clients':concurrency,'requests':a.requests,'seconds':elapsed,'completed_per_second':len(results)/elapsed,'http_statuses':codes,'successful_p50_ms':percentile(latency,.5),'successful_p95_ms':percentile(latency,.95),'successful_p99_ms':percentile(latency,.99),'all_successful_payloads_valid':all(r[2]for r in results if r[0]==200)}
            report['levels'].append(level);print(json.dumps(level),flush=True)
            assert level['all_successful_payloads_valid'];assert not any(r[0]==500 for r in results)
            assert lab.get('a','info')['genesis_hash']==identity,'node liveness after bounded reads'
        pid=lab.nodes['a'].pid
        if os.name=='posix'and pathlib.Path(f'/proc/{pid}/status').exists():
            values={line.split(':',1)[0]:line.split(':',1)[1].strip()for line in pathlib.Path(f'/proc/{pid}/status').read_text().splitlines()if ':'in line}
            report['node_resources']={key:values.get(key)for key in ['VmRSS','VmHWM','Threads']}
            report['node_resources']['open_fds']=len(list(pathlib.Path(f'/proc/{pid}/fd').iterdir()))
        report['healthy_after_load']=True
    finally:
        for node in list(lab.nodes):lab.stop(node)
        pathlib.Path(a.report).parent.mkdir(parents=True,exist_ok=True);pathlib.Path(a.report).write_text(json.dumps(report,indent=2)+'\n')
if __name__=='__main__':main()
