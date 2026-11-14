# BTC ⇄ GNR atomic swaps

Status: **live on mainnet since block 3,300** (release 1.0.13). From release 1.0.23 every
new swap runs **GNR first**: the seller locks the GNR before the buyer pays. Implemented and
tested:

* the Bitcoin HTLC (`crates/krypton-swap`);
* GNR lock notes (consensus, circuit and wallet);
* the swap engine (`engine.rs`), the offer board and the wallet's **Swap** tab.
## Goal

Two people exchange bitcoin and GNR directly, from their own wallets:

* **Atomic.** Either both receive the other's coins, or both get their own coins back.
  Neither has to trust the other, and nobody can run off with the coins halfway through.
* **No custody.** There is no exchange, escrow, account or KYC. The project never holds
  anyone's coins or keys.
* **Recoverable.** Every key and secret of a swap is derived from the wallet's 24 words, so a
  swap in progress can be finished or refunded after a crash or a reinstall.

## How it works

Both chains lock coins under the same hash lock `h = SHA-256(s)`. In the **GNR-first flow**
(1.0.23 on) only the **GNR seller** knows the 32-byte secret `s` at first, and locks first.

| Step | Who | What |
|---|---|---|
| 1 | Buyer | Sends a request: amounts, its bitcoin refund key and its GNR claim outputs. |
| 2 | GNR seller | Answers with `h`, its bitcoin claim key and the GNR lock terms, and **locks the GNR at once** in a lock note: claimable with `s` by the buyer only, refundable to the seller after `T_gnr` (≈ 32 h). |
| 3 | Buyer | Waits for the lock to confirm: 6 GNR blocks for up to 50 GNR, 15 up to 500, 30 above (`Timing::gnr_confs_for`), so undoing the lock costs more than the trade. The lock is found only if it holds exactly the agreed amount, pays the buyer's own claim outputs and lasts long enough; only then does the wallet show where to pay. The buyer pays the bitcoin into the HTLC (refund after `T_btc`, ≈ 6 h) within 2 hours. |
| 4 | GNR seller | Waits for the bitcoin to confirm (1 block up to 0.01 BTC, else 2), then **claims the bitcoin** with `s`, at least 6 blocks before `T_btc`. This publishes `s` on Bitcoin. |
| 5 | Buyer | Its wallet reads `s` from that bitcoin transaction and **claims the GNR**, long before `T_gnr`. |

If anyone stops halfway, the timelocks return the coins:

* **The seller has no GNR, locks less, or never locks:** no confirmed lock matches the terms,
  so the buyer's wallet never shows where to pay. The buyer pays nothing.
* **The buyer never pays:** the seller refunds the GNR after `T_gnr`.
* **The seller never claims the bitcoin:** `s` stays secret; the buyer refunds the bitcoin
  after `T_btc` and the seller refunds the GNR after `T_gnr`.
* **The seller claims the bitcoin:** `s` is public, so the buyer can always claim the GNR,
  because the GNR lock outlasts the bitcoin lock by at least 6 hours. The buyer's check
  (`Timing::check_pay`) counts bitcoin blocks slow (20 min) and GNR blocks fast (45 s), and
  adds the 2-hour payment window.
* **No swap may last longer than a week:** both wallets refuse longer timelocks.

The free option is now on the buyer's side: a buyer who takes an offer and never pays keeps
the seller's GNR locked until `T_gnr` (≈ 32 h), but cannot take it.

Swaps started before 1.0.23 used the bitcoin-first flow (the buyer held `s` and paid first);
they finish as they began. A 1.0.23 wallet answers only GNR-first requests: older wallets
are told to update.
## Bitcoin side (implemented)

`crates/krypton-swap` builds a standard HTLC as a P2WSH output:

```
OP_IF
    OP_SIZE 32 OP_EQUALVERIFY OP_SHA256 <h> OP_EQUALVERIFY <claim_pubkey>
OP_ELSE
    <T_btc> OP_CHECKLOCKTIMEVERIFY OP_DROP <refund_pubkey>
OP_ENDIF
OP_CHECKSIG
```

