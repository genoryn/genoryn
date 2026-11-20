# Native mining rewards (for developers)

Genoryn Krypton pays miners by shares in consensus itself. There is no pool server, no pool
operator and no fee; every node checks every share and every payout. The rules are
specified in [`docs/spec/rewards.md`](../spec/rewards.md); this page shows where they live in
the code and how the pieces fit.

**No external pools. No pool fees. Every miner is rewarded directly by the blockchain in
proportion to their valid shares.**

## The flow

1. **Work.** A miner asks a node for a template paying its own address:
   `POST /v1/mining/template {"address"}`. The header names the miner's owner tag
   (`BlockHeader.miner`). The miner checks that before it hashes anything
   (`apps/krypton-miner/src/main.rs`, `verify_work`), so a node cannot have it work for
   someone else.
2. **Shares.** Every hash is compared with the share target,
   `share_difficulty = max(1, difficulty / 32)` (`krypton_core::reward::share_difficulty`).
   A hash that meets it is a share; one that also meets the block difficulty is a block.
   About 32 shares are found per block across the network.
3. **Submit.** `POST /v1/mining/submit {"job_id","nonce"}`. The node recomputes the proof of
   work (`apps/kryptond/src/node.rs`, `accept_share`). A share it already has is answered as
   known; a valid one enters its share pool and is relayed to peers, so it does not matter
   which node a miner uses.
4. **Inclusion.** The next block includes the shares of the last 32 blocks it can credit,
   at most 256 (`MAX_SHARES_PER_BLOCK`), its own header first. Consensus checks each:
   height within the window, the right predecessor and difficulty, the share target, and
   that no earlier main-chain block credited the same hash
   (`crates/krypton-core/src/consensus.rs`, `crates/krypton-core/src/reward.rs`).
5. **Credit.** The block adds its subsidy and fees to the reward pool `P` and credits each
   share `P · min(w, 4·D) / (8 · 32 · D)` to its owner's account
   ([reward-calculation.md](reward-calculation.md)).
6. **Payout.** Every account older than `payout_interval` (360 blocks on mainnet) is paid
   as one note, at most 64 per block, oldest first. The note matures after 100 blocks.
   The header commits all payouts (`payout_digest`); light clients recompute them.

## Constants (`crates/krypton-core/src/params.rs`)

| Constant | Value | Meaning |
|---|---|---|
| `SHARE_RATIO` | 32 | share difficulty = block difficulty / 32 |
| `SHARE_WINDOW` | 32 | a share can be credited within 32 blocks |
| `POOL_SPREAD` | 8 | the pool drains over about 8 blocks |
| `MAX_SHARE_WEIGHT` | 4 | a share counts at most 4 × the block difficulty |
| `MAX_SHARES_PER_BLOCK` | 256 | |
| `MAX_PAYOUTS_PER_BLOCK` | 64 | |
| `payout_interval` | 360 (mainnet, testnet) | about 6 hours |
| `coinbase_maturity` | 100 (mainnet) | |

## Properties the tests check

- Credits follow hashrate, and minted equals pool + unpaid credits + paid notes at every height (`reward.rs`: `rewards_follow_hashrate_and_conserve_supply`).
- Leaving other miners' shares out of a block does not raise the block finder's own credit (`excluding_other_shares_does_not_raise_own_credit`).
- Rewards on real devnet blocks are proportional (`crates/krypton-core/tests/chain.rs`: `proportional_rewards_on_devnet_blocks`).
- Reorganisations undo credits exactly (`crates/krypton-store/src/tests.rs`: `shares_rewards_and_reorgs_are_exact`).

## Reading it

- `GET /v1/miner/{address or owner hex}`: unpaid credit, `since`, `next_payout_height`, recent and pending shares.
- `GET /v1/info`: `reward_pool`, `share_difficulty`, `share_ratio`, `payout_interval`, `hashrate`.
- `GET /v1/shares`: the node's pending shares.

Mining income is public per owner tag, as coinbase rewards are on every chain. Use a separate
wallet for mining if you want it apart from the address you receive payments at.
