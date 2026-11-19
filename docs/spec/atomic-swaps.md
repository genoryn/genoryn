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
| 3 | Buyer | Waits for the lock to confirm: 3 GNR blocks for a payment up to 0.001 BTC, 6 up to 0.01 BTC, 15 up to 0.1 BTC, 30 above (`Timing::gnr_confs_for`), so undoing the lock costs more than the trade. The lock is found only if it holds exactly the agreed amount, pays the buyer's own claim outputs and lasts long enough; only then does the wallet show where to pay. The buyer pays the bitcoin into the HTLC (refund after `T_btc`, ≈ 6 h) within 2 hours. |
| 4 | GNR seller | Waits for the bitcoin to confirm (1 block up to 0.01 BTC, else 2), then **claims the bitcoin** with `s`, at least 6 blocks before `T_btc`. This publishes `s` on Bitcoin. |
| 5 | Buyer | Its wallet reads `s` from that bitcoin transaction and **claims the GNR**, long before `T_gnr`. |

If anyone stops halfway, the timelocks return the coins:

* **The seller has no GNR, locks less, or never locks:** no confirmed lock matches the terms,
  so the buyer's wallet never shows where to pay. The buyer pays nothing.
* **The buyer never pays:** the seller refunds the GNR after `T_gnr`.
* **The seller never claims the bitcoin:** `s` stays secret; the buyer refunds the bitcoin
  after `T_btc` and the seller refunds the GNR after `T_gnr`.
* **The seller claims the bitcoin:** `s` is public, so the buyer can always claim the GNR,
  because the GNR lock outlasts the bitcoin lock by at least 4 hours (6 with a wallet before 1.0.43). The buyer's check
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

* **Offers:** side (sell or buy GNR), amounts, expiry (1 hour to a year) and a one-time hybrid
  X25519 + ML-KEM-1024 key. The key is derived from the wallet's swap seed and is used for
  nothing else, so offers cannot be linked to an address or to each other. A hash of a secret
  lets only the maker cancel the offer.
* **Mailboxes:** messages are sealed to the recipient's offer key, and carry the sender's key
  for the reply. The board stores ciphertext for a day under `inbox = H(key)`.
* **What travels:** taking an offer starts a swap, and the two wallets send each other the
  request, accept and deposit codes through the mailboxes by themselves. A wallet accepts
  only a request with exactly the offer's amounts, and an offer is taken once. The only
  manual step left is the buyer paying the bitcoin into the contract address.
* **Spam:** offers, messages and trade reports carry an 18-bit proof of work; an offer that
  runs longer than a day needs one more bit per doubling (oard::offer_bits: a week 21,
  a month 23, a year 27), so keeping junk listed costs as much more as it lasts. There are
  per-client limits, caps on offers, messages and inbox size, and expiry. The board keeps
  offers, swaps in progress and mailboxes in `swap-board.json` across restarts.
* **Instant takes:** a maker's wallet pings the board for all its open offers at once
  (`POST /v1/swap/offers/ping`, every 45 s, each offer proven with its cancel secret). An
  offer whose maker was seen in the last 2 minutes is listed as `online`. The wallet checks
  the board mailboxes every 5 s while it has open offers or takes, and answers a take by
  itself, so a swap starts within seconds with no approval. Wallets refuse to take an
  offline offer. An offline maker's offers stay listed (not takeable) for up to a week, so a
  long offer survives a computer that sleeps; then they are removed and the wallet frees the
  reserved GNR.
