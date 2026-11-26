"""Builds the static genoryn.io site into website/dist (no npm, no bundler).

Inserts the shared head script, SEO metadata, navbar and footer into every page,
copies the shared design assets and writes sitemap.xml, robots.txt and the manifest.
Run: python website/build.py
"""
import hashlib
import json
import os
import pathlib
import re
import shutil
import subprocess
import tempfile
import urllib.request

import i18n
import seo
import updates

ROOT = pathlib.Path(__file__).resolve().parent
DESIGN = ROOT.parent / "design"
DIST = ROOT / "dist"
JS_FILES = [ROOT / "site.js", ROOT / "explorer.js", ROOT / "market.js", ROOT / "miner-check.js",
            DESIGN / "assets" / "value.js", DESIGN / "assets" / "charts.js"]
_TR = {}
RELEASE = "https://github.com/genoryn/genoryn/releases/download/v{v}/{f}"
# The files the download page offers, by placeholder (<!--SUM:win-setup--> and so on).
DOWNLOADS = {
    "win-setup": "GenorynKrypton-{v}-Windows-x64-Setup.exe",
    "win-zip": "genoryn-krypton-{v}-windows-x64-portable.zip",
    "deb": "genoryn-krypton_{v}_amd64.deb",
    "tgz": "genoryn-krypton-{v}-linux-x64.tar.gz",
}
KEY = ROOT.parent / "docs" / "release-key.pub"
_SUMS = {}


def key_fingerprint():
    """SHA-256 of the release key (the first 16 digits are the short fingerprint)."""
    return hashlib.sha256(bytes.fromhex(KEY.read_text(encoding="utf-8").strip().lstrip("\ufeff"))).hexdigest()


def release_sums():
    """SHA-256 of each file of the current release, from its SHA256SUMS, taken only when
    SHA256SUMS.sig checks out against docs/release-key.pub. They come from GK_RELEASE_DIR
    when set (a release about to be published), otherwise from the GitHub release of
    seo.VERSION. Without them, or without the krypton-release checker, the page shows no
    hashes; a list whose signature fails stops the build."""
    tool = next((t for t in (ROOT.parent / "target" / "release" / n for n in ("krypton-release.exe", "krypton-release")) if t.is_file()), None)
    if tool is None:
        print("note: no krypton-release checker built; the download page shows no hashes")
        return {}

    def fetch(name):
        d = os.environ.get("GK_RELEASE_DIR")
        if d:
            return (pathlib.Path(d) / name).read_bytes()
        with urllib.request.urlopen(RELEASE.format(v=seo.VERSION, f=name), timeout=20) as r:
            return r.read(1 << 20)

    try:
        sums, sig = fetch("SHA256SUMS"), fetch("SHA256SUMS.sig")
    except Exception as e:
        print(f"note: no checksums for {seo.VERSION} yet ({e}); the download page shows no hashes")
        return {}
    with tempfile.TemporaryDirectory() as t:
        (pathlib.Path(t) / "SHA256SUMS").write_bytes(sums)
        (pathlib.Path(t) / "SHA256SUMS.sig").write_bytes(sig)
        r = subprocess.run([str(tool), "verify", str(KEY), str(pathlib.Path(t) / "SHA256SUMS"), str(pathlib.Path(t) / "SHA256SUMS.sig")],
                           capture_output=True, text=True)
    if r.returncode != 0:
        raise SystemExit(f"SHA256SUMS of {seo.VERSION} does not verify against docs/release-key.pub: {r.stderr.strip() or r.stdout.strip()}")
    out = {}
    for line in sums.decode("utf-8").splitlines():
        m = re.fullmatch(r"([0-9a-f]{64})  (\S+)", line.strip())
        if m:
            out[m.group(2)] = m.group(1)
    want = [f.format(v=seo.VERSION) for f in DOWNLOADS.values()]
    if not all(f in out for f in want):
        raise SystemExit(f"SHA256SUMS of {seo.VERSION} does not list every download: {', '.join(f for f in want if f not in out)}")
    print(f"download hashes: {len(want)} files of {seo.VERSION}, signature checked")
    return out


def sums_html(html):
    """The verified SHA-256 under each download button (code, never translated)."""
    for k, f in DOWNLOADS.items():
        name = f.format(v=seo.VERSION)
        h = _SUMS.get(name)
        html = html.replace(f"<!--SUM:{k}-->", f'<p class="dl-sum"><code>{name}</code><code>SHA-256 {h}</code></p>' if h else "")
    return html


def built_langs():
    """English, and every language with a catalog."""
    return ["en"] + [c for c in i18n.OTHER if (i18n.LOCALES / c).is_dir() and any((i18n.LOCALES / c).glob("*.json"))]


def translations(lang):
    if lang not in _TR:
        _TR[lang] = i18n.load(lang)
    return _TR[lang]


