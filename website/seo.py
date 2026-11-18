"""SEO metadata for genoryn.io: canonical URLs, Open Graph, Twitter cards, JSON-LD,
sitemap and web manifest. Used by build.py; all claims mirror visible page content."""
import datetime
import json
import pathlib
import re

SITE = "https://genoryn.io"
NAME = "Genoryn Krypton"
VERSION = "1.0.40"
OG = SITE + "/assets/og.png"
TODAY = datetime.date.today().isoformat()
KEYWORDS = ("Genoryn Krypton, GNR, quantum-resistant cryptocurrency, post-quantum cryptocurrency, quantum-safe Bitcoin alternative, "
            "private cryptocurrency, anonymous cryptocurrency, privacy coin, zk-STARK, ML-KEM, post-quantum privacy coin, "
            "GPU mining, CPU mining, new cryptocurrency to mine, fair launch, no premine, 21 million supply, "
            "atomic swap bitcoin, light wallet, KryptoHash, "
            "quantum tokens, private tokens, private stablecoins, deploy a token")
INDEXNOW_KEY = "4cd0c3283a0fa22bc5f39f8d68fb4209"

PAGES = {
    "home": {"path": "/", "file": "index.html", "priority": "1.0", "freq": "daily",
             "og_title": "Genoryn Krypton (GNR): quantum-resistant, private money you can mine today",
             "og_desc": "Fair launch, no premine, 21,000,000 GNR. zk-STARK shielded transfers, ML-KEM encryption, GPU and CPU mining, light wallet that verifies everything."},
    "explorer": {"path": "/explorer", "file": "explorer.html", "priority": "0.8", "freq": "always",
                 "og_title": "Genoryn Krypton explorer: live blocks and network stats",
                 "og_desc": "Live Genoryn Krypton blocks, transactions, supply and hashrate. Shielded transfers reveal no amounts or addresses."},
    "mining": {"path": "/mining", "file": "mining.html", "priority": "0.9", "freq": "weekly",
               "og_title": "Mine Genoryn Krypton (GNR) on GPU or CPU",
               "og_desc": "5 GNR per block, fair launch, no premine. KryptoHash is GPU-first and CPU-capable: start mining from the wallet in one click."},
    "get": {"path": "/get", "file": "get.html", "priority": "0.9", "freq": "daily",
            "og_title": "Get Genoryn Krypton (GNR): mine it today",
            "og_desc": "No premine, no presale: mine GNR on a GPU or CPU, or swap bitcoin for it on the offer board at the live market price."},
    "market": {"path": "/market", "file": "market.html", "priority": "0.9", "freq": "always",
               "og_title": "GNR market: live offers and price of Genoryn Krypton",
               "og_desc": "Live offer board and market price of GNR from atomic swaps with bitcoin. No exchange, no custody: people set the price."},
    "download": {"path": "/download", "file": "download.html", "priority": "0.9", "freq": "weekly",
                 "og_title": "Download the Genoryn Krypton wallet for Windows and Linux",
                 "og_desc": "Quantum-resistant private wallet with built-in miner and optional full node. Windows installer, Linux packages, source code."},
    "docs": {"path": "/docs", "file": "docs.html", "type": "article", "priority": "0.8", "freq": "weekly",
             "og_title": "Genoryn Krypton documentation",
             "og_desc": "Wallet, security, cryptography, emission and scarcity, mining, atomic swaps with bitcoin, the market price, light clients, the P2P network, node operation, API, SDKs and integration."},
    "tokens": {"path": "/tokens", "file": "tokens.html", "type": "article", "priority": "0.9", "freq": "daily",
              "og_title": "Quantum Tokens: planned for Genoryn Krypton",
              "og_desc": "Planned for a future release, not live yet: stablecoins and other tokens anyone will be able to deploy, as private and quantum-safe as GNR, with every issuer power declared up front."},
    "faq": {"path": "/faq", "file": "faq.html", "priority": "0.9", "freq": "weekly", "type": "article",
            "og_title": "Genoryn Krypton (GNR) FAQ: quantum security, privacy, mining, swaps",
            "og_desc": "Straight answers about Genoryn Krypton: what it is, how it resists quantum computers, what is private, how to mine and buy GNR, wallets, fees, audits and who controls it."},
    "quantum": {"path": "/quantum", "file": "quantum.html", "priority": "0.9", "freq": "monthly", "type": "article",
                "og_title": "Quantum-resistant cryptocurrency: what quantum computers break, and what they do not",
                "og_desc": "Which cryptography a quantum computer breaks (elliptic curves), which it does not (hashes, ML-KEM), what that means for Bitcoin and privacy coins, and how Genoryn Krypton is built against it."},
    "updates": {"path": "/updates", "file": "updates.html", "priority": "0.8", "freq": "daily",
                "og_title": "Genoryn Krypton updates: every release and change since launch",
                "og_desc": "Every update to Genoryn Krypton (GNR) since the mainnet launch, newest first: releases of the wallet, node and miner, and changes to genoryn.io."},
    "about": {"path": "/about", "file": "about.html", "type": "article", "priority": "0.9", "freq": "monthly",
              "og_title": "What is Genoryn Krypton (GNR)? Quantum-resistant, private money with Bitcoin's monetary design",
              "og_desc": "21 million coins and halvings like Bitcoin, funds protected by hash-based zk-STARKs instead of elliptic curves, every transfer shielded, fair GPU/CPU mining, atomic swaps with bitcoin."},
}

