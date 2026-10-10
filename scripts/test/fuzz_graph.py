#!/usr/bin/env python3
"""Require the fuzz workspace to use the same corrected native binding."""
import json, pathlib, subprocess

repository = pathlib.Path(__file__).resolve().parents[2]
metadata = json.loads(subprocess.check_output([
    'cargo', 'metadata', '--locked', '--manifest-path', str(repository / 'fuzz/Cargo.toml'),
    '--format-version', '1',
], text=True))
bindings = [package for package in metadata['packages'] if package['name'] == 'randomx-rs']
assert len(bindings) == 1
binding = bindings[0]
assert binding['source'] is None
assert pathlib.Path(binding['manifest_path']).resolve() == (repository / 'third-party/randomx-rs/Cargo.toml').resolve()
assert binding['id'] in {node['id'] for node in metadata['resolve']['nodes']}
print('Fuzz graph uses the exact local RandomX binding patch and committed lockfile.')
