# Consensus v1

GENORYN selects the valid branch with the greatest sum of verified per-block
difficulty. Equal work keeps the current branch. Height 1 follows a deterministic
network-specific zero-issuance genesis identifier. The mainnet genesis was finalized
on 10 October 2026 (`c5fc87d996c7fd9dc69ff0cdf96183c65e1be5c9293748b33c31cd8fb6e1d708`);
devnet and testnet remain for development.

A header commits version, network, height, parent, Unix timestamp, integer
difficulty, RandomX seed, nonce, body digest, matured-note root and nullifier root.
The body contains miner issuance, scheduled treasury issuance, and at most 64
ordinary transfers. A block's maximum canonical size is 2,000,000 bytes.

Timestamp must exceed the median of up to 11 preceding timestamps and be no more
than 120 seconds ahead of observed UTC. Difficulty retargets per block with the
bounded integer LWMA-style rule in ADR0005. RandomX output is interpreted as a
little-endian 256-bit integer; multiplying by difficulty must fit 256 bits.

An epoch seed uses the ancestor at `floor(saturating_sub(height,65)/2048)*2048`,
or genesis for epochzero, then the GENORYN RandomX domain hash. This is a single
explicit algorithm version. No alternate easy verifier or mock PoW is compiled
into production node consensus.

Mature note commitments form the upstream 32-level tree. Mining outputs wait 60
mainnet or testnet blocks (2 on devnet); devnet and testnet treasury outputs use the
independent vesting schedule, and the mainnet treasury output is paid to the
provably unspendable burn address.
Only recent admitted roots (120-block window) may anchor transfers. Nullifiers
are unique across all accepted actions. Reorg replay reconstructs maturity,
anchors and nullifiers from the competing branch, then reconsiders mempool value.

A block that does not extend the current tip is first checked against the headers
of its own branch: height, median-time and future-time bounds, the retargeted
difficulty, the RandomX seed and its RandomX work. Only then are the stored
blocks of that branch replayed from genesis, so forcing a branch replay always
costs real work at the branch's difficulty. Blocks whose parent is not stored are
refused before any replay.

SQLite atomically stores blocks and the best-tip change. A process-exclusive lock
prevents two daemon writers in one directory. Schema, network and genesis checks
reject incompatible databases; integrity and full replay reject corruption.
Every stored block was fully validated before it was written and its bytes are
bound to the stored hash chain, so startup and branch replays re-execute every
state transition (nullifier uniqueness, anchors, maturity, issuance amounts,
treasury receiver, note commitment root, difficulty, seed and timestamps) but do
not repeat RandomX work, Halo2 proof and signature verification or the
whole-set nullifier root. Newly received blocks always receive full validation.
Current storage favors full replay correctness over production-scale pruning.
