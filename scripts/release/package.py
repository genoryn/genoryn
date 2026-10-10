#!/usr/bin/env python3
"""Package already-built development binaries, provenance, checksums and licenses.
Never creates a production signing key or publishes a release by itself.
"""
import argparse,hashlib,json,os,pathlib,platform,re,shutil,subprocess,tomllib,zipfile
from urllib.parse import unquote,urlsplit
VENDORED_SOURCES=(('randomx-rs','randomx-source.json'),('saturating-time','saturating-time-source.json'))
BINARIES=['genorynd','genoryn-miner','genoryn-wallet-cli','genoryn-cli','genoryn-emission','genoryn-vectors','genoryn-wallet']
# GENORYN Lite: wallet and full node only. Without genoryn-miner the wallet has no Mining page.
LITE_BINARIES=['genorynd','genoryn-wallet-cli','genoryn-cli','genoryn-wallet']
EDITIONS={'full':BINARIES,'lite':LITE_BINARIES}
def packages_for_target(metadata):
 """Use Cargo's filtered graph, not every inactive platform in the lockfile."""
 nodes=(metadata.get('resolve')or{}).get('nodes')
 if not nodes:raise ValueError('Missing resolved target dependency graph')
 ids={node['id']for node in nodes};packages=metadata['packages']
 if not ids.issubset({package['id']for package in packages}):raise ValueError('Incomplete resolved dependency metadata')
 workspace=set(metadata.get('workspace_members',[]))
 selected=[package for package in packages if package['id']in ids and (package.get('source')is not None or (workspace and package['id']not in workspace))]
 if not selected:raise ValueError('Empty target dependency inventory')
 return selected
def validate_vendored_sources(repository,name='randomx-rs',manifest='randomx-source.json'):
 """Fail if a reviewed vendored patch or any pinned upstream source changes."""
 repository=pathlib.Path(repository);manifest=repository/'third-party'/manifest
 provenance=json.loads(manifest.read_text(encoding='utf-8'));root=repository/'third-party'/name
 original=provenance['original_file_sha256'];patched=provenance['patched_files']
 if not set(patched).issubset(original):raise ValueError('Unrecorded vendored patch path')
 entries=list(root.rglob('*'))
 if any(file.is_symlink()for file in entries):raise ValueError('Vendored source contains a symlink')
 if {file.relative_to(root).as_posix()for file in entries if file.is_file()}!=set(original):raise ValueError('Vendored source file inventory mismatch')
 for relative,digest in original.items():
  path=pathlib.PurePosixPath(relative)
  if path.is_absolute()or '..'in path.parts:raise ValueError('Unsafe vendored source path')
  file=root/path
  if not file.is_file()or file.is_symlink()or hashlib.sha256(file.read_bytes()).hexdigest()!=patched.get(relative,digest):raise ValueError(f'Vendored source provenance mismatch: {relative}')
 return provenance
def validate_all_vendored_sources(repository):
 """Every [patch.crates-io] override must be a recorded, byte-verified vendored source."""
 repository=pathlib.Path(repository)
 patches=tomllib.loads((repository/'Cargo.toml').read_text(encoding='utf-8')).get('patch',{}).get('crates-io',{})
 if {name:entry.get('path')for name,entry in patches.items()}!={name:f'third-party/{name}'for name,_ in VENDORED_SOURCES}:raise ValueError('Unrecorded or relocated crates.io patch')
 return [validate_vendored_sources(repository,name,manifest)for name,manifest in VENDORED_SOURCES]