* **Claim** (GNR seller): witness `<sig> <s> 1 <script>`; nLockTime 0.
* **Refund** (BTC seller): witness `<sig> "" <script>`; nLockTime `T_btc`, sequence non-final.
  Valid only in blocks at or above `T_btc`.
* Both spends signal replace-by-fee, so they can be re-signed with a higher fee.
* The parties exchange **terms, never scripts.** Each wallet rebuilds the script itself and checks
  that the funding output pays exactly that script, the agreed amount, and that
  `SHA-256(s) = h` before it signs anything.
* Keys and secrets are derived from the wallet seed:
  `SHA-256(tag ‖ tag ‖ seed ‖ swap_index ‖ counter)` with tags `Genoryn/swap/btc-key` and
  `Genoryn/swap/secret` (BIP-340 style tagged hashes).
* Tests run every claim and refund through Bitcoin Core's script interpreter
  (libbitcoinconsensus). They cover a valid claim and refund, a refund before the timelock, a final
  sequence, a wrong secret, wrong keys, tampered outputs, dust, address encoding and the timelock
  rules.

Funding needs no special wallet support. The GNR wallet shows the HTLC address and the exact
amount, and the user pays it from any Bitcoin wallet. Claim and refund are signed by the GNR
wallet with the swap key and sent to an address the user chooses.

## GNR side: lock notes (implemented; mainnet from block 3,300)

Every GNR transaction is a shielded transfer. A note can only be spent by proving knowledge of
its owner's secret key inside the zk-STARK (`spend-circuit.md`). Rescue hashes are not
homomorphic, so there is no "scriptless" trick that would let a secret revealed on Bitcoin unlock
a GNR note. GNR has a small, explicit hash and time lock instead (`crates/krypton-core/src/lock.rs`).

**Terms.** Both sides agree on

```
terms = (h, T_gnr, claim_outputs[2], refund_outputs[2])
```

where `h` is the hash lock, `T_gnr` the refund height and the outputs are the two note
commitments a claim, respectively a refund, must create. The claimer chooses the claim
outputs and the refunder the refund outputs, so only each of them knows the openings.

**Lock note.** Its key and commitment randomness are derived from the terms, so both sides
can compute them:

```
sk    = SHA3("Krypton/lock/sk/v1"  ‖ terms) → field
rcm   = SHA3("Krypton/lock/rcm/v1" ‖ terms) → field
owner = H(LOCK_OWNER; AK(sk), NK(sk))
cm    = NoteCommit(owner, value, rcm)
```

Ordinary spends hash owners under the `OWNER` domain, so **no ordinary transaction can spend
a lock note**, not even by someone who knows `sk`. Creating a lock note is an ordinary
transfer. Its memo carries the terms to the refunder: tag, swap number, `T_gnr`, the refund
fee, `h` and the claim outputs. The refund outputs follow from the wallet seed, so a refund
works from the 24 words alone.

**Spending it.** Transaction version 2 appends the public `lock` field (terms and mode) to the
signed body:

```
lock: { terms, mode: Claim { secret: s } | Refund }
```

* **Circuit.** For input 0 the owner is hashed under `LOCK_OWNER` and must equal the
  public `lock_owner`, which the verifier computes from the terms; input 0 cannot be a dummy.
  This is the only change. Ordinary spends keep exactly the statement, constraints and proofs
  they had, so every existing proof stays valid.
* **Consensus (outside the proof):**
  * Claim: `SHA-256(s) = h`, `0 < expiry_height < T_gnr` (a claim can never be mined at or
    after `T_gnr`), and the outputs are exactly `claim_outputs`.
  * Refund: the block height is at least `T_gnr`, and the outputs are exactly `refund_outputs`.
    The mempool holds refunds back until then.
  * Lock spends are valid only from `lock_height` on.
* **One spend only.** Claim and refund spend the same note with the same key, so they have the
  same nullifier, and at most one of them can ever be mined.