ORG = {"@type": "Organization", "@id": SITE + "/#org", "name": NAME, "alternateName": ["Genoryn", "GNR", "Genoryn Krypton (GNR)"],
       "url": SITE + "/", "foundingDate": "2026-11-03", "founder": {"@type": "Person", "name": "Zarazi Sokani"},
       "knowsAbout": ["post-quantum cryptography", "quantum-resistant cryptocurrency", "zk-STARK", "ML-KEM", "privacy coin",
                      "proof of work", "GPU mining", "atomic swap", "Bitcoin"],
       "logo": {"@type": "ImageObject", "url": SITE + "/assets/icon-512.png", "width": 512, "height": 512},
       "sameAs": ["https://github.com/genoryn/genoryn", "https://github.com/genoryn"],
       "description": "Genoryn Krypton (GNR) is a quantum-resistant, private-by-default proof-of-work cryptocurrency with Bitcoin's monetary design."}
CODE = {"@type": "SoftwareSourceCode", "@id": SITE + "/#code", "name": "Genoryn Krypton",
        "codeRepository": "https://github.com/genoryn/genoryn", "programmingLanguage": "Rust",
        "license": "https://opensource.org/licenses/MIT", "author": {"@type": "Person", "name": "Zarazi Sokani"},
        "description": "Full node, miner, wallets and swap engine of Genoryn Krypton: zk-STARK shielded transfers, ML-KEM encryption, KryptoHash proof of work, light clients and atomic swaps with bitcoin."}
WEBSITE = {"@type": "WebSite", "@id": SITE + "/#website", "name": NAME, "alternateName": ["Genoryn", "GNR"], "url": SITE + "/",
           "description": "Genoryn Krypton (GNR): quantum-resistant, private-by-default proof-of-work cryptocurrency with Bitcoin's monetary design.",
           "inLanguage": "en", "publisher": {"@id": SITE + "/#org"}}
APP = {"@type": "SoftwareApplication", "@id": SITE + "/#wallet", "name": "Genoryn Krypton Wallet",
       "applicationCategory": "FinanceApplication", "applicationSubCategory": "Cryptocurrency wallet and miner",
       "operatingSystem": "Windows 10, Windows 11, Linux", "softwareVersion": VERSION,
       "downloadUrl": "https://github.com/genoryn/genoryn/releases/latest", "url": SITE + "/download",
       "image": OG, "screenshot": SITE + "/assets/shots/wallet-1440.webp", "license": "https://opensource.org/licenses/MIT",
       "offers": {"@type": "Offer", "price": "0", "priceCurrency": "USD"},
       "featureList": ["Quantum-resistant zk-STARK shielded transfers", "Hybrid X25519 + ML-KEM-1024 encryption",
                       "GPU and CPU mining", "Verified light sync without a full node", "Optional full node"],
       "publisher": {"@id": SITE + "/#org"}}


def crumbs(name, path):
    return {"@type": "BreadcrumbList", "itemListElement": [
        {"@type": "ListItem", "position": 1, "name": "Home", "item": SITE + "/"},
        {"@type": "ListItem", "position": 2, "name": name, "item": SITE + path}]}


def faq(pairs):
    return {"@type": "FAQPage", "mainEntity": [
        {"@type": "Question", "name": q, "acceptedAnswer": {"@type": "Answer", "text": strip_tags(a)}} for q, a in pairs]}


