"""Everything that changed in Genoryn Krypton, newest first: the source of genoryn.io/updates,
its Atom feed and the repository's CHANGELOG.md.

To record an update, add an entry at the top of UPDATES and run `python website/build.py`:

* a release: {"v": "1.0.41", "date": "2026-11-20", "title": ..., "tags": [...], "items": [...]}
* a change to the site or the repository only: the same without "v".

Write what changed for the person using it, in plain words, one fact per item. Dates are UTC.
An item may name its kind, ("Fixed", "text"), from CATEGORIES; CHANGELOG.md then groups the
items of that update under those headings. Entries written before categories stay as they were.
"""
import datetime
import html as _html

SITE = "https://genoryn.io"
RELEASES = "https://github.com/genoryn/genoryn/releases/tag/v"

UPDATES = [
    {"v": "1.0.48", "date": "2026-11-21", "title": "Quick Swap stays near the market price, and a burst of swaps cannot jump the price",
     "tags": ["Swap", "Wallet", "Site"],
     "items": [
         "No change to the rules of the chain: nodes and wallets on older versions keep working with this one. Coins, wallets and swaps in progress carry over.",
         ("Added", "Price guard: Quick Swap takes no offer more than 20 % from the market price (10 % while the price is forming or the market is thin), and waits while the price is catching up with fast swaps. Offers further away stay on the board, marked far from the market, and can still be taken by hand."),
         ("Changed", "The market price comes only from swaps whose bitcoin was found on the Bitcoin chain, of at least 0.001 BTC each, newer ones weighing more, and it moves at most 10 % an hour. Until there are three such swaps it is shown as forming."),
         ("Added", "Quick Swap offers follow the market price by default: at the market price or a margin above or below it, never past your limit, and taken off the board by themselves when the market reaches a trigger price you set."),
         ("Added", "The offer board shows how far each offer is from the market price, how much can be swapped within 2, 5, 10 and 20 % of it, and the bitcoin buyers have deposited."),
         ("Fixed", "\"Ready to buy\" counts only what Quick Swap can really swap near the market price, not offers at many times the price."),
         ("Fixed", "More of the wallet's messages are translated (Quick Swap, offers, passwords, updates)."),
     ]},
    {"v": "1.0.47", "date": "2026-11-20", "title": "The wallet and genoryn.io in ten languages, Hive OS for mining rigs",
     "tags": ["Wallet", "Mining", "Site"],
     "items": [
         "No change to the rules of the chain: nodes and wallets on older versions keep working with this one.",
         ("Added", "The wallet and genoryn.io in ten languages: English, Deutsch, Français, Español, Português, Русский, Türkçe, 简体中文, 日本語 and 한국어. Choose the wallet's language in the new Settings tab. Recovery-phrase words, addresses and swap codes are never translated. The translations are machine translations for now; corrections are welcome."),
         ("Added", "Hive OS: a custom miner package for GPU rigs that checks the miner's SHA-256 checksum before it starts, with a setup guide. Reports from real rigs are welcome."),
         ("Added", "Check your miner on genoryn.io/mining: enter your address to see whether its shares are reaching the network, an estimate of its hashrate, the rewards accruing and the next payout."),
         ("Added", "The wallet's Mining tab shows rejected shares, the last share, how long the miner has been running and its average hashrate over 24 hours."),
         ("Added", "The miner accepts the short address (gnrs1…) as its payout address and checks it against the full address the node returns; its JSON output includes its version, uptime and the time of the last share."),
         ("Changed", "Display and connection settings moved from the Network tab to the new Settings tab."),
         ("Fixed", "About one swap in 256 could not start (\"lock commitment mismatch\"); such swaps now start like any other. No coins were at risk: nothing was locked."),
         ("Fixed", "On phones, the Network tab no longer scrolls sideways."),
         ("Documentation", "Guides for miners: how native rewards work and are calculated, mining compared with pools, security and a FAQ. A contributing guide for translations and mining integrations, a security policy with response times, and a roadmap."),
     ]},
    {"v": "1.0.46", "date": "2026-11-20", "title": "Quick Swap: buy or sell any amount of GNR in one click",
     "tags": ["Swap", "Wallet", "Site"],
     "items": [
         "Quick Swap, at the top of the Swap tab: say how much you want to buy or sell, in dollars, bitcoin or GNR, and press one button. The wallet finds the best prices on the board and swaps exactly that amount across up to three offers whose makers are online, each part its own atomic swap. The seller's GNR is still locked on chain before any bitcoin moves.",
         "A part that cannot start (someone else took that offer a moment earlier, or its maker went offline) is replaced by another offer, never at a worse price than the one you accepted.",
         "Your Bitcoin balance for Quick Swap: an address of the wallet's own, from your 24 words. Buying pays from it at once, selling is paid into it, and you can send bitcoin out of it at any time.",
         "Quick Swap offers: tick Quick Swap when you publish, and anyone can buy (or sell you) any part of the offer, from its smallest part up to all of it, at your price. It is listed as up to three lots so several people can swap at once, and after a part the rest goes back on the board by itself.",
         "The board adds up what a buyer's deposit is promised to across all sellers, so bitcoin for one swap cannot have the GNR of many sellers locked at once.",
         "Buying at the market price is clearer: you set only the bitcoin you spend, and the wallet shows the GNR you get now and the least you get at your limit.",
         "genoryn.io/market shows Quick Swap live: what can be bought and sold right now, how many parts are swapping, and what an amount would get.",
         "No change to the rules of the chain: nodes and wallets on older versions keep working with this one, and see Quick Swap offers as ordinary offers.",
     ]},
    {"v": "1.0.45", "date": "2026-11-19", "title": "Old 24-word recovery phrases restore as people wrote them down",
     "tags": ["Wallet"],
     "items": [
         "A recovery phrase written down with the positions the wallets show next to each word (1. abandon, 2. ability …) restores as it is, one word per line or on one line, with or without commas. Before, the positions made the wallet expect a numbered Krypton phrase and refuse it.",
         "A phrase of 24 words without numbers, as Genoryn Mainnet 1.0 and other wallets made them, restores as before, and the wallet now says what that means: it finds what those words received on Genoryn Krypton. Mainnet 1.0 was a separate chain and its coins are not on Krypton.",
         "When a phrase mixes words with and without numbers, the wallet explains the two kinds of phrase instead of only asking for a number.",
         "No change to the rules of the chain or to swaps.",
     ]},
    {"v": "1.0.44", "date": "2026-11-19", "title": "A console in the wallet, offers that follow the market, a market price that rests on bitcoin really paid",
     "tags": ["Wallet", "Swap", "Site"],
     "items": [
         "The wallet has a Console: type a command and read the answer, as in Bitcoin's wallet. It completes commands as you type, keeps earlier lines, and `help` lists everything: blocks, transactions, peers, addresses, the supply, your balance. It only reads: no command can send GNR or show a recovery phrase or a key.",
         "History links straight to the explorer: View opens a payment's transaction on genoryn.io, Block the block that holds it. The explorer never shows an amount or who paid whom.",
         "An offer can follow the market price. A sell offer keeps its GNR and a buy offer its bitcoin; what is asked for it moves with the market, at most every ten minutes and by at most 5 % at a time. It never passes the limit you set: a sell offer never asks less, a buy offer never pays more.",
         "The market price now rests on bitcoin found on the Bitcoin chain. A swap's report names its bitcoin contract and is signed by one of the swap's two wallets; the board looks the payment up. With three such swaps the price is their volume-weighted median, and no single swap weighs more than a fifth. Reports nobody proved weigh nothing then.",
         "genoryn.io has a guide to running a full node on Linux, Windows and macOS, pages on validation, privacy, requirements, the user interface and network support, and pages for getting help, reporting bugs and contributing.",
         "The Linux service script works from the release packages (its line endings broke it before), finds the programs the package installed, and no longer switches a firewall on that was off.",
         "No change to the rules of the chain: nodes and wallets on older versions keep working with this one.",
     ]},
    {"v": "1.0.43", "date": "2026-11-19", "title": "Offers only for buyers whose bitcoin is ready, faster swaps, one wallet on two computers",
     "tags": ["Swap", "Wallet"],
     "items": [
         "A sell offer can be for buyers whose bitcoin is already deposited (the default). The seller's wallet checks on the Bitcoin chain that the bitcoin is waiting before it locks any GNR, so nobody can take the offer and leave the GNR locked unpaid.",
         "For other buyers a wallet locks GNR for at most two at a time who have not paid.",
         "Fewer confirmations before a buyer pays, by the size of the payment: 3 GNR blocks up to 0.001 BTC, 6 up to 0.01 BTC, 15 up to 0.1 BTC, 30 above. A small swap now starts in about three minutes instead of fifteen.",
         "Shorter locks when both wallets are on this version: GNR of a swap nobody pays comes back after about 21 hours instead of 32, and the bitcoin lock is about 3 hours instead of 6.",
         "One wallet on two computers: each shows the swaps the other runs, with their progress, and can carry a swap on if the other computer is switched off or lost.",
         "Updates install by themselves while no swap is running, unless you turn that off under About. Linux nodes update with one command.",
     ]},
    {"v": "1.0.42", "date": "2026-11-18", "title": "A sell offer commits its GNR for a week; nothing waits without saying why",
     "tags": ["Swap", "Wallet"],
     "items": [
         "Publishing a sell offer commits its GNR for 7 days. Within that time it can be sold or put into another sell offer at once, but not sent, also after you cancel or the offer runs its time. The wallet shows this before you publish.",
         "A seller whose buyer did not pay sees the block at which the GNR returns and about how many hours that is. The GNR is safe; nobody can open such a lock sooner, and the wallet takes it back by itself.",
         "Taking a buy offer that the other wallet does not answer is shown while it waits and ends by itself after ten minutes. Before, it stayed open without being shown.",
         "A swap that ran out ends even while the Bitcoin server cannot be reached, and a swap that waits for that server says so.",
         "A seller whose bitcoin claim lost to the buyer's refund takes the GNR back at its refund block.",
     ]},
    {"v": "1.0.41", "date": "2026-11-18", "title": "Buy offers with the bitcoin deposited: the swap completes by itself",
     "tags": ["Swap", "Security", "Wallet"],
     "items": [
         "A buy offer is now paid in advance, to an address of the buyer's own wallet. The wallet shows the address and the amount, and publishes the offer by itself once the bitcoin has one confirmation.",
         "The offer board checks on the Bitcoin chain that the bitcoin is there and marks the offer with a ✓. It keeps the address to itself.",
         "When a seller takes the offer, the buyer does nothing: once the seller's GNR is locked and confirmed, the buyer's wallet pays the swap contract from the deposit, exactly the agreed amount, and then claims the GNR.",
         "Buying from a sell offer works the same way: pay once, to an address of your wallet, and the swap starts and completes by itself. The seller's GNR is locked only once the buyer's bitcoin is there.",
         "If the offer is cancelled, nobody sells, or the offer you wanted is gone, the wallet sends the bitcoin back to your address. What was set aside for the network fee and not used comes back too.",
         "A wallet restored from its 24 words finds bitcoin left from buy offers again: Swap, Bitcoin connection.",
         "The same wallet open on two computers: the second one now shows GNR that the first has locked in a swap, with the block at which it returns, and takes it back itself if the first is gone when the lock ends. Offers of two copies no longer share a mailbox.",
         "A seller whose buyer cancelled or did not pay is told so, with the block at which the GNR returns.",
     ]},
    {"v": "1.0.40", "date": "2026-11-18", "title": "Sell offers prove their GNR",
     "tags": ["Swap", "Security", "Site"],
     "items": [
         "A sell offer is listed only after its GNR is set aside in a note of its own and the offer board has checked a zero-knowledge proof that it is there. Nobody can offer GNR they do not have.",
         "The board takes an offer down the moment the GNR behind it moves. The same GNR cannot back two offers, or an offer and a swap in progress.",
         "The wallet shows the rules before you publish. GNR of a cancelled sell offer is free again 24 hours later (or when the offer would have ended, if sooner) and can go into a new offer at once.",
         "A take locks exactly the GNR that was set aside, so a seller's lock no longer waits for notes to be merged.",
         "GNR set aside for an offer that ran its time, or whose swap ended, is free again by itself, also while the board cannot be reached.",
         "A swap accepted just before the wallet was closed carries on when the wallet starts again.",
         "The offer board was cleared when it switched to these rules: every offer on it now follows them.",
         "The market shows offers that were taken but not paid apart from swaps whose bitcoin has been paid; only the latter count as running.",
         "Miners: next to the addresses mining in the last 6 hours, the number active in 24 hours.",
         "This page: every update since the launch, with a feed.",
     ]},
    {"v": "1.0.39", "date": "2026-11-18", "title": "The balance in krypton, totals in money, a tidier history",
     "tags": ["Wallet", "Site"],
     "items": [
         "The balance is shown in GNR and in krypton, its smallest unit.",
         "Swap tab: the bitcoin price and the totals on the board in dollars (for sale, wanted, in progress, completed).",
         "A payment that needs several notes says so before you confirm and shows its progress; the history lists the merging as one line per step.",
         "Only one wallet runs per network, and switching wallets no longer flashes the password form.",
         "genoryn.io: a new home page with live charts and a preview of every page; price history on the market page.",
     ]},
    {"date": "2026-11-18", "title": "Questions and answers, quantum security explained, contributor guides",
     "tags": ["Site", "GitHub"],
     "items": [
         "New pages: questions and answers (/faq) and what quantum computers break and what they do not (/quantum).",
         "Shorter page titles and descriptions, structured data on every page, a sitemap with real modification dates.",
         "The repository has a contributing guide, a code of conduct, issue forms and a pull request template; its automatic checks pass on Linux, Windows and macOS.",
     ]},
    {"v": "1.0.38", "date": "2026-11-18", "title": "A new wallet design, a two-sided order book, the krypton",
     "tags": ["Wallet", "Swap", "Security"],
     "items": [
         "A modern wallet layout: navigation in a sidebar, a title on every page, a larger scale.",
         "Swap: sell offers and buy offers side by side; a completed swap is shown as a receipt.",
         "The smallest unit of GNR is named the krypton: 1 GNR = 100,000,000 krypton.",
         "Quantum tokens (planned, not live) replace what was announced as quantum coins.",
         "Security hardening from the miner to the network: the miner checks that its work pays its own address; JSON-RPC has the same limits as the REST API.",
     ]},
    {"v": "1.0.37", "date": "2026-11-14", "title": "Quantum coins move to a future release",
     "tags": ["Network"],
     "items": [
         "The activation of coins on mainnet was called off before it took effect: nothing was deployed and no coin exists.",
         "Nodes refuse coin transactions until a future release activates them.",
     ]},
    {"v": "1.0.36", "date": "2026-11-14", "title": "A rebuilt Coins tab and a cleaner design",
     "tags": ["Wallet"],
     "items": [
         "The Coins tab was rebuilt (overview, send, market, issue) in the wallet's monochrome design.",
         "Coins were postponed with 1.0.37, later the same day.",
     ]},
    {"v": "1.0.35", "date": "2026-11-14", "title": "Quantum coins announced",
     "tags": ["Network", "Wallet"],
     "items": [
         "Design and code for coins anyone can issue on Genoryn Krypton: private transfers, and every power of an issuer declared up front.",
         "Postponed with 1.0.37; planned now as quantum tokens.",
     ]},
    {"v": "1.0.34", "date": "2026-11-14", "title": "Offers for a week, a month or a year; a new market page; full documentation",
     "tags": ["Swap", "Site"],
     "items": [
         "An offer can stay on the board from one hour to one year; longer offers need a little more proof of work.",
         "While a maker's wallet is offline, its offers stay listed but cannot be taken, for up to a week.",
         "The market page has a cleaner summary and an order book with depth; the documentation doubled to 37 sections.",
     ]},
    {"v": "1.0.33", "date": "2026-11-14", "title": "Security fixes, a fair market price, free offer prices",
     "tags": ["Security", "Swap"],
     "items": [
         "Light wallets follow the chain with the most proof of work.",
         "Swap secrets are never reused, even in a wallet restored from its recovery phrase.",
         "The market price is the plain average of the swaps completed in the last 72 hours; any price may be offered.",
     ]},
    {"date": "2026-11-14", "title": "What is Genoryn Krypton, in one page",
     "tags": ["Site", "GitHub"],
     "items": [
         "A new /about page: definition, key facts, the relation to Bitcoin and a comparison with Bitcoin, Monero and Zcash.",
         "Plain-text summaries for language models (llms.txt), real 404 pages, and a README that is a full guide.",
     ]},
    {"v": "1.0.32", "date": "2026-11-12", "title": "Scarcity models in Mining",
     "tags": ["Wallet", "Mining"],
     "items": [
         "The Mining tab shows how scarce GNR becomes with each halving, and what-if calculators, labelled as calculations and not forecasts.",
     ]},
    {"v": "1.0.31", "date": "2026-11-12", "title": "What your computers would mine",
     "tags": ["Wallet", "Mining"],
     "items": [
         "Reads the network's hashrate live and shows what any number of GPUs would mine now, and by year as the reward halves.",
     ]},
    {"v": "1.0.30", "date": "2026-11-12", "title": "The mining calculator moves to Mining",
     "tags": ["Wallet", "Mining"],
     "items": [
         "What one computer mines, now and in any year, with the halvings taken from the block height.",
     ]},
    {"v": "1.0.29", "date": "2026-11-12", "title": "GPUs that will not mine say why",
     "tags": ["Mining"],
     "items": [
         "A GPU that cannot mine now says why, and the miner names the device it picked.",
         "`krypton-miner --list-devices` tests every GPU.",
     ]},
    {"v": "1.0.28", "date": "2026-11-07", "title": "Faster swaps, the payment pops up",
     "tags": ["Swap"],
     "items": [
         "Small swaps need fewer confirmations, and the buyer is shown where to pay as soon as the seller's GNR is locked.",
     ]},
    {"v": "1.0.27", "date": "2026-11-07", "title": "Keep the wallet open during a swap",
     "tags": ["Swap"],
     "items": [
         "The Swap tab explains why the wallet must stay open during a swap; closing the window keeps it working in the background.",
     ]},
    {"v": "1.0.26", "date": "2026-11-07", "title": "Only offers you can take",
     "tags": ["Swap"],
     "items": ["The offer board shows only offers whose maker is online, so every offer listed can be taken at once."]},
    {"v": "1.0.25", "date": "2026-11-07", "title": "The miner keeps running",
     "tags": ["Mining"],
     "items": ["A watchdog restarts the miner if it stops, and closing the window never stops mining."]},
    {"v": "1.0.24", "date": "2026-11-07", "title": "A tidy offer board",
     "tags": ["Swap"],
     "items": ["Choose how long an offer stays up; the board lists only live offers."]},
    {"v": "1.0.23", "date": "2026-11-07", "title": "The GNR is locked before anyone pays",
     "tags": ["Swap", "Security"],
     "items": [
         "Swaps run GNR first: the seller's GNR is locked on chain before the buyer pays, so nobody pays for coins that are not there.",
         "No swap lasts longer than a week, and the GNR price is the market price only.",
     ]},
    {"v": "1.0.22", "date": "2026-11-07", "title": "Offers cannot fake the price; prices in BTC",
     "tags": ["Swap"],
     "items": ["Joke offers no longer set the market price, and prices are shown in bitcoin."]},
    {"v": "1.0.21", "date": "2026-11-07", "title": "A real swap counts",
     "tags": ["Swap"],
     "items": ["The market price no longer drops a real swap because of a single offer."]},
    {"v": "1.0.20", "date": "2026-11-07", "title": "Instant swaps, a smarter price, a richer Mining tab",
     "tags": ["Swap", "Mining"],
     "items": [
         "Taking an offer starts the swap within seconds, with no approval: the maker's wallet answers by itself.",
         "GNR in open sell offers is reserved in the maker's wallet.",
     ]},
    {"v": "1.0.19", "date": "2026-11-06", "title": "A fairer market median", "tags": ["Swap"],
     "items": ["When volume is split evenly, the market price is the midpoint of the two middle prices instead of the lower one."]},
    {"v": "1.0.18", "date": "2026-11-06", "title": "Swaps in progress on the board", "tags": ["Swap"],
     "items": ["Taken offers stay on the board as in progress while their swaps run."]},
    {"v": "1.0.17", "date": "2026-11-06", "title": "The offer board keeps its offers", "tags": ["Swap"],
     "items": ["Offers and sealed messages on the board survive a node restart or update."]},
    {"v": "1.0.16", "date": "2026-11-06", "title": "A smoother wallet and a clearer market", "tags": ["Wallet", "Swap"],
     "items": [
         "Switching wallets updates everything at once; the total of all wallets is in the header.",
         "A redesigned Swap tab: a market header, an order book with depth bars, a trading panel that stays in view.",
     ]},
    {"v": "1.0.15", "date": "2026-11-05", "title": "Clearer buttons before swaps start", "tags": ["Swap"],
     "items": ["Buy and Sell on the offer board explain when swaps start instead of being disabled."]},
    {"v": "1.0.14", "date": "2026-11-05", "title": "The GNR market", "tags": ["Swap", "Site"],
     "items": [
         "A price set by people, and a live market page on genoryn.io.",
         "The wallet values GNR at the market price only.",
     ]},
    {"v": "1.0.13", "date": "2026-11-05", "title": "Atomic swaps go live at block 3,300", "tags": ["Swap", "Network"],
     "items": [
         "BTC and GNR are swapped between wallets from block 3,300 on.",
         "The offer board: anonymous offers, and the wallets pass the swap codes through sealed mailboxes by themselves.",
     ]},
    {"v": "1.0.12", "date": "2026-11-05", "title": "Wallet fixes", "tags": ["Wallet"],
     "items": ["Fixes to the wallet switcher and the balance card."]},
    {"v": "1.0.11", "date": "2026-11-05", "title": "BTC and GNR atomic swaps, built in", "tags": ["Swap"],
     "items": [
         "The Swap tab: two people exchange bitcoin and GNR with three codes; timeouts refund both sides.",
         "GNR lock notes in consensus, not active on mainnet yet.",
     ]},
    {"v": "1.0.10", "date": "2026-11-04", "title": "Get GNR; atomic swaps begin", "tags": ["Site", "Swap"],
     "items": ["A Get GNR page, and the Bitcoin side of atomic swaps."]},
    {"v": "1.0.9", "date": "2026-11-04", "title": "See payments before the next block", "tags": ["Wallet"],
     "items": ["Payment proofs work the moment you send, and incoming payments show up before they are in a block."]},
    {"v": "1.0.8", "date": "2026-11-04", "title": "Reliable payments between your wallets, in-app dialogs", "tags": ["Wallet"],
     "items": [
         "Sending to a short address that nobody had published now works between your own wallets.",
         "In-app dialogs replace the browser's boxes; the send confirmation shows the total.",
     ]},
    {"v": "1.0.7", "date": "2026-11-04", "title": "Security audit fixes and a fee market", "tags": ["Security", "Network"],
     "items": [
         "Fixes from the security audit in the desktop wallet, light wallets, the 24-hour lock, updates and the public node.",
         "Fees are a market: Economy, Normal, Fast or Custom, from the node's live estimate.",
     ]},
    {"v": "1.0.6", "date": "2026-11-04", "title": "One Wallet tab for all your wallets", "tags": ["Wallet"],
     "items": ["Every wallet on one tab, with a selector at the top; built for dozens or hundreds of wallets."]},
    {"v": "1.0.5", "date": "2026-11-04", "title": "Many wallets, paged history, a bigger Network tab", "tags": ["Wallet"],
     "items": ["A paged history, eight live network figures, new charts, the emission schedule and a full About page."]},
    {"v": "1.0.4", "date": "2026-11-04", "title": "Live overview; short addresses that resolve everywhere", "tags": ["Wallet"],
     "items": ["An overview on the Wallet tab; mining keeps running when you close the window."]},
    {"v": "1.0.3", "date": "2026-11-04", "title": "Several wallets, one total", "tags": ["Wallet"],
     "items": ["Several wallets, each with its own password, a 24-hour unlock, and one total."]},
    {"date": "2026-11-03", "title": "Explorer: addresses, mining activity, payment proofs", "tags": ["Site"],
     "items": ["The explorer shows address pages with their mining activity and verifies payment proofs."]},
    {"v": "1.0.2", "date": "2026-11-03", "title": "Security release", "tags": ["Security", "Network"],
     "items": [
         "Fixed: altered block bodies could split nodes off the network, and anchors could disappear after a reorganization.",
         "Network hardening against peers that send bad data.",
     ]},
    {"v": "1.0.1", "date": "2026-11-03", "title": "Network dashboard and automatic updates", "tags": ["Wallet", "Network"],
     "items": ["Updates verified against the release signature, a network dashboard, and the number of estimated miners."]},
    {"v": "1.0.0", "date": "2026-11-03", "title": "Mainnet", "tags": ["Network"],
     "items": [
         "Genoryn Krypton launches. The genesis block commits to Bitcoin block 974,438, so no GNR could be mined before that moment.",
         "No premine: 5 GNR per block, halving every 2,100,000 blocks, 21,000,000 GNR at most.",
     ]},
]


