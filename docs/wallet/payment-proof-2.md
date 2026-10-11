# Verified Payment Proof 2.0

Alice sends 10 GNR to Bob. Bob wants cryptographic confirmation that the payment is real, paid
his address the exact amount, and — optionally — that the person he is talking to controls the
payment's sender credential. Neither needs the other's seed, spending key or viewing key, and
no other payment or balance is disclosed.

## Three levels of assurance, kept apart

| Level | What it establishes | How |
|---|---|---|
| **Payment verification** | An output of a valid transaction on the canonical chain this wallet validated pays this amount to this address, with this many confirmations | Note opening (`rho`, `rseed`, recipient, amount) checked against the output's commitment; full transaction proof and signatures checked |
| **Sender-origin disclosure** | The proof was produced with the paying wallet's outgoing key for this output | The output's outgoing cipher key (`ock`, GNRPROOF1 since 1.0.5) decrypts the sender ciphertext to the same note. Forwardable like any file: it does **not** show who presents it now |
| **Fresh sender authentication** | Whoever answered a fresh challenge holds the secret key the payment committed to when it was created | Ed25519 signature over the verifier's fresh challenge, checked against the public key in the payment's own encrypted memo |

The wallet never labels a note opening, a transaction id or a replayed `ock` as fresh sender
authentication.

## The payment-bound key

New payments (GENORYN 1.1 and later) commit a payment-specific Ed25519 public key in the last
40 bytes of the output's 512-byte memo:

```
memo[0..471]    user text (UTF-8, at most 471 bytes), then zero bytes
memo[471]       0x00 (always; earlier wallets read text only up to the first zero byte)
memo[472..476]  "GNRA"
memo[476]       version 1
memo[477..480]  zero (reserved)
memo[480..512]  Ed25519 public key
```

- **Consensus-compatible.** The memo is part of the note ciphertext; the transaction's spend
  authorization and binding signatures cover it, so only the transaction's creator chose this
  key. No consensus field or rule changes; earlier wallets show the memo text exactly as before.
- **User memos are preserved.** A memo of up to 471 bytes gets the key; a 472–480 byte memo is
  sent whole, without one (the existing 480-byte limit is never exceeded or truncated).
- **Secret derivation.** `root = HKDF(seed, "GENORYN/sender-auth/root/v1", network ‖ account)`;
  `key = HKDF(root, "GENORYN/sender-auth/payment/v1", genesis ‖ anchor_nullifier ‖ recipient ‖
  amount ‖ ordinal)`, where `anchor_nullifier` is the smallest nullifier among the paying wallet's
  own spends in the transaction (public once mined, unique because a note is spent once) and
  `ordinal` the output's position in the requested payments. The root comes from the recovery
  seed, **not** from viewing keys: a recipient, a watch-only wallet or anyone holding a forwarded
  proof sees the public key but cannot sign. A wallet restored from its 24 words derives the same
  keys (tested), so nothing extra must be backed up.
- **Unlinkable.** Every payment gets a different key; there is no reusable sender identifier.

## Challenge and response

Bob, after verifying the payment, presses **Request sender verification**:

```
GNRCHAL1-<base64url(version ‖ network ‖ txid ‖ output ‖ recipient(43) ‖ amount ‖ nonce(32) ‖ expiry ‖ checksum)>   184 characters
```

Alice pastes it into **Answer payment challenge**. Her wallet answers only if it made exactly
that payment: it finds the output with its outgoing viewing key, checks network, recipient,
amount and expiry, reads the committed key from its own memo, re-derives the matching secret
from its seed and signs

```
SHA-256-domain("GENORYN/payment-challenge/response/v1",
               genesis ‖ chain_id ‖ challenge bytes ‖ committed public key)
```

and returns

```
GNRRESP1-<base64url(version ‖ challenge id ‖ Ed25519 signature ‖ checksum)>   144 characters
```

Bob pastes the response into **Verify sender response**. His wallet checks the payment on the
chain again, reads the committed key from the payment's memo (from Alice's proof, or from the
note his own wallet received), checks that the response answers his exact challenge and that it
has not expired, and verifies the signature (`verify_strict`).