def copy_documentation(stage,repository):
 repository=pathlib.Path(repository);stage=pathlib.Path(stage)
 for filename in ['README.md','DEVELOPMENT_STATUS.md','SECURITY.md','THIRD_PARTY.md','LICENSE-MIT','LICENSE-APACHE']:
  shutil.copy2(repository/filename,stage/filename)
 shutil.copytree(repository/'docs',stage/'docs');shutil.copytree(repository/'config',stage/'config')
 sdk=stage/'crates'/'genoryn-connect';sdk.mkdir(parents=True);shutil.copy2(repository/'crates'/'genoryn-connect'/'README.md',sdk/'README.md')
 vendor=stage/'third-party';vendor.mkdir()
 for _,manifest in VENDORED_SOURCES:
  shutil.copy2(repository/'third-party'/manifest,vendor/manifest)
  record=json.loads((repository/'third-party'/manifest).read_text(encoding='utf-8'))
  for patch in ([record['patch']]if record.get('patch')else[])+record.get('wrapper_patches',[]):shutil.copy2(repository/patch,stage/patch)
 # Fonts compiled into the wallet (SIL Open Font License 1.1).
 fonts=vendor/'fonts';fonts.mkdir()
 for notice in sorted((repository/'apps'/'genoryn-wallet'/'assets'/'fonts').glob('*-OFL.txt')):shutil.copy2(notice,fonts/notice.name)
def check_document_links(stage):
 stage=pathlib.Path(stage).resolve()
 documents=list(stage.glob('*.md'))+list((stage/'docs').rglob('*.md'))+list((stage/'crates').rglob('*.md'))
 for document in documents:
  for target in re.findall(r'!?\[[^\]]*\]\(([^)]+)\)',document.read_text(encoding='utf-8')):
   url=urlsplit(target)
   if url.scheme in ('http','https','mailto'):continue
   if not url.path:continue
   path=(document.parent/unquote(url.path)).resolve()
   if url.scheme or not path.is_relative_to(stage)or not path.exists():raise ValueError(f'Broken packaged documentation link: {document.relative_to(stage)} -> {target}')
def dependency_license_files(root,license_file=None):
 """Keep nested vendor/font notices with their paths; never follow an escape."""
 root=pathlib.Path(root).resolve();candidates=[]
 for file in root.rglob('*'):
  name=file.name.casefold()
  license_directory=any(part.casefold()in ('license','licenses','licence','licences')for part in file.relative_to(root).parts[:-1])
  named_notice=(license_directory or 'license'in name or 'licence'in name or name.startswith(('copying','notice','ofl','ufl','unlicense','copyright')))
  # Some bundled fonts name the license after the font, e.g. Hack-Regular.txt.
  font_notice=file.suffix.casefold()=='.txt'and any(file.with_suffix(ext).is_file()for ext in ('.ttf','.otf','.woff','.woff2'))
  if named_notice or font_notice:candidates.append(file)
 if license_file:candidates.append(root/license_file)
 selected={}
 for file in candidates:
  resolved=file.resolve()
  if file.is_relative_to(root)and resolved.is_relative_to(root)and resolved.is_file()and resolved.stat().st_size<1_000_000 and substantive_notice(resolved):
   # Normalise a declared path containing '..' before using it as a destination.
   relative=resolved.relative_to(root)
   selected[relative]=resolved
 return sorted(selected.items())
def substantive_notice(file):
 """Exclude empty markers, checker scripts and archived symlink placeholders."""
 if file.suffix.casefold()in ('.ignore','.sh','.py','.rs','.js','.ts','.c','.cpp','.h','.toml','.json','.yml'):return False
 text=file.read_text(encoding='utf-8',errors='replace').strip()
 if not text:return False
 if len(text.splitlines())==1 and not any(character.isspace()for character in text):
  if '/'in text or '\\'in text or text.upper().startswith(('LICENSE','LICENCE','COPYING')):return False
  if text in ('MIT','Apache-2.0','BSD-3-Clause','CC0-1.0','OFL-1.1','Ubuntu-font-1.0','Unlicense'):return False
 return True
def copy_dependency_licenses(root,destination,license_file=None):
 selected=dependency_license_files(root,license_file)
 for relative,source in selected:
  target=pathlib.Path(destination)/relative;target.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(source,target)
 return len(selected)