CATEGORIES = ["Added", "Changed", "Fixed", "Security", "Performance", "Documentation"]


def _text(i):
    return i[1] if isinstance(i, tuple) else i


def _date(s):
    return datetime.date.fromisoformat(s)


def _long(d):
    return f"{d.day} {d.strftime('%B %Y')}"


def _anchor(u):
    return "v" + u["v"].replace(".", "-") if "v" in u else "u-" + u["date"] + "-" + "".join(
        c if c.isalnum() else "-" for c in u["title"].lower())[:40].strip("-")


def _inline(s):
    """Escapes text; `code` becomes <code>."""
    out, parts = "", s.split("`")
    for i, p in enumerate(parts):
        out += f"<code>{_html.escape(p)}</code>" if i % 2 else _html.escape(p)
    return out


def latest():
    return next(u for u in UPDATES if "v" in u)


def has(version):
    return any(u.get("v") == version for u in UPDATES)


def page_html():
    """The timeline: one heading per day, one card per update."""
    out, day = [], None
    for u in UPDATES:
        d = _date(u["date"])
        if d != day:
            if day is not None:
                out.append("    </section>")
            day = d
            out.append(f'    <section class="up-day" id="{u["date"]}">\n      <h2><time datetime="{u["date"]}">{_long(d)}</time></h2>')
        tags = "".join(f"<i>{_html.escape(t)}</i>" for t in u.get("tags", []))
        badge = f'<span class="up-v">{u["v"]}</span>' if "v" in u else '<span class="up-v up-site">Site</span>'
        items = "".join(f"<li>{_inline(_text(i))}</li>" for i in u["items"])
        link = (f'<a class="up-link" href="{RELEASES}{u["v"]}" rel="noopener">Release notes and downloads'
                '<svg class="ic" aria-hidden="true"><use href="#u-arrow"/></svg></a>') if "v" in u else ""
        out.append(f'      <article class="glass up" id="{_anchor(u)}"><header>{badge}<h3>{_inline(u["title"])}</h3>'
                   f'<span class="up-tags">{tags}</span></header><ul>{items}</ul>{link}</article>')
    out.append("    </section>")
    return "\n".join(out)


