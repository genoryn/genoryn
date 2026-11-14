"""SEO metadata for genoryn.io: canonical URLs, Open Graph, Twitter cards, JSON-LD,
sitemap and web manifest. Used by build.py; all claims mirror visible page content."""
import datetime
import json

SITE = "https://genoryn.io"
NAME = "Genoryn Krypton"
VERSION = "1.0.34"
OG = SITE + "/assets/og.png"
TODAY = datetime.date.today().isoformat()
KEYWORDS = ("Genoryn Krypton, GNR, quantum-resistant cryptocurrency, post-quantum cryptocurrency, quantum-safe Bitcoin alternative, "
            "private cryptocurrency, anonymous cryptocurrency, privacy coin, zk-STARK, ML-KEM, post-quantum privacy coin, "
            "GPU mining, CPU mining, new cryptocurrency to mine, fair launch, no premine, 21 million supply, "
            "atomic swap bitcoin, light wallet, KryptoHash")
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
    "docs": {"path": "/docs", "file": "docs.html", "priority": "0.8", "freq": "weekly",
             "og_title": "Genoryn Krypton documentation",
             "og_desc": "Wallet, security, cryptography, emission and scarcity, mining, atomic swaps with bitcoin, the market price, light clients, the P2P network, node operation, API, SDKs and integration."},
    "about": {"path": "/about", "file": "about.html", "priority": "0.9", "freq": "monthly",
              "og_title": "What is Genoryn Krypton (GNR)? Quantum-resistant, private money with Bitcoin's monetary design",
              "og_desc": "21 million coins and halvings like Bitcoin, funds protected by hash-based zk-STARKs instead of elliptic curves, every transfer shielded, fair GPU/CPU mining, atomic swaps with bitcoin."},
}

ORG = {"@type": "Organization", "@id": SITE + "/#org", "name": NAME, "url": SITE + "/",
       "logo": {"@type": "ImageObject", "url": SITE + "/assets/icon-512.png", "width": 512, "height": 512},
       "sameAs": ["https://github.com/genoryn/genoryn"],
       "description": "Genoryn Krypton (GNR) is a quantum-resistant, private-by-default proof-of-work cryptocurrency with Bitcoin's monetary design."}
CODE = {"@type": "SoftwareSourceCode", "@id": SITE + "/#code", "name": "Genoryn Krypton",
        "codeRepository": "https://github.com/genoryn/genoryn", "programmingLanguage": "Rust",
        "license": "https://opensource.org/licenses/MIT", "author": {"@type": "Person", "name": "Zarazi Sokani"},
        "description": "Full node, miner, wallets and swap engine of Genoryn Krypton: zk-STARK shielded transfers, ML-KEM encryption, KryptoHash proof of work, light clients and atomic swaps with bitcoin."}
WEBSITE = {"@type": "WebSite", "@id": SITE + "/#website", "name": NAME, "alternateName": "GNR", "url": SITE + "/",
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
        {"@type": "Question", "name": q, "acceptedAnswer": {"@type": "Answer", "text": a}} for q, a in pairs]}


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
        '<meta property="og:type" content="website">',
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
        f'<script type="application/ld+json">{graph(page)}</script>',
    ])


def sitemap():
    urls = "".join(
        f"<url><loc>{SITE}{p['path']}</loc><lastmod>{TODAY}</lastmod><changefreq>{p['freq']}</changefreq><priority>{p['priority']}</priority></url>"
        for p in PAGES.values())
    return f'<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">{urls}</urlset>\n'


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
