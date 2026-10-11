#!/usr/bin/env python3
"""Portable practical verification; failures stop the suite and are never skipped.
Use --release for slow rotation/capacity/audit/fuzz/package gates too.
"""
import argparse,datetime,json,os,pathlib,shutil,subprocess,sys,uuid


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--release',action='store_true',help='also run actual default rotation, 50K identities, RPC capacity, audit, fuzz and package checks')
    parser.add_argument('--output',type=pathlib.Path)
    args=parser.parse_args()
    repository=pathlib.Path(__file__).resolve().parents[2];os.chdir(repository)
    root=(args.output or pathlib.Path('.devnet')/('verify-'+datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ')+'-'+uuid.uuid4().hex[:8])).resolve()
    if root.exists() and any(root.iterdir()):raise SystemExit('Use a fresh empty verification output directory')
    root.mkdir(parents=True,exist_ok=True)
    env=dict(os.environ,RAYON_NUM_THREADS=os.environ.get('RAYON_NUM_THREADS','2'))
    platform_name='Windows' if os.name=='nt' else 'Linux'
    python=sys.executable;suffix='.exe' if os.name=='nt' else '';binary=pathlib.Path(env.get('CARGO_TARGET_DIR',repository/'target')).resolve()/'debug'
    results=[]
    def run(label,command,timeout=1800,output=None):
        print('VERIFY:',label,flush=True)
        start=datetime.datetime.now(datetime.timezone.utc)
        try:
            with (open(output,'w',encoding='utf-8') if output else open(os.devnull,'w')) as file:
                subprocess.run([str(x) for x in command],env=env,check=True,timeout=timeout,stdout=file if output else None)
        except (subprocess.SubprocessError,OSError):
            results.append({'stage':label,'passed':False});raise
        results.append({'stage':label,'passed':True,'seconds':(datetime.datetime.now(datetime.timezone.utc)-start).total_seconds()})
    def native(label,executable):
        command=[str(executable),'--smoke-test-frames','5']
        if sys.platform.startswith('linux'):
            env['LIBGL_ALWAYS_SOFTWARE']='1'
            if not env.get('DISPLAY') and not env.get('WAYLAND_DISPLAY'):
                xvfb=shutil.which('xvfb-run')
                if not xvfb:raise SystemExit('Native GUI verification needs a desktop or the official xvfb package (xvfb-run).')
                command=[xvfb,'-a',*command]
        run(label,command,90)
    completed=False
    try:
        run('format',['cargo','fmt','--all','--','--check'])
        run('strict lint',['cargo','clippy','--locked','--workspace','--all-targets','--','-D','warnings'])
        run('unit and property tests',['cargo','test','--locked','--workspace'])
        run('network privacy and Tor transport policy',['cargo','test','--locked','-p','genoryn','--test','network_privacy'],output=root/('network-privacy-'+platform_name+'.txt'))
        for script in ['launchers','package_licenses','package_archive','randomx_vendor']:run(script,[python,f'scripts/test/{script}.py'])
        run('real proof and tamper fixtures',['cargo','test','--locked','-p','genoryn-core','--','--ignored'])
        run('real Payment Proof 2.0 sender challenge on a mined chain',['cargo','test','--locked','-p','genoryn-wallet-core','--test','payment_proof_v2','--','--ignored'])
        run('build native programs',['cargo','build','--locked','--workspace'])
        native('native wallet rendered frames',binary/('genoryn-wallet'+suffix))
        if os.name=='nt':
            run('actual Windows native node controls',[python,'scripts/test/windows_native_controls.py','--bin-dir',binary,'--work-dir',root/'windows-native','--report',root/'windows-native-controls.json'],240)
        for script in ['peer_cache','policy_limits']:
            run(script,[python,f'scripts/test/{script}.py','--bin-dir',binary,'--work-dir',root/script,'--report',root/(script.replace('_','-')+'-'+platform_name+'.json')])
        run('real mining payment recovery reorg restart',[python,'scripts/test/devnet.py','--bin-dir',binary,'--work-dir',root/'payment-lab','--report',root/('devnet-'+platform_name+'.json')])
        run('real shared mining: PPLNS credits, maturity, payouts, restart',[python,'scripts/test/pool.py','--bin-dir',binary,'--work-dir',root/'pool-lab','--report',root/('pool-'+platform_name+'.json'),'--mining-seconds','150'],2400)
        run('cryptographic and database benchmark',['cargo','run','--locked','-p','genoryn-core','--example','benchmark'],output=root/('benchmark-'+platform_name+'.json'))
        testnet_seconds=900 if os.name=='nt' else 300
        run('isolated real testnet block',[python,'scripts/test/testnet.py','--bin-dir',binary,'--work-dir',root/'testnet','--report',root/('testnet-'+platform_name+'.json'),'--max-seconds',str(testnet_seconds)],testnet_seconds+120)
        run('consensus vectors',[python,'scripts/test/vectors.py','--bin-dir',binary,'--output',root/('vectors-'+platform_name+'.json')])
        if args.release:
            run('actual default peer rotation',[python,'scripts/test/peer_rotation.py','--bin-dir',binary,'--work-dir',root/'rotation','--report',root/'rotation.json','--max-wait','1900'],2100)
            run('50000 registered identities',[binary/('genoryn-identity-bench'+suffix),'--count','50000','--workers','2','--database',root/'capacity'/'usernames.sqlite'],output=root/'identities.json')
            run('dataset-backed RPC reads',[python,'scripts/test/rpc_load.py','--bin-dir',binary,'--work-dir',root/'rpc','--report',root/'rpc.json','--registry-fixture',root/'capacity'/'usernames.sqlite','--registered-count','50000','--requests','1000'])
            run('locked dependency advisory scan',['cargo','audit','--json'],output=root/'audit.json')
            run('locked fuzz binding graph',[python,'scripts/test/fuzz_graph.py'])
            corpus=repository/'fuzz'/'corpus'/'block_parser';corpus.mkdir(parents=True,exist_ok=True)
            shutil.copyfile(repository/'tests'/'vectors'/'devnet-block-1.bin',corpus/'real-block')
            for target,maximum in [('block_parser','65536'),('transaction_parser','65536'),('username_parser','8192')]:
                run('bounded fuzz '+target,['cargo','+nightly','fuzz','run',target,'--','-max_total_time=120','-max_len='+maximum,'-rss_limit_mb=2048'],2100)
            run('package native binaries',[python,'scripts/release/package.py','--bin-dir',binary,'--output',root/'packages'])
            run('verify actual archive',[python,'scripts/release/verify_archive.py','--packages',root/'packages','--output',root/'archive-verification.json'])
            packages=[path for path in (root/'packages').iterdir() if path.is_dir()]
            if len(packages)!=1:raise RuntimeError('Expected one native package')
            if os.name=='nt':
                run('packaged Windows launcher',[os.environ.get('COMSPEC','cmd.exe'),'/d','/c',str(packages[0]/'Start-Wallet.cmd'),'--smoke-test-frames','5'],90)
            else:native('packaged Linux launcher',packages[0]/'start-wallet')
        completed=True
    finally:
        (root/'suite.json').write_text(json.dumps({'schema':1,'suite':'release' if args.release else 'practical','commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),'working_tree_dirty':bool(subprocess.check_output(['git','status','--porcelain'],text=True).strip()),'stages':results,'passed':completed and bool(results) and all(item['passed'] for item in results)},indent=2)+'\n')
        print('Verification evidence:',root,flush=True)
if __name__=='__main__':main()