* **Nobody else can redirect the coins.** Anyone who learns the terms could build the key, but
  the outputs are fixed by consensus, and only their creator knows how to open them. A third
  party cannot even build a valid proof.
* **What becomes public:** the terms and `s` of the swap input, which link the GNR spend to the
  Bitcoin HTLC (the Bitcoin side is public anyway). The amount and both parties' GNR addresses
  stay private.
* **Activation:** `lock_height` is 0 on devnet, 3,300 on mainnet (announced with 1.0.13) and
  not set (`u64::MAX`) on testnet. Mainnet activation is a hard fork. The in-app updater
  ships it, and nodes that do not upgrade reject blocks with lock spends from that height. The
  reward-timing items R-1 and R-2 from the
  [2026-11-05 audit](../audit/2026-11-05-security-audit.md) are not part of this upgrade; they
  need their own. Until activation, wallets refuse to create lock notes.
* **Tests:**
  * Circuit (`krypton-zk`): a lock proof binds its lock owner. A lock proof does not pass as
    an ordinary spend or as a spend of another lock, and an ordinary proof does not pass as a
    lock spend. Two forgeries fail: an ordinary spend of a lock note, and a lock statement
    whose trace uses the ordinary owner hash.
  * Consensus (`krypton-core`): the claim and refund rules, and activation.
  * Wallet (`krypton-wallet-core`), end to end on devnet with real proofs: lock, claim, the
    secret seen in the mempool by the other side, a replayed claim rejected, an early refund
    refused, a late claim refused, and a refund by a wallet restored from its phrase.

## The swap in the wallet (implemented)

`krypton_swap::engine` runs inside the desktop wallet. The two people exchange three codes
(`gks1…`, copy and paste, later through the offer board). Everything else the wallets do on
their own, every 15 seconds while the wallet runs:

| Code | From → to | Carries |
|---|---|---|
| Request | BTC buyer → GNR seller | amounts, `h`, the buyer's bitcoin refund key, its GNR claim outputs |
| Accept | GNR seller → buyer | the seller's bitcoin claim key, the full GNR terms (refund height, refund outputs) |
| Deposit | buyer → seller | the bitcoin refund height and the deposit outpoint |

Because the accept code fixes the GNR terms, the buyer's wallet knows the lock note in
advance and finds it on chain by itself. The engine then:

1. **Seller:** waits for the deposit's confirmations and checks the timelocks, then locks
   the GNR.
2. **Buyer:** waits for the lock's confirmations, then claims the GNR. The claim publishes
   the secret.
3. **Seller:** reads the secret from the mempool, or from the block, and claims the bitcoin.
4. **Timeouts:**
   * If nobody claims, the seller refunds the GNR at `T_gnr`.
   * If the GNR is not claimed, the buyer refunds the bitcoin once nLockTime is final.
   * The buyer counts a swap as received only when its own claim output appears. A lock that
     was refunded under a pending claim falls back to the bitcoin refund.

Bitcoin is read through Esplora (mempool.space by default) or a Bitcoin Core node over RPC. On
GNR mainnet the Bitcoin network is always Bitcoin mainnet. Swaps are stored inside the
encrypted wallet file.

**Tests** (`crates/krypton-swap/tests/engine.rs`) run two wallets against a GNR devnet chain
with real proofs and a simulated Bitcoin chain. That chain runs every transaction through
Bitcoin Core's interpreter and enforces nLockTime. The cases:

* a completed swap;
* the secret taken from a block when the mempool was missed;
* both sides refunding when nobody claims.

`genoryn-ref/qa/swap_e2e.ps1` runs the same swap between two desktop wallets and a real
Bitcoin Core regtest node.

## Safety rules the wallet enforces

* **Timelocks** (`Timing::check`): when the GNR lock expires, at least 12 hours must remain on
  the bitcoin lock. Bitcoin blocks are counted fast (8 min) and GNR blocks slow (90 s), so a
  burst of bitcoin blocks or slow GNR blocks cannot squeeze the GNR seller's window. On
  mainnet, bitcoin is locked for at least 288 blocks (≈ 48 h) and the GNR lock window is about
  720 blocks (≈ 12 h). Test networks use short locks (`Timing::TEST`).