HOME_FAQ = [
    ("Is Genoryn Krypton really quantum-resistant?", "Spends rely on zk-STARKs (hash functions only), note encryption uses ML-KEM-1024 (FIPS 203) combined with X25519, peers use ML-KEM-768. Shor's algorithm breaks none of these. The code has not had an independent audit yet."),
    ("Was there a premine or an early bonus?", "No. The genesis block issues nothing and every block pays the same schedule: 5 GNR halving every 2,100,000 blocks. The mainnet genesis commits to the hash of Bitcoin block 974,438, the newest Bitcoin block at launch, so no Genoryn Krypton block could be mined before it."),
    ("Do I need to download the whole blockchain?", "No. The light wallet verifies the chain with MMR proofs (FlyClient-style sampling) and checks every block it reads against its header."),
    ("Should I mine with a GPU or a CPU?", "A GPU is roughly 40 to 50 times faster per device. CPUs still work, which is useful on cloud servers; the miner can cap CPU usage."),
    ("Do small miners earn anything?", "Yes. Every block's reward is shared among all miners by the work they did: about 32 shares per block each earn an equal slice, so 1 % of the hashrate earns about 1 % of the rewards, without a pool."),
    ("Who controls the project?", "Nobody. It was created by the pseudonymous Zarazi Sokani, whose identity will never be revealed. No one can change the rules without the nodes that run them."),
]

ABOUT_FAQ = [
    ("What is Genoryn Krypton?", "A quantum-resistant, private-by-default proof-of-work cryptocurrency (ticker GNR) with Bitcoin's monetary design: 21 million coins, halvings, mining and a fair launch with no premine."),
    ("Is Genoryn Krypton a fork of Bitcoin?", "No. It is an independent network with its own code and chain. It follows Bitcoin's monetary rules, its launch is tied to a Bitcoin block hash, and it trades with bitcoin through atomic swaps, but it shares no code or coins with Bitcoin."),
    ("What makes it quantum-resistant?", "Funds are protected by zk-STARK proofs over hash functions, not by elliptic curves, so Shor's algorithm does not apply. Payment data is encrypted with hybrid X25519 + ML-KEM-1024, and releases are signed with ML-DSA and SLH-DSA."),
    ("Is it anonymous?", "Transfers hide amounts, senders, recipients and memos for everyone, always. Mining payouts are public per miner, as on every chain; mine to a separate wallet to keep them unlinked."),
    ("How is it different from Monero and Zcash?", "Monero and Zcash protect funds with elliptic-curve cryptography; GNR uses hash-based zk-STARKs and post-quantum encryption. Unlike Zcash, every GNR transfer is shielded; unlike Monero, supply is capped at 21 million."),
    ("How can I get GNR?", "Mine it with a GPU or CPU from the wallet, or buy it with bitcoin through an atomic swap on the offer board. There was no presale and GNR is not listed on exchanges."),
    ("Who created it, and who controls it?", "It was created by the pseudonymous Zarazi Sokani. Nobody controls the protocol: rules change only when node operators choose to run new software."),
    ("Has it been audited?", "It has had an internal audit and a privately reported external review, whose findings are fixed. A full independent audit has not been done yet."),
]

COINS_FAQ = [
    ("When will quantum tokens arrive?", "In a future release. The network turns them on at a block height announced with that update, so every node and wallet can update first."),
    ("Will token transfers be private?", "Yes, exactly like GNR: the token, the amount, the sender and the recipient will be hidden by a zero-knowledge proof. Only the creation and destruction of tokens will be public."),
    ("Could an issuer freeze or take my tokens?", "No. No issuer will be able to take tokens, see balances or freeze a single holder; anything an issuer may do will be declared before the token exists."),
    ("Do I need to do anything now?", "No. Keep your wallet up to date; its Tokens tab turns on by itself when tokens are activated."),
]

