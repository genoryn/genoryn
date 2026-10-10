#!/usr/bin/env python3
"""Capture the actual packaged wallet's empty native window, then verify exit."""
import argparse, ctypes, hashlib, json, os, pathlib, subprocess, tempfile, time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--packages', type=pathlib.Path, default=pathlib.Path('dist'))
    parser.add_argument('--output', type=pathlib.Path, required=True)
    parser.add_argument('--report', type=pathlib.Path, required=True)
    args = parser.parse_args()
    packages = [path for path in args.packages.iterdir() if path.is_dir()]
    assert len(packages) == 1
    package = packages[0].resolve()
    executable = package / ('genoryn-wallet.exe' if os.name == 'nt' else 'genoryn-wallet')
    output = args.output.resolve(); output.parent.mkdir(parents=True, exist_ok=True)
    manifest = json.loads((package / 'release-manifest.json').read_text())
    report = {'schema': 1, 'scope': 'Actual packaged native wallet, fresh empty profile, no wallet creation, mining or payments', 'commit': manifest['commit'], 'platform': manifest['platform'], 'passed': False}
    with executable.open('rb') as stream:
        report['native_binary_sha256'] = hashlib.file_digest(stream, 'sha256').hexdigest()
    process = None
    try:
        with tempfile.TemporaryDirectory(prefix='genoryn-packaged-ui-') as directory:
            root = pathlib.Path(directory)
            with (root / 'stdout.txt').open('w') as stdout, (root / 'stderr.txt').open('w') as stderr:
                process = subprocess.Popen([str(executable), '--wallet', 'wallet.gnr', '--data-dir', 'node', '--bin-dir', str(package), '--smoke-test-frames', '50'], cwd=root, stdout=stdout, stderr=stderr)
                if os.name == 'nt':
                    from windows_native_controls import Desktop
                    from ctypes import wintypes
                    desktop = Desktop()
                deadline = time.monotonic() + 30
                window = None
                while time.monotonic() < deadline and process.poll() is None:
                    if os.name == 'nt':
                        window = desktop.window(process.pid)
                    else:
                        found = subprocess.run(['xdotool', 'search', '--onlyvisible', '--pid', str(process.pid), '--name', 'GENORYN'], capture_output=True, text=True)
                        window = found.stdout.splitlines()[0] if found.returncode == 0 and found.stdout.strip() else None
                    if window:
                        break
                    time.sleep(.1)
                assert window and process.poll() is None, 'Native window did not appear'
                time.sleep(1)
                if os.name == 'nt':
                    rectangle = wintypes.RECT()
                    desktop.bind(desktop.user, 'GetWindowRect', [wintypes.HWND, ctypes.POINTER(wintypes.RECT)], wintypes.BOOL)
                    assert desktop.user.GetWindowRect(window, ctypes.byref(rectangle))
                    x, y = rectangle.left, rectangle.top
                    width, height = rectangle.right - x, rectangle.bottom - y
                    # Standard screen capture of this owned window's rectangle.
                    # No desktop/session/security settings are changed.
                    script = f'''Add-Type -AssemblyName System.Drawing
$image = [System.Drawing.Bitmap]::new({width}, {height})
$graphics = [System.Drawing.Graphics]::FromImage($image)
try {{
  $graphics.CopyFromScreen({x}, {y}, 0, 0, $image.Size)
  $image.Save($env:GENORYN_CAPTURE_PATH, [System.Drawing.Imaging.ImageFormat]::Png)
}} finally {{ $graphics.Dispose(); $image.Dispose() }}
'''
                    subprocess.run(['powershell', '-NoProfile', '-Command', script], env=dict(os.environ, GENORYN_CAPTURE_PATH=str(output)), check=True, timeout=30)
                else:
                    subprocess.run(['import', '-window', str(window), str(output)], check=True, timeout=30)
                data = output.read_bytes()
                assert data.startswith(b'\x89PNG\r\n\x1a\n') and len(data) > 8000, 'Missing or nearly blank native capture'
                report['screenshot_dimensions'] = [int.from_bytes(data[16:20], 'big'), int.from_bytes(data[20:24], 'big')]
                report['screenshot_sha256'] = hashlib.sha256(data).hexdigest()
                process.wait(timeout=45)
                assert process.returncode == 0, 'Packaged native wallet did not exit cleanly'
                assert not (root / 'wallet.gnr').exists(), 'Capture unexpectedly created a wallet'
            report.update(passed=True, native_exit_code=process.returncode, screenshot_file=output.name)
    except Exception as error:
        report['error'] = str(error)
        raise
    finally:
        if process and process.poll() is None:
            process.terminate(); process.wait(timeout=10)
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
