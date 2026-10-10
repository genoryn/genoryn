#!/usr/bin/env python3
"""Stream-check a built ZIP without running code or extracting its contents."""
import argparse, hashlib, json, pathlib, posixpath, re, subprocess, zipfile
from urllib.parse import unquote, urlsplit

FULL = ['genorynd', 'genoryn-miner', 'genoryn-wallet-cli', 'genoryn-cli', 'genoryn-emission', 'genoryn-vectors', 'genoryn-wallet']
EDITIONS = {'full': FULL, 'lite': ['genorynd', 'genoryn-wallet-cli', 'genoryn-cli', 'genoryn-wallet']}


def digest(stream):
    result = hashlib.sha256()
    for chunk in iter(lambda: stream.read(1024 * 1024), b''):
        result.update(chunk)
    return result.hexdigest()


def verify(archive, expected_commit, expected_vendor):
    archive = pathlib.Path(archive)
    with archive.open('rb') as stream:
        archive_sha256 = digest(stream)
    with zipfile.ZipFile(archive) as container:
        entries = container.namelist()
        assert len(entries) == len(set(entries)), 'Duplicate ZIP entries'
        assert all(not pathlib.PurePosixPath(name).is_absolute() and '..' not in pathlib.PurePosixPath(name).parts and '\\' not in name for name in entries), 'Unsafe ZIP path'
        manifests = [name for name in entries if name.endswith('/release-manifest.json')]
        assert len(manifests) == 1, 'Expected one package manifest'
        prefix = manifests[0].removesuffix('release-manifest.json')
        manifest = json.loads(container.read(manifests[0]))
        assert manifest['commit'] == expected_commit and not manifest['working_tree_dirty'], 'Source identity is not the expected clean commit'
        records = manifest['files']
        expected_files = {prefix + item['name'] for item in records}
        assert len(expected_files) == len(records)
        assert set(entries) == expected_files | {prefix + 'release-manifest.json', prefix + 'SHA256SUMS'}, 'Missing or unmanifested files'
        for item in records:
            name = prefix + item['name']
            assert container.getinfo(name).file_size == item['size'], name
            with container.open(name) as stream:
                assert digest(stream) == item['sha256'], name
        assert container.read(prefix + 'SHA256SUMS').decode().splitlines() == [f"{item['sha256']}  {item['name']}" for item in records]
        vendor = json.loads(container.read(prefix + 'VENDORED-SOURCE-PROVENANCE.json'))
        assert vendor == expected_vendor, 'Vendored provenance differs'
        inventory = json.loads(container.read(prefix + 'DEPENDENCIES.json'))
        paths = set(entries)
        notices = 0
        for item in inventory:
            directory = prefix + f"third-party-licenses/{item['name']}-{item['version']}/"
            names = [name for name in entries if name.startswith(directory)]
            assert len(names) == item['license_text_count'] and names, directory
            assert all(container.read(name).strip() for name in names), directory
            notices += len(names)
        patched = [item for item in inventory if item.get('dependency_source') == 'vendored path dependency']
        assert sorted(item['name'] for item in patched) == sorted(item['name'] for item in expected_vendor)
        suffix = '.exe' if manifest['platform'] == 'windows' else ''
        edition = manifest.get('edition', 'full')
        assert edition in EDITIONS, 'Unknown package edition'
        binaries = EDITIONS[edition]
        if edition == 'lite':
            assert prefix + 'genoryn-miner' + suffix not in paths, 'GENORYN Lite must not contain the miner'
        binary_hashes = {}
        for binary in binaries:
            name = prefix + binary + suffix
            assert name in paths
            if manifest['platform'] != 'windows':
                assert (container.getinfo(name).external_attr >> 16) & 0o111, name
            binary_hashes[binary + suffix] = next(item['sha256'] for item in records if item['name'] == binary + suffix)
        for name in entries:
            if not name.endswith('.md'):
                continue
            for reference in re.findall(r'!?\[[^\]]*\]\(([^)]+)\)', container.read(name).decode('utf-8')):
                url = urlsplit(reference)
                if url.scheme in ('https', 'http', 'mailto') or not url.path:
                    continue
                target = posixpath.normpath(posixpath.join(posixpath.dirname(name), unquote(url.path)))
                assert not url.scheme and target.startswith(prefix) and (target in paths or any(path.startswith(target.rstrip('/') + '/') for path in paths)), (name, reference)
        expected_notices = {'wgpu-hal-24.0.4': 1, 'epaint_default_fonts-0.31.1': 5, 'randomx-rs-1.6.0': 2}
        for package, count in expected_notices.items():
            names = [name for name in entries if name.startswith(prefix + 'third-party-licenses/' + package + '/')]
            assert len(names) == count and all(len(container.read(name).strip()) > 100 for name in names), package
        if manifest['platform'] == 'windows':
            launcher = container.read(prefix + 'Start-Wallet.cmd')
            assert b'\r\r\n' not in launcher and b'\n' not in launcher.replace(b'\r\n', b'')
            assert b'genoryn-wallet.exe %*\r\n' in launcher
    return {'schema': 1, 'scope': 'CI streaming verification of the actual native ZIP, clean source, every manifest hash, notice inventory, critical notice contents and documentation links; not legal/security certification or independent download evidence', 'commit': expected_commit, 'archive_name': archive.name, 'archive_bytes': archive.stat().st_size, 'archive_sha256': archive_sha256, 'platform': manifest['platform'], 'edition': edition, 'manifest_files_verified': len(records), 'binary_sha256': binary_hashes, 'dependency_count': len(inventory), 'notice_count': notices, 'critical_notices': expected_notices, 'vendor_provenance_matches': True, 'passed': True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--packages', type=pathlib.Path, default=pathlib.Path('dist'))
    parser.add_argument('--output', type=pathlib.Path, required=True)
    args = parser.parse_args()
    archives = sorted(args.packages.glob('*.zip'))
    assert archives, 'Expected a native package'
    commit = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
    vendor = [json.loads(pathlib.Path(f'third-party/{name}').read_text()) for name in ('randomx-source.json', 'saturating-time-source.json')]
    reports = [verify(archive, commit, vendor) for archive in archives]
    editions = [report['edition'] for report in reports]
    assert len(editions) == len(set(editions)), 'Expected at most one package per edition'
    report = reports[0] if len(reports) == 1 else {'schema': 1, 'archives': reports, 'passed': all(r['passed'] for r in reports)}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))


if __name__ == '__main__':
    main()
