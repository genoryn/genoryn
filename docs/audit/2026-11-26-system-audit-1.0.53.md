# Security audit of Genoryn Krypton 1.0.53

**Date:** 26 November 2026 · **Code reviewed:** the 1.0.53 release branch, live mainnet ·
**Fixes released in:** 1.0.53

*Internal, AI-assisted review of the whole system. Four independent reviewers each covered one
area in depth: the atomic swaps with bitcoin; the node, its peer-to-peer layer, the offer board
and the server configuration; the desktop wallet and its local API; cryptography, transaction
decoding and the light client. Every finding was checked against the code before it was fixed;
where a fix could change what honest users see, it was tested against the real mainnet chain
first. The reviews were read-only. This is not a substitute for an audit by a specialised firm.
It follows the [1.0.52 audit](2026-11-26-full-audit.md).*

## Summary

No issue was found that lets anyone mint coins beyond the schedule, spend someone else's GNR,
break the privacy of transfers or forge a block for a full node. Nothing here changes the rules of
the chain: every fix is in the wallet, the node's handling of its peers and its API, the swap
engine or the server configuration.

The most serious findings were about what a **light wallet connected to a dishonest node** could
be shown, and about **one peer slowing down a node** for all others:

* A dishonest node could get a light wallet to accept blocks that were not backed by real proof
  of work, and show it payments that never happened (L-1). Light wallets now check every block
  above the release checkpoint as a full node does.
* A tiny payment could be made to show as a huge amount in the recipient's wallet (L-2). Such
  notes are now refused.
* One peer could hold up a node's handling of every other peer's messages (N-1).
* With only the wallet window's session token, and without any password, a wallet of someone
  else's could be added and made the one that receives payments and mining rewards (W-9).

The wallet's default node is genoryn.io; the light-wallet findings matter to anyone who adds
other nodes. Every finding below is fixed in 1.0.53.

## How to stay safe

* **Update to 1.0.53.** Wallets with automatic updates (the default) install it as soon as no swap
  is running. Otherwise: About, Check for updates.