# Shown on /faq (build.py renders them) and mirrored in its JSON-LD. Answers may hold links.
FAQ_GROUPS = [
    ("basics", "The basics", [
        ("What is Genoryn Krypton (GNR)?", "Genoryn Krypton, ticker GNR, is a quantum-resistant, private-by-default proof-of-work cryptocurrency with Bitcoin's monetary design: at most 21,000,000 coins, halvings about every four years, mining and a fair launch with no premine. Coins are protected by hash-based zk-STARK proofs instead of elliptic curves, and every transfer is shielded. <a href=\"/about\">More about it</a>."),
        ("When did it launch?", "Mainnet started on 3 November 2026. Its genesis block commits to the hash of Bitcoin block 974,438, the newest Bitcoin block at that moment, so no GNR could be mined earlier, not even by its author."),
        ("How many GNR will ever exist?", "At most 21,000,000 GNR, enforced by every node. A block is found about every 60 seconds and pays 5 GNR; the reward halves every 2,100,000 blocks, about every four years."),
        ("What is a krypton?", "The smallest unit of GNR: 1 GNR = 100,000,000 krypton, as a satoshi is to a bitcoin."),
        ("Is it a fork of Bitcoin, or a token on another chain?", "Neither. It is an independent layer-1 network with its own code, written in Rust, and its own chain. It follows Bitcoin's monetary rules and trades with bitcoin through atomic swaps, but shares no code or coins with it."),
        ("Who created it, and who controls it?", "It was created by the pseudonymous Zarazi Sokani, whose identity will never be revealed. No company, foundation or person controls the protocol: rules change only when the people who run nodes choose to run new software."),
        ("Is it open source?", "Yes. The node, miner, wallets and swap engine are open source under MIT or Apache-2.0 at <a href=\"https://github.com/genoryn/genoryn\">github.com/genoryn/genoryn</a>."),
    ]),
    ("quantum", "Quantum security", [
        ("Why are quantum computers a threat to cryptocurrencies?", "Almost every cryptocurrency protects coins with elliptic-curve signatures. A large enough quantum computer running Shor's algorithm could compute a private key from its public key and spend other people's coins. It could also decrypt private transactions that were recorded years earlier. <a href=\"/quantum\">The full explanation</a>."),
        ("What makes Genoryn Krypton quantum-resistant?", "No elliptic curve protects coins or privacy. Spending is a zk-STARK proof over hash functions, payment data is encrypted with hybrid X25519 + ML-KEM-1024 (NIST FIPS 203), peers connect over X25519 + ML-KEM-768, and releases are signed with ML-DSA-87 and SLH-DSA (FIPS 204 and 205). Shor's algorithm breaks none of these."),
        ("Is Bitcoin quantum-safe?", "Not today. Bitcoin's coins are guarded by ECDSA and Schnorr signatures on the secp256k1 curve, which Shor's algorithm breaks. Coins whose public key is already visible on chain (early pay-to-public-key outputs, reused addresses, Taproot outputs) are the most exposed. No quantum computer can do this yet, and Bitcoin's developers are discussing post-quantum upgrades."),
        ("What does \"harvest now, decrypt later\" mean?", "Encrypted data recorded today can be stored and decrypted once a quantum computer exists. For a privacy coin that relies on elliptic curves this means past transactions could be exposed later, and no upgrade can undo it. GNR encrypts payment data with post-quantum ML-KEM from its first block."),
        ("Do quantum computers break hash functions or mining?", "No. Grover's algorithm gives at most a quadratic speed-up against hashes, so a 256-bit hash keeps a 128-bit security margin. Proof of work adjusts its difficulty to whatever hardware mines."),
    ]),
    ("privacy", "Privacy", [
        ("Is GNR anonymous?", "Every transfer is shielded: amounts, senders, recipients and memos are hidden for everyone, always. There is no transparent mode. Mining payouts are public per miner, as on every chain; mine to a separate wallet to keep them unlinked."),
        ("Can anyone see my balance?", "No. A balance can be computed only by a wallet that holds the keys."),
        ("Can I prove that I paid someone?", "Yes. The wallet creates a payment proof for one payment, which shows that payment to whoever you give it to and reveals nothing else."),
    ]),
    ("mining", "Mining", [
        ("How do I mine GNR?", "Install the wallet, create a wallet, open the Mining tab, choose your GPU or CPU and press Start mining. No account, no pool and no configuration files are needed. <a href=\"/mining\">Mining guide</a>."),
        ("Should I mine with a GPU or a CPU?", "A GPU is roughly 40 to 50 times faster: an RTX 5090 does about 75 MH/s and a 24-core CPU about 1.4 MH/s. NVIDIA cards mine with CUDA, AMD and Intel cards with OpenCL. CPUs work too, and the miner can cap their usage."),
        ("Do I need a mining pool?", "No. Every block's reward is shared among all miners by the work they did: about 32 shares per block each earn an equal slice. With 1 % of the hashrate you earn about 1 % of the rewards, with no pool operator in between."),
        ("When are mining rewards paid?", "Every 360 blocks, about every six hours, each miner receives one payout, which can be spent 100 blocks later."),
        ("Was there a premine, a presale or a developer fee?", "No. The genesis block issues nothing, there was no presale, and every block pays miners only, under the same schedule since block 1."),
    ]),
    ("trading", "Getting and trading GNR", [
        ("How can I buy GNR?", "In the wallet's Swap tab, with bitcoin, through an atomic swap with another person: take a sell offer or publish a buy offer. There is no exchange, no account and no custody. <a href=\"/get\">How to get GNR</a>."),
        ("Is GNR listed on an exchange?", "No. GNR is earned by mining or exchanged with bitcoin directly between wallets."),
        ("Where does the GNR price come from?", "Only from completed swaps: the market price is their plain average over the last 72 hours. Offers and swaps in progress never move it. <a href=\"/market\">Live market</a>."),
        ("Why swap bitcoin for GNR?", "Bitcoin's coins are guarded by elliptic-curve signatures, which a large quantum computer could break. A swap moves value into a system guarded by hash-based proofs, directly from your own wallet."),
        ("Can someone offer GNR they do not have?", "No. A sell offer is listed only after its GNR is set aside in a note of its own and the offer board has checked a zero-knowledge proof that the seller can spend it. The board takes the offer down the moment that GNR moves, and the same GNR cannot back two offers. <a href=\"/market#rules\">The rules of an offer</a>."),
        ("Why is my GNR held after I cancel a sell offer?", "The wallet frees the GNR of a cancelled sell offer 24 hours later, or when the offer would have ended if that is sooner, so that offers are made in earnest. You can put it into a new offer at once. An offer that runs its time frees its GNR at once."),
        ("Why is the bitcoin of a buy offer not locked in advance?", "A bitcoin contract has to name the seller's key and the hash of the seller's secret, and neither exists before a seller takes the offer. Bitcoin locked earlier would only be bitcoin that its owner can still move. So a buyer pays into the contract of a swap that has started, after the seller's GNR is locked for them on chain, and never before."),
        ("Can a swap cheat me?", "Both sides lock their coins under the same hash lock with a time lock. The seller's GNR is locked on chain before the buyer pays, taking the bitcoin reveals the secret that releases the GNR, and if anyone stops, both sides are refunded after the time locks."),
    ]),
    ("wallet", "Wallet", [
        ("Which systems does the wallet run on?", "Windows 10 and 11 (x64) and Linux (Ubuntu 24.04, Debian 13 or newer). Wallets for Android, iOS and macOS are in development. <a href=\"/download\">Download</a>."),
        ("Do I need to download the whole blockchain?", "No. The light wallet verifies the chain itself with proofs of work and Merkle Mountain Range proofs, and checks every block it reads. A full node is optional."),
        ("How do I back up my wallet?", "Write down the 24-word recovery phrase with its numbers when you create the wallet. The phrase restores the wallet on any computer. Nobody can recover coins without the phrase, or the wallet file and its password."),
        ("What does a transfer cost?", "A small fee paid to miners, about 0.00003 GNR when the network is not busy."),
        ("How do I know a download is genuine?", "Every release publishes SHA256SUMS signed with the project's hybrid post-quantum release key (ML-DSA-87 + SLH-DSA). The wallet checks that signature before it installs an update."),
    ]),
    ("tokens", "Quantum tokens", [
        ("What are quantum tokens?", "A planned feature, not live yet: stablecoins and other tokens that anyone will be able to deploy on Genoryn Krypton, as private and quantum-safe as GNR, with everything an issuer may do declared before the token exists. <a href=\"/tokens\">The plan</a>."),
        ("Can I deploy or buy a token today?", "No. No token exists on Genoryn Krypton today. Tokens will be turned on at a block height announced with a future release."),
    ]),
    ("trust", "Project and trust", [
        ("Has the code been audited?", "It has had an internal audit and a privately reported external review, whose findings are fixed. A full independent audit has not been done yet; the zk-STARK spend circuit and the consensus code are original work and should be reviewed before significant value is trusted to them."),
        ("What happened to the earlier Genoryn Mainnet 1.0?", "Genoryn Krypton is a new chain with different cryptography, started from block 1 with no premine. Balances of the earlier Mainnet 1.0 are not migrated; see the <a href=\"https://github.com/genoryn/genoryn/issues/1\">published policy</a>."),
        ("Is GNR an investment?", "Nothing here is investment advice. Mining or holding GNR is not a promise of value."),
        ("How do I report a security problem?", "Privately, through a <a href=\"https://github.com/genoryn/genoryn/security/advisories/new\">GitHub security advisory</a>. Please do not post vulnerabilities in public issues."),
    ]),
]

