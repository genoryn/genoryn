# ADR 0003: exact finite emission

One GNR is 10^12 gori. Totals use u128. Genesis creates no currency. Height 1 is
the first rewarded block. The block target is 60 seconds; a protocol year is
525,600 blocks, and a halving interval is 2,102,400 blocks. Calendar leap years
do not enter consensus.

Budgets: ordinary mining 20,690,000 GNR, bootstrap 100,000 GNR, treasury 210,000
GNR. Initial ordinary subsidy is floor(normal budget / (2 * halving interval));
each epoch right-shifts it once. The sum of all integer-shifted rewards is at
most the geometric budget. Residual rounding dust is never minted. Issuance
also checks the cap before accepting a block. There is no tail emission.

Bootstrap adds exactly 2 GNR at heights 1..50,000. Treasury emits exactly 1 GNR
at heights 1..210,000, split into ten groups of 21,000 outputs. The first group
matures at height 5,256,000, each subsequent group 525,600 blocks later. This is
10% per protocol year, years 10 through 19. A locked commitment cannot be used
in an admitted spend anchor. Mainnet treasury receiving/governance keys have
not been selected; mainnet cannot run in this release. The deterministic
devnet test treasury identity is explicitly unsuitable for real funds.

Every possible nonzero reward epoch is tested, along with all boundaries,
210,000 treasury maturity assignments and arbitrary cumulative heights.
RPC encodes money as decimal strings to avoid JavaScript precision loss.
