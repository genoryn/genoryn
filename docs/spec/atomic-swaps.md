# BTC ⇄ GNR atomic swaps

Status: **in development.** The Bitcoin side is implemented and tested
(`crates/krypton-swap`). The GNR side needs a consensus upgrade (lock notes, below) that is
not active yet. Nothing here can be used on mainnet until that upgrade is announced and
activated.

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

## GNR side: lock notes (consensus upgrade)

Every GNR transaction is a shielded transfer. A note can only be spent by proving knowledge of
its owner's secret key inside the zk-STARK (`spend-circuit.md`). Rescue hashes are not
homomorphic, so there is no "scriptless" trick that would let a secret revealed on Bitcoin unlock
a GNR note. GNR needs a small, explicit hash and time lock:

**Lock note.** A normal note whose owner field is

```
terms    = H(LOCK_TERMS; H(LOCK_HASH; h[0..16], h[16..32]), [T_gnr, 0, 0, 0])
parties  = H(LOCK_PARTIES; owner_claim, owner_refund)
nk_lock  = H(NK_LOCK; terms, parties)
owner    = H(LOCK_OWNER; terms, parties)
```

Here `owner_claim` and `owner_refund` are ordinary GNR owners (`H(OWNER; ak, nk)`), and `h` is
split into 32-bit limbs so it fits the field. Creating a lock note is an ordinary transfer to
`owner`. The note's encrypted memo carries the terms to the refund party, so the refund also
works from the 24 words.

**Spending it.** Transaction version 2 adds public fields to input 0:

```
lock: { hash: h, refund_height: T_gnr, mode: Claim { secret: s } | Refund }
```

* **Consensus checks (outside the proof):**
  * Claim: `SHA-256(s) = h`, and `expiry_height < T_gnr`, so a claim can never be mined at or
    after `T_gnr`.
  * Refund: the block height is at least `T_gnr`. The mempool holds refunds back until then.
* **Circuit (StarkSpend v2):**
  * New public inputs: the lock mode and `terms` (the verifier computes `terms` from `h` and
    `T_gnr`).
  * The circuit proves that input 0's note has owner `H(LOCK_OWNER; terms, parties)` for some
    `parties`. It also proves that the prover knows the secret key of `owner_claim` (claim mode)
    or `owner_refund` (refund mode).
  * The nullifier uses `nk_lock`, so a claim and a refund of the same note have the **same**
    nullifier, and at most one of them can ever be mined.
  * All other inputs and outputs are unchanged. Amounts, the outputs' owners and the other
    party's identity stay hidden.
* **What becomes public:** `h`, `s` and `T_gnr` for the swap input, which links the GNR claim to
  the Bitcoin HTLC (the Bitcoin side is public anyway). The GNR amount and both parties' GNR
  addresses stay private.
* **Activation:** a hard fork at a height announced at least two weeks ahead. The in-app updater
  ships it. Nodes that do not upgrade stop following the chain at that height. The same upgrade
  can fix the reward-timing items R-1 and R-2 from the
  [2026-11-05 audit](../audit/2026-11-05-security-audit.md).

  Until activation, wallets must not create lock notes: the v1 circuit cannot spend them.

## Safety rules the wallet enforces

* **Timelocks** (`Timelocks::check`): when the GNR lock expires, at least 12 hours must remain on
  the bitcoin lock. Bitcoin blocks are counted fast (8 min) and GNR blocks slow (90 s), so a
  burst of bitcoin blocks or slow GNR blocks cannot squeeze the GNR seller's window. The default
  plan is 288 bitcoin blocks (≈ 48 h) and 720 GNR blocks (≈ 12 h).
* **Claim deadline:** the BTC seller claims the GNR at least 120 blocks (≈ 2 h) before `T_gnr`.
  Otherwise the GNR seller could refund the GNR and claim the bitcoin with the secret seen in the
  mempool.
* **Confirmations:** the GNR seller waits for the bitcoin deposit to confirm (1 block for small
  amounts, 3–6 for large ones). The BTC seller waits for the GNR lock to be deep enough that
  rewriting it would cost more than the trade (30 GNR blocks by default, more for large amounts).
  GNR's hashrate is small today, so this is the main limit on swap size.
* **Amounts and addresses** are checked against the agreed terms before each step. Any mismatch
  aborts and refunds.
* **Crash safety:** swap state is stored encrypted next to the wallet after every step. Keys and
  secrets come from the seed, so a restored wallet can finish or refund any swap it finds on
  either chain.

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
| 0 | Bitcoin HTLC, claim, refund, secret extraction, timelock rules, seed derivation, tests against Bitcoin Core's interpreter | **done** (`crates/krypton-swap`) |
| 1 | GNR lock notes: StarkSpend v2, transaction v2, consensus and mempool rules, tests on devnet | next |
| 2 | Wallet swap engine (state machine, Esplora client, encrypted swap state) and the Swap tab; end-to-end swaps on Bitcoin signet and GNR devnet | after 1 |
| 3 | Offer board and message relay on genoryn.io | after 2 |
| 4 | Review of the circuit change, public testnet swaps, then the mainnet activation height | before mainnet |
