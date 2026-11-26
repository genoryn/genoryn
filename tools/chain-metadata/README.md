# chain-metadata

Measures the metadata every observer of the Genoryn Krypton chain sees: transaction kinds,
fees, sizes, input/output shape, how transactions spread over blocks and over the day, and
how many carry a combination of these that no other transaction has. Read-only, Python 3.8+
standard library only, polite to the node (a pause between requests, a local cache).

```
python chain_metadata.py --node https://genoryn.io --json metadata.json
```

Options: `--from`/`--to` heights, `--cache` file (default `chain-metadata-cache.jsonl`),
`--delay` seconds between requests (default 0.25). Results and their reading:
[docs/research/privacy-sietch.md](../../docs/research/privacy-sietch.md).
