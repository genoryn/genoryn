"""SEO metadata for genoryn.io: canonical URLs, Open Graph, Twitter cards, JSON-LD,
sitemap and web manifest. Used by build.py; all claims mirror visible page content."""
import datetime
import json

SITE = "https://genoryn.io"
NAME = "Genoryn Krypton"
VERSION = "1.0.1"
OG = SITE + "/assets/og.png"
TODAY = datetime.date.today().isoformat()
KEYWORDS = ("Genoryn Krypton, GNR, quantum-resistant cryptocurrency, post-quantum crypto, private cryptocurrency, "
            "zk-STARK, ML-KEM, privacy coin, GPU mining, CPU mining, new cryptocurrency to mine, fair launch, "
            "no premine, 21 million supply, light wallet, KryptoHash")

PAGES = {
    "home": {"path": "/", "file": "index.html", "priority": "1.0", "freq": "daily",
             "og_title": "Genoryn Krypton (GNR): quantum-resistant, private money you can mine today",
             "og_desc": "Fair launch, no premine, 21,000,000 GNR. zk-STARK shielded transfers, ML-KEM encryption, GPU and CPU mining, light wallet that verifies everything."},
    "explorer": {"path": "/explorer.html", "file": "explorer.html", "priority": "0.8", "freq": "always",
                 "og_title": "Genoryn Krypton explorer: live blocks and network stats",
                 "og_desc": "Live Genoryn Krypton blocks, transactions, supply and hashrate. Shielded transfers reveal no amounts or addresses."},
    "mining": {"path": "/mining.html", "file": "mining.html", "priority": "0.9", "freq": "weekly",
               "og_title": "Mine Genoryn Krypton (GNR) on GPU or CPU",
               "og_desc": "5 GNR per block, fair launch, no premine. KryptoHash is GPU-first and CPU-capable: start mining from the wallet in one click."},
    "download": {"path": "/download.html", "file": "download.html", "priority": "0.9", "freq": "weekly",
                 "og_title": "Download the Genoryn Krypton wallet for Windows and Linux",
                 "og_desc": "Quantum-resistant private wallet with built-in miner and optional full node. Windows installer, Linux packages, source code."},
    "docs": {"path": "/docs.html", "file": "docs.html", "priority": "0.8", "freq": "weekly",
             "og_title": "Genoryn Krypton documentation",
             "og_desc": "Protocol, cryptography, emission, mining, light clients, node operation, REST and JSON-RPC API, SDKs and exchange integration."},
}

ORG = {"@type": "Organization", "@id": SITE + "/#org", "name": NAME, "url": SITE + "/",
       "logo": {"@type": "ImageObject", "url": SITE + "/assets/icon-512.png", "width": 512, "height": 512},
       "sameAs": ["https://github.com/genoryn/genoryn"]}
WEBSITE = {"@type": "WebSite", "@id": SITE + "/#website", "name": NAME, "alternateName": "GNR", "url": SITE + "/",
           "inLanguage": "en", "publisher": {"@id": SITE + "/#org"}}
APP = {"@type": "SoftwareApplication", "@id": SITE + "/#wallet", "name": "Genoryn Krypton Wallet",
       "applicationCategory": "FinanceApplication", "applicationSubCategory": "Cryptocurrency wallet and miner",
       "operatingSystem": "Windows 10, Windows 11, Linux", "softwareVersion": VERSION,
       "downloadUrl": "https://github.com/genoryn/genoryn/releases/latest", "url": SITE + "/download.html",
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

HOWTO = {"@type": "HowTo", "name": "How to mine Genoryn Krypton (GNR)", "totalTime": "PT5M",
         "tool": [{"@type": "HowToTool", "name": "NVIDIA or AMD GPU, or any x86-64 CPU"}],
         "step": [
             {"@type": "HowToStep", "name": "Install the wallet", "text": "Download the Genoryn Krypton wallet for Windows or Linux.", "url": SITE + "/download.html"},
             {"@type": "HowToStep", "name": "Create a wallet", "text": "Choose a password and write down the 24-word recovery phrase with its numbers."},
             {"@type": "HowToStep", "name": "Choose your hardware", "text": "In the Mining tab select Automatic, NVIDIA (CUDA), AMD/Intel (OpenCL) or CPU and set a CPU limit."},
             {"@type": "HowToStep", "name": "Start mining", "text": "Press Start mining. You earn in proportion to your hashrate; rewards are paid every 360 blocks and mature after 100 blocks."}]}


def graph(page):
    p = PAGES[page]
    url = SITE + p["path"]
    items = [ORG, WEBSITE]
    if page == "home":
        items += [APP, faq(HOME_FAQ),
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


ROBOTS = f"User-agent: *\nAllow: /\nDisallow: /v1/\nDisallow: /rpc\n\nSitemap: {SITE}/sitemap.xml\n"

MANIFEST = json.dumps({
    "name": NAME, "short_name": "Genoryn", "description": "Quantum-resistant, private money.",
    "start_url": "/", "display": "standalone", "background_color": "#000000", "theme_color": "#000000",
    "icons": [{"src": "/assets/icon-192.png", "sizes": "192x192", "type": "image/png"},
              {"src": "/assets/icon-512.png", "sizes": "512x512", "type": "image/png", "purpose": "any maskable"}]}, indent=1)
