"""Translation catalogs of the desktop wallet (apps/krypton-wallet).

    python website/i18n_wallet.py extract   locales/en from ui.html and the Rust sources
    python website/i18n_wallet.py build     locales/build/<lang>.json, embedded in the wallet

The wallet translates its own page in the browser (design/assets/i18n.js): the page's blocks,
cut as website/i18n.py cuts the site's pages (in DOM mode), the texts its script writes (T()),
and the messages of the Rust code, matched as templates ("share at height {1}"). The English
catalogs and the translations are keyed like the site's; a text the site already translated
is used as it is.
"""
import html
import json
import pathlib
import re
import sys

import i18n

ROOT = pathlib.Path(__file__).resolve().parent.parent
WALLET = ROOT / "apps" / "krypton-wallet"
UI = WALLET / "src" / "ui.html"
LOCALES = WALLET / "locales"
SHARED_JS = [ROOT / "design" / "assets" / "value.js", ROOT / "design" / "assets" / "charts.js"]
RUST = (sorted((WALLET / "src").rglob("*.rs")) + sorted((ROOT / "crates" / "krypton-swap" / "src").rglob("*.rs"))
        + sorted((ROOT / "crates" / "krypton-wallet-core" / "src").rglob("*.rs")))
# Where a Rust string reaches the screen: messages, statuses and errors.
RUST_SITES = re.compile(r'(?:\bsay\(|\bbail!\(|\banyhow!\(|\bensure!\([^;]*?,|\.context\(|\.with_context\(\|\|\s*|\b(?:status|note|last|message|error|why|text)\s*[:=]\s*(?:format!\()?|\bErr\()\s*(?:format!\(\s*)?"((?:[^"\\]|\\.)*)"', re.S)
RUST_PH = re.compile(r"\{\{|\}\}|\{[^{}]*\}")
# Messages chosen in a branch: `why = if .. { "..".into() } else ..` and `match side { A => "..", }`.
RUST_MORE = re.compile(r'"((?:[^"\\]|\\.){12,})"\s*\.into\(\)|=>\s*"((?:[^"\\]|\\.){12,})"')


def rust_template(lit):
    """A Rust literal as a template: its {} / {name} / {:?} become {1}, {2}, ... in order."""
    s = lit.encode().decode("unicode_escape") if "\\" in lit else lit
    n = [0]

    def ph(m):
        t = m.group(0)
        if t in ("{{", "}}"):
            return t[0]
        n[0] += 1
        return "{" + str(n[0]) + "}"
    return RUST_PH.sub(ph, s)


def rust_strings():
    out = {}
    for f in RUST:
        if "tests" in f.parts or f.name.endswith("_test.rs"):
            continue
        text = f.read_text(encoding="utf-8")
        text = re.sub(r"#\[cfg\(test\)\]\s*mod tests\s*\{.*", "", text, flags=re.S)
        found = [(m.group(1), 2) for m in RUST_SITES.finditer(text)]
        found += [(m.group(1) or m.group(2), 4) for m in RUST_MORE.finditer(text)]
        for lit, least in found:
            t = rust_template(lit).strip()
            words = re.findall(r"[A-Za-z]{2,}", t)
            if len(words) < least or len(t) > 600 or re.search(r"::|https?:|/v1/|\.rs\b|^\{\d\}$|[=<>]{2}|_[a-z]+_|charset=", t):
                continue
            out.setdefault(i18n.key(t), t)
    return out


def ui_parts():
    """The page's blocks and readable attributes, as the browser will see them."""
    doc = UI.read_text(encoding="utf-8")
    body = re.search(r"<body\b[^>]*>(.*)</body>", doc, re.S).group(1)
    i18n.DOM = True
    try:
        root, segs, attrs = i18n.segments(body)
    finally:
        i18n.DOM = False
    texts = {s.key: s.src for s in segs}
    attr = {}
    for _, _, _, _, v in attrs:
        v = i18n.norm(html.unescape(v))
        attr[i18n.key(v)] = v
    return texts, attr, doc