def latest_html():
    """One line for the home page: the newest release."""
    u = latest()
    return (f'<a class="glass lp-update reveal" href="/updates#{_anchor(u)}"><span class="up-v">{u["v"]}</span>'
            f'<span class="lp-update-t"><em>Latest update · {_long(_date(u["date"]))}</em><b>{_inline(u["title"])}</b></span>'
            '<span class="lp-update-go">All updates<svg class="ic" aria-hidden="true"><use href="#i-arrow"/></svg></span></a>')


def markdown():
    """CHANGELOG.md for the repository."""
    out = ["# Changelog", "",
           "Everything that changed in Genoryn Krypton, newest first. The same list, with a feed, is at",
           f"[genoryn.io/updates]({SITE}/updates). This file is written by `website/build.py` from",
           "`website/updates.py`; edit that file, not this one.", ""]
    for u in UPDATES:
        head = f"[{u['v']}]({RELEASES}{u['v']})" if "v" in u else "Site"
        out.append(f"## {head} · {u['date']}: {u['title']}")
        out.append("")
        plain = [f"- {i}" for i in u["items"] if not isinstance(i, tuple)]
        if plain:
            out += plain + [""]
        for c in CATEGORIES:
            kind = [_text(i) for i in u["items"] if isinstance(i, tuple) and i[0] == c]
            if kind:
                out += [f"### {c}", ""] + [f"- {t}" for t in kind] + [""]
    return "\n".join(out)


def atom():
    """An Atom feed of the updates."""
    def entry(u):
        url = f"{SITE}/updates#{_anchor(u)}"
        title = (f"Genoryn Krypton {u['v']}: " if "v" in u else "") + u["title"]
        body = "<ul>" + "".join(f"<li>{_inline(_text(i))}</li>" for i in u["items"]) + "</ul>"
        return (f"<entry><title>{_html.escape(title)}</title><link href=\"{url}\"/><id>{url}</id>"
                f"<updated>{u['date']}T00:00:00Z</updated><content type=\"html\">{_html.escape(body)}</content></entry>")
    return ('<?xml version="1.0" encoding="utf-8"?>\n<feed xmlns="http://www.w3.org/2005/Atom">'
            f"<title>Genoryn Krypton updates</title><link href=\"{SITE}/updates\"/>"
            f"<link rel=\"self\" href=\"{SITE}/updates.xml\"/><id>{SITE}/updates</id>"
            f"<updated>{UPDATES[0]['date']}T00:00:00Z</updated><author><name>Zarazi Sokani</name></author>"
            + "".join(entry(u) for u in UPDATES) + "</feed>\n")


if __name__ == "__main__":
    print(markdown())
