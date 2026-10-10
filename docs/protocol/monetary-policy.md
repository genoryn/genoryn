# Monetary policy v1

Consensus uses u128 gori totals. One GNR is 10^12 gori. Individual shielded note
values use the upstream u64 representation and a transaction's net balance must
fit i64; wallets reject a larger single payment rather than overflowing. The
21,000,000 GNR network total intentionally exceeds u64 and is never stored there.

Height zero creates no money. Ordinary subsidy at height h>0 is
`floor(20,690,000 * 10^12 / (2 * 2,102,400)) >> floor((h-1)/2,102,400)`.
The first ordinary subsidy is 4,920,566,971,080 gori. Integer halving eventually
reaches zero; there is no tail reward and no administrative mint authority.

Bootstrap adds 2 GNR at every height 1..50,000 inclusive. Treasury adds 1 GNR at
every height 1..210,000 inclusive. All three budgets fit inside 21M. Ordinary
rounding dust stays unissued forever, as allowed by the maximum-supply rule.
`genoryn-emission --height H` and `/v1/supply` call the identical library.

Treasury note group `floor((issuance_height-1)/21,000)` matures at
`5,256,000 + group*525,600`. Before maturity, the commitment is absent from all
admissible spend trees. The first 10% matures at protocol year 10, the last at
year 19. Devnet treasury receiving material is reproducible public test material;
mainnet multisig governance remains an explicit production-design gate.

Fees transfer existing shielded value to the miner and are not new issuance.
Only coinbase classes may have negative value balance; their exact amounts,
height, parent, network, spend-disabled flags and treasury receiver are validated.
All ordinary transfers have positive public fees and no transparent value output.

See `crates/genoryn-core/src/emission.rs` and ADR 0003 for tests and decisions.