* **Proof of funds (from 1.0.40):** a sell offer is listed only after the board has checked
  that its GNR is there. See [Proven sell offers](#proven-sell-offers-from-1040).
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

## Proven sell offers (from 1.0.40)

Before 1.0.40 a sell offer only reserved its GNR inside the maker's own wallet, so a wallet
that ignored the reservation could list GNR it did not have. Such an offer could never take
anyone's bitcoin (the buyer pays only after a lock of exactly the agreed amount has confirmed),
but it made the board lie. Now the board checks.

**What cannot be done.** GNR cannot be locked on chain *for a buyer* before the buyer exists:
a lock note commits to the buyer's claim outputs. The offer is therefore backed by an
ordinary note of the maker, proven to the board, and locked for the buyer when the offer is
taken, in one transaction.

**Setting the GNR aside** (`Wallet::set_aside`). Publishing puts the offer's amount plus the
fee of the lock (`RESERVE_FEE`, the wallet's default fee) into a note of its own: a transfer
of the wallet to itself, after merging notes if the amount is spread over more than two. The
wallet marks that note as held (`Wallet::holds`): sends, merges and other offers never select
it. A hold is a record in the wallet's own encrypted file, not a rule of the chain: it can only
keep coins in the wallet, never lose them, and a wallet restored from its recovery phrase has
none.

**The proof** (`Wallet::prove_reserve`, `board::check_reserve`). The wallet builds an ordinary
version-1 shielded transfer that spends the note and

* pays the offer's amount to a note whose owner tag and randomness are hashes of the offer
  (id, key, cancel hash, amount, network), so its first output commitment is a public
  function of the offer, `reserve_cm(offer)`;
* declares a fee of at least `RESERVE_FEE`;
* has `expiry_height = 1`.

Consensus refuses a transaction whose expiry height is below the height of the block that
would include it (`check_tx_state`, applied to every transaction of every block and of the
mempool). With an expiry height of 1 that is every block after the first, and the first block
of a chain is long mined: a wallet makes the proof only when its chain is past that height
(and never with an expiry of 0, which means "no expiry"), and a board accepts one only then.
So this transfer can never be mined: nothing moves, and nothing is sent to the network. Its
STARK proof is nevertheless a full proof of the spend statement. From it the board learns that

* the prover can spend notes in the note tree (membership under the anchor, knowledge of the
  spending key), and they are plain GNR notes (version 1, no lock, no coin);
* their value is at least the offer's amount plus the lock fee (the balance equation, with
  the first output's value fixed by its public commitment and the fee public);
* their nullifiers.

The proof fits one offer: another offer has another `reserve_cm`. It reveals no address, no
balance and no other note of the wallet. The output it "pays" has no spending key anyone
knows; this never matters, because the transfer cannot be mined.

**What the board does** (`apps/kryptond/src/swapboard.rs`). For a sell offer it

1. checks the offer's proof of work first, then the proof of funds off the async runtime;
2. requires the anchor to be a note root of its own main chain and both nullifiers unspent,
   in the chain and in its mempool;
3. refuses a nullifier that already backs another listed offer or a swap in progress. The
   check and the listing are one step under the board's lock, so of two offers posted at the
   same moment with the same coins only the first is listed;
4. keeps the nullifiers and the anchor, not the proof, and never publishes them: the list
   shows `backed: true`;
5. looks the nullifiers of open offers up again every few seconds and takes an offer down
   when one is spent, or when a reorganisation removed the anchor. The maker's wallet then
   proves the same note again if it still has it, and otherwise ends the offer;
6. keeps the nullifiers reserved when the offer is taken and moves to "in progress", until
   that entry leaves the board (the swap ended, or its maker stopped confirming it). A maker
   cannot move an offer to "in progress" by itself and list the same coins once more.

Posting to `/v1/swap/offers` is limited to 10 requests a minute per client. A board that
switches to these rules drops the offers it stored before (it cannot tell which were real);
swaps in progress and mailboxes are kept.

**Taking a proven offer.** The maker's wallet accepts with `Engine::accept_from`, which names
the note, and `Wallet::lock_funds` spends exactly that note into the lock. The lock no longer
depends on which notes the wallet happens to have, or on merging them first. By then the
offer is "in progress" on the board, with the note's nullifiers still reserved; the lock
spends them for good. An offer counts as taken only once its swap exists in the wallet, and
a request the wallet accepted just before it was closed is picked up again when it starts:
the swap carries on and its codes reach the buyer.

**A week's commitment** (wallet rule from 1.0.42, shown before publishing; `COMMIT_SECS`).
GNR that the board listed in a sell offer is committed for 7 days from that moment
(`Reserve::listed`). Sold within the week, it goes to the buyer. If the offer is cancelled,
runs its time, is taken down by the board, or its swap ends before anything was locked, the
wallet keeps the note out of sends until the week is over (`commit_until`); a new sell offer
of the same amount takes it at once, with no new transfer, and starts a new week. A note that
was still committed when it went into a new offer stays committed at least as long as before,
however the new offer ends (`Hold::keep`). GNR that never reached the board is free at once,
and so is a note whose offer the wallet has no record of (`reconcile_holds`, run in every
background round, with or without the board). Offers published before 1.0.42 keep the rule
they were published under: 24 hours after a cancellation, or the offer's end if sooner.

This is a rule of the wallet and not of the chain. It cannot be forced on modified software
or on a wallet restored from its recovery phrase, and no consensus rule could close that gap
for good: the chain does not see whether a sale was paid in bitcoin, so a seller who wanted
out could always sell to themselves. Locking the note on chain for a week would hold, but
nobody could buy it then: a swap's lock names the buyer's outputs, which do not exist before
there is a buyer. What the chain and the board do guarantee is that an offer whose GNR moved
is no longer listed.

**Buyers whose bitcoin is ready** (1.0.43). A sell offer can be for prepaid buyers only
(`MyOffer::prepaid_only`, the default; the board lists `prepaid: true`). A buyer whose
bitcoin waits at a deposit address puts a proof into its request (`Msg::Request2::prepaid`):
the address's public key and its signature over the request's id, amounts, refund key and
Bitcoin network (`engine::prepaid_address`). The seller's wallet looks the address up with
its own Bitcoin server and locks only if at least the swap's amount is there, confirmed and
unspent (`deposit_shown`). A buyer could still move that bitcoin afterwards instead of
paying, but has to hold it for every offer it takes, and an unmodified wallet pays by
itself. For buyers whose bitcoin the wallet has not seen, it holds at most
`MAX_UNPAID_LOCKS` (2) unpaid locks at a time and answers further requests with "gone". A
higher proof of work for takes was considered and left out: requests are sealed, so the
board cannot weigh them, and a wallet-side rule would shut out older wallets while costing
an attacker seconds.

**Shorter locks and fewer confirmations** (1.0.43). The buyer waits for confirmations on
the seller's lock by the size of its own payment: 3 blocks up to 0.001 BTC, 6 up to
0.01 BTC, 15 up to 0.1 BTC, 30 above (`Timing::gnr_confs_for`); only the buyer's wallet
applies this. A buyer's wallet now locks its bitcoin for 18 blocks and keeps 4 hours to
claim, which needs 1,080 GNR blocks left when it pays; a request says `v: 2`
(`engine::PROTOCOL`), and for such a buyer the seller locks 1,260 blocks (about 21 hours)
instead of 1,900. A request without `v` comes from a wallet that would refuse the shorter
lock and gets the old one. A new buyer accepts either from a seller.

**One wallet on several computers** (1.0.43). Each copy of a wallet seals reports of its
swaps (the swap record and its board link, on every change of stage, and a sign of life
every hour) to a mailbox whose key derives from the wallet's seed (`MIRROR_N`); a report
carries a tag made with the seed (`mirror_tag`), so only a copy of the wallet can have
written it. Other copies read the mailbox every two minutes (`mirror_read`), show those
swaps, and can adopt one (`adopt_swap`, "Carry on here"): the record becomes a swap of that
copy, which then does whatever step is open. Both copies running a swap cannot lock or pay
twice, because each step spends a note, a lock or a bitcoin output that exists once; a
seller's copy that finds the lock already made adopts it. The board sees an inbox with
encrypted reports, not whose they are.

**Nothing waits without saying why** (1.0.42). A seller's swap whose buyer did not pay
shows the refund block and the hours until then; the lock cannot be opened sooner by anyone.
A take of a buy offer that its maker's wallet does not answer ends after ten minutes
(`TAKE_ANSWER_SECS`) and is shown meanwhile. `Engine::step` ends swaps that ran out even
while the Bitcoin server cannot be reached, and says so on the swaps that wait for it. A
seller whose bitcoin claim lost to the buyer's refund takes the GNR back at its refund height.

**What this does not solve.** A taker can still take an offer and never pay: the seller's GNR
then waits in its lock for the refund height. That is the free option every atomic swap gives
the side that acts second; it costs the seller time, never coins. The board counts such a
swap as "taken, not paid yet", and as running only once the maker reports that the bitcoin is
in the contract.

**Buy offers** cannot put bitcoin into the swap contract when they are published. The
contract names the seller's claim key and the hash of the seller's secret, and neither exists
before a seller takes the offer. See the next section for what a buy offer does instead.

## Buy offers with the bitcoin deposited (from 1.0.41)

A buyer pays the bitcoin when publishing the offer, to an address of the buyer's own wallet,
and from then on does nothing: the wallet pays the contract when a seller's GNR is locked,
and sends the bitcoin back if nobody sells.

**The deposit address** (`krypton_swap::deposit`). A pay-to-witness-public-key-hash address
whose key derives from the wallet's swap seed and a small number (`deposit::key(seed, n)`,
its own derivation tag), so the 24 words find it again. The wallet asks for the offer's
amount plus a fee reserve: twice today's fee for the payment into the contract, at least
1,000 sat (`deposit::fee_reserve`). The bitcoin is the buyer's the whole time: nobody else
has the key, and there is no script on it.

**Listing** (`apps/krypton-wallet/src/board.rs`, `advance_buy`). The wallet watches the
address. When unspent outputs worth the offer and at least half of the reserve have one
confirmation, it seals the offer's proof of work and posts the offer with
`funding = {key, sig}`: the address's public key and an ECDSA signature over the offer (id,
offer key, cancel hash, amounts, both networks; `board::sign_funding`). The signature shows
that whoever posts the offer controls the address; it fits no other offer.