* **Claim deadline:** the BTC seller claims the GNR at least 120 blocks (≈ 2 h) before `T_gnr`.
  Otherwise the GNR seller could refund the GNR and claim the bitcoin with the secret seen in the
  mempool.
* **Confirmations:** the GNR seller waits for 2 bitcoin confirmations. The BTC seller waits for
  30 GNR confirmations on the lock, so that rewriting it would cost more than the trade. GNR's
  hashrate is small today, so this is the main limit on swap size.
* **Cancel and expiry:** either side can cancel while nothing is locked: the buyer before its
  bitcoin deposit, the seller before the deposit code arrives.
  * **Expiry:** a swap whose deposit has not arrived 3 hours after it started ends by itself.
    The seller waits one hour longer, so a last-minute deposit still finds it.
  * **Board swaps:** the other side is told, and a maker's offer goes back on the board.
  * **Paid anyway:** a cancelled buyer's wallet keeps watching the contract address. If the
    bitcoin is paid after all, it is refunded at the timelock.
* **Amounts, keys, outputs and networks** are checked against the agreed terms before each step.
  A request for another network, or a lock that pays other outputs, is refused.
* **Crash safety:** swaps are saved in the encrypted wallet file after every step. Keys, secrets
  and outputs come from the seed, and a lock note's memo carries its terms, so a restored wallet
  can still refund its GNR.

## Known limits

* **Free option.** Between steps 2 and 4 the BTC seller can walk away if the price moves; the
  GNR seller then waits for the refund. Short timelocks keep this small. Offers can require a
  small deposit later.