QUANTUM_FAQ = [
    ("What is a quantum-resistant cryptocurrency?", "A cryptocurrency whose coins cannot be stolen, and whose private data cannot be decrypted, by a quantum computer running the known quantum algorithms. In practice it must not rely on elliptic curves or RSA for ownership, privacy or network encryption."),
    ("Which cryptocurrencies are quantum-resistant?", "Bitcoin, Ethereum, Monero and Zcash protect funds with elliptic-curve cryptography and are not quantum-resistant today. A few newer networks are built on post-quantum cryptography. Genoryn Krypton (GNR) is one of them: no elliptic curve protects its coins or its privacy, spends are zk-STARK proofs over hash functions, and payment data is encrypted with ML-KEM."),
    ("When will quantum computers break Bitcoin?", "Nobody knows. No quantum computer today can break a 256-bit elliptic curve. NIST's transition plan proposes to stop using quantum-vulnerable public-key algorithms by 2035, and data recorded now can be attacked later."),
    ("Can an existing coin simply upgrade later?", "Signatures can be replaced if every holder moves their coins in time; coins that are lost or never moved stay exposed. Privacy cannot be repaired afterwards: transactions already recorded with elliptic-curve encryption stay decryptable."),
]


def strip_tags(html):
    return re.sub(r"<[^>]+>", "", html).replace("&amp;", "&")


