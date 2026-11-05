# BTC ⇄ GNR atomic swaps

Status: **implemented, live on devnet, not active on mainnet.** Implemented and tested so far:

* the Bitcoin HTLC (`crates/krypton-swap`);
* GNR lock notes (consensus, circuit and wallet);
* the swap engine;
* the wallet's **Swap** tab.

On mainnet, lock spends need a hard fork at an announced height (`lock_height`). Until
then the Swap tab explains this and stays disabled.

## Goal

Two people exchange bitcoin and GNR directly, from their own wallets:

* **Atomic.** Either both receive the other's coins, or both get their own coins back.
  Neither has to trust the other, and nobody can run off with the coins halfway through.
* **No custody.** There is no exchange, escrow, account or KYC. The project never holds
  anyone's coins or keys.
* **Recoverable.** Every key and secret of a swap is derived from the wallet's 24 words, so a
  swap in progress can be finished or refunded after a crash or a reinstall.

## How it works

Both chains lock coins under the same hash lock `h = SHA-256(s)`, where only the BTC seller
knows the 32-byte secret `s` at first. The flow below is the default; the BTC seller is the
initiator.

| Step | Who | What |
|---|---|---|
| 1 | Both | Agree on amounts and exchange terms: `h`, public keys, GNR addresses, timelocks. |
| 2 | BTC seller | Pays the bitcoin into the HTLC address (P2WSH). Refund possible after `T_btc` (≈ 48 h). |
| 3 | GNR seller | Waits for the bitcoin to confirm, checks the timelocks, then locks the GNR in a **lock note**: claimable with `s`, refundable to them after `T_gnr` (≈ 12 h). |
| 4 | BTC seller | Waits for enough GNR confirmations, then **claims the GNR**. The claim publishes `s` on the GNR chain. |
| 5 | GNR seller | Their wallet sees `s` and **claims the bitcoin** with it, long before `T_btc`. |

If anyone stops halfway, the timelocks return the coins:

* The GNR seller never locks GNR: the BTC seller refunds the bitcoin after `T_btc`.
* The BTC seller never claims: the GNR seller refunds the GNR after `T_gnr`, and later the BTC
  seller refunds the bitcoin after `T_btc`. `s` was never published, so the bitcoin is safe.
* The BTC seller claims the GNR: `s` is public, so the GNR seller can always claim the bitcoin,
  because the bitcoin lock lasts at least 12 hours longer than the GNR lock.

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

## GNR side: lock notes (implemented, not active on mainnet)

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
* **Activation:** `lock_height` is 0 on devnet and not set (`u64::MAX`) on testnet and
  mainnet. Mainnet activation is a hard fork at a height announced at least two weeks ahead.
  The in-app updater ships it, and nodes that do not upgrade stop following the chain at that
  height. The same upgrade can fix the reward-timing items R-1 and R-2 from the
  [2026-11-05 audit](../audit/2026-11-05-security-audit.md). Until activation, wallets refuse to
  create lock notes.
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

## Finding a counterparty

genoryn.io will run an **offer board**: public offers (side, amount range, price, expiry) and an
encrypted message relay between the two wallets. Messages are encrypted to the other party's
post-quantum GNR address, so the board only sees ciphertext. It never holds keys or coins and
cannot take part in a swap. Anyone can run another board, and two people can also swap by
exchanging the terms directly.

## Plan

| Phase | Work | Status |
|---|---|---|
| 0 | Bitcoin HTLC, claim, refund, secret extraction, timelock rules, seed derivation, tests against Bitcoin Core's interpreter | **done** |
| 1 | GNR lock notes: circuit, transaction v2, consensus and mempool rules, wallet lock, claim, refund and recovery | **done**, active on devnet |
| 2 | Swap engine (codes, state machine, Esplora and Bitcoin Core clients, encrypted swap state) and the Swap tab | **done**, tested on devnet + Bitcoin regtest |
| 3 | Offer board and encrypted message relay on genoryn.io, so codes need no copy and paste | next |
| 4 | Review of the circuit change, public testnet (signet) swaps, then the mainnet activation height | before mainnet |
