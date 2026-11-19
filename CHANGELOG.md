# Changelog

Everything that changed in Genoryn Krypton, newest first. The same list, with a feed, is at
[genoryn.io/updates](https://genoryn.io/updates). This file is written by `website/build.py` from
`website/updates.py`; edit that file, not this one.

## [1.0.43](https://github.com/genoryn/genoryn/releases/tag/v1.0.43) · 2026-11-19: Offers only for buyers whose bitcoin is ready, faster swaps, one wallet on two computers

- A sell offer can be for buyers whose bitcoin is already deposited (the default). The seller's wallet checks on the Bitcoin chain that the bitcoin is waiting before it locks any GNR, so nobody can take the offer and leave the GNR locked unpaid.
- For other buyers a wallet locks GNR for at most two at a time who have not paid.
- Fewer confirmations before a buyer pays, by the size of the payment: 3 GNR blocks up to 0.001 BTC, 6 up to 0.01 BTC, 15 up to 0.1 BTC, 30 above. A small swap now starts in about three minutes instead of fifteen.
- Shorter locks when both wallets are on this version: GNR of a swap nobody pays comes back after about 21 hours instead of 32, and the bitcoin lock is about 3 hours instead of 6.
- One wallet on two computers: each shows the swaps the other runs, with their progress, and can carry a swap on if the other computer is switched off or lost.
- Updates install by themselves while no swap is running, unless you turn that off under About. Linux nodes update with one command.

## [1.0.42](https://github.com/genoryn/genoryn/releases/tag/v1.0.42) · 2026-11-18: A sell offer commits its GNR for a week; nothing waits without saying why

- Publishing a sell offer commits its GNR for 7 days. Within that time it can be sold or put into another sell offer at once, but not sent, also after you cancel or the offer runs its time. The wallet shows this before you publish.
- A seller whose buyer did not pay sees the block at which the GNR returns and about how many hours that is. The GNR is safe; nobody can open such a lock sooner, and the wallet takes it back by itself.
- Taking a buy offer that the other wallet does not answer is shown while it waits and ends by itself after ten minutes. Before, it stayed open without being shown.
- A swap that ran out ends even while the Bitcoin server cannot be reached, and a swap that waits for that server says so.
- A seller whose bitcoin claim lost to the buyer's refund takes the GNR back at its refund block.

## [1.0.41](https://github.com/genoryn/genoryn/releases/tag/v1.0.41) · 2026-11-18: Buy offers with the bitcoin deposited: the swap completes by itself

- A buy offer is now paid in advance, to an address of the buyer's own wallet. The wallet shows the address and the amount, and publishes the offer by itself once the bitcoin has one confirmation.
- The offer board checks on the Bitcoin chain that the bitcoin is there and marks the offer with a ✓. It keeps the address to itself.
- When a seller takes the offer, the buyer does nothing: once the seller's GNR is locked and confirmed, the buyer's wallet pays the swap contract from the deposit, exactly the agreed amount, and then claims the GNR.
- Buying from a sell offer works the same way: pay once, to an address of your wallet, and the swap starts and completes by itself. The seller's GNR is locked only once the buyer's bitcoin is there.
- If the offer is cancelled, nobody sells, or the offer you wanted is gone, the wallet sends the bitcoin back to your address. What was set aside for the network fee and not used comes back too.
- A wallet restored from its 24 words finds bitcoin left from buy offers again: Swap, Bitcoin connection.
- The same wallet open on two computers: the second one now shows GNR that the first has locked in a swap, with the block at which it returns, and takes it back itself if the first is gone when the lock ends. Offers of two copies no longer share a mailbox.
- A seller whose buyer cancelled or did not pay is told so, with the block at which the GNR returns.

## [1.0.40](https://github.com/genoryn/genoryn/releases/tag/v1.0.40) · 2026-11-18: Sell offers prove their GNR

- A sell offer is listed only after its GNR is set aside in a note of its own and the offer board has checked a zero-knowledge proof that it is there. Nobody can offer GNR they do not have.
- The board takes an offer down the moment the GNR behind it moves. The same GNR cannot back two offers, or an offer and a swap in progress.
- The wallet shows the rules before you publish. GNR of a cancelled sell offer is free again 24 hours later (or when the offer would have ended, if sooner) and can go into a new offer at once.
- A take locks exactly the GNR that was set aside, so a seller's lock no longer waits for notes to be merged.
- GNR set aside for an offer that ran its time, or whose swap ended, is free again by itself, also while the board cannot be reached.
- A swap accepted just before the wallet was closed carries on when the wallet starts again.
- The offer board was cleared when it switched to these rules: every offer on it now follows them.
- The market shows offers that were taken but not paid apart from swaps whose bitcoin has been paid; only the latter count as running.
- Miners: next to the addresses mining in the last 6 hours, the number active in 24 hours.
- This page: every update since the launch, with a feed.

## [1.0.39](https://github.com/genoryn/genoryn/releases/tag/v1.0.39) · 2026-11-18: The balance in krypton, totals in money, a tidier history

- The balance is shown in GNR and in krypton, its smallest unit.
- Swap tab: the bitcoin price and the totals on the board in dollars (for sale, wanted, in progress, completed).
- A payment that needs several notes says so before you confirm and shows its progress; the history lists the merging as one line per step.
- Only one wallet runs per network, and switching wallets no longer flashes the password form.
- genoryn.io: a new home page with live charts and a preview of every page; price history on the market page.

## Site · 2026-11-18: Questions and answers, quantum security explained, contributor guides

- New pages: questions and answers (/faq) and what quantum computers break and what they do not (/quantum).
- Shorter page titles and descriptions, structured data on every page, a sitemap with real modification dates.
- The repository has a contributing guide, a code of conduct, issue forms and a pull request template; its automatic checks pass on Linux, Windows and macOS.

## [1.0.38](https://github.com/genoryn/genoryn/releases/tag/v1.0.38) · 2026-11-18: A new wallet design, a two-sided order book, the krypton

- A modern wallet layout: navigation in a sidebar, a title on every page, a larger scale.
- Swap: sell offers and buy offers side by side; a completed swap is shown as a receipt.
- The smallest unit of GNR is named the krypton: 1 GNR = 100,000,000 krypton.
- Quantum tokens (planned, not live) replace what was announced as quantum coins.
- Security hardening from the miner to the network: the miner checks that its work pays its own address; JSON-RPC has the same limits as the REST API.

## [1.0.37](https://github.com/genoryn/genoryn/releases/tag/v1.0.37) · 2026-11-14: Quantum coins move to a future release

- The activation of coins on mainnet was called off before it took effect: nothing was deployed and no coin exists.
- Nodes refuse coin transactions until a future release activates them.

## [1.0.36](https://github.com/genoryn/genoryn/releases/tag/v1.0.36) · 2026-11-14: A rebuilt Coins tab and a cleaner design

- The Coins tab was rebuilt (overview, send, market, issue) in the wallet's monochrome design.
- Coins were postponed with 1.0.37, later the same day.

## [1.0.35](https://github.com/genoryn/genoryn/releases/tag/v1.0.35) · 2026-11-14: Quantum coins announced

- Design and code for coins anyone can issue on Genoryn Krypton: private transfers, and every power of an issuer declared up front.
- Postponed with 1.0.37; planned now as quantum tokens.

## [1.0.34](https://github.com/genoryn/genoryn/releases/tag/v1.0.34) · 2026-11-14: Offers for a week, a month or a year; a new market page; full documentation

- An offer can stay on the board from one hour to one year; longer offers need a little more proof of work.
- While a maker's wallet is offline, its offers stay listed but cannot be taken, for up to a week.
- The market page has a cleaner summary and an order book with depth; the documentation doubled to 37 sections.

## [1.0.33](https://github.com/genoryn/genoryn/releases/tag/v1.0.33) · 2026-11-14: Security fixes, a fair market price, free offer prices

- Light wallets follow the chain with the most proof of work.
- Swap secrets are never reused, even in a wallet restored from its recovery phrase.
- The market price is the plain average of the swaps completed in the last 72 hours; any price may be offered.

## Site · 2026-11-14: What is Genoryn Krypton, in one page

- A new /about page: definition, key facts, the relation to Bitcoin and a comparison with Bitcoin, Monero and Zcash.
- Plain-text summaries for language models (llms.txt), real 404 pages, and a README that is a full guide.

## [1.0.32](https://github.com/genoryn/genoryn/releases/tag/v1.0.32) · 2026-11-12: Scarcity models in Mining

- The Mining tab shows how scarce GNR becomes with each halving, and what-if calculators, labelled as calculations and not forecasts.

## [1.0.31](https://github.com/genoryn/genoryn/releases/tag/v1.0.31) · 2026-11-12: What your computers would mine

- Reads the network's hashrate live and shows what any number of GPUs would mine now, and by year as the reward halves.

## [1.0.30](https://github.com/genoryn/genoryn/releases/tag/v1.0.30) · 2026-11-12: The mining calculator moves to Mining

- What one computer mines, now and in any year, with the halvings taken from the block height.

## [1.0.29](https://github.com/genoryn/genoryn/releases/tag/v1.0.29) · 2026-11-12: GPUs that will not mine say why

- A GPU that cannot mine now says why, and the miner names the device it picked.
- `krypton-miner --list-devices` tests every GPU.

## [1.0.28](https://github.com/genoryn/genoryn/releases/tag/v1.0.28) · 2026-11-07: Faster swaps, the payment pops up

- Small swaps need fewer confirmations, and the buyer is shown where to pay as soon as the seller's GNR is locked.

## [1.0.27](https://github.com/genoryn/genoryn/releases/tag/v1.0.27) · 2026-11-07: Keep the wallet open during a swap

- The Swap tab explains why the wallet must stay open during a swap; closing the window keeps it working in the background.

## [1.0.26](https://github.com/genoryn/genoryn/releases/tag/v1.0.26) · 2026-11-07: Only offers you can take

- The offer board shows only offers whose maker is online, so every offer listed can be taken at once.

## [1.0.25](https://github.com/genoryn/genoryn/releases/tag/v1.0.25) · 2026-11-07: The miner keeps running

- A watchdog restarts the miner if it stops, and closing the window never stops mining.

## [1.0.24](https://github.com/genoryn/genoryn/releases/tag/v1.0.24) · 2026-11-07: A tidy offer board

- Choose how long an offer stays up; the board lists only live offers.

## [1.0.23](https://github.com/genoryn/genoryn/releases/tag/v1.0.23) · 2026-11-07: The GNR is locked before anyone pays

- Swaps run GNR first: the seller's GNR is locked on chain before the buyer pays, so nobody pays for coins that are not there.
- No swap lasts longer than a week, and the GNR price is the market price only.

## [1.0.22](https://github.com/genoryn/genoryn/releases/tag/v1.0.22) · 2026-11-07: Offers cannot fake the price; prices in BTC

- Joke offers no longer set the market price, and prices are shown in bitcoin.

## [1.0.21](https://github.com/genoryn/genoryn/releases/tag/v1.0.21) · 2026-11-07: A real swap counts

- The market price no longer drops a real swap because of a single offer.

## [1.0.20](https://github.com/genoryn/genoryn/releases/tag/v1.0.20) · 2026-11-07: Instant swaps, a smarter price, a richer Mining tab

- Taking an offer starts the swap within seconds, with no approval: the maker's wallet answers by itself.
- GNR in open sell offers is reserved in the maker's wallet.

## [1.0.19](https://github.com/genoryn/genoryn/releases/tag/v1.0.19) · 2026-11-06: A fairer market median

- When volume is split evenly, the market price is the midpoint of the two middle prices instead of the lower one.

## [1.0.18](https://github.com/genoryn/genoryn/releases/tag/v1.0.18) · 2026-11-06: Swaps in progress on the board

- Taken offers stay on the board as in progress while their swaps run.

## [1.0.17](https://github.com/genoryn/genoryn/releases/tag/v1.0.17) · 2026-11-06: The offer board keeps its offers

- Offers and sealed messages on the board survive a node restart or update.

## [1.0.16](https://github.com/genoryn/genoryn/releases/tag/v1.0.16) · 2026-11-06: A smoother wallet and a clearer market

- Switching wallets updates everything at once; the total of all wallets is in the header.
- A redesigned Swap tab: a market header, an order book with depth bars, a trading panel that stays in view.

## [1.0.15](https://github.com/genoryn/genoryn/releases/tag/v1.0.15) · 2026-11-05: Clearer buttons before swaps start

- Buy and Sell on the offer board explain when swaps start instead of being disabled.

## [1.0.14](https://github.com/genoryn/genoryn/releases/tag/v1.0.14) · 2026-11-05: The GNR market

- A price set by people, and a live market page on genoryn.io.
- The wallet values GNR at the market price only.

## [1.0.13](https://github.com/genoryn/genoryn/releases/tag/v1.0.13) · 2026-11-05: Atomic swaps go live at block 3,300

- BTC and GNR are swapped between wallets from block 3,300 on.
- The offer board: anonymous offers, and the wallets pass the swap codes through sealed mailboxes by themselves.

## [1.0.12](https://github.com/genoryn/genoryn/releases/tag/v1.0.12) · 2026-11-05: Wallet fixes

- Fixes to the wallet switcher and the balance card.

## [1.0.11](https://github.com/genoryn/genoryn/releases/tag/v1.0.11) · 2026-11-05: BTC and GNR atomic swaps, built in

- The Swap tab: two people exchange bitcoin and GNR with three codes; timeouts refund both sides.
- GNR lock notes in consensus, not active on mainnet yet.

## [1.0.10](https://github.com/genoryn/genoryn/releases/tag/v1.0.10) · 2026-11-04: Get GNR; atomic swaps begin

- A Get GNR page, and the Bitcoin side of atomic swaps.

## [1.0.9](https://github.com/genoryn/genoryn/releases/tag/v1.0.9) · 2026-11-04: See payments before the next block

- Payment proofs work the moment you send, and incoming payments show up before they are in a block.

## [1.0.8](https://github.com/genoryn/genoryn/releases/tag/v1.0.8) · 2026-11-04: Reliable payments between your wallets, in-app dialogs

- Sending to a short address that nobody had published now works between your own wallets.
- In-app dialogs replace the browser's boxes; the send confirmation shows the total.

## [1.0.7](https://github.com/genoryn/genoryn/releases/tag/v1.0.7) · 2026-11-04: Security audit fixes and a fee market

- Fixes from the security audit in the desktop wallet, light wallets, the 24-hour lock, updates and the public node.
- Fees are a market: Economy, Normal, Fast or Custom, from the node's live estimate.

## [1.0.6](https://github.com/genoryn/genoryn/releases/tag/v1.0.6) · 2026-11-04: One Wallet tab for all your wallets

- Every wallet on one tab, with a selector at the top; built for dozens or hundreds of wallets.

## [1.0.5](https://github.com/genoryn/genoryn/releases/tag/v1.0.5) · 2026-11-04: Many wallets, paged history, a bigger Network tab

- A paged history, eight live network figures, new charts, the emission schedule and a full About page.

## [1.0.4](https://github.com/genoryn/genoryn/releases/tag/v1.0.4) · 2026-11-04: Live overview; short addresses that resolve everywhere

- An overview on the Wallet tab; mining keeps running when you close the window.

## [1.0.3](https://github.com/genoryn/genoryn/releases/tag/v1.0.3) · 2026-11-04: Several wallets, one total

- Several wallets, each with its own password, a 24-hour unlock, and one total.

## Site · 2026-11-03: Explorer: addresses, mining activity, payment proofs

- The explorer shows address pages with their mining activity and verifies payment proofs.

## [1.0.2](https://github.com/genoryn/genoryn/releases/tag/v1.0.2) · 2026-11-03: Security release

- Fixed: altered block bodies could split nodes off the network, and anchors could disappear after a reorganization.
- Network hardening against peers that send bad data.

## [1.0.1](https://github.com/genoryn/genoryn/releases/tag/v1.0.1) · 2026-11-03: Network dashboard and automatic updates

- Updates verified against the release signature, a network dashboard, and the number of estimated miners.

## [1.0.0](https://github.com/genoryn/genoryn/releases/tag/v1.0.0) · 2026-11-03: Mainnet

- Genoryn Krypton launches. The genesis block commits to Bitcoin block 974,438, so no GNR could be mined before that moment.
- No premine: 5 GNR per block, halving every 2,100,000 blocks, 21,000,000 GNR at most.
