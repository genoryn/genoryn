# Krypton coins (assets)

*"Bring your coin to the quantum network."*

Krypton coins are coins issued by anyone on Genoryn Krypton: a dollar coin backed by an
issuer's reserves (`Dollar Krypton`, USDK), a euro coin (`Euro Krypton`, EURK), a bridged
coin from another chain (`Solana Krypton`, SOLK), a community coin. They are as private and
as quantum-safe as GNR, and they follow three rules that ordinary token platforms do not:

1. **Transfers are always shielded.** Amounts, senders, recipients and even *which coin*
   moves are hidden. Only issuance, burns and pauses are public, so every coin's supply can
   be audited by anyone.
2. **No hidden powers.** An issuer declares, once and in public, what it may do (mint more,
   let holders burn, pause transfers). The declaration is part of the coin's identity and can
   never change; wallets show every power as a plain risk label. There is no per-address
   freeze or blacklist at all: shielded notes make targeting a holder impossible.
3. **GNR is the base.** Every coin transaction pays its fee in GNR, deploying a coin costs
   GNR (paid to the miners), and coins trade against GNR.

## Identity: the manifest

A coin is defined by its **manifest**, a small declarative document (Krypton Asset Manifest,
KAM v1). It is not code: there is nothing to execute and nothing to hide.

```
kam: 1
name: Dollar Krypton
ticker: USDK
decimals: 6
supply: mintable
cap: none
burn: holders
pause: no
backing: issuer-reserves
reserves: https://genoryn.io/coins/usdk/reserves
website: https://genoryn.io/coins/usdk
terms: https://genoryn.io/coins/usdk/terms
logo: 3f1c…(SHA-256 of the logo file, hex)
issuer: 9a07…(ML-DSA-87 public key, hex)
```

| Field | Rule |
|---|---|
| `kam` | `1` |
| `name` | 3–40 printable ASCII characters, must end with ` Krypton` |
| `ticker` | 2–9 characters `A–Z 0–9`, must end with `K` |
| `decimals` | 0–8 |
| `supply` | `fixed` (issued once, exactly `cap`) or `mintable` |
| `cap` | `none` or a whole number of base units ≤ 2^60 − 1; `fixed` coins issue exactly `cap` |
| `burn` | `holders` (anyone holding the coin may destroy it, e.g. to redeem it) or `none` |
| `pause` | `yes` (the issuer may stop all transfers of this coin) or `no` |
| `backing` | `none`, `issuer-reserves` (needs `reserves`), or `bridge` (needs `origin` and `reserves`) |
| `origin` | for bridges: the original chain and coin, e.g. `solana:SOL` |
| `reserves`, `website`, `terms` | optional HTTPS URLs (≤ 200 characters) |
| `logo` | optional SHA-256 of the logo image (shown only once the coin is verified) |
| `issuer` | the issuer's ML-DSA-87 public key (post-quantum signatures) |

The canonical encoding is the fields above in this order, one `key: value` per line, LF line
endings, no trailing spaces. Anything else is refused, so one coin has exactly one manifest.

**Coin ID.** `id = H("Krypton/asset/id", canonical manifest)`; inside proofs a coin is the triple
`(a0, a1, p)`: `a0, a1` are the first two 64-bit words of the id reduced into the field, and
`p = 1` if `pause: yes`, else 0. GNR is the coin `(0, 0, 0)`. Consensus refuses a deploy whose
`(a0, a1)` is already registered or is `(0, 0)`.

**Reserved names.** Consensus refuses tickers `GNRK`, `BTCK`, `XBTK`, `KRK`, `KRYPTONK`,
`GENORYNK` and names containing `Genoryn` or `Bitcoin` (any case). Wallets additionally mark
coins using the ticker of a major coin (`ETHK`, `SOLK`, `USDTK`, `USDCK`, …) as possible
impersonations until verified (below).

## Risk labels

Generated from the manifest, word for word the same in the wallet, the explorer and on
genoryn.io:

| Manifest | Label |
|---|---|
| `supply: fixed` | Fixed supply: nobody can ever create more |
| `supply: mintable`, `cap: N` | The issuer can create more, up to N in total |
| `supply: mintable`, `cap: none` | The issuer can create any amount at any time |
| `burn: holders` | Holders can burn (destroy) their coins, e.g. to redeem them with the issuer |
| `pause: yes` | The issuer can stop all transfers of this coin at any time; its transfers show which coin moves |
| `backing: issuer-reserves` | Its value depends on the issuer actually holding the reserves and honouring redemptions |
| `backing: bridge` | Custodial bridge: its value depends on the custodian holding the original coins |
| `backing: none` | Not backed by anything: its value is only what people pay for it |
| not verified | Not verified by the Genoryn Krypton project: anyone can deploy a coin with any name |

## Transactions

Coin transactions are transaction version 3: an **action** (a 2-in / 2-out shielded transfer
with a version-2 proof that hides the coin) plus an **asset part**:

| Field | Meaning |
|---|---|
| `op` | `deploy`, `issue`, `transfer`, `burn`, `pause` or `unpause` |
| `asset` | the coin ID (public for every op except `transfer`) |
| `burn` | amount destroyed (op `burn`) |
| `manifest` | op `deploy` only |
| `issuer_sig` | ML-DSA-87 signature by the manifest's issuer over the sighash (ops `deploy`, `issue`, `pause`, `unpause`) |
| `fee_action` | an ordinary GNR spend (version-1 proof) that pays the fee, bound to the same sighash (ops `issue`, `transfer`, `burn`) |