def faq_html(pairs):
    return "\n".join(f'      <details class="glass"><summary>{q}</summary><p>{a}</p></details>' for q, a in pairs)


def faq_page_html():
    toc = " ".join(f'<a class="chip" href="#{gid}">{title}</a>' for gid, title, _ in FAQ_GROUPS)
    out = [f'    <nav class="faq-toc" aria-label="Topics">{toc}</nav>']
    for gid, title, pairs in FAQ_GROUPS:
        out.append(f'    <section id="{gid}" class="faq">\n      <h2>{title}</h2>\n{faq_html(pairs)}\n    </section>')
    return "\n".join(out)


HOWTO = {"@type": "HowTo", "name": "How to mine Genoryn Krypton (GNR)", "totalTime": "PT5M",
         "tool": [{"@type": "HowToTool", "name": "NVIDIA or AMD GPU, or any x86-64 CPU"}],
         "step": [
             {"@type": "HowToStep", "name": "Install the wallet", "text": "Download the Genoryn Krypton wallet for Windows or Linux.", "url": SITE + "/download"},
             {"@type": "HowToStep", "name": "Create a wallet", "text": "Choose a password and write down the 24-word recovery phrase with its numbers."},
             {"@type": "HowToStep", "name": "Choose your hardware", "text": "In the Mining tab select Automatic, NVIDIA (CUDA), AMD/Intel (OpenCL) or CPU and set a CPU limit."},
             {"@type": "HowToStep", "name": "Start mining", "text": "Press Start mining. You earn in proportion to your hashrate; rewards are paid every 360 blocks and mature after 100 blocks."}]}


