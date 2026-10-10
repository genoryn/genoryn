#!/usr/bin/env python3
"""Actual Windows wallet/node lifecycle through its own native window.

Uses documented Win32 mouse/keyboard input and close messages, real binaries and
fresh loopback ports. Navigation requires an unlocked wallet, so a fresh valueless
test wallet is created and unlocked first. No miner, payment, remote peer or test
replacement RPC is created. Only direct genorynd children of the spawned GUI are
eligible for cleanup.
"""
import argparse,ctypes,json,os,pathlib,socket,subprocess,time,urllib.request
from ctypes import wintypes as w

class PROCESSENTRY32W(ctypes.Structure):
    _fields_=[('dwSize',w.DWORD),('cntUsage',w.DWORD),('th32ProcessID',w.DWORD),('th32DefaultHeapID',ctypes.c_size_t),('th32ModuleID',w.DWORD),('cntThreads',w.DWORD),('th32ParentProcessID',w.DWORD),('pcPriClassBase',w.LONG),('dwFlags',w.DWORD),('szExeFile',w.WCHAR*260)]

class MOUSEINPUT(ctypes.Structure):
    _fields_=[('dx',w.LONG),('dy',w.LONG),('mouseData',w.DWORD),('dwFlags',w.DWORD),('time',w.DWORD),('dwExtraInfo',ctypes.c_size_t)]

class KEYBDINPUT(ctypes.Structure):
    _fields_=[('wVk',w.WORD),('wScan',w.WORD),('dwFlags',w.DWORD),('time',w.DWORD),('dwExtraInfo',ctypes.c_size_t)]

class INPUTDATA(ctypes.Union):
    _fields_=[('mi',MOUSEINPUT),('ki',KEYBDINPUT)]

class INPUT(ctypes.Structure):
    _fields_=[('type',w.DWORD),('data',INPUTDATA)]