def lang_menu(key, lang, langs, drawer=False):
    """Links to this page in every language, each named in its own language."""
    path = seo.PAGES[key]["path"] if key else "/"
    links = "".join(
        f'<a href="{i18n.local_path(c, path)}" hreflang="{i18n.INFO[c]["lang"]}" lang="{i18n.INFO[c]["lang"]}" data-lang="{c}"'
        + (' aria-current="true" class="on"' if c == lang else "") + f'>{i18n.INFO[c]["name"]}</a>' for c in langs)
    if drawer:
        return f'<span class="drawer-h">Language</span><div class="drawer-langs" translate="no">{links}</div>'
    code = "中" if lang == "zh-cn" else lang.upper()
    return ('<div class="nav-dd nav-lang">'
            '<button type="button" class="btn icon-btn nav-dd-btn nav-lang-btn" aria-expanded="false" aria-haspopup="true" '
            f'aria-controls="menu-lang" aria-label="Language" title="Language"><svg class="ni" aria-hidden="true"><use href="#n-lang"/></svg><span translate="no">{code}</span></button>'
            f'<div class="nav-menu nav-lang-menu" id="menu-lang" translate="no">{links}</div></div>')


def i18n_note(lang):
    if lang == "en":
        return ""
    return f'<p class="i18n-note">{i18n.NOTE} <a href="/contribute#translations">{i18n.NOTE_LINK}</a>.</p>'


def assemble(page, key, lang, langs, head, nav, foot):
    """The English page with its shared parts, its head written for `lang`."""
    html = page.read_text(encoding="utf-8")
    scripts = "" if lang == "en" else f'\n<script src="/assets/i18n/{lang}.js"></script>'
    html = html.replace("<!--FX_HEAD-->", head + scripts).replace("<!--NAV-->", nav).replace("<!--FOOTER-->", foot)
    html = html.replace("<!--LANG-->", lang_menu(key, lang, langs)).replace("<!--LANG_DRAWER-->", lang_menu(key, lang, langs, True))
    html = html.replace("<!--I18N_NOTE-->", i18n_note(lang))
    if lang != "en":
        html = html.replace("<html ", f'<html data-path-prefix="/{lang}" ', 1)
    for k in seo.PAGES:
        html = html.replace(f"<!--SEO:{k}-->", seo.head(k, lang, langs))
    # Questions and answers come from seo.py, so the page and its JSON-LD never differ.
    html = html.replace("<!--FAQ:all-->", seo.faq_page_html())
    html = html.replace("<!--FAQ:quantum-->", seo.faq_html(seo.QUANTUM_FAQ))
    # Every update since the launch comes from updates.py: the page, the home page's
    # line about the latest one, the feed and the repository's CHANGELOG.md.
    html = html.replace("<!--UPDATES-->", updates.page_html())
    html = html.replace("<!--LATEST_UPDATE-->", updates.latest_html())
    html = sums_html(html).replace("<!--KEYFP-->", key_fingerprint())
    # Guides name the files of the current release.
    return html.replace("<!--VERSION-->", seo.VERSION)


def extract():
    """locales/en/*.json from the English pages as built (python website/i18n.py extract)."""
    head = (DESIGN / "fx-head.html").read_text(encoding="utf-8")
    nav = (ROOT / "_nav.html").read_text(encoding="utf-8")
    foot = (ROOT / "_footer.html").read_text(encoding="utf-8")
    langs = ["en"] + i18n.OTHER
    tmp = ROOT / "dist-extract"
    shutil.rmtree(tmp, ignore_errors=True)
    tmp.mkdir()
    for k, p in seo.PAGES.items():
        (tmp / p["file"]).write_text(assemble(ROOT / p["file"], k, "en", langs, head, nav, foot), encoding="utf-8")
    snippets = [nav.replace("<!--LANG-->", lang_menu("home", "de", langs)).replace("<!--LANG_DRAWER-->", lang_menu("home", "de", langs, True)),
                foot.replace("<!--I18N_NOTE-->", i18n_note("de"))]
    files = i18n.extract(tmp, seo.PAGES, snippets, i18n.js_strings(JS_FILES))
    shutil.rmtree(tmp)
    words = {n: sum(len(re.sub(r"<[^>]+>", " ", v if isinstance(v, str) else " ".join(v.values())).split()) for v in c.values()) for n, c in files.items()}
    for n in sorted(words, key=words.get, reverse=True):
        print(f"{n:14} {len(files[n]):5} texts {words[n]:7} words")
    print(f"{'total':14} {sum(len(c) for c in files.values()):5} texts {sum(words.values()):7} words")


def llms_full():
    """README.md as plain Markdown for language models: no HTML, absolute links."""
    text = (ROOT.parent / "README.md").read_text(encoding="utf-8")
    text = text[text.index("## Why this exists"):]
    repo = "https://github.com/genoryn/genoryn/blob/main/"
    text = re.sub(r"\]\((?!https?:|#)([^)]+)\)", lambda m: f"]({repo}{m.group(1)})", text)
    text = re.sub(r"</?(details|summary|b)>", "", text)
    head = (ROOT / "llms.txt").read_text(encoding="utf-8").split("\n## ", 1)[0].rstrip()
    return head + "\n\n" + text