def copy_supplemental_licenses(package,destination,supplements,records):
 """Pinned upstream notices omitted from some crates.io source archives."""
 supplements=pathlib.Path(supplements).resolve();count=0
 for record in records:
  if not any(p['name']==package['name']and p['version']==package['version']for p in record['packages']):continue
  if record.get('source_kind')=='canonical-license-text'and package.get('license')!=record['spdx_license']:raise ValueError('Canonical license does not match the declared package license')
  relative=pathlib.PurePosixPath(record['filename'])
  if relative.is_absolute()or '..'in relative.parts:raise ValueError('Unsafe supplemental license path')
  source=(supplements/relative).resolve()
  if not source.is_relative_to(supplements)or not source.is_file():raise ValueError('Missing or escaping supplemental license')
  if source.stat().st_size>=1_000_000 or not substantive_notice(source):raise ValueError('Supplemental notice is empty, oversized or only a pointer')
  if hashlib.sha256(source.read_bytes()).hexdigest()!=record['sha256']:raise ValueError('Supplemental license checksum mismatch')
  target=pathlib.Path(destination)/'upstream-supplement'/relative;target.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(source,target);count+=1
 return count
def write_launcher(stage,windows):
 if windows:
  # Explicit bytes avoid doubled CRLF translation on Windows. Use the GUI's own
  # profile defaults so launching the EXE or this wrapper opens the same wallet.
  lines=['@echo off','setlocal','cd /d "%~dp0"',
   'set "GENORYN_CRT_DIR=%SystemRoot%\\System32"',
   'if exist "%SystemRoot%\\Sysnative\\cmd.exe" set "GENORYN_CRT_DIR=%SystemRoot%\\Sysnative"',
   'for %%D in (MSVCP140.dll VCRUNTIME140.dll VCRUNTIME140_1.dll) do if not exist "%GENORYN_CRT_DIR%\\%%D" goto missing_runtime',
   'genoryn-wallet.exe %*','exit /b %errorlevel%',':missing_runtime',
   'echo GENORYN requires the Microsoft Visual C++ v14 x64 runtime.',
   'echo Install the latest x64 runtime from Microsoft, then run Start-Wallet.cmd again.',
   'echo https://aka.ms/vc14/vc_redist.x64.exe',
   'echo This launcher does not download or install software.',
   'if not defined CI pause','exit /b 2']
  (stage/'Start-Wallet.cmd').write_bytes(('\r\n'.join(lines)+'\r\n').encode('ascii'))
 else:
  starter=stage/'start-wallet';starter.write_text('#!/usr/bin/env sh\nset -eu\ncd "$(dirname "$0")"\nexec ./genoryn-wallet "$@"\n');starter.chmod(0o755)
def workspace_version(repository):
 """The single version source is [workspace.package] in the root Cargo.toml."""
 text=(pathlib.Path(repository)/'Cargo.toml').read_text(encoding='utf-8')
 match=re.search(r'^\[workspace\.package\][^\[]*?^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"',text,re.M|re.S)
 if not match:raise SystemExit('Missing [workspace.package] version in Cargo.toml')
 return match.group(1)