def extract():
    texts, attr, doc = ui_parts()
    script = "\n".join(re.findall(r"<script>(.*?)</script>", doc, re.S))
    tmp = WALLET / "src" / ".ui-script.js"
    tmp.write_text(script, encoding="utf-8")
    try:
        js = i18n.js_strings([tmp] + SHARED_JS)
    finally:
        tmp.unlink()
    files = {"ui": {**texts, **{k: v for k, v in attr.items() if k not in texts}}, "js": js, "rust": rust_strings()}
    out = LOCALES / "en"
    out.mkdir(parents=True, exist_ok=True)
    for name, cat in files.items():
        (out / f"{name}.json").write_text(json.dumps(cat, ensure_ascii=False, indent=1) + "\n", encoding="utf-8", newline="\n")
    (out / "_attrs.json").write_text(json.dumps(sorted(attr), indent=0) + "\n", encoding="utf-8", newline="\n")
    for name, cat in files.items():
        words = sum(len(re.sub(r"<[^>]+>", " ", v if isinstance(v, str) else " ".join(v.values())).split()) for v in cat.values())
        print(f"{name:6} {len(cat):5} texts {words:7} words")


def load(lang):
    """The wallet's translations of a language, then the site's for the same English texts."""
    d = {}
    site = i18n.LOCALES / lang
    for f in sorted(site.glob("*.json")) if site.is_dir() else []:
        if not f.stem.startswith("_"):
            d.update({k: v for k, v in json.loads(f.read_text(encoding="utf-8-sig")).items() if not k.startswith("_")})
    own = LOCALES / lang
    for f in sorted(own.glob("*.json")) if own.is_dir() else []:
        if not f.stem.startswith("_"):
            d.update({k: v for k, v in json.loads(f.read_text(encoding="utf-8-sig")).items() if not k.startswith("_")})
    return d


def build():
    en = {n: json.loads((LOCALES / "en" / f"{n}.json").read_text(encoding="utf-8")) for n in ("ui", "js", "rust")}
    attrs = set(json.loads((LOCALES / "en" / "_attrs.json").read_text(encoding="utf-8")))
    out_dir = LOCALES / "build"
    out_dir.mkdir(exist_ok=True)
    for lang in i18n.OTHER:
        tr = load(lang)
        dom, js, rust, missing = {}, {}, [], 0
        for k, src in en["ui"].items():
            t = tr.get(k)
            ok = isinstance(t, str) and not (i18n.check_attr(src, t) if k in attrs else i18n.check_translation(src, t))
            if ok:
                dom[src] = i18n.local_numbers(t, lang)
            else:
                missing += 1
        for k, src in en["js"].items():
            t = tr.get(k)
            if t is not None and not i18n.check_js(src, t):
                t = {f: i18n.local_numbers(s, lang) for f, s in t.items()} if isinstance(t, dict) else i18n.local_numbers(t, lang)
                js[src["one"] + "|" + src["other"] if isinstance(src, dict) else src] = t
            else:
                missing += 1
        for k, src in en["rust"].items():
            t = tr.get(k)
            if isinstance(t, str) and not i18n.check_js(src, t):
                rust.append([src, i18n.local_numbers(t, lang)])
            else:
                missing += 1
        rust.sort(key=lambda p: -len(p[0]))
        data = {"lang": i18n.INFO[lang]["lang"], "dom": dom, "js": js, "rust": rust}
        (out_dir / f"{lang}.json").write_text(json.dumps(data, ensure_ascii=False, separators=(",", ":")) + "\n", encoding="utf-8", newline="\n")
        print(f"{lang:6} {len(dom) + len(js) + len(rust)} texts, {missing} still in English")


if __name__ == "__main__":
    {"extract": extract, "build": build}.get(sys.argv[1] if len(sys.argv) > 1 else "", lambda: print(__doc__))()