class Desktop:
    def __init__(self):
        self.user=ctypes.WinDLL('user32',use_last_error=True);self.kernel=ctypes.WinDLL('kernel32',use_last_error=True)
        self.callback=ctypes.WINFUNCTYPE(w.BOOL,w.HWND,ctypes.c_ssize_t)
        self.bind(self.user,'EnumWindows',[self.callback,ctypes.c_ssize_t],w.BOOL)
        self.bind(self.user,'GetWindowThreadProcessId',[w.HWND,ctypes.POINTER(w.DWORD)],w.DWORD)
        self.bind(self.user,'IsWindowVisible',[w.HWND],w.BOOL)
        self.bind(self.user,'GetWindowTextW',[w.HWND,w.LPWSTR,ctypes.c_int],ctypes.c_int)
        self.bind(self.user,'GetDpiForWindow',[w.HWND],w.UINT)
        self.bind(self.user,'GetClientRect',[w.HWND,ctypes.POINTER(w.RECT)],w.BOOL)
        self.bind(self.user,'GetWindowRect',[w.HWND,ctypes.POINTER(w.RECT)],w.BOOL)
        self.bind(self.user,'ClientToScreen',[w.HWND,ctypes.POINTER(w.POINT)],w.BOOL)
        self.bind(self.user,'SetForegroundWindow',[w.HWND],w.BOOL)
        self.bind(self.user,'GetForegroundWindow',[],w.HWND)
        self.bind(self.user,'SetCursorPos',[ctypes.c_int,ctypes.c_int],w.BOOL)
        self.bind(self.user,'SendInput',[w.UINT,ctypes.POINTER(INPUT),ctypes.c_int],w.UINT)
        self.bind(self.user,'PostMessageW',[w.HWND,w.UINT,ctypes.c_size_t,ctypes.c_ssize_t],w.BOOL)
        self.bind(self.kernel,'CreateToolhelp32Snapshot',[w.DWORD,w.DWORD],w.HANDLE)
        self.bind(self.kernel,'Process32FirstW',[w.HANDLE,ctypes.POINTER(PROCESSENTRY32W)],w.BOOL)
        self.bind(self.kernel,'Process32NextW',[w.HANDLE,ctypes.POINTER(PROCESSENTRY32W)],w.BOOL)
        self.bind(self.kernel,'OpenProcess',[w.DWORD,w.BOOL,w.DWORD],w.HANDLE)
        self.bind(self.kernel,'WaitForSingleObject',[w.HANDLE,w.DWORD],w.DWORD)
        self.bind(self.kernel,'TerminateProcess',[w.HANDLE,w.UINT],w.BOOL)
        self.bind(self.kernel,'CloseHandle',[w.HANDLE],w.BOOL)
    @staticmethod
    def bind(library,name,arguments,result):
        function=getattr(library,name);function.argtypes=arguments;function.restype=result
    def window(self,pid):
        found=[]
        @self.callback
        def visit(window,_):
            owner=w.DWORD();self.user.GetWindowThreadProcessId(window,ctypes.byref(owner))
            if owner.value==pid and self.user.IsWindowVisible(window):
                title=ctypes.create_unicode_buffer(256);self.user.GetWindowTextW(window,title,256)
                if title.value.startswith('GENORYN'):found.append(window)
            return True
        assert self.user.EnumWindows(visit,0)
        return found[0] if found else None
    def post(self,window,message,word=0,parameter=0):
        if not self.user.PostMessageW(window,message,word,parameter):raise ctypes.WinError(ctypes.get_last_error())
    def click(self,window,x,y):
        scale=self.user.GetDpiForWindow(window)/96
        assert .5<=scale<=4,'unsupported display scale'
        self.user.SetForegroundWindow(window);time.sleep(.15)
        if self.user.GetForegroundWindow()!=window:raise RuntimeError('Windows did not permit focusing the owned test window; no input sent')
        point=w.POINT(round(x*scale),round(y*scale))
        bounds=w.RECT()
        if not self.user.GetClientRect(window,ctypes.byref(bounds)):raise ctypes.WinError(ctypes.get_last_error())
        assert bounds.left<=point.x<bounds.right and bounds.top<=point.y<bounds.bottom,'click falls outside owned client area'
        if not self.user.ClientToScreen(window,ctypes.byref(point)):raise ctypes.WinError(ctypes.get_last_error())
        if not self.user.SetCursorPos(point.x,point.y):raise ctypes.WinError(ctypes.get_last_error())
        time.sleep(.1)
        for flags in [0x0002,0x0004]:
            event=INPUT();event.type=0;event.data.mi.dwFlags=flags
            if self.user.SendInput(1,ctypes.byref(event),ctypes.sizeof(INPUT))!=1:raise RuntimeError('Windows rejected ordinary mouse input to the owned test window')
            time.sleep(.08)
        time.sleep(.3)
    def type_text(self,window,text):
        self.user.SetForegroundWindow(window);time.sleep(.15)
        if self.user.GetForegroundWindow()!=window:raise RuntimeError('Windows did not permit focusing the owned test window; no input sent')
        for character in text:
            for flags in [0x0004,0x0006]:
                event=INPUT();event.type=1;event.data.ki.wScan=ord(character);event.data.ki.dwFlags=flags
                if self.user.SendInput(1,ctypes.byref(event),ctypes.sizeof(INPUT))!=1:raise RuntimeError('Windows rejected ordinary keyboard input to the owned test window')
                time.sleep(.01)
        time.sleep(.3)
    def screenshot(self,window,path):
        rectangle=w.RECT()
        if not self.user.GetWindowRect(window,ctypes.byref(rectangle)):raise ctypes.WinError(ctypes.get_last_error())
        x,y=rectangle.left,rectangle.top;width=rectangle.right-x;height=rectangle.bottom-y
        script=f'''Add-Type -AssemblyName System.Drawing
$image = [System.Drawing.Bitmap]::new({width}, {height})
$graphics = [System.Drawing.Graphics]::FromImage($image)
try {{ $graphics.CopyFromScreen({x}, {y}, 0, 0, $image.Size); $image.Save($env:GENORYN_CAPTURE_PATH, [System.Drawing.Imaging.ImageFormat]::Png) }}
finally {{ $graphics.Dispose(); $image.Dispose() }}
'''
        subprocess.run(['powershell','-NoProfile','-Command',script],env=dict(os.environ,GENORYN_CAPTURE_PATH=str(path)),check=True,timeout=30)
    def owned_nodes(self,parent):
        snapshot=self.kernel.CreateToolhelp32Snapshot(2,0)
        if snapshot==ctypes.c_void_p(-1).value:raise ctypes.WinError(ctypes.get_last_error())
        result=[]
        try:
            entry=PROCESSENTRY32W();entry.dwSize=ctypes.sizeof(entry)
            valid=self.kernel.Process32FirstW(snapshot,ctypes.byref(entry))
            while valid:
                if entry.th32ParentProcessID==parent and entry.szExeFile.lower()=='genorynd.exe':result.append(entry.th32ProcessID)
                valid=self.kernel.Process32NextW(snapshot,ctypes.byref(entry))
        finally:self.kernel.CloseHandle(snapshot)
        return result