def graph(page):
    p = PAGES[page]
    url = SITE + p["path"]
    items = [ORG, WEBSITE]
    if page == "home":
        items += [APP, CODE, faq(HOME_FAQ),
                  {"@type": "WebPage", "@id": url, "url": url, "name": p["og_title"], "isPartOf": {"@id": SITE + "/#website"},
                   "about": {"@id": SITE + "/#wallet"}, "primaryImageOfPage": OG, "dateModified": TODAY}]
    elif page == "download":
        items += [APP, crumbs("Download", p["path"])]
    elif page == "mining":
        items += [HOWTO, crumbs("Mining", p["path"])]
    elif page == "docs":
        items += [{"@type": "TechArticle", "headline": "Genoryn Krypton documentation", "url": url, "inLanguage": "en",
                   "dateModified": TODAY, "author": {"@type": "Person", "name": "Zarazi Sokani"},
                   "publisher": {"@id": SITE + "/#org"}, "image": OG,
                   "about": ["post-quantum cryptography", "zk-STARK", "cryptocurrency mining", "light clients"]},
                  crumbs("Documentation", p["path"])]
    elif page == "about":
        items += [{"@type": "Article", "headline": "What is Genoryn Krypton?", "url": url, "inLanguage": "en",
                   "dateModified": TODAY, "author": {"@type": "Person", "name": "Zarazi Sokani"},
                   "publisher": {"@id": SITE + "/#org"}, "image": OG, "mainEntityOfPage": url,
                   "about": ["Genoryn Krypton", "quantum-resistant cryptocurrency", "privacy coin", "post-quantum cryptography", "Bitcoin"]},
                  CODE, faq(ABOUT_FAQ), crumbs("About", p["path"])]
    elif page == "tokens":
        items += [{"@type": "Article", "headline": "Quantum Tokens: planned for Genoryn Krypton", "url": url, "inLanguage": "en",
                   "dateModified": TODAY, "author": {"@type": "Person", "name": "Zarazi Sokani"},
                   "publisher": {"@id": SITE + "/#org"}, "image": OG, "mainEntityOfPage": url,
                   "about": ["quantum tokens", "private tokens", "stablecoins", "zk-STARK", "Genoryn Krypton"]},
                  faq(COINS_FAQ), crumbs("Quantum tokens", p["path"])]
    elif page == "faq":
        items += [faq([qa for _, _, pairs in FAQ_GROUPS for qa in pairs]), crumbs("FAQ", p["path"])]
    elif page == "quantum":
        items += [{"@type": "Article", "headline": "Quantum-resistant cryptocurrency: what quantum computers break, and what they do not",
                   "url": url, "inLanguage": "en", "datePublished": "2026-11-18", "dateModified": TODAY,
                   "author": {"@type": "Person", "name": "Zarazi Sokani"}, "publisher": {"@id": SITE + "/#org"},
                   "image": OG, "mainEntityOfPage": url,
                   "about": ["quantum-resistant cryptocurrency", "post-quantum cryptography", "Shor's algorithm", "Bitcoin",
                             "zk-STARK", "ML-KEM", "harvest now, decrypt later"]},
                  faq(QUANTUM_FAQ), crumbs("Quantum security", p["path"])]
    elif page == "market":
        items += [{"@type": "WebPage", "url": url, "name": p["og_title"], "isPartOf": {"@id": SITE + "/#website"}},
                  crumbs("Market", p["path"])]
    elif page == "updates":
        import updates
        items += [{"@type": "CollectionPage", "url": url, "name": p["og_title"], "isPartOf": {"@id": SITE + "/#website"},
                   "dateModified": updates.UPDATES[0]["date"],
                   "mainEntity": {"@type": "ItemList", "itemListElement": [
                       {"@type": "ListItem", "position": i + 1, "url": f"{url}#{updates._anchor(u)}",
                        "name": ("Genoryn Krypton " + u["v"] + ": " if "v" in u else "") + u["title"]}
                       for i, u in enumerate(updates.UPDATES[:20])]}},
                  crumbs("Updates", p["path"])]
    elif page == "get":
        items += [{"@type": "WebPage", "url": url, "name": p["og_title"], "isPartOf": {"@id": SITE + "/#website"}},
                  crumbs("Get GNR", p["path"])]
    elif page == "explorer":
        items += [{"@type": "WebPage", "url": url, "name": p["og_title"], "isPartOf": {"@id": SITE + "/#website"}},
                  crumbs("Explorer", p["path"])]
    return json.dumps({"@context": "https://schema.org", "@graph": items}, ensure_ascii=False, separators=(",", ":"))


def head(page):
    p = PAGES[page]
    url = SITE + p["path"]
    t = lambda s: s.replace("&", "&amp;").replace('"', "&quot;")
    return "\n".join([
        f'<link rel="canonical" href="{url}">',
        '<meta name="robots" content="index, follow, max-image-preview:large, max-snippet:-1, max-video-preview:-1">',
        f'<meta name="keywords" content="{KEYWORDS}">',
        '<meta name="author" content="Zarazi Sokani">',
        '<meta name="application-name" content="Genoryn Krypton">',
        '<meta name="theme-color" content="#000000" media="(prefers-color-scheme: dark)">',
        '<meta name="theme-color" content="#f5f5f5" media="(prefers-color-scheme: light)">',
        f'<link rel="alternate" hreflang="en" href="{url}">',
        f'<link rel="alternate" hreflang="x-default" href="{url}">',
        '<link rel="manifest" href="/site.webmanifest">',
        '<link rel="icon" href="/favicon.ico" sizes="any">',
        '<link rel="apple-touch-icon" href="/assets/apple-touch-icon.png">',
        f'<meta property="og:type" content="{p.get("type", "website")}">',
        f'<meta property="og:site_name" content="{NAME}">',
        '<meta property="og:locale" content="en_US">',
        f'<meta property="og:url" content="{url}">',
        f'<meta property="og:title" content="{t(p["og_title"])}">',
        f'<meta property="og:description" content="{t(p["og_desc"])}">',
        f'<meta property="og:image" content="{OG}">',
        '<meta property="og:image:width" content="1200">',
        '<meta property="og:image:height" content="630">',
        '<meta property="og:image:alt" content="Genoryn Krypton: quantum-resistant, private money">',
        '<meta name="twitter:card" content="summary_large_image">',
        f'<meta name="twitter:title" content="{t(p["og_title"])}">',
        f'<meta name="twitter:description" content="{t(p["og_desc"])}">',
        f'<meta name="twitter:image" content="{OG}">',
        '<meta name="twitter:image:alt" content="Genoryn Krypton: quantum-resistant, private money">',
        '<link rel="alternate" type="application/atom+xml" title="Genoryn Krypton releases" href="https://github.com/genoryn/genoryn/releases.atom">',
        f'<script type="application/ld+json">{graph(page)}</script>',
    ])