**What the board does** (`apps/kryptond/src/swapboard.rs`). It checks the signature
(`board::check_funding`). If it is configured with a Bitcoin server (`KRYPTON_BTC_API`: an
Esplora URL, or `rpc:` and a Bitcoin Core URL) it looks the address up: confirmed, unspent
outputs worth at least the offer's bitcoin must be there, or the offer is refused. It then
lists the offer with `deposited: true`, keeps the address to itself, refuses a second offer
on the same address, and looks the deposits of open offers up again every five minutes,
taking the mark off an offer whose bitcoin moved. A board without a Bitcoin server lists the
offer unmarked. The mark is a statement of the board; a seller who takes a marked offer
risks what a seller always risks, GNR locked until its refund height if the buyer does not
pay, never coins.

**The payment** (`Engine::pay_from_deposit`). A swap that comes from such an offer carries
the deposit's number (`Swap::fund`). When it reaches the point where a person would be shown
where to pay (the seller's lock holds exactly the agreed amount, only the buyer can claim it,
it has its confirmations, and the time locks leave the safe margins), the wallet checks once
more that the payment window is open, the lock is still on chain and the margins still hold,
and then spends the deposit: output 0 is the contract, exactly the agreed amount; what is
left after the fee goes to the buyer's own address. The fee follows the fee rate of the
moment; if the reserve does not cover it, a lower fee is paid, down to the relay minimum,
and never anything less than the agreed amount. A deposit that no longer covers the amount
is not spent at all. From here the swap is an ordinary one, and the bitcoin refunds from
the contract at its time lock if the seller never takes it.