* Add only nodes you trust to a light wallet, or run your own full node.
* Download only from [genoryn.io/download](https://genoryn.io/download) or the GitHub releases,
  and compare the SHA-256 shown under each button.

## Threat model

| Who | Can | Must not be able to |
|---|---|---|
| A node a light wallet reads from | Send any data, refuse to answer, be slow | Make the wallet accept blocks nobody worked for, fake payments, hide the wallet's coins for long, crash it |
| A peer of a node | Send any message at line rate from many addresses | Stall the node, push honest peers out, fill its memory or disk |
| A swap counterparty | Send any code, pay late or not at all, vanish | Take both coins, leave the other side's coins stuck for good |
| A Bitcoin server | Lie about amounts, confirmations and spends | Make a seller give away the secret without being paid |
| A program or page on the same computer | Reach 127.0.0.1, read files of other users where permissions allow | Use the wallet without its session token, spend or redirect coins without the password |

## Fixed in 1.0.53

### Light wallet and cryptography

| ID | Severity | What was wrong | Fix |
|---|---|---|---|
| L-1 | **Critical** | The tip proof a light wallet asked a node for could be made to check almost nothing, and the blocks the wallet then applied were checked for their links and contents but not for proof of work. A dishonest node could show a short chain of cheap blocks, with payments in them. | The tip proof must be for exactly the height the node reported. Every block above the release checkpoint is checked as a full node checks it (proof of work, the difficulty the rule sets, its time), from headers linked to the wallet's own last block, and the chain must pass through the checkpoint. A fresh wallet starts at the checkpoint. Checked against mainnet: a new wallet verifies the about 960 blocks since the checkpoint in 7 seconds. Regression tests: `sync_checks_every_header_as_a_node_does`, `headers_need_the_rules_difficulty_work_and_time`, `earlier_headers_must_link_to_the_last_block`. |
| L-2 | High | A note's commitment reduces its value modulo the field, so a sender could encrypt a value far larger than what the commitment pays, and the recipient's wallet showed that. | Notes whose value is above what a spend can prove are refused. Regression test in `address_roundtrip_and_note_encryption`. |
| L-3 | High | The work a tip claimed was remembered before the blocks up to it had been checked, so a tip that then failed could keep the wallet from honest nodes for hours. | Remembered only once every block up to the tip passed. |
| L-4 | Medium | Payment proofs counted confirmations from the height the node named. | From the tip proven by proof of work. |
| L-5 | Medium | Payments to the wallet waiting in the mempool were shown without any check. | Shown only for well-formed transactions whose zk-STARK proof verifies. |
| L-6 | Medium | A malformed note-tree frontier from a node could make the wallet stop with an error at every sync. | Only well-shaped frontiers decode. Regression test: `only_well_shaped_frontiers_decode`. |
| L-7 | Low | A few node responses were read without a size limit, a witness response's shape was not checked, and work sums in proofs could wrap around. | Bounded, checked and saturating. |

### Node, peer-to-peer and server

| ID | Severity | What was wrong | Fix |
|---|---|---|---|
| N-1 | **Critical** | The node checked a peer's transaction proofs in its message loop, so one peer could slow the handling of every peer's blocks and messages. | Transactions from peers are checked outside the loop, a few at a time and apart from wallets' submissions, within a budget per peer. A transaction found invalid costs nothing again, and messages a dropped peer left queued are not handled. |
| N-2 | High | Block announcements could fill the table of blocks being requested. | A budget for announcements, a few blocks each, and a full table makes room by dropping the older requests of the peer holding the most. |
| N-3 | High | A bare connection could push an inbound peer out before completing its handshake. | Peers make room only for a connection that completed its handshake, and connections in their handshake count for their address. |
| N-4 | High | One address could fill the board's relay of sealed swap messages. | A daily byte quota per client, on top of the message counts. |
| N-5 | High | The client address the node budgets by came from a header that only the firewall protected. | The web server takes the client address from Cloudflare's header only for Cloudflare's addresses (`trusted_proxies`, `client_ip_headers`); deployed on genoryn.io. The BTC price request carries nothing about the visitor. |
| N-6 | Medium | A board file that could not be read was replaced by an empty one at the next write, and board files were not synced to disk before replacing the old ones. | Moved aside and logged; written to disk before the rename. Regression test: `an_unreadable_board_file_is_kept_aside`. |
| N-7 | Medium | Proofs of funds of sell offers were checked without a limit on how many at once. | Within the same limit as transactions. |

### Desktop wallet

| ID | Severity | What was wrong | Fix |
|---|---|---|---|
| W-9 | High | With the session token alone, a wallet could be added and made the active one, so the receiving address and mining payouts would be someone else's. | Once there is a wallet, adding another needs the open wallet's password. |
| W-10 | High (Linux, macOS) | The file with the session link could be read by other users of the computer, and the link was printed when the wallet started. | The data folder and the wallet files are for their user alone, and the link is no longer printed. |
| W-11 | Medium | Switching between light and full mode, and letting other nodes connect, did not need the password. | They do, like the node list. |
| W-12 | Medium | A second start could follow the link of a wallet that had crashed before checking that a wallet was running. | The wallet takes its lock first and only then follows a running wallet's link. |
| W-13 | Low | The wallet's answers carried no frame or content-type protection. | `X-Frame-Options`, `frame-ancestors 'none'`, `nosniff` and `no-referrer`. |

### Atomic swaps

| ID | Severity | What was wrong | Fix |
|---|---|---|---|
| S-12 | High | A seller could reveal the secret with only six blocks of the bitcoin lock left, and a claim waiting in a busy mempool got more fee only once. | The secret is revealed only with ten blocks left, and an unconfirmed claim is signed again every block with more fee as the lock nears. A claim sent before the wallet stopped is picked up instead of a new one. |
| S-13 | Medium | A payment the Bitcoin server listed was taken at the server's word even when the server's own copy of the transaction did not pay the contract. | Where the server shows the transaction, it must pay the contract. |
| S-14 | Medium | A seller whose buyer never claimed the GNR did not get it back once the swap was done. | Refunded when the lock ends. Regression test: `a_done_sellers_gnr_comes_back_when_the_buyer_never_claims`. |
| S-15 | Medium | A swap record the wallet could not read made it drop all swap records at the next save. | Records are read one by one and unreadable ones are kept. Regression test: `unreadable_swap_records_are_kept`. |
| S-16 | Medium | A deposit code that arrived before the lock was written down left the lock untracked. | The lock is taken on in any stage. |
| S-17 | Low | Timelock checks multiplied heights from the other side without overflow protection. | Saturating arithmetic. Property tests against exact arithmetic: `the_lock_check_holds_exactly`, `the_pay_check_holds_exactly`, `a_lock_length_that_would_wrap_is_refused`. |
| S-18 | Low | A Bitcoin server whose block heights ran ahead of its tip could make the confirmation count wrap. | Counted as none. |
| S-19 | Low | A code from a linked peer was accepted for any swap. | Only for its own. |

## Fixed in 1.0.54

Lower-severity findings of this review, fixed in 1.0.54:

| ID | Severity | What was wrong | Fix |
|---|---|---|---|
| S-20 | Medium | A buyer's bitcoin refund that waited in a busy mempool was sent again only when dropped, never with more fee. | Signed again with more fee as the blocks go by (twice from three blocks, up to four times). Regression test: `a_refund_waiting_in_a_busy_mempool_gets_more_fee`. |
| S-21 | Medium | Bitcoin paid into a contract besides the swap's own payment (a second payment, or one short of the amount) had no way back. | Sent back to the buyer once the bitcoin lock ends, also after the swap completed. Regression test: `extra_bitcoin_left_in_the_contract_comes_back`. |
| N-8 | Medium | The address directory was limited per client and per minute, but not for everyone together by the day. | 20,000 new entries a day in all. |
| W-14 | Low | Answers from the offer board and to the miner were read without a size limit. | 64 MiB and 4 MiB. |
| W-15 | Low | The command-line wallet printed memos, which the sender writes, with their control characters. | Removed before printing. |
| W-16 | Low | A few sums of amounts from the board could wrap around. | Saturating. |
| N-9 | Low | History pages at any depth were found by walking from the newest swap. | Deeper than 2,000,000 swaps, `since` is asked for. |

## What was checked and found correct

* **Consensus:** unchanged in 1.0.53. Decoders bound every length and reject trailing bytes; the
  checkpoint is enforced before proof of work; reorganisations commit block by block.
* **Light wallet:** FlyClient samples, MMR proofs and the multi-node genesis check; spend
  witnesses are checked against the verified note root; compact blocks are hash-linked to the
  proven tip and checked against their headers.
* **Keys and randomness:** note randomness and key encapsulation use the operating system's
  generator; keys are derived with SHAKE per network and account; AEAD nonces never repeat.
* **Swaps:** the bitcoin contract under Bitcoin Core's script interpreter for any keys, amounts and
  fee rates (property test `the_contract_behaves`); GNR claim and refund windows do not overlap;
  the buyer's checks before paying; secrets are found only after a SHA-256 match.
* **Wallet:** the local API's Host and token checks, escaping of everything the board, nodes and
  coins send, the read-only console, signed updates, and the password on every action that moves
  coins.
* **Node API:** the local-only checks, CORS, request limits, and no panics on peer or HTTP input.
* **Dependencies:** `cargo audit` against 1,568 advisories finds no vulnerability; three
  warnings concern unmaintained or unsound crates used only by the Linux window toolkit.

## Tests

New in 1.0.53: property tests for the swap timelock rules, the bitcoin contract, confirmation
tiers, swap codes, offer board records, sealed messages and the market price (`tests/props.rs`,
`tests/board_props.rs` in `krypton-swap`), and the regression tests named above. All tests of the
workspace pass; see the [1.0.53 report](../reports/1.0.53.md) for the numbers and benchmarks.

## Earlier reports

[1.0.52 audit](2026-11-26-full-audit.md) · [5 November audit](2026-11-05-security-audit.md) ·
[1.0.2 review](2026-11-security-review.md)