_HERE = pathlib.Path(__file__).resolve().parent
_IMAGES = {"home": ["/assets/og.png", "/assets/shots/wallet-1440.webp", "/assets/shots/swap-1440.webp", "/assets/shots/mining-1440.webp"],
           "download": ["/assets/shots/wallet-1440.webp"], "mining": ["/assets/shots/mining-1440.webp"]}


def lastmod(key):
    """The day the page's own source last changed (search engines ignore a date that is always today)."""
    files = [_HERE / PAGES[key]["file"]]
    if key in ("faq", "quantum"):
        files.append(_HERE / "seo.py")
    if key in ("updates", "home"):
        files.append(_HERE / "updates.py")
    t = max(f.stat().st_mtime for f in files if f.exists())
    return datetime.datetime.fromtimestamp(t, datetime.timezone.utc).date().isoformat()


def sitemap():
    urls = ""
    for key, p in PAGES.items():
        imgs = "".join(f"<image:image><image:loc>{SITE}{i}</image:loc></image:image>"
                       for i in _IMAGES.get(key, []) if (_HERE / i.lstrip("/")).exists() or (_HERE.parent / "design" / i.lstrip("/")).exists())
        urls += (f"<url><loc>{SITE}{p['path']}</loc><lastmod>{lastmod(key)}</lastmod>"
                 f"<changefreq>{p['freq']}</changefreq><priority>{p['priority']}</priority>{imgs}</url>")
    return ('<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" '
            f'xmlns:image="http://www.google.com/schemas/sitemap-image/1.1">{urls}</urlset>\n')


_BOTS = ["Googlebot", "Google-Extended", "Bingbot", "GPTBot", "OAI-SearchBot", "ChatGPT-User", "ClaudeBot",
         "Claude-SearchBot", "Claude-User", "PerplexityBot", "Perplexity-User", "Applebot", "Applebot-Extended",
         "DuckDuckBot", "YandexBot", "CCBot", "Meta-ExternalAgent", "Amazonbot", "cohere-ai", "MistralAI-User"]
ROBOTS = ("# Search engines and AI assistants are welcome. A plain-text summary for language models:\n"
          f"# {SITE}/llms.txt (and the full version at {SITE}/llms-full.txt)\n\n"
          + "".join(f"User-agent: {b}\n" for b in _BOTS)
          + "Allow: /\nDisallow: /v1/\nDisallow: /rpc\n\n"
          "User-agent: *\nAllow: /\nDisallow: /v1/\nDisallow: /rpc\n\n"
          f"Sitemap: {SITE}/sitemap.xml\n")

SECURITY_TXT = ("Contact: https://github.com/genoryn/genoryn/security/advisories/new\n"
                "Policy: https://github.com/genoryn/genoryn/blob/main/SECURITY.md\n"
                "Preferred-Languages: en\n"
                f"Canonical: {SITE}/.well-known/security.txt\n"
                "Expires: " + (datetime.date.today() + datetime.timedelta(days=365)).isoformat() + "T00:00:00Z\n")

MANIFEST = json.dumps({
    "name": NAME, "short_name": "Genoryn", "description": "Quantum-resistant, private money.",
    "start_url": "/", "display": "standalone", "background_color": "#000000", "theme_color": "#000000",
    "icons": [{"src": "/assets/icon-192.png", "sizes": "192x192", "type": "image/png"},
              {"src": "/assets/icon-512.png", "sizes": "512x512", "type": "image/png", "purpose": "any maskable"}]}, indent=1)