* **Bitcoin privacy.** The wallet reads Bitcoin through a public Esplora API (mempool.space by
  default, or the user's own). That API sees which HTLC addresses the wallet asks about. Users
  can point the wallet at their own Bitcoin node instead.
* **Fees.** The GNR seller pays the bitcoin claim fee and the BTC seller the GNR claim fee, plus
  each side's funding. The wallet shows them before anything is locked.

## Offer board and market price (implemented)

Every node serves an offer board under `/v1/swap/` (genoryn.io for mainnet wallets). Its
code is in `crates/krypton-swap/src/board.rs` and `apps/kryptond/src/swapboard.rs`.

* **Offers:** side (sell or buy GNR), amounts, expiry (at most 24 h) and a one-time hybrid
  X25519 + ML-KEM-1024 key. The key is derived from the wallet's swap seed and is used for
  nothing else, so offers cannot be linked to an address or to each other. A hash of a secret
  lets only the maker cancel the offer.
* **Mailboxes:** messages are sealed to the recipient's offer key, and carry the sender's key
  for the reply. The board stores ciphertext for a day under `inbox = H(key)`.
* **What travels:** taking an offer starts a swap, and the two wallets send each other the
  request, accept and deposit codes through the mailboxes by themselves. A wallet accepts
  only a request with exactly the offer's amounts, and an offer is taken once. The only
  manual step left is the buyer paying the bitcoin into the contract address.
* **Spam:** offers, messages and trade reports carry an 18-bit proof of work, and there are
  per-client limits, caps on offers, messages and inbox size, and expiry. The board keeps
  offers, swaps in progress and mailboxes in `swap-board.json` across restarts.
* **Instant takes:** a maker's wallet pings the board for all its open offers at once
  (`POST /v1/swap/offers/ping`, every 45 s, each offer proven with its cancel secret). An
  offer whose maker was seen in the last 2 minutes is listed as `online`. The wallet checks
  the board mailboxes every 5 s while it has open offers or takes, and answers a take by
  itself, so a swap starts within seconds with no approval. Wallets refuse to take an
  offline offer.
* **Reserved GNR:** an open sell offer reserves its amount plus the fee in the maker's wallet.
  Sends and other offers cannot use it, so a buyer always finds the coins there. GNR cannot be
  locked on chain before a buyer exists (the lock commits to the buyer's hash and keys), so the
  reservation is enforced by the maker's wallet; on chain the coins are locked once the
  buyer's bitcoin deposit has confirmed, as before. Cancelling the offer frees them.
* **Market price:** after a swap completes, wallets report its amounts and hash lock (on by
  default; the setting `share_prices` turns it off). The board accepts a report only if a
  claim with that hash lock is on its own chain, and it counts each hash once.
  * The market price is the **arithmetic mean of completed swaps**, stored in sat per GNR
    and shown in BTC (`board::market`, `board::priced_swaps`):
    * the swaps completed in the last 72 hours, each counting the same whatever its size
      (all recorded swaps while none is that recent);
    * with three or more, a swap more than 4× away from their median is left out: amounts
      are reported by the wallets (they are private on chain), so a typo or a fake report
      must not move the average;
    * open offers and swaps in progress are listed and counted, but **never** move the
      price: anyone can post an offer at any price, and nothing proves a swap in progress
      runs. The best sell and buy offers are shown next to the price.

    The board also reports how many swaps were averaged (`priced`), `confidence`,
    `v / (v + 25)` for the GNR `v` behind them, and a `depth` of thin (< 0.4), moderate or
    deep (≥ 0.8).
  * **Swaps in progress:** a taken offer moves to "in progress" when its maker proves it
    with the offer's cancel secret, checked against the offer stored on the board (never a
    copy sent with the request). The maker's wallet confirms every 10 minutes that the swap
    still runs; an entry not confirmed for 30 minutes, or older than 12 hours, is dropped.
    The wallet removes it as soon as the swap can no longer complete: finished, or the other
    side gave up before any bitcoin arrived (its locked GNR then waits for the refund).  * **The GNR price** shown by the wallet and genoryn.io is this market price and nothing
    else (`GKValue.market`). The wallet values balances and mining earnings at it, and shows
    nothing before there is one.
  * **Any price may be offered.** The board refuses only offers for more than 1,000 BTC
    (`MAX_OFFER_SATS`) and offers without proof of work. Offers of makers offline for
    2 minutes cannot be taken.
  * **Swap details on genoryn.io/market:** for each completed swap the board shows what the
    GNR chain holds: the claim transaction, its block and time, the hash lock, the secret it
    revealed (the same secret unlocks the bitcoin side) and the refund height. Amounts are
    private on chain; the reported ones count only once their claim is on chain.
  * The wallet and genoryn.io convert it to dollars with the BTC price, which genoryn.io
    fetches from mempool.space, so visitors' browsers contact no third party.
  * Before the first completed swap there is no price, and nothing is shown.
* **Known limit:** the GNR amount of a trade cannot be checked on chain (it is private). Wash
  trades (someone trading with themselves at a made-up price) are possible, as on any
  exchange. The outlier cut, the equal weight of every swap (a large made-up amount counts no
  more than a small one) and the confidence limit their effect.
* **Swap secrets are never reused:** every swap number is random (from the upper half of
  32-bit numbers), so a wallet restored from its recovery phrase, which has no swap records,
  or the same phrase in a second wallet, never derives an earlier swap's secret, keys or
  outputs again. A repeated secret already revealed by an earlier swap would let anyone
  claim the new lock.

## Plan

| Phase | Work | Status |
|---|---|---|
| 0 | Bitcoin HTLC, claim, refund, secret extraction, timelock rules, seed derivation, tests against Bitcoin Core's interpreter | **done** |
| 1 | GNR lock notes: circuit, transaction v2, consensus and mempool rules, wallet lock, claim, refund and recovery | **done**, active on devnet |
| 2 | Swap engine (codes, state machine, Esplora and Bitcoin Core clients, encrypted swap state) and the Swap tab | **done**, tested on devnet + Bitcoin regtest |
| 3 | Offer board, sealed mailboxes and the market price from completed swaps | **done** |
| 4 | Mainnet activation at block 3,300 (release 1.0.13); first swaps with small amounts; an independent review of the circuit change | **scheduled** |