def free_port():
    with socket.socket()as s:s.bind(('127.0.0.1',0));return s.getsockname()[1]

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--bin-dir',default='target/debug');parser.add_argument('--work-dir',required=True);parser.add_argument('--report',required=True);a=parser.parse_args()
    if os.name!='nt':raise SystemExit('This native control test requires Windows; it is not a simulated pass on another OS.')
    root=pathlib.Path(a.work_dir).resolve();binary=pathlib.Path(a.bin_dir).resolve();report_path=pathlib.Path(a.report).resolve()
    report_path.parent.mkdir(parents=True,exist_ok=True)
    if root.exists()and any(root.iterdir()):raise SystemExit('Use a fresh empty work directory')
    root.mkdir(parents=True,exist_ok=True);(root/'node').mkdir();rpc=free_port();p2p=free_port()
    (root/'node.toml').write_text(f'[p2p]\nlisten="127.0.0.1:{p2p}"\n[limits]\nconnections=4\ninbound_connections=2\noutbound_connections=2\n')
    password='native-controls-test-password';wallet=root/'node'/'native-test.gnr'
    created=subprocess.run([str(binary/'genoryn-wallet-cli.exe'),'--json-stdio'],input=json.dumps({'op':'create','network':'mainnet','wallet_path':str(wallet),'password':password}),capture_output=True,text=True,timeout=120)
    if created.returncode!=0 or not json.loads(created.stdout or '{}').get('ok'):raise SystemExit('Could not create the valueless test wallet')
    desktop=Desktop();handles={};gui=None;window=None;report={'schema':1,'scope':'actual Windows native GUI controls and its owned real loopback node; a fresh valueless test wallet is unlocked first; no mining/payment','input_delivery':'ordinary mouse and keyboard input to the verified foreground owned window; no elevation or OS policy change','checks':[],'passed':False}
    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
    def info():
        try:
            with opener.open(f'http://127.0.0.1:{rpc}/v1/info',timeout=1)as response:return json.load(response)
        except (OSError,ValueError):return None
    def wait(predicate,label,timeout=45):
        until=time.monotonic()+timeout
        while time.monotonic()<until:
            if gui and gui.poll()is not None:raise RuntimeError('Wallet exited unexpectedly during '+label)
            result=predicate()
            if result:return result
            time.sleep(.1)
        raise AssertionError('Timed out: '+label)
    def capture_owned():
        pids=desktop.owned_nodes(gui.pid)
        for pid in pids:
            if pid not in handles:
                # Keep a handle to the exact owned process, avoiding PID-reuse cleanup.
                handle=desktop.kernel.OpenProcess(0x00100001,False,pid)
                if not handle:raise ctypes.WinError(ctypes.get_last_error())
                handles[pid]=handle
        return pids
    def record(check):report['checks'].append({'check':check,'passed':True});print(json.dumps(report['checks'][-1]),flush=True)
    try:
        with open(root/'wallet.stdout.txt','w')as out,open(root/'wallet.stderr.txt','w')as err:
            gui=subprocess.Popen([str(binary/'genoryn-wallet.exe'),'--data-dir','node','--wallet',str(wallet),'--node-config','node.toml','--auto-node','off','--rpc-url',f'http://127.0.0.1:{rpc}','--bin-dir',str(binary)],cwd=root,stdout=out,stderr=err)
            window=wait(lambda:desktop.window(gui.pid),'wallet native window')
            rectangle=w.RECT();assert desktop.user.GetClientRect(window,ctypes.byref(rectangle))
            report['window_dpi']=desktop.user.GetDpiForWindow(window)
            report['client_pixels']=[rectangle.right-rectangle.left,rectangle.bottom-rectangle.top]
            time.sleep(1);desktop.click(window,461,531);desktop.type_text(window,password);desktop.click(window,356,587)
            # Unlocking runs the CLI's Argon2id key derivation before navigation opens.
            time.sleep(5);desktop.screenshot(window,report_path.with_name('windows-native-unlocked.png'))
            record('test_wallet_unlocked_before_navigation')
            desktop.click(window,100,461)  # Node: 7th menu entry, 46 px apart
            desktop.screenshot(window,report_path.with_name('windows-native-node-before-start.png'))
            desktop.click(window,343,279)
            first=wait(info,'owned node started');assert first['network']=='mainnet'and first['height']==0
            assert desktop.user.GetForegroundWindow()==window,'owned backend obscured the wallet with another window'
            desktop.screenshot(window,report_path.with_name('windows-native-node-after-start.png'))
            original=wait(capture_owned,'owned node process');assert len(original)==1
            desktop.click(window,343,279);assert capture_owned()==original
            record('explicit_start_and_repeat_start_have_one_owned_node')
            desktop.click(window,333,327)
            new=wait(lambda:(pids if (pids:=capture_owned())and pids!=original else None),'owned node restart')
            assert len(new)==1;assert desktop.kernel.WaitForSingleObject(handles[original[0]],0)==0
            wait(info,'RPC after restart');record('restart_reaps_original_and_starts_new_owned_node')
            desktop.click(window,481,327);time.sleep(.5)
            assert (root/'node'/'genorynd.log').stat().st_size>0
            record('log_view_click_keeps_native_window_responsive')
            desktop.click(window,494,279)
            wait(lambda:not info()and not capture_owned(),'owned node stopped');record('stop_releases_node_process_and_listener')
            # Allow the periodic health check to clear the stale endpoint indicator.
            time.sleep(6);desktop.click(window,343,279);wait(info,'node start again');wait(capture_owned,'new owned child')
            desktop.post(window,0x0010);gui.wait(timeout=20);assert gui.returncode==0
            assert not info();assert all(desktop.kernel.WaitForSingleObject(handle,0)==0 for handle in handles.values())
            record('normal_window_close_exits_zero_and_reaps_owned_nodes')
            report['passed']=True
    except Exception as error:
        report['error']=str(error)
        if window and gui and gui.poll()is None:
            try:desktop.screenshot(window,report_path.with_name('windows-native-failure.png'))
            except Exception as capture_error:report['screenshot_error']=str(capture_error)
        log=root/'node'/'genorynd.log'
        report['node_log_exists']=log.exists()
        if log.exists():report['node_log_tail']=log.read_text(errors='replace')[-8192:]
        raise
    finally:
        if gui and gui.poll()is None:
            capture_owned()
            if window:desktop.post(window,0x0010)
            try:gui.wait(timeout=10)
            except subprocess.TimeoutExpired:gui.terminate();gui.wait(timeout=10)
        for handle in handles.values():
            if desktop.kernel.WaitForSingleObject(handle,0)!=0:
                desktop.kernel.TerminateProcess(handle,1);desktop.kernel.WaitForSingleObject(handle,10000)
            desktop.kernel.CloseHandle(handle)
        stderr=root/'wallet.stderr.txt'
        if stderr.exists():report['native_stderr_tail']=stderr.read_text(errors='replace')[-8192:]
        report_path.parent.mkdir(parents=True,exist_ok=True);report_path.write_text(json.dumps(report,indent=2)+'\n')
if __name__=='__main__':main()
