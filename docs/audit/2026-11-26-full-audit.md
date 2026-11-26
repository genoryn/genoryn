# Security audit of Genoryn Krypton 1.0.51 (fixed in 1.0.52)

**Date:** 26 November 2026 · **Code reviewed:** 1.0.51 (`1ae203f`), compared with 1.0.2 (`430bb05`),
live mainnet · **Fixes released in:** 1.0.52

*Internal, AI-assisted review of the whole system. Six independent reviewers each covered one
area in depth: the desktop wallet and its local API; atomic swaps and the offer board; Bitcoin
keys, signing, deposits and withdrawals; the node, peer-to-peer layer, updater and installers;
consensus and cryptography; the website, the Genoryn Talks forum and the server configuration.
Every finding was checked against the code before it was fixed, and the most serious ones were
reproduced first: against the real swap engine, and against Bitcoin's own script interpreter
(libbitcoinconsensus). The reviews were read-only: nothing was sent to the live server except
plain page requests, and no secret was read. This is not a substitute for an audit by a
specialised firm. It follows the [5 November audit](2026-11-05-security-audit.md) and the
[1.0.2 review](2026-11-security-review.md).*

## Summary

No issue was found that lets anyone mint coins beyond the schedule, spend someone else's GNR,
break the privacy of transfers or forge a block. The consensus review found nothing Critical or
High; its 93 tests pass. The spend circuit, the post-quantum note encryption, the wallet file
encryption and the release signatures held up.

The serious findings were in the **atomic swaps with bitcoin**, at the step where a wallet
decides that a Bitcoin transaction has done what it should:

* A buyer's refund could end without the buyer getting either the bitcoin or the GNR, while the
  wallet showed the swap as refunded (S-1).
* A seller's wallet could count its bitcoin claim as done before it was in a block, and stop
  watching it (S-3).

Both are fixed in 1.0.52, together with every other finding listed below. Both depended on the
other side of the swap acting against the protocol at the right moment, or on a Bitcoin
transaction that stayed unconfirmed.

**1.0.52 changes no consensus rule.** Blocks valid under 1.0.51 are valid under 1.0.52 and vice
versa, nodes and wallets of both versions work together, and no coin, balance or address
changes. The swap changes are local to each wallet: a 1.0.52 wallet swaps with older ones as
before.

Some findings are still being worked on. They are not described here and will be published when
they are fixed, as with every earlier report.

## How to stay safe

* **Update to 1.0.52.** Wallets with automatic updates (the default) install it within the hour,
  as soon as no swap is running. Otherwise: About, Check for updates.
* Let a running swap finish with the wallet open. From 1.0.52 the wallet stays unlocked by itself
  while its coins are in a swap contract.
