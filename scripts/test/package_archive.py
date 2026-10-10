#!/usr/bin/env python3
"""Actual ZIP parser regressions using explicitly synthetic temporary fixtures."""
import hashlib, importlib.util, json, pathlib, tempfile, unittest, zipfile

path = pathlib.Path(__file__).resolve().parents[1] / 'release/verify_archive.py'
spec = importlib.util.spec_from_file_location('verify_archive', path)
verifier = importlib.util.module_from_spec(spec); spec.loader.exec_module(verifier)


class ArchiveVerification(unittest.TestCase):
    def fixture(self, path, changed=False, extra=False, duplicate=False):
        prefix = 'fixture/'
        vendor = [{'name': 'randomx-rs', 'synthetic': True}]
        inventory = []
        files = {'README.md': b'[Guide](docs/guide.md)', 'docs/guide.md': b'Synthetic fixture', 'start-wallet': b'fixture'}
        for binary in ['genorynd', 'genoryn-miner', 'genoryn-wallet-cli', 'genoryn-cli', 'genoryn-emission', 'genoryn-vectors', 'genoryn-wallet']:
            files[binary] = b'Synthetic executable fixture'
        for name, version, count in [('wgpu-hal', '24.0.4', 1), ('epaint_default_fonts', '0.31.1', 5), ('randomx-rs', '1.6.0', 2)]:
            inventory.append({'name': name, 'version': version, 'license_text_count': count, 'dependency_source': 'vendored path dependency' if name == 'randomx-rs' else 'registry'})
            for index in range(count):
                files[f'third-party-licenses/{name}-{version}/NOTICE-{index}'] = b'Synthetic unit-test notice text. ' * 8
        files['DEPENDENCIES.json'] = json.dumps(inventory).encode()
        files['VENDORED-SOURCE-PROVENANCE.json'] = json.dumps(vendor).encode()
        records = [{'name': name, 'size': len(data), 'sha256': hashlib.sha256(data).hexdigest()} for name, data in files.items()]
        manifest = {'commit': 'fixture-commit', 'working_tree_dirty': False, 'platform': 'linux', 'files': records}
        files['release-manifest.json'] = json.dumps(manifest).encode()
        files['SHA256SUMS'] = ''.join(f"{item['sha256']}  {item['name']}\n" for item in records).encode()
        if changed: files['genorynd'] += b'changed'
        if extra: files['unmanifested'] = b'extra'
        with zipfile.ZipFile(path, 'w') as archive:
            for name, data in files.items():
                info = zipfile.ZipInfo(prefix + name); info.external_attr = 0o100755 << 16
                archive.writestr(info, data)
            if duplicate: archive.writestr(prefix + 'README.md', b'duplicate')
        return vendor

    def test_valid_archive_passes_and_wrong_source_or_vendor_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / 'fixture.zip'; vendor = self.fixture(path)
            self.assertTrue(verifier.verify(path, 'fixture-commit', vendor)['passed'])
            with self.assertRaises(AssertionError): verifier.verify(path, 'wrong-commit', vendor)
            with self.assertRaises(AssertionError): verifier.verify(path, 'fixture-commit', [{'name': 'randomx-rs', 'wrong': True}])

    def test_modified_missing_membership_and_duplicate_entries_fail(self):
        for mode in ['changed', 'extra', 'duplicate']:
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as directory:
                path = pathlib.Path(directory) / 'fixture.zip'; vendor = self.fixture(path, **{mode: True})
                with self.assertRaises(AssertionError): verifier.verify(path, 'fixture-commit', vendor)


if __name__ == '__main__': unittest.main()
