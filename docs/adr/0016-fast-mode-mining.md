# ADR 0016: full-memory RandomX for the miner

Light mode keeps a 256 MiB cache per worker and recomputes every dataset item it
reads. On a 24-core Windows desktop the shipped miner measured about 136 H/s with
4 threads and 435 H/s with 24, while one shared 2 GiB dataset ("fast" or
full-memory mode) gave 760 H/s and 3,200-3,700 H/s with the same threads. On
2026-10-10 the mainnet difficulty implied about 100 kH/s, so the light-mode miner
left ordinary participants with a fraction of a percent of the blocks.

`genoryn-miner --randomx-mode auto` (the default) builds the full-memory dataset
for the current PoW seed on a background thread, about 45 seconds; mining starts
at once in light mode and the running template is searched again in fast mode as
soon as the dataset is ready. The seed changes every 2048 blocks, so the dataset
is rebuilt about once every 34 hours. If the dataset or a fast VM cannot be
created, the miner logs the reason and stays in light mode. `--randomx-mode light`
keeps the previous behavior. Fast mode allows one worker per logical CPU; light
mode keeps its 16-worker bound.

Nodes keep verifying in light mode. Full-memory mode changes neither the RandomX
version, input bytes, seed, target comparison nor any validity rule. Large pages
stay masked: they need an operating-system privilege. `GENORYN_RANDOMX_PORTABLE=1`
still selects the interpreter, now also for the dataset VM.

Sharing one dataset between threads needs the vendored binding to mark
`RandomXDataset` as `Send` and `Sync`, which GENORYN crates cannot do because they
forbid unsafe code. A built dataset and the cache it retains are only read, and
the native API is designed for several VMs to share one dataset. The two-line
addition is recorded in [the patch](../../third-party/randomx-shared-dataset.patch)
on top of [ADR 0012](0012-randomx-binding-safety.md)'s safety patch; native RandomX
sources are unchanged. An ignored test builds a real dataset and checks that fast
VMs on four threads reproduce light-mode hashes.