* Download only from [genoryn.io/download](https://genoryn.io/download) or the GitHub releases,
  and compare the SHA-256 shown under each button.

## Fixed in 1.0.52

### Atomic swaps and Bitcoin

| ID | Severity | What was wrong | Fix |
|---|---|---|---|
| S-1 | **Critical** | After the bitcoin refund height, the buyer's wallet counted any spend of the bitcoin contract as its own refund. A spend that was in fact the seller's claim, published after the refund, left the buyer's wallet believing the bitcoin was back, so it never used the secret in that claim to take the GNR. | The wallet reads the transaction that spent the contract. If it carries the secret, the wallet takes the GNR with it (the GNR lock outlasts the bitcoin lock for exactly this). The swap counts as refunded only once the wallet's own refund is in a block; until then the refund is sent again as needed. Regression test: `a_refund_replaced_by_the_sellers_claim_still_gets_the_buyer_its_gnr`. |
| S-2 | High | The secret was read from a claim only in one exact witness layout. A claim written in another valid layout hid it from the buyer. | The secret is found in any input that spends the contract, by checking every 32-byte item against the hash lock. Regression test: `the_secret_is_found_in_a_claim_with_a_non_minimal_branch_argument`. |
| S-3 | High | The seller's wallet called its bitcoin claim done as soon as the Bitcoin server listed it, before it was in a block, and never sent it again. | Done only when the claim is in a block. A dropped claim is sent again, and near the deadline with twice the fee rate. Regression test: `the_sellers_claim_is_done_only_in_a_block_and_is_sent_again_when_dropped`. |
| S-4 | Medium | The amounts signed for Bitcoin balance and deposit payments came from the Bitcoin server's lists. | Every coin's value and script are read from its raw previous transaction, which must hash to the coin's transaction id. |
| S-5 | Medium | A contract paid with more than the agreed amount could not be claimed: the claim was signed for the agreed amount. | The claim is signed for the amount actually paid, read from the verified transaction. Regression test: `an_overpaid_contract_can_still_be_claimed`. |
| S-6 | Medium | Every Bitcoin payment spent all coins of the address, the fee ceiling was a tenth of everything spent, and the fee rate from the server was trusted up to 2,000 sat/vB. | Coins are selected: the largest first, only as many as the payment and its fee need. The fee is at most a tenth of what the payment moves (at least 3,000 sat allowed), and the fee rate is capped at 500 sat/vB for every Bitcoin transaction. |
| S-7 | Medium | A watched contract took a payment of any size as the swap's payment. | Only a payment of at least the agreed amount. |
| S-8 | Medium | The Bitcoin data source could be changed without the password, and test GNR networks could be paired with real bitcoin. | Changing the Bitcoin data source or network needs the wallet's password once a wallet exists. Real bitcoin trades only against mainnet GNR. |
| S-9 | Low | The seller waited for at most 2 Bitcoin confirmations before revealing the secret, whatever the size of the swap. | 1 up to 0.01 BTC, 2 up to 0.1 BTC, 3 up to 1 BTC, 4 above. |
| S-10 | Low | Bitcoin addresses with witness versions that have no rules yet were accepted as destinations. Coins sent there can be taken by anyone. | Refused. |
| S-11 | Info | Copies of the swap seed in memory were not wiped. | Wiped after use. |

### Desktop wallet

| ID | Severity | What was wrong | Fix |
|---|---|---|---|
| W-1 | High | Offer ids and sides from the offer board were placed into the wallet window's click handlers, escaped for HTML but not for JavaScript. A hostile or hijacked board could have run script in the wallet window. | The wallet's backend checks everything from the board before the window sees it: ids must have the form every wallet makes, sides must be known values, and swaps in progress are rebuilt from known fields only. Values inside handlers are encoded as JavaScript strings, and the HTML escape covers every quote character. |
| W-2 | Medium | A swap request's id, chosen by the other wallet, was stored without a format check. | Only ids of the form every wallet makes (32 hexadecimal digits) are accepted. |
| W-3 | Medium | The list of nodes the wallet reads the chain from could be changed without the password. | Changing it needs the wallet's password once a wallet exists. |
| W-4 | Medium | The 24-hour lock could lock a wallet in the middle of a swap that only that wallet can finish. | While coins of the wallet are in a swap contract, the lock waits until the swap is done (seven days at most). Locking it by hand then asks first. |
| W-5 | Low | The checks that a node or Bitcoin server runs on this computer compared the start of the address. | Addresses are parsed: https to a named host, or plain http to a loopback address without a user name in front. Regression test: `only_this_computer_counts_as_local`. |
| W-6 | Low | Unlocking, showing the recovery phrase and changing the password were not rate-limited. | One limit for every password check: after 5 wrong passwords, 5 minutes of refusals. One key derivation runs at a time. |
| W-7 | Low | A second start of the app trusted any address in its instance file. | Only an address on this computer. |
| W-8 | Low | Restoring from the phrase could miss a swap lock whose memo ended in bytes that look like a coin marker. | The lock is recovered from the memo exactly as sent. |

### Installer, updates and release signing

| ID | Severity | What was wrong | Fix |
|---|---|---|---|
| R-1 | Medium | Uninstalling deleted everything in the chosen install folder. A folder such as a drive or a profile folder chosen at install time would have been emptied. | Installs go into a folder of their own (a "Genoryn Krypton" folder is created inside any folder that has other files), every installed file is listed, and uninstalling removes only those files and then the folder if it is empty. Regression tests in `krypton-setup`. |
| R-2 | Low | The installer started `powershell` and `cmd` by name, and Windows looks for those in the installer's own folder first (often Downloads). | They are started from the Windows system folder, and DLLs load from the system folder only. |
| R-3 | Medium | The documented node update script came from the moving `main` branch and checked the package against checksums only. | The script is run from the release tag, read before running, and checks the release signature against the key the install carries (packages from 1.0.52 carry the key and the checker). It never installs an older or the same version, and a copy cut short by a dropped download runs nothing. |
| R-4 | Low | The release key generator wrote the key file readable by other users on Unix and did not wipe it from memory. | Created exclusively with owner-only permissions, and wiped. |

### Website and forum

| ID | Severity | What was wrong | Fix |
|---|---|---|---|
| G-1 | Medium | The download buttons pointed to whatever GitHub called the latest release, and the page showed no hash to compare with. | The buttons point to the exact release. Under each one the page shows the file's SHA-256 from the release's signed checksums, checked against the release key when the site is built; a list whose signature does not verify stops the build. The full key fingerprint and the key itself are on genoryn.io. |
| G-2 | Medium | Forum names that only look like the administrator's or another member's (accents, fullwidth or mathematical letters, dotless ı) were accepted. | Names are compared as they look, after Unicode compatibility folding and with marks removed, against reserved words and every existing name. |
| G-3 | Medium | The forum's administrator sign-in was limited per address only, and a new administrator password kept the old sessions. | An overall limit on administrator sign-ins, and a new password ends every administrator session. |
| G-4 | Low | A forum post with many unclosed code tags took quadratic time to render. | Rendering is linear. Regression test in `markup.rs`. |
| G-5 | Low | Anyone could keep a member from signing in by spending the per-name sign-in budget, and names of any length were used as keys. | Sign-in attempts are counted per name and address, with a larger overall budget per name; names longer than the limit are refused at once. |
| G-6 | Low | Brand-new forum accounts could post clickable links. | Links become clickable from the Member rank on. |
| G-7 | Info | The site's HTML escaping did not cover the apostrophe. | It does. |

## What was checked and found correct

**Consensus and cryptography**

* Exactly two consensus changes since 1.0.2, both behind activation heights: swap locks
  (version-2 transactions, active since block 3,300) and Krypton coins (version 3, dormant on
  mainnet). Emission, reward accounting, difficulty, the note tree and version-1 validity are
  unchanged.
* Swap locks: a claim needs the 32-byte secret whose SHA-256 is the hash lock, before the refund
  height; a refund is valid only from the refund height. Both pay exactly the pinned outputs,
  and claim and refund share one nullifier, so a lock is spent once.
* Coins (dormant): unreachable on mainnet; issuance bounded with checked arithmetic; issuer
  actions need an ML-DSA-87 signature; no GNR can be created or destroyed through them.
* Spend circuit: public inputs, constraints, trace shape and proof options unchanged since 1.0.2;
  historical proofs still verify.
* Hybrid X25519 + ML-KEM note encryption, XChaCha20-Poly1305 with one-time keys, the wallet file
  (Argon2id with authenticated parameters, a fresh salt and nonce, written atomically), SHAKE256
  key derivation with distinct domain tags, and randomness only from the operating system.

**Bitcoin keys, entropy and signing**

* New phrases carry 384 bits of entropy from the operating system. The swap seed is derived with
  its own domain tag, separate from the GNR spending and encryption keys, and the Bitcoin balance,
  deposit and per-swap keys and secrets each have their own tagged derivation.
* Only cryptographic random generators are used. ECDSA nonces are deterministic (RFC 6979) and
  signatures low-S. No key or secret is logged, serialised to the window or sent to the board.
* Destination addresses are checked for their checksum and network. Dust is checked per script
  type. Change always returns to the wallet's own address or the user's own address.
* The Bitcoin contract script and both of its witnesses are correct, and the timelock is enforced
  correctly (non-final sequence, lock time at the refund height, sent only from that height).
* Signing is BIP143 with SIGHASH_ALL, with scripts built locally; the contract script is rebuilt
  from the agreed terms and matched against the address it pays.
* The buyer pays only against a confirmed GNR lock of the exact amount and outputs, and checks the
  timelocks again right before paying. The secret learned from a claim is checked against the
  hash lock, from a transaction whose id is verified.

**Desktop wallet**

* The local API listens on 127.0.0.1 only, checks the Host header against DNS rebinding, and
  requires a 192-bit session token, compared in constant time, on every route that uses a wallet.
* Every action that moves coins asks for the wallet's password: sends, swaps, offers, Quick Swap,
  Bitcoin balance withdrawals and coin actions.
* No command goes through a shell; links leaving the app are limited to genoryn.io and the
  project's GitHub; the window cannot navigate elsewhere.
* The recovery phrase is shown only after the password, cleared after 60 seconds, and never put on
  the clipboard. The console cannot send coins, show a secret or change a setting.
* The updater accepts only strictly newer releases signed with the compiled-in hybrid ML-DSA-87 +
  SLH-DSA key, from the project's own GitHub, and re-hashes the installer right before it runs.

**Node and network**

* The node's API is bound to the local machine and reached only through the web server; per-client
  budgets, request timeouts and size caps are in place; no wallet, key or admin method is exposed.
* Mining: the job header binds the payout address, so nobody can redirect a reward; job memory is
  bounded. The miner checks that each job pays its own address and re-verifies GPU results.
* Peer-to-peer: bounded decoding, a deadline for the first message, per-address limits and bans,
  and a public-only address book.
* The checkpoint and difficulty floor for light wallets are enforced.

**Website and forum**

* Every value from the node is escaped or forced into a number or hex pattern before it reaches a
  page. No inline script, no `eval`, no third-party script, style or font; a strict
  Content-Security-Policy and the usual security headers are live.
* Translations cannot change links, code, commands, checksums or the key fingerprint.
* The forum: parameterised SQL only; every value escaped; links only to allowed sites; CSRF tokens
  on every change; Argon2id password hashing; sessions stored only as hashes; no uploads; proof of
  work against bots.

## Tests

Besides the regression tests named above, the full test suite of the workspace passes on Windows,
Linux and macOS in CI, the swap engine's suite runs complete swaps against a mock Bitcoin chain
on GNR devnet, and the release was checked end to end on devnet before it was published.

## Earlier reports

* [5 November 2026, internal audit](2026-11-05-security-audit.md) (1.0.6, fixed in 1.0.7)
* [1.0.2 review](2026-11-security-review.md)
* A private report by an independent researcher, fixed in 1.0.33.

Please report vulnerabilities privately as described in [SECURITY.md](../../SECURITY.md).
