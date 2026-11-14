"""Builds the static genoryn.io site into website/dist (no npm, no bundler).

Inserts the shared head script, SEO metadata, navbar and footer into every page,
copies the shared design assets and writes sitemap.xml, robots.txt and the manifest.
Run: python website/build.py
"""
import hashlib
import pathlib
import re
import shutil

import seo

ROOT = pathlib.Path(__file__).resolve().parent
DESIGN = ROOT.parent / "design"
DIST = ROOT / "dist"


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
    asset_ref = re.compile(r'((?:href|src)="|srcset="|, )(/(?:assets/[^"\s,?]+|site\.css|site\.js|explorer\.js|market\.js))')
    for page in ROOT.glob("*.html"):
        if page.name.startswith("_"):
            continue
        html = page.read_text(encoding="utf-8")
        html = html.replace("<!--FX_HEAD-->", head).replace("<!--NAV-->", nav).replace("<!--FOOTER-->", foot)
        for key in seo.PAGES:
            html = html.replace(f"<!--SEO:{key}-->", seo.head(key))
        html = asset_ref.sub(lambda m: f"{m.group(1)}{m.group(2)}?v={ver}", html)
        html = html.replace('data-base="/assets/"', f'data-base="/assets/" data-v="{ver}"')
        (DIST / page.name).write_text(html, encoding="utf-8")
    for f in ["site.css", "site.js", "explorer.js", "market.js"]:
        shutil.copy(ROOT / f, DIST / f)
    (DIST / "sitemap.xml").write_text(seo.sitemap(), encoding="utf-8")
    (DIST / "robots.txt").write_text(seo.ROBOTS, encoding="utf-8")
    (DIST / "site.webmanifest").write_text(seo.MANIFEST, encoding="utf-8")
    # For language models: the summary, and the full README with absolute links.
    shutil.copy(ROOT / "llms.txt", DIST / "llms.txt")
    (DIST / "llms-full.txt").write_text(llms_full(), encoding="utf-8")
    (DIST / ".well-known").mkdir()
    (DIST / ".well-known" / "security.txt").write_text(seo.SECURITY_TXT, encoding="utf-8")
    (DIST / f"{seo.INDEXNOW_KEY}.txt").write_text(seo.INDEXNOW_KEY, encoding="utf-8")
    if (ROOT / "assets" / "favicon.ico").exists():
        shutil.copy(ROOT / "assets" / "favicon.ico", DIST / "favicon.ico")
    print("built", DIST)


if __name__ == "__main__":
    main()
