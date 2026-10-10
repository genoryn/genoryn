# Solo CPU mining

Mining is never enabled at wallet or daemon startup. Start it explicitly with
`genoryn-miner`, specifying network, loopback node, token file and receiving identity.
`--threads` accepts one worker per logical CPU. Each worker has its own
thread-confined RandomX VM; by default (`--randomx-mode auto`) all of them share one
full-memory dataset of about 2 GiB, built in the background in about 45 seconds while
the miner hashes in light mode (approximately 256 MiB cache per worker, at most 16
workers). `--randomx-mode light` never builds the dataset. See
[ADR 0016](../adr/0016-fast-mode-mining.md). No custom GPU/ASIC resistance claim is made.

The node constructs real shielded reward proofs and a candidate state transition;
the miner searches nonces and submits a candidate. Every node revalidates work,
proofs, issuance and roots. No reward is silently redirected to a developer.
`--max-blocks N` and `--max-seconds N` bound test runs. Without either, an explicitly
started miner continues until stopped. GUI Stop/Close reaps its owned process.

JSON status lines contain actual hash attempts, accepted/rejected blocks, height,
thread count and an observed process-wide average hash rate. Proof preparation and
network delay reduce this end-to-end rate; it is not a claimed hardware benchmark.
PoW first-confirmation target is 60 seconds at equilibrium, not deterministic finality.

Pooling and P2Pool-style protocols are roadmap work. Only reliable solo mining is
in the local milestone.
