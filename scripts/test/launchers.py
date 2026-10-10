#!/usr/bin/env python3
"""Portable launcher contract: one profile default, exact Windows line endings."""
import importlib.util,pathlib,tempfile,unittest,os,subprocess
path=pathlib.Path(__file__).resolve().parents[1]/'release'/'package.py'
spec=importlib.util.spec_from_file_location('genoryn_package',path);package=importlib.util.module_from_spec(spec);spec.loader.exec_module(package)
class Launchers(unittest.TestCase):
 def test_windows_preserves_gui_profile_and_crlf(self):
  with tempfile.TemporaryDirectory()as directory:
   root=pathlib.Path(directory);package.write_launcher(root,True);data=(root/'Start-Wallet.cmd').read_bytes()
   self.assertNotIn(b'\r\r\n',data);self.assertIn(b'genoryn-wallet.exe %*\r\n',data);self.assertNotIn(b'--wallet',data);self.assertNotIn(b'--data-dir',data)
   self.assertNotIn(b'\n',data.replace(b'\r\n',b''))
   for name in [b'MSVCP140.dll',b'VCRUNTIME140.dll',b'VCRUNTIME140_1.dll']:self.assertIn(name,data)
   self.assertIn(b'Sysnative',data);self.assertIn(b'https://aka.ms/vc14/vc_redist.x64.exe',data)
   self.assertIn(b'if not defined CI pause',data);self.assertTrue(data.endswith(b'exit /b 2\r\n'))
 @unittest.skipUnless(os.name=='nt','Requires the real Windows command interpreter')
 def test_missing_runtime_shows_official_setup_and_fails_before_launch(self):
  with tempfile.TemporaryDirectory()as directory:
   root=pathlib.Path(directory);package.write_launcher(root,True);launcher=root/'Start-Wallet.cmd'
   # Redirect only this disposable fixture's lookup path; no system files or
   # process-wide system environment variables are modified.
   empty=root/'empty-runtime';empty.mkdir();text=launcher.read_bytes().decode('ascii')
   text=text.replace('set "GENORYN_CRT_DIR=%SystemRoot%\\System32"',f'set "GENORYN_CRT_DIR={empty}"')
   text=text.replace('if exist "%SystemRoot%\\Sysnative\\cmd.exe" set "GENORYN_CRT_DIR=%SystemRoot%\\Sysnative"\r\n','')
   launcher.write_bytes(text.encode('ascii'))
   result=subprocess.run([os.environ.get('COMSPEC','cmd.exe'),'/d','/c',str(launcher)],env=dict(os.environ,CI='true'),capture_output=True,text=True,timeout=15)
   self.assertEqual(result.returncode,2);self.assertIn('https://aka.ms/vc14/vc_redist.x64.exe',result.stdout)
   self.assertIn('Microsoft Visual C++ v14 x64 runtime',result.stdout)
 def test_linux_preserves_arguments_and_is_executable(self):
  with tempfile.TemporaryDirectory()as directory:
   root=pathlib.Path(directory);package.write_launcher(root,False);file=root/'start-wallet';text=file.read_text()
   self.assertIn('exec ./genoryn-wallet "$@"',text);self.assertNotIn('--wallet',text)
   if os.name!='nt':self.assertTrue(file.stat().st_mode&0o111)
if __name__=='__main__':unittest.main()
