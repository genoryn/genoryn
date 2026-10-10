#!/usr/bin/env python3
"""Cross-platform exact monetary and serialization fixture verifier."""
import argparse,json,os,pathlib,subprocess,hashlib,struct
def main():
 p=argparse.ArgumentParser();p.add_argument('--bin-dir',default='target/debug');p.add_argument('--output',required=True);a=p.parse_args()
 exe=pathlib.Path(a.bin_dir)/('genoryn-emission.exe'if os.name=='nt'else'genoryn-emission')
 heights=[0,1,499,500,501,502,210500,210501,2102900,2102901,4205300,4205301,5255999,5256000,2**64-1]
 values={str(h):json.loads(subprocess.check_output([str(exe),'--height',str(h)]))for h in heights}
 raw=json.dumps(values,sort_keys=True,separators=(',',':')).encode();digest=hashlib.sha256(raw).hexdigest()
 vector_exe=pathlib.Path(a.bin_dir)/('genoryn-vectors.exe'if os.name=='nt'else'genoryn-vectors')
 protocol=json.loads(subprocess.check_output([str(vector_exe)]))
 golden=json.loads(pathlib.Path('tests/vectors/protocol.json').read_text())
 assert protocol==golden,'cross-platform consensus/RandomX vector mismatch'
 report=dict(version=1,monetary_sha256=digest,protocol=protocol,heights=values)
 expected=pathlib.Path('tests/vectors/monetary.sha256')
 if expected.exists():assert expected.read_text().strip()==digest,(digest,'cross-platform monetary mismatch')
 out=pathlib.Path(a.output);out.parent.mkdir(parents=True,exist_ok=True);out.write_text(json.dumps(report,indent=2)+'\n')
 print(json.dumps(dict(monetary_sha256=digest,passed=True)))
if __name__=='__main__':main()