**The bitcoin comes back** when the offer is cancelled, runs its time, is removed by the
board, or its swap is over: `Engine::sweep_deposit` sends everything at the address to the
buyer's address (`settle_funds`), and the wallet keeps looking for three days so a payment
that arrives late goes back too. A swap that ends before the buyer paid puts the offer back
on the board with the same deposit. A wallet restored from its phrase has no record of its
offers: `POST /api/board/recover-btc` (Swap, Bitcoin connection) looks at its deposit
addresses and sends what it finds to an address the user gives.

**Taking a sell offer with the bitcoin paid first** (`take_offer`, `advance_takes`). A
buyer who takes a sell offer can pay in advance in the same way. The wallet then does not
send its request at once: it waits until the deposit has one confirmation and the offer is
still listed, proven and its maker online, and only then starts the swap, which pays from
the deposit by itself. A seller's GNR is therefore locked only for a buyer whose bitcoin is
ready (an honest wallet's rule; the cost of taking without paying is still open, see issue
9). If the offer is gone by then, no bitcoin arrives within two hours, or the swap cannot
start for six, the purchase is called off and what was sent goes back.

**Limits on fees and inputs.** No transaction of a deposit pays more fee than a tenth of
what it moves (3,000 sat for small ones), whatever fee rate the Bitcoin server reports, and
it spends at most the 50 largest outputs at the address, so dust sent to the address by
someone else cannot stand in the way. Deposit numbers come in 64 lanes; each copy of a
wallet picks one at random and passes over an address that has been paid before, so two
copies of a wallet do not hand out the same address, and the numbers stay few enough to
look through when a wallet is restored (`recover_btc`).