def main():
    _SUMS.update(release_sums())
    if DIST.exists():
        shutil.rmtree(DIST)
    DIST.mkdir()
    shutil.copytree(DESIGN / "assets", DIST / "assets")
    if (ROOT / "assets").exists():
        shutil.copytree(ROOT / "assets", DIST / "assets", dirs_exist_ok=True)
    head = (DESIGN / "fx-head.html").read_text(encoding="utf-8")
    nav = (ROOT / "_nav.html").read_text(encoding="utf-8")
    foot = (ROOT / "_footer.html").read_text(encoding="utf-8")
    # Content version for cache busting (Cloudflare and browsers keep /assets for days).
    h = hashlib.sha256()
    for f in sorted(list(DESIGN.rglob("*")) + list(ROOT.glob("*.*")) + list((ROOT / "assets").rglob("*"))):
        if f.is_file() and "dist" not in f.parts:
            h.update(f.read_bytes())
    ver = h.hexdigest()[:10]
    asset_ref = re.compile(r'((?:href|src)="|srcset="|, )(/(?:assets/[^"\s,?]+|site\.css|site\.js|explorer\.js|market\.js|miner-check\.js))')
    langs = built_langs()
    by_file = {p["file"]: k for k, p in seo.PAGES.items()}
    en_js = i18n.js_strings(JS_FILES)
    report = []
    for page in sorted(ROOT.glob("*.html")):
        if page.name.startswith("_"):
            continue
        key = by_file.get(page.name)
        for lang in langs if key else ["en"]:
            html = assemble(page, key, lang, langs, head, nav, foot)
            if lang != "en":
                html = i18n.localize(html, lang, translations(lang), report, key, seo.PAGES)
            html = asset_ref.sub(lambda m: f"{m.group(1)}{m.group(2)}?v={ver}", html)
            html = html.replace('data-base="/assets/"', f'data-base="/assets/" data-v="{ver}"')
            out = DIST / (page.name if lang == "en" else f"{lang}/{page.name}")
            out.parent.mkdir(exist_ok=True)
            out.write_text(html, encoding="utf-8")
    (DIST / "assets" / "i18n").mkdir(exist_ok=True)
    for lang in langs[1:]:
        cat = json.dumps(i18n.js_catalog(lang, en_js), ensure_ascii=False, separators=(",", ":"))
        (DIST / "assets" / "i18n" / f"{lang}.js").write_text(f"GKI18N.add({cat});\n", encoding="utf-8")
    if report:
        missing = sum(1 for r in report if r[3] == "missing")
        print(f"translations: {missing} missing, {len(report) - missing} refused (English shown); details: python website/i18n_check.py")
    for f in ["site.css", "site.js", "explorer.js", "market.js", "miner-check.js"]:
        shutil.copy(ROOT / f, DIST / f)
    # The project's signed list of verified Krypton coins (wallets check the signature).
    for f in ["verified-assets.json", "verified-assets.json.sig"]:
        shutil.copy(ROOT.parent / "docs" / f, DIST / f)
    # The release key, so the download page can be checked without GitHub.
    shutil.copy(KEY, DIST / "release-key.pub")
    (DIST / "sitemap.xml").write_text(seo.sitemap(langs), encoding="utf-8")
    (DIST / "robots.txt").write_text(seo.ROBOTS, encoding="utf-8")
    (DIST / "site.webmanifest").write_text(seo.MANIFEST, encoding="utf-8")
    # For language models: the summary, and the full README with absolute links.
    shutil.copy(ROOT / "llms.txt", DIST / "llms.txt")
    (DIST / "llms-full.txt").write_text(llms_full(), encoding="utf-8")
    (DIST / "updates.xml").write_text(updates.atom(), encoding="utf-8")
    changelog = ROOT.parent / "CHANGELOG.md"
    text = updates.markdown().encode("utf-8")
    if not changelog.exists() or changelog.read_bytes() != text:
        changelog.write_bytes(text)
    if not updates.has(seo.VERSION):
        print(f"WARNING: website/updates.py has no entry for {seo.VERSION}")
    (DIST / ".well-known").mkdir()
    (DIST / ".well-known" / "security.txt").write_text(seo.SECURITY_TXT, encoding="utf-8")
    (DIST / f"{seo.INDEXNOW_KEY}.txt").write_text(seo.INDEXNOW_KEY, encoding="utf-8")
    if (ROOT / "assets" / "favicon.ico").exists():
        shutil.copy(ROOT / "assets" / "favicon.ico", DIST / "favicon.ico")
    print("built", DIST)


if __name__ == "__main__":
    main()
