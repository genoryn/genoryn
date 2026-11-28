# Changelog

Everything that changed in Genoryn Krypton, newest first. The same list, with a feed, is at
[genoryn.io/updates](https://genoryn.io/updates). This file is written by `website/build.py` from
`website/updates.py`; edit that file, not this one.

## [1.0.59](https://github.com/genoryn/genoryn/releases/tag/v1.0.59) · 2026-11-28: The wallet on several screens, and a miner that stops without freezing the display

- No change to the rules of the chain: nodes and wallets on older versions keep working with this one. Coins, wallets, offers and swaps in progress carry over.

### Added

- Any section of the wallet opens in a window of its own: Mining on one screen, Swap on another, the Wallet on a third. Right-click a section in the list and choose Open in a separate window, or press the small button at the foot of the list or the one beside the page's title; Ctrl+click and the middle mouse button do the same. Every window is the same live wallet: what you do in one shows in the others at once, and they share the theme, the language and the lock.
- A section's window shows that section alone, without the list; its menu button brings the list in when you want it. There is one window per section: asked for again, it comes to the front.
- The wallet remembers where its windows were and which sections were open, and opens them there again at the next start, after an update as well. A window whose screen is no longer attached opens on a screen that is.

### Changed

- Closing the main window leaves the sections' windows open. With no window left, the wallet keeps running only while it has work (the miner, an offer, a swap), as before. The desktop icon shows the main window of the wallet that is running.
- The update from 1.0.58 to this version still ends a running miner the old way, one last time: it is the old wallet that does it.
- The Stop button says that the miner is stopping until it has ended, and the miner's status then reads stopped, without a line of its progress after it.
- krypton-miner has a new option, --stdin-control: with it the miner ends in order when the line stop arrives on its standard input. The wallet starts its miner this way. Without the option nothing changes, so a miner on a rig or under Hive OS runs as before.

### Fixed

- Stopping the miner froze the display for a moment and could leave a blank, white window. The wallet ended the miner's process by force in the middle of its work on the graphics card, and the display driver recorded an error each time. The miner is now told to stop: it finishes the batch that is running (about a tenth of a second), releases the card and ends by itself. The same holds when the wallet quits, when Start is pressed again and when an update is installed. On the card we test with, an NVIDIA RTX 5090, the driver logged its error when the miner was ended by force and none when it was told to stop.
- A new window is dark from its first moment; it used to be white until its page had drawn.

## Site · 2026-11-27: How many addresses mine in each block, and room at the end of every page

### Added

- The Mining page shows, live from the blockchain, how many payout addresses have shares in each block: the average over the last hour's blocks, the latest block, a bar for every block, and how many different addresses mined in the last 60 and 1,440 blocks. An address is where rewards are paid, not a person or a machine.
- The explorer says the same for every block: the list of latest blocks has a column Addresses, and a block's page says how many addresses it credits.

### Changed

- The payment proof page says what becomes of a proof: it is not on the blockchain, it shows one payment to whoever has the text, and the page sends it to the node only to be checked. Its two fields have their labels above them.

### Fixed

- Every page ends with the same room above the footer. The explorer's pages of a proof, a block, a transaction and an address ended in a card that touched the footer, and several other pages left only a few pixels.
- On a touch screen the figure of a chart's point stays after a tap, until the next tap elsewhere; it used to vanish as the finger lifted.

## [1.0.58](https://github.com/genoryn/genoryn/releases/tag/v1.0.58) · 2026-11-27: Payments you can follow, addresses that are checked, and swaps you can prove on both chains

- No change to the rules of the chain: nodes and wallets on older versions keep working with this one. Coins, wallets, offers and swaps in progress carry over.

### Added

- A payment that merges coins first shows what it is doing. When your coins are in many small notes, the wallet merges them before it can pay. Instead of a line of text there is now a count of the transactions that are left, a bar, and the three steps: merge, next block, send. The payment follows by itself; keep the wallet open until it is sent.
- A payment is in the list at once. As soon as it is sent it stands there as on its way, and as confirmed once it is in a block. A window shows the amount and the recipient, with Copy payment proof and a link to the explorer, so the proof is at hand without a walk through History.
- The latest five transactions on the Wallet page, below Send and Receive and above the charts. A payment of yours has Copy proof, every other row its transaction ID or its block, and each opens in the explorer.
- Your Bitcoin addresses are in the Bitcoin card, on the Wallet tab and on the Swap tab: the address of the balance, and the deposit address of each buy offer that is open, each with Copy and a QR code.
- A completed swap can be checked on both chains. In the trade history, in the wallet and on genoryn.io/trades, a swap opens to its proof: the claim of the GNR and its block, the hash lock, the secret, the Bitcoin payment into the swap's contract and the Bitcoin transaction that took it with the secret, with links to public explorers. The page works out SHA-256 of the secret itself and shows that it is the hash lock. Nodes on 1.0.58 write the two Bitcoin transactions down for every swap whose bitcoin they find; older swaps show the GNR side, the secret and the mark that the bitcoin was found.

### Changed

- The balance says what is moving instead of one figure called pending: what is on its way out, the change that comes back with the next block, and what is coming in. A payment to you counts in the balance once it is in a block. If a payment of yours does not get into a block in time, the wallet says so and that the amount is back in your balance.
- Receive shows your full address first, with its QR code. A full address works always, straight from wallet to wallet. The short address is one press away and has its own QR code, with a note on what it depends on: a node has to know it. A small (i) in Receive and in Send explains the two.
- /v1/swap/history and the swaps of /v1/market have a new field btc for a verified swap: funding (transaction and output), funded_height and claim. Fields that were there stay as they are.
- Buy GNR comes first, in Publish an offer and in Quick Swap, and is what the form opens with. Max is on both sides: on the GNR you can sell and on the bitcoin you can spend, less the fees, in every unit. The market's filter takes a ceiling in bitcoin as well as in GNR.

### Fixed

- The command-line wallet accepts an address that was pasted with line breaks or in capitals, as the wallet window does, and reports a merge with its steps.
- With a Bitcoin Core node as the wallet's Bitcoin server, a look at the balance that met another look failed at once, because Bitcoin Core scans for one caller at a time. The wallet now waits a few seconds for its turn.
- On genoryn.io the headings of number columns are translated in every language, and on the market page the total of the swaps in progress has room on a phone.

### Security

- An address is checked where you use it. After Copy the wallet reads the clipboard back and says whether it holds exactly your address, so a program that swaps addresses in the clipboard is noticed. In Send, the recipient is checked as you type or paste: a valid full address, a short address the nodes know, an address of another network, or not an address at all.

## [1.0.57](https://github.com/genoryn/genoryn/releases/tag/v1.0.57) · 2026-11-27: A swap your wallet pays never asks you to pay, change is shown as change, and a Refresh button

- No change to the rules of the chain: nodes and wallets on older versions keep working with this one. Coins, wallets and swaps in progress carry over. The market's rules of 1.0.56 stay as they are.

### Added

- Refresh: a small button in the top bar loads everything again, now. You stay on the same tab and at the same place, and what you typed stays. F5 and Ctrl+R do the same. A page that is loaded anew, as after choosing another language, comes back on the tab it was on.

### Changed

- What change can do before its block: swaps that are already running are paid from it at once, and a buy offer can be published from it. A new purchase starts once it is in a block, usually within about ten minutes, because a seller's wallet counts only bitcoin it can see in a block. When a block is all that is missing, the wallet says so and no longer asks you to add bitcoin.

### Fixed

- A swap that your wallet pays from the Bitcoin balance could, for a moment, tell you to pay its contract yourself and show the contract's address. It happened when two purchases were paid one after the other: right after the first payment the Bitcoin server did not list the change yet, the wallet took the balance for empty, and the page offered a payment by hand until the wallet looked again and paid by itself. With a Bitcoin Core node as the wallet's Bitcoin server that lasted until the first payment's block. Anyone who had followed it would have paid a second time. No bitcoin was lost: a second payment into a contract comes back when the contract's lock ends. The wallet now knows the change of its own payments without asking a server, so the next payment follows at once.
- Change is shown as change. When your wallet pays with a coin, what is left of it comes back to the balance as a new coin, and that coin waits for a block like any other. The balance showed it as confirming, like a deposit someone had sent, with a note about its sender. It now says that this is your change, on its way into a block, and what your own payment pays.

### Security

- A swap the wallet pays never shows where to pay. Its card has no contract address, no payment details and no Pay now, in any state. If the balance cannot pay, the wallet says that it pays by itself as soon as the bitcoin is there, and that otherwise the swap ends, nothing is paid and the seller's GNR returns to them.

## [1.0.56](https://github.com/genoryn/genoryn/releases/tag/v1.0.56) · 2026-11-27: A strict market: every offer has its coins behind it, and nobody buys on a promise

- No change to the rules of the chain: nodes and wallets on older versions keep working with this one. Coins, wallets and swaps in progress carry over. What changes is what the offer board lists and what a wallet agrees to.

### Added

- A bitcoin deposit that waits says why. Under the Bitcoin balance the wallet shows how long the payment has been seen, the fee its sender paid, what blocks take now, and within about how many blocks that fee is enough. The bar is the time waited against that. When the fee is below what blocks take, the wallet says that it waits until fees fall, and the bar does not pretend to fill.
- Speed up: when a deposit's sender paid too little, the wallet can pay the difference. It sends the waiting bitcoin on to the balance's own address with a fee that makes miners take both payments together. The exact fee is shown first, the wallet's password is asked, the fee is never more than a twentieth of the deposit, and nothing leaves the wallet but the fee.
- Send: a Max button fills in the most one payment can send. Coins that sit in many small notes are merged first, and each merge pays the fee; the amount allows for that. GNR set aside for an open sell offer is not part of it.

### Changed

- Paying by hand is gone. The dialog that takes an offer no longer offers to pay to a new address or to pay by hand after the seller's GNR is locked, and a seller can no longer open an offer to any buyer. Someone could use those to have a seller's GNR locked for a day and never pay: it cost the seller time, never coins, and it cost the taker nothing.
- Buy offers that were on the board without bitcoin were removed when the node updated. Sell offers, which already had to prove their GNR, stayed, and so did swaps in progress. A swap that was running when you update finishes the way it started.
- Market: buy offers come first and sell offers second, in the wallet and on genoryn.io.
- Addresses are shown by their two ends, as in gnrs1k7q2m9x4…y0u1i2, in the list of wallets, the wallet switcher and the history.
- For node operators: a node lists buy offers only if it can read the Bitcoin chain (KRYPTON_BTC_API: an Esplora address, or rpc: and a Bitcoin Core address on the same computer). Without it the board lists sell offers only. Nothing else changes for a node.

### Fixed

- Market totals say what is there. For sale is now an amount of GNR, with its share of all GNR issued and its worth at the market price. It used to show, in dollars beside a GNR coin, the sum of what sellers ask, which read as if more GNR were offered than exists. All of it is GNR its sellers have proven. Wanted is the bitcoin on deposit. /v1/swap/offers returns the sums as for_sale_gnr, wanted_sats and issued.
- The wallet fits every window width. In a window narrower than a wide monitor the History table squeezed the recipient into a column one letter wide, and the total of all wallets was cut to three dots. The history now turns into a list when seven columns have no room, and bars, tables and figures give way before they are squeezed.
- The top bar measures itself: when it does not fit it drops the words on its buttons, then the hashrate, then the total of all wallets, instead of pushing the page sideways. This showed with two or more wallets on a 1366-pixel screen.
- The menu of a wallet in the list of wallets (Rename, Copy short address, Delete) opens above the list instead of being cut off by it.
- genoryn.io on phones: tables in the documentation and on the help pages turn into cards with each column named, figures are no longer cut to three dots, and a label the light theme made unreadable can be read again.

### Security

- A buy offer is listed only when its bitcoin is there. Publishing one pays its bitcoin from your Bitcoin balance to an address of your own wallet made for that offer; the board looks the address up on the Bitcoin chain and lists the offer once the amount is there, confirmed and unspent. A buy offer with nothing behind it is refused. Until now one could be published without any bitcoin.
- A buy offer whose bitcoin leaves its address is taken down. The board looks every deposit up again every few minutes; when the bitcoin is clearly gone, the offer comes off the board and its wallet shows it as ended and says why. A Bitcoin server that does not answer takes nothing down.
- Buying is paid from the wallet's Bitcoin balance, and from nowhere else. Add bitcoin to the balance once; every purchase then starts and completes by itself. When the balance is short the wallet starts nothing, says how much is missing and offers to add bitcoin. Bitcoin that a running purchase still has to pay is kept aside: it cannot buy a second time and cannot be sent out.
- A seller's wallet locks GNR only for a buyer whose bitcoin it has seen itself on the Bitcoin chain, in the buyer's own wallet, and only if the board does not count that bitcoin as promised to another seller. This holds for offers on the board and for swaps by codes.

## [1.0.55](https://github.com/genoryn/genoryn/releases/tag/v1.0.55) · 2026-11-26: A new design, a trade history that shows verified swaps first, and a new Genoryn Talks

- No change to the rules of the chain: nodes and wallets on older versions keep working with this one. Coins, wallets and swaps in progress carry over.

### Added

- Search with Ctrl+K. On genoryn.io it finds a page, a block, a transaction or an address. In the wallet it goes to a tab, locks the wallet or switches the theme; it never sends, swaps or signs.
- Genoryn Talks: a live chat like a messenger (rooms with what is new in each, the members' pictures, answers shown with what they answer, members named with @, messages over several lines, who is in the room), a picture for every member, notifications under a bell, and a channel that collects everything Zarazi Sokani writes.
- genoryn.io: a new home page, and a section about the author, Zarazi Sokani, with his portrait. The portrait is an illustration: a figure in a hood, the face in shadow.

### Changed

- A new design for the wallet, genoryn.io and Genoryn Talks: solid surfaces divided by thin lines, tighter corners, larger numbers. The colours are the same: black, white and grey.
- The wallet has a top bar with its statuses, its tabs down the left edge, and a new look for every tab, every dialog and the unlock screen. What the wallet does with your keys and coins did not change.
- Trade history charts keep a place for every day, also a day without a swap, and a few swaps are set out one after another, so one to five swaps read clearly. The price chart on the home page follows the same rule.
- Nodes keep, for every day, the figures of the verified swaps alone; /v1/swap/history/summary returns them as verified_figures next to the fields it had.

### Fixed

- Trade history: the figures and charts show verified swaps first, the ones whose bitcoin was found on the Bitcoin chain. A few early reports at odd prices no longer stretch the charts or move the low, the high and the median: they are counted apart, and one switch shows them.
- Genoryn Talks: the administrator's session lasts while it is in use, so an open chat no longer signs him out after eight hours.

## [1.0.54](https://github.com/genoryn/genoryn/releases/tag/v1.0.54) · 2026-11-26: Swap refunds that do not get stuck, and bitcoin paid twice comes back

- No change to the rules of the chain: nodes and wallets on older versions keep working with this one. Coins, wallets and swaps in progress carry over.

### Added

- Bitcoin paid into a swap contract a second time, or short of the amount, comes back to the buyer by itself once the contract's lock ends, also after the swap completed.

### Changed

- Swaps: a buyer's bitcoin refund that waits for a block is sent again with more fee as the blocks go by, so it is in a block in time.
- The documentation says what the board's sealed swap messages are: relayed from wallet to wallet by the nodes. No e-mail is involved.

### Security

- Answers from the offer board and to the miner are read only up to a limit, the node caps how many new addresses its directory takes a day, and the command-line wallet prints memos without control characters.

## [1.0.53](https://github.com/genoryn/genoryn/releases/tag/v1.0.53) · 2026-11-26: Light wallets that check every block, a sturdier market price, trade history and Tor

- No change to the rules of the chain: nodes and wallets on older versions keep working with this one. Coins, wallets and swaps in progress carry over.

### Added

- Trade history in the wallet: every completed swap of the board, newest first, with charts, your own trades and a CSV export. It loads in parts, so it stays quick with millions of swaps; genoryn.io/trades shows the whole history.
- A sturdier market price (method 2): the shortest window with enough verified swaps, swaps far from the rest left out, limits per swap and per six hours, and a confidence from 0 to 100 with a status. The market page shows the depth of the board, and anyone can recompute the price with verifymarketprice in the wallet's console.
- Confirmations before every payment and swap: a bitcoin payment is checked first and shows the fee, the total and what stays; taking an offer or a Quick Swap shows what goes and what comes, the price against the market, the fees, the confirmations the seller waits for and what happens if the swap does not complete.
- Connect through Tor: an optional SOCKS5 proxy for all of the wallet's own connections, in Settings, Connection mode.
- Research: what a transaction still shows on chain, measured on mainnet, and why Sietch-style dummy outputs are not needed here: every transaction already has two inputs and two outputs.

### Changed

- Swaps: the seller takes the bitcoin only with ten blocks of its lock left, and a claim waiting in a busy Bitcoin network is sent again every block with more fee. A seller whose buyer never claims the GNR gets it back when the lock ends.
- Nodes keep the whole trade history on disk and answer /v1/swap/history in pages.

### Security

- A new review of the whole system: the swaps with bitcoin, the node and its network, the desktop wallet, cryptography and the light client. Every finding it lists is fixed in this release; what was checked and what was fixed is at genoryn.io/audits.
- Light wallets check every block above the release checkpoint as a full node does: its proof of work, the difficulty the rule sets and its time. A node you add can no longer show your wallet blocks nobody worked for.
- Adding a wallet next to yours, switching between light and full mode and letting other nodes connect need the wallet's password.
- Nodes check their peers' transactions outside their message loop and share their attention fairly, so one peer can no longer slow a node down for the others.

## [1.0.52](https://github.com/genoryn/genoryn/releases/tag/v1.0.52) · 2026-11-26: A full security audit, and trading straight from your Bitcoin balance

- No change to the rules of the chain: nodes and wallets on older versions keep working with this one. Coins, wallets and swaps in progress carry over.

### Added

- Trade from your Bitcoin balance: take an offer and pay from the balance at once, publish a buy offer paid from it, and have what you sell paid into it. No address to type, no transfer each time.
- Swap, Market, Publish an offer and Your swaps are one Trade group, with your GNR and Bitcoin balances at the top of each page. Publishing an offer has a page of its own.
- Delete a wallet from this computer: with its password, never while a swap or an offer of it is open; keep an encrypted copy or erase it for good.
- genoryn.io: a Trade menu, the latest completed swaps on the home page, the security audits page, and a SHA-256 under every download button.

### Changed

- A new Wallet page with the Bitcoin balance beside your GNR, a QR code to receive and a clearer send form; a console that looks like a terminal; a bigger About page with Genoryn Talks and the security audits; a start animation and a lock animation.
- The number of miners is no longer shown anywhere: it cannot be measured honestly. The hashrate and the difficulty are.

### Fixed

- The totals on the History page no longer run into each other with long numbers.

### Security

- A full audit of the whole system: the wallet, the swaps with bitcoin, Bitcoin keys and signing, the node and updates, consensus, the website and the forum. Every finding it lists is fixed in this release; what was checked and what was fixed is at genoryn.io/audits.
- Swaps with bitcoin are safer at the step where a wallet decides that a Bitcoin transaction has done its job: a refund or a claim counts only once it is in a block, a dropped one is sent again, and a refund overtaken by the other side's claim still gets you your GNR.
- Changing the nodes or the Bitcoin connection needs the wallet's password, and the 24-hour lock waits while your coins are in a swap.

## [1.0.51](https://github.com/genoryn/genoryn/releases/tag/v1.0.51) · 2026-11-23: Swap and Market, each on a tab and a page of its own

- No change to the rules of the chain: nodes and wallets on older versions keep working with this one. Coins, wallets and swaps in progress carry over.

### Added

- genoryn.io/swap: Quick Swap on a page of its own, with the live price and what the board can swap right now. genoryn.io/market keeps the price, the order book, the price history and every completed swap.

### Changed

- The wallet has a Market tab of its own: the GNR price, the offer board and publishing an offer. The Swap tab keeps Quick Swap, your Bitcoin balance and your swaps.
- A cleaner design for both: in Quick Swap what you pay stands above what you get, with a button between them that switches between buying and selling; the order book shows the market price between its sell and buy offers; the board's totals stand in one row, and its long explanation folds away.

## [1.0.50](https://github.com/genoryn/genoryn/releases/tag/v1.0.50) · 2026-11-22: Swaps that ended elsewhere close, and each wallet shows its own Bitcoin balance and miner

- No change to the rules of the chain: nodes and wallets on older versions keep working with this one. Coins, wallets and swaps in progress carry over.

### Added

- The Mining tab says by name which wallet the miner pays, and offers to show it when you are looking at another wallet.

### Changed

- Your own Quick Swap offers are not counted as ready to swap in your own wallet (you cannot swap with yourself); the wallet says they are listed for others.

### Fixed

- A sale whose GNR came back through another copy of the wallet (a second computer, or the wallet restored from its phrase) no longer waits forever under Your swaps: it ends as refunded. The GNR was never lost; it was back in the wallet.
- Switching wallets shows the new wallet's Bitcoin balance and Quick Swap at once, not the last wallet's.

## [1.0.49](https://github.com/genoryn/genoryn/releases/tag/v1.0.49) · 2026-11-22: The offer board's sell and buy tables no longer overlap

- No change to the rules of the chain: nodes and wallets on older versions keep working with this one. Coins, wallets and swaps in progress carry over.

### Fixed

- In the Swap tab, the sell and buy offers no longer run into each other: they stand side by side only where both fit, otherwise one under the other, and amounts of GNR are written shorter.

## [1.0.48](https://github.com/genoryn/genoryn/releases/tag/v1.0.48) · 2026-11-21: Quick Swap stays near the market price, and a burst of swaps cannot jump the price

- No change to the rules of the chain: nodes and wallets on older versions keep working with this one. Coins, wallets and swaps in progress carry over.

### Added

- Price guard: Quick Swap takes no offer more than 20 % from the market price (10 % while the price is forming or the market is thin), and waits while the price is catching up with fast swaps. Offers further away stay on the board, marked far from the market, and can still be taken by hand.
- Quick Swap offers follow the market price by default: at the market price or a margin above or below it, never past your limit, and taken off the board by themselves when the market reaches a trigger price you set.
- The offer board shows how far each offer is from the market price, how much can be swapped within 2, 5, 10 and 20 % of it, and the bitcoin buyers have deposited.

### Changed

- The market price comes only from swaps whose bitcoin was found on the Bitcoin chain, of at least 0.001 BTC each, newer ones weighing more, and it moves at most 10 % an hour. Until there are three such swaps it is shown as forming.

### Fixed

- "Ready to buy" counts only what Quick Swap can really swap near the market price, not offers at many times the price.
- More of the wallet's messages are translated (Quick Swap, offers, passwords, updates).

## [1.0.47](https://github.com/genoryn/genoryn/releases/tag/v1.0.47) · 2026-11-20: The wallet and genoryn.io in ten languages, Hive OS for mining rigs

- No change to the rules of the chain: nodes and wallets on older versions keep working with this one.

### Added

- The wallet and genoryn.io in ten languages: English, Deutsch, Français, Español, Português, Русский, Türkçe, 简体中文, 日本語 and 한국어. Choose the wallet's language in the new Settings tab. Recovery-phrase words, addresses and swap codes are never translated. The translations are machine translations for now; corrections are welcome.
- Hive OS: a custom miner package for GPU rigs that checks the miner's SHA-256 checksum before it starts, with a setup guide. Reports from real rigs are welcome.
- Check your miner on genoryn.io/mining: enter your address to see whether its shares are reaching the network, an estimate of its hashrate, the rewards accruing and the next payout.
- The wallet's Mining tab shows rejected shares, the last share, how long the miner has been running and its average hashrate over 24 hours.
- The miner accepts the short address (gnrs1…) as its payout address and checks it against the full address the node returns; its JSON output includes its version, uptime and the time of the last share.

### Changed

- Display and connection settings moved from the Network tab to the new Settings tab.

### Fixed

- About one swap in 256 could not start ("lock commitment mismatch"); such swaps now start like any other. No coins were at risk: nothing was locked.
- On phones, the Network tab no longer scrolls sideways.

### Documentation

- Guides for miners: how native rewards work and are calculated, mining compared with pools, security and a FAQ. A contributing guide for translations and mining integrations, a security policy with response times, and a roadmap.

## [1.0.46](https://github.com/genoryn/genoryn/releases/tag/v1.0.46) · 2026-11-20: Quick Swap: buy or sell any amount of GNR in one click

- Quick Swap, at the top of the Swap tab: say how much you want to buy or sell, in dollars, bitcoin or GNR, and press one button. The wallet finds the best prices on the board and swaps exactly that amount across up to three offers whose makers are online, each part its own atomic swap. The seller's GNR is still locked on chain before any bitcoin moves.
- A part that cannot start (someone else took that offer a moment earlier, or its maker went offline) is replaced by another offer, never at a worse price than the one you accepted.
- Your Bitcoin balance for Quick Swap: an address of the wallet's own, from your 24 words. Buying pays from it at once, selling is paid into it, and you can send bitcoin out of it at any time.
- Quick Swap offers: tick Quick Swap when you publish, and anyone can buy (or sell you) any part of the offer, from its smallest part up to all of it, at your price. It is listed as up to three lots so several people can swap at once, and after a part the rest goes back on the board by itself.
- The board adds up what a buyer's deposit is promised to across all sellers, so bitcoin for one swap cannot have the GNR of many sellers locked at once.
- Buying at the market price is clearer: you set only the bitcoin you spend, and the wallet shows the GNR you get now and the least you get at your limit.
- genoryn.io/market shows Quick Swap live: what can be bought and sold right now, how many parts are swapping, and what an amount would get.
- No change to the rules of the chain: nodes and wallets on older versions keep working with this one, and see Quick Swap offers as ordinary offers.

## [1.0.45](https://github.com/genoryn/genoryn/releases/tag/v1.0.45) · 2026-11-19: Old 24-word recovery phrases restore as people wrote them down

- A recovery phrase written down with the positions the wallets show next to each word (1. abandon, 2. ability …) restores as it is, one word per line or on one line, with or without commas. Before, the positions made the wallet expect a numbered Krypton phrase and refuse it.
- A phrase of 24 words without numbers, as Genoryn Mainnet 1.0 and other wallets made them, restores as before, and the wallet now says what that means: it finds what those words received on Genoryn Krypton. Mainnet 1.0 was a separate chain and its coins are not on Krypton.
- When a phrase mixes words with and without numbers, the wallet explains the two kinds of phrase instead of only asking for a number.
- No change to the rules of the chain or to swaps.

## [1.0.44](https://github.com/genoryn/genoryn/releases/tag/v1.0.44) · 2026-11-19: A console in the wallet, offers that follow the market, a market price that rests on bitcoin really paid

- The wallet has a Console: type a command and read the answer, as in Bitcoin's wallet. It completes commands as you type, keeps earlier lines, and `help` lists everything: blocks, transactions, peers, addresses, the supply, your balance. It only reads: no command can send GNR or show a recovery phrase or a key.
- History links straight to the explorer: View opens a payment's transaction on genoryn.io, Block the block that holds it. The explorer never shows an amount or who paid whom.
- An offer can follow the market price. A sell offer keeps its GNR and a buy offer its bitcoin; what is asked for it moves with the market, at most every ten minutes and by at most 5 % at a time. It never passes the limit you set: a sell offer never asks less, a buy offer never pays more.
- The market price now rests on bitcoin found on the Bitcoin chain. A swap's report names its bitcoin contract and is signed by one of the swap's two wallets; the board looks the payment up. With three such swaps the price is their volume-weighted median, and no single swap weighs more than a fifth. Reports nobody proved weigh nothing then.
- genoryn.io has a guide to running a full node on Linux, Windows and macOS, pages on validation, privacy, requirements, the user interface and network support, and pages for getting help, reporting bugs and contributing.
- The Linux service script works from the release packages (its line endings broke it before), finds the programs the package installed, and no longer switches a firewall on that was off.
- No change to the rules of the chain: nodes and wallets on older versions keep working with this one.

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