def main():
 p=argparse.ArgumentParser();p.add_argument('--bin-dir',default='target/debug');p.add_argument('--output',default='dist');p.add_argument('--edition',choices=sorted(EDITIONS),default='full');a=p.parse_args()
 suffix='.exe'if os.name=='nt'else'';system=platform.system().lower();out=pathlib.Path(a.output);out.mkdir(parents=True,exist_ok=True)
 version=workspace_version(pathlib.Path(__file__).resolve().parents[2])
 edition='-lite'if a.edition=='lite'else'';name=f'genoryn-{version}-mainnet{edition}-{system}-{platform.machine().lower()}';stage=out/name
 if stage.exists():raise SystemExit('Refusing to overwrite an existing package directory')
 stage.mkdir();source=pathlib.Path(a.bin_dir)
 for binary in EDITIONS[a.edition]:
  src=source/(binary+suffix)
  if not src.is_file():raise SystemExit(f'Missing verified build: {src}')
  try:os.link(src,stage/src.name)
  except OSError:shutil.copy2(src,stage/src.name)
 copy_documentation(stage,pathlib.Path.cwd());check_document_links(stage)
 write_launcher(stage,os.name=='nt')
 compiler=subprocess.check_output(['rustc','--version','--verbose'],text=True)
 target=next((line.removeprefix('host: ').strip()for line in compiler.splitlines()if line.startswith('host: ')),None)
 if not target:raise SystemExit('Compiler did not identify its native target')
 if os.getenv('CARGO_BUILD_TARGET',target)!=target:raise SystemExit('This packager requires native verified binaries; cross-target packaging needs its own runtime validation')
 metadata=json.loads(subprocess.check_output(['cargo','metadata','--locked','--format-version','1','--filter-platform',target]))
 inventory=[];licenses=stage/'third-party-licenses';licenses.mkdir()
 supplements=pathlib.Path(__file__).resolve().parents[2]/'third-party'/'license-supplements'
 vendor_provenance=validate_all_vendored_sources(pathlib.Path(__file__).resolve().parents[2])
 supplement_manifest=supplements/'manifest.json';records=json.loads(supplement_manifest.read_text())if supplement_manifest.exists()else[]
 selected=packages_for_target(metadata)
 selected_versions={(package['name'],package['version'])for package in selected}
 records=[record for record in records if any((package['name'],package['version'])in selected_versions for package in record['packages'])]
 for package in selected:
  item={k:package.get(k)for k in ['name','version','license','repository','authors']}
  item['dependency_source']=package.get('source')or'vendored path dependency'
  inventory.append(item)
  root=pathlib.Path(package['manifest_path']).parent.resolve()
  destination=licenses/f"{package['name']}-{package['version']}"
  count=copy_dependency_licenses(root,destination,package.get('license_file'))
  count+=copy_supplemental_licenses(package,destination,supplements,records)
  if not count:raise SystemExit(f"No redistribution notice collected for {package['name']} {package['version']}; review upstream source before packaging")
  item['license_text_count']=count
 (stage/'DEPENDENCIES.json').write_text(json.dumps(inventory,indent=2)+'\n')
 (stage/'DEPENDENCY-LICENSE-SOURCES.json').write_text(json.dumps(records,indent=2)+'\n')
 (stage/'VENDORED-SOURCE-PROVENANCE.json').write_text(json.dumps(vendor_provenance,indent=2)+'\n')
 commit=os.getenv('GITHUB_SHA')or subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip()
 files=[]
 for file in sorted(stage.rglob('*')):
  if file.is_file():files.append(dict(name=file.relative_to(stage).as_posix(),size=file.stat().st_size,sha256=hashlib.sha256(file.read_bytes()).hexdigest()))
 dirty=bool(subprocess.check_output(['git','status','--porcelain'],text=True).strip())
 manifest=dict(format_version=1,project='GENORYN',version=f'{version}-mainnet',network='mainnet',edition=a.edition,commit=commit,working_tree_dirty=dirty,platform=system,architecture=platform.machine(),target_triple=target,dependency_scope='Cargo native-target filtered workspace graph, conservatively including build/test dependencies',rustc=compiler.splitlines()[0],signing_status='UNSIGNED DEVELOPMENT BUILD; no production signing identity',files=files)
 (stage/'release-manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
 sums=''.join(f"{x['sha256']}  {x['name']}\n"for x in files);(stage/'SHA256SUMS').write_text(sums)
 archive=out/(name+'.zip')
 with zipfile.ZipFile(archive,'w',zipfile.ZIP_DEFLATED,compresslevel=6,strict_timestamps=False)as z:
  for file in sorted(stage.rglob('*')):
   if file.is_file():z.write(file,file.relative_to(out))
 digest=hashlib.sha256(archive.read_bytes()).hexdigest();(out/(archive.name+'.sha256')).write_text(f'{digest}  {archive.name}\n')
 print(json.dumps({'archive':str(archive),'sha256':digest,'signing_status':manifest['signing_status']}))
if __name__=='__main__':main()