The sighash covers the whole transaction except the two proofs and the issuer signature, so
neither proof can be moved into another transaction and the signature covers everything the
proofs do. A version-3 transaction may be up to 320 KB (two proofs); versions 1 and 2 keep
160 KB. Since a coin movement is about twice a transfer's size, wallets pay twice the fee of
one transfer for it.

* **deploy**: the action itself is a GNR transfer paying a fee of at least the deploy fee
  (10 GNR) to the miners' reward pool; the coin is registered with its manifest.
* **issue**: the action mints `mint` new units of the coin (public) into two hidden outputs;
  inputs are dummies. `fixed` coins issue exactly `cap`, once; `mintable` coins may issue
  until the cap.
* **transfer**: inputs and outputs are notes of the same hidden coin; fee 0 in the action,
  the GNR fee in `fee_action`. Input 0 may be a lock note (`lock` set), as in version 2.
* **burn**: destroys `burn` units of the public coin (only if `burn: holders`).
* **pause / unpause**: the issuer stops or resumes all transfers of a `pause: yes` coin.

## Proofs (StarkSpend v2)

The version-2 statement adds a coin section: a public `burn` amount and, when the coin is
public, its triple. The trace gains three columns `(a0, a1, p)`, constant over the program.
Every note body becomes `H(BODY; owner, [v, a0, a1, p])`, so a GNR note (`[v, 0, 0, 0]`) is
unchanged and all four notes of a proof hold the same coin. Boundary rules:

* the coin is public → the columns equal it;
* the coin is hidden → `p = 0` (coins that may be paused always reveal which coin moves);
* `fee > 0` → the coin is GNR;
* balance: `v0 + v1 + mint = w0 + w1 + fee + burn`.

Version-1 proofs (GNR only) stay valid. Consensus requires `mint > 0` only with op `issue`,
`burn > 0` only with op `burn`, both with the coin public, and `fee = 0` in coin actions.

## Activation and reserved tickers

Version-3 transactions are valid from `assets_height` on: block **17,100** on mainnet
(released with 1.0.35, about a day earlier), devnet from genesis, testnet not yet. Older
nodes reject them, so this is a hard fork like the activation of lock spends.

Each network may reserve tickers for one issuer key, by the key's fingerprint
`SHA3("Krypton/asset/issuer", key)` in the network parameters. On mainnet USDK and EURK are
reserved for the project's issuer key, so nobody can deploy them first. Tickers and names are
otherwise first come, first served.

## Supply and state

Consensus keeps, per coin: the manifest, issued, burned, paused, and the deploy height.
Every block's coin changes are undone on a reorganization. Supply = issued − burned, never
above the cap, never above 2^60 − 1 base units.

## Notes and wallets

A coin note's encrypted plaintext carries its coin triple in the last 18 bytes of the memo
(a marker byte and 17 bytes), so token memos hold 110 bytes. Wallets recompute the commitment
with the coin to check it. Balances, history and payment proofs work per coin.

## Verification

Anyone may deploy a coin; verification is separate and off chain. The project publishes
`verified-assets.json`, signed with the project's release key, at genoryn.io and in the
repository. A coin is verified when its issuer has shown, publicly:

* who it is (for coins that claim backing or represent an existing coin);
* for `issuer-reserves`: where the reserves are and how redemptions work, with an attestation;
* for `bridge`: the custodian and the addresses holding the original coins;
* that the logo, name and ticker do not imitate another coin.

Wallets and genoryn.io trust only the signed list: verified coins show their logo and a check
mark, unverified ones a warning and their ID fingerprint, never a logo. Verification can be
withdrawn by a new signed list; it never changes the coin itself.

## Trading

Coins trade against GNR in **one transaction**: the seller's coin spend is the action and the
buyer's GNR spend is the fee action. Both proofs bind the same sighash, which covers every
output, so the transaction is valid only whole and neither side can change what the other
receives. No lock notes are needed.

1. The buyer builds an unproven GNR spend paying the seller the agreed GNR (and the fee) and
   sends its public part (anchor, nullifiers, outputs, fee, expiry) and its address.
2. The seller checks that one of those outputs decrypts to the agreed GNR for it, builds its
   coin spend paying the buyer, proves it and returns the transaction.
3. The buyer checks that its half is unchanged and that an output pays it the agreed coin,
   checks the seller's proof, proves its half and submits.

The transaction expires 20 blocks after it is built, so neither side holds a free option. The
wallets exchange these messages through the offer board's sealed mailboxes, under one-time
keys per offer; the seller's message (about 160 KB in base32) is a large message, which needs
4 more bits of proof of work and counts against the board's byte budget.

The board lists coin offers apart from bitcoin offers (`/v1/swap/coins`), always against GNR.
A coin's market price in GNR is the average of its completed swaps in the last 72 hours (or
of all of them if there are none), without outliers more than 4x from the median once there
are three. The buyer reports a completed swap with the one-time keys and owner tags of the two
outputs that cross; the board opens them on the mined transaction and counts the swap only if
they hold exactly the reported coin and amounts, so a report cannot invent a price.