- A copied old response does not satisfy a new challenge (different nonce, different id).
- A response for another payment, recipient, amount, output or network is rejected (the signed
  message covers all of them, plus the genesis hash and chain id).
- Bob's own receiving keys cannot produce a response; neither can any other wallet.
- Challenges stay valid for one hour by default (at most a day).

## Outcomes the wallet shows

| Outcome | Shown when |
|---|---|
| Payment verified | The transaction is on the validated chain and the output opens to the stated recipient and amount |
| Recipient mismatch / Amount mismatch | The payment is real, but not to the address or for the amount entered |
| Payment pending or not found | Not in any block of the validated chain (yet) |
| Payment replaced or reorganized | Confirmed earlier in this session, no longer on the validated chain |
| Verification unavailable | Quick mode could not reach any source: no current verified chain view |
| Legacy sender evidence only | Valid proof with sender-origin disclosure, but the payment has no committed key |
| Sender challenge verified | The response is a valid signature by the committed key over Bob's fresh challenge |
| Invalid proof / Sender verification failed | Any cryptographic or binding check failed |

Nothing is shown green unless it was verified on the chain this wallet validated.

## What it does not prove

A verified challenge proves control of the payment's credential, not a legal identity, name or
stable sender address. If Bob must know that the responder is Alice, he has to associate the
exchange with Alice through a channel he already trusts. Secrets can be shared or stolen; no
system can rule out that someone else holds them.

## Compatibility

- `GNRPROOF1-…` codes, JSON and PDF proofs from 1.0.5 stay valid and verify as before.
- Payments made before 1.1 (or with the key switched off, `sender_auth: false`, or a memo over
  471 bytes) report *legacy sender evidence only*; no fresh authentication is offered for them,
  and historical transactions are never modified.
- Quick mode supports every operation over Tor with local validation.

## Privacy

- A proof opens one output: its recipient, amount and, with the outgoing cipher key, its memo
  (which now includes the payment-specific public key). No other output, balance or key.
- The challenge reveals what Bob already knows (txid, output, his address, the amount); the
  response is a signature only.
- Verification uses the wallet's own validated chain; no remote verification service is asked.

## Checking a proof on genoryn.io

[genoryn.io/verify](https://genoryn.io/verify) offers the same checks to anyone without a wallet:
paste the code, the JSON or the PDF, optionally the expected address and amount, and the page
shows the same outcomes; for proofs made by the paying wallet it also runs the fresh sender
challenge. The site's server runs this release's `genoryn-wallet-cli` (`verify_proof`,
`create_payment_challenge`, `verify_payment_challenge`) against the genoryn.io full node, with a
valueless verifier wallet that only keeps the validated chain.

- The proof is sent to genoryn.io only to be checked, so the server sees that one payment's
  recipient, amount and memo. It is not stored or logged, and it cannot move coins.
- The answer is only as trustworthy as genoryn.io and its node. A check that depends on no one is
  **Verify payment** in your own wallet.
- Proof text is passed on unchanged: amounts above 2^53 gori would lose precision in JavaScript.
- `scripts/test/website_proof_verify.py --lab <devnet.py --keep work dir>` checks the site
  against a real devnet payment: verified proof, mismatches, a forged amount, a receiving-side
  proof, an unknown payment, a challenge answered by the paying wallet and a replayed answer;
  with `--screenshots DIR` it also drives the page in Edge.

## Evidence

- `crates/genoryn-wallet-core/tests/payment_proof_v2.rs`: a real devnet chain (RandomX blocks,
  Halo 2 proofs): Alice pays Bob exactly 10 GNR; proof; fresh challenge; Alice's and a restored
  wallet's answers verify; Bob's and Carol's wallets cannot answer; replayed answer; wrong
  payment, recipient, amount and network; tampered proof; missing transaction; legacy payment;
  long memo; unrelated outputs stay closed; a heavier competing chain removes the payment.
- `scripts/test/devnet.py` check `payment_proof_2_fresh_sender_challenge_impostors_and_replay_rejected`:
  the same flow through the real wallet CLI on a three-node devnet.
- `crates/genoryn-wallet-core/src/sender_auth.rs` unit tests: memo trailer bounds, key
  separation and recovery, code damage detection, hostile bytes.