**Privacy.** The deposit address is a Bitcoin address like any other; the board that checks
it learns it, the public listing does not show it. The payment into the contract links the
deposit to the swap on the Bitcoin chain, as any payment into a contract does.

**The same wallet on two computers.** Offers and swaps are records of the wallet that
started them; a second copy of the wallet (the same phrase) does not have them. It does see
the lock note it funded, because that note is encrypted to the wallet's own key
(`Wallet::recover_lock`), and shows it as "locked in a swap on another computer"
(`engine::elsewhere`). It leaves the lock to the copy that runs the swap and refunds it
itself only `ELSEWHERE_REFUND_DELAY` (30) blocks after the refund height, if it is still
there. Numbers of offers and takes, from which their keys and mailboxes derive, are random
from 1.0.41, so two copies never give two offers the same key.


**Tests.** `crates/krypton-swap/tests/engine.rs` (`a_sell_offer_proves_its_gnr`): the proof is
sound, fits one offer, cannot be mined, is refused when it could be mined, leaves no fee or
was altered, cannot be made without the coins, and a take spends exactly the note.
`crates/krypton-wallet-core/src/tests.rs` (`notes_set_aside_are_not_spent`): held notes are
never selected, survive a restart and go into a new offer without a transfer.
`apps/kryptond/src/swapboard.rs`: no listing without a proof, the same coins behind one thing
at a time (an open offer or a swap in progress), and coins free again when their entry ends.
`apps/krypton-wallet/src/board.rs`: when GNR stays set aside and when it is freed.
Before each release the whole flow is also run end to end with two desktop wallets, a node
and Bitcoin Core on regtest: listing only after the proof, refusals without one, a wallet
restored from the same phrase that tries to list the same coins again (refused) and then
moves them (the board takes the offers down within seconds), and a complete swap.

## Plan

| Phase | Work | Status |
|---|---|---|
| 0 | Bitcoin HTLC, claim, refund, secret extraction, timelock rules, seed derivation, tests against Bitcoin Core's interpreter | **done** |
| 1 | GNR lock notes: circuit, transaction v2, consensus and mempool rules, wallet lock, claim, refund and recovery | **done**, active on devnet |
| 2 | Swap engine (codes, state machine, Esplora and Bitcoin Core clients, encrypted swap state) and the Swap tab | **done**, tested on devnet + Bitcoin regtest |
| 3 | Offer board, sealed mailboxes and the market price from completed swaps | **done** |
| 3a | Sell offers prove their GNR to the board (1.0.40) | **done** |
| 4 | Mainnet activation at block 3,300 (release 1.0.13); first swaps with small amounts; an independent review of the circuit change | **scheduled** |
