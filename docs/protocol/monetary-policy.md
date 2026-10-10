# Monetary policy (Mainnet 1.0)

The complete schedule, its reasoning and the early-miner risks are in
[GENORYN Monetary Policy & Early Mining Incentives](emission.md). This page lists the exact
consensus formulas.

Consensus uses u128 gori totals. One GNR is 10^12 gori. Individual shielded note values use
the upstream u64 representation and a transaction's net balance must fit i64; wallets reject
a larger single payment rather than overflowing. The 21,000,000 GNR network total
intentionally exceeds u64 and is never stored there.

Height zero creates no money. For heights 1..500 the whole subsidy is the early reward of
exactly 1,000 GNR (10^15 gori); no ordinary subsidy and no treasury are paid at those
heights. From height 501 the ordinary subsidy is

`floor(20,290,000 * 10^12 / (2 * 2,102,400)) >> floor((h-501)/2,102,400)`,

that is 4,825,437,595,129 gori in the first epoch. Integer halving eventually reaches zero;
there is no tail reward and no administrative mint authority. The treasury adds 1 GNR at
every height 501..210,500 inclusive; on mainnet its receiver is a provably unspendable
address, so it is burned. The three budgets (500,000 early, 20,290,000 ordinary and
210,000 treasury) sum to 21,000,000 GNR; ordinary rounding dust (0.000049936 GNR) stays
unissued forever. Every block is rejected if issuance at its height would exceed
21,000,000 GNR. `genoryn-emission --height H` and `/v1/supply` call the identical library.

Treasury note group `floor((issuance_height-501)/21,000)` matures at
`5,256,000 + group*525,600`. Before maturity the commitment is absent from all admissible
spend trees. This vesting matters only on devnet and testnet, whose treasury receiving
material is reproducible public test material.

Fees transfer existing shielded value to the miner and are not new issuance. Only coinbase
classes may have a negative value balance; their exact amounts, height, parent, network,
spend-disabled flags and treasury receiver are validated. All ordinary transfers have
positive public fees and no transparent value output.

See `crates/genoryn-core/src/emission.rs` for the implementation and its tests.
