"""Translation of the built site: segments, catalogs and localized pages.

A page is cut into segments: the text of each block (paragraph, heading, list item, cell,
button...) with its inline markup replaced by numbered placeholders, `<0>text</0>` for an
element with text and `<1/>` for one without (code, icons, line breaks). Translators only see
those; links, attributes and code always come from the English page, and a translation with
any other markup is refused (the English text is used instead). Attributes people read
(title, alt, placeholder, aria-label, data-label, meta descriptions) are segments too.

Catalogs: locales/en/<name>.json maps a key (a hash of the English text) to the English text;
locales/<lang>/<name>.json maps the same keys to translations. A changed English text gets a
new key, so it shows as missing, never as a stale translation.

    python i18n.py extract      rebuilds locales/en from website/dist (run build.py first)
"""
import hashlib
import html
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent
LOCALES = ROOT / "locales"

# path code, native name, html lang / hreflang, og:locale, Intl locale
LANGS = [
    ("en", "English", "en", "en_US", "en-US"),
    ("de", "Deutsch", "de", "de_DE", "de-DE"),
    ("fr", "Français", "fr", "fr_FR", "fr-FR"),
    ("es", "Español", "es", "es_ES", "es-ES"),
    ("pt", "Português", "pt", "pt_BR", "pt-BR"),
    ("ru", "Русский", "ru", "ru_RU", "ru-RU"),
    ("tr", "Türkçe", "tr", "tr_TR", "tr-TR"),
    ("zh-cn", "简体中文", "zh-CN", "zh_CN", "zh-CN"),
    ("ja", "日本語", "ja", "ja_JP", "ja-JP"),
    ("ko", "한국어", "ko", "ko_KR", "ko-KR"),
]
CODES = [c for c, *_ in LANGS]
OTHER = CODES[1:]
INFO = {c: {"name": n, "lang": l, "og": o, "intl": i} for c, n, l, o, i in LANGS}

INLINE = {"a", "abbr", "b", "bdi", "br", "cite", "code", "data", "dfn", "em", "i", "img", "kbd", "mark", "q",
          "s", "samp", "small", "span", "strong", "sub", "sup", "time", "u", "var", "wbr"}
OPAQUE = {"br", "code", "img", "kbd", "samp", "var", "wbr"}
VOID = {"area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"}
RAW = re.compile(r"<!--.*?-->|<(script|style|svg|pre|textarea|template)\b[^>]*>.*?</\1\s*>", re.S | re.I)
TOKEN = re.compile(r"<!--.*?-->|<![^>]*>|<(script|style|svg|pre|textarea|template)\b[^>]*>.*?</\1\s*>|</?[a-zA-Z][^>]*>|[^<]+|<", re.S | re.I)
ATTR = re.compile(r'''(\s)([a-zA-Z_:][-a-zA-Z0-9_:.]*)(\s*=\s*)("[^"]*"|'[^']*')''')
READ_ATTRS = {"title", "alt", "placeholder", "aria-label", "data-label"}
META_READ = re.compile(r'''(?:name|property)\s*=\s*["'](?:description|og:title|og:description|og:image:alt|twitter:title|twitter:description|twitter:image:alt)["']''', re.I)
LETTERS = re.compile(r"[^\W\d_]{2,}")
PH = re.compile(r"<(\d+)>|</(\d+)>|<(\d+)/>")
LD_KEYS = {"name", "headline", "description", "text", "featureList", "applicationSubCategory"}
NOTE = "Machine translation. Corrections from people who speak the language are welcome:"
NOTE_LINK = "help translate"


def key(src):
    return hashlib.sha256(src.encode("utf-8")).hexdigest()[:10]


def norm(s):
    return re.sub(r"\s+", " ", s).strip()


class Node:
    __slots__ = ("kind", "tag", "start", "end", "open_end", "close_start", "children", "raw")

    def __init__(self, kind, tag="", start=0, end=0, raw=""):
        self.kind, self.tag, self.start, self.end, self.raw = kind, tag, start, end, raw
        self.open_end = end
        self.close_start = end
        self.children = []


# DOM mode (the wallet translates its page in the browser): texts as the DOM holds them,
# entities decoded and comments left out, so design/assets/i18n.js finds the same texts.
DOM = False


def parse(doc):
    root = Node("el", "#root", 0, len(doc))
    stack = [root]
    for m in TOKEN.finditer(doc):
        s, e, tok = m.start(), m.end(), m.group(0)
        if DOM and tok.startswith("<!"):
            continue
        if tok.startswith("<!") or m.group(1):
            stack[-1].children.append(Node("raw", (m.group(1) or "!").lower(), s, e, tok))
        elif tok.startswith("</"):
            name = tok[2:].split(">")[0].strip().lower()
            for i in range(len(stack) - 1, 0, -1):
                if stack[i].tag == name:
                    for n in stack[i:]:
                        n.close_start, n.end = s, e
                    del stack[i:]
                    break
        elif re.match(r"<[a-zA-Z]", tok):
            name = re.match(r"<([a-zA-Z][-a-zA-Z0-9]*)", tok).group(1).lower()
            n = Node("el", name, s, e, tok)
            n.open_end = e
            stack[-1].children.append(n)
            if name not in VOID and not tok.endswith("/>"):
                stack.append(n)
        else:
            stack[-1].children.append(Node("text", "", s, e, tok))
    for n in stack[1:]:
        n.close_start = n.end = len(doc)
    return root


def text_of(n):
    if n.kind == "text":
        return n.raw
    if n.kind == "raw":
        return ""
    return "".join(text_of(c) for c in n.children)


def no_translate(n):
    t = n.raw
    return bool(re.search(r'''translate\s*=\s*["']no["']''', t) or re.search(r'''class\s*=\s*["'][^"']*\b(mono|num|notranslate)\b''', t))


def opaque(n):
    return n.kind == "raw" or n.tag in OPAQUE or no_translate(n) or not LETTERS.search(html.unescape(text_of(n)))


def inline(n):
    return n.kind in ("text", "raw") or n.tag in INLINE


class Segment:
    def __init__(self, nodes):
        self.nodes = nodes
        self.start, self.end = nodes[0].start, nodes[-1].end
        self.parts = {}
        k = [0]

        def walk(ns):
            out = []
            for n in ns:
                if n.kind == "text":
                    out.append(html.escape(html.unescape(n.raw), quote=False) if DOM else n.raw)
                elif opaque(n):
                    i = k[0]
                    k[0] += 1
                    self.parts[i] = ("opaque", n)
                    out.append(f"<{i}/>")
                else:
                    i = k[0]
                    k[0] += 1
                    self.parts[i] = ("pair", n)
                    out.append(f"<{i}>" + walk(n.children) + f"</{i}>")
            return "".join(out)

        self.src = norm(walk(nodes))
        self.key = key(self.src)


def runs(container, out):
    if container.kind != "el" or no_translate(container) or container.tag in ("head", "#raw"):
        if container.tag == "head":
            for c in container.children:
                if c.kind == "el" and c.tag == "title":
                    runs(c, out)
        return
    cur = []

    def flush():
        ns = list(cur)
        cur.clear()
        while ns and (ns[0].kind == "text" and not ns[0].raw.strip() or ns[0].kind != "text" and opaque(ns[0])):
            ns.pop(0)
        while ns and (ns[-1].kind == "text" and not ns[-1].raw.strip() or ns[-1].kind != "text" and opaque(ns[-1])):
            ns.pop()
        if not ns or not LETTERS.search(html.unescape("".join(text_of(n) for n in ns if not (n.kind != "text" and opaque(n))))):
            return
        bare = any(n.kind == "text" and n.raw.strip() for n in ns)
        if not bare:
            # Only elements: each is a container of its own (menus, stat boxes, a link alone).
            for n in ns:
                if n.kind == "el" and not opaque(n):
                    runs(n, out)
            return
        out.append(Segment(ns))

    for c in container.children:
        if inline(c):
            cur.append(c)
        else:
            flush()
            runs(c, out)
    flush()


def attr_segments(root, out):
    """(tag node, attribute name, value span start, end, raw value) for every readable attribute."""
    def walk(n):
        for c in n.children:
            if c.kind != "el":
                continue
            for m in ATTR.finditer(c.raw):
                name = m.group(2).lower()
                ok = name in READ_ATTRS or (c.tag == "meta" and name == "content" and META_READ.search(c.raw))
                v = m.group(4)[1:-1]
                if ok and LETTERS.search(html.unescape(v)) and not no_translate(c):
                    s = c.start + m.start(4) + 1
                    out.append((c, name, s, s + len(v), v))
            walk(c)
    walk(root)


def segments(doc):
    root = parse(doc)
    segs = []
    runs(root, segs)
    attrs = []
    attr_segments(root, attrs)
    return root, segs, attrs


def ld_strings(doc):
    out = []
    for m in re.finditer(r'<script type="application/ld\+json">(.*?)</script>', doc, re.S):
        def walk(v, k=None):
            if isinstance(v, dict):
                for kk, vv in v.items():
                    walk(vv, kk)
            elif isinstance(v, list):
                for x in v:
                    walk(x, k)
            elif isinstance(v, str) and k in LD_KEYS and LETTERS.search(v):
                out.append(v)
        walk(json.loads(m.group(1)))
    return out


# Thousands and decimal separators of the languages that do not write numbers as English does.
SEPARATORS = {"de": (".", ","), "es": (".", ","), "pt": (".", ","), "tr": (".", ","),
              "fr": ("\u202f", ","), "ru": ("\u00a0", ",")}
THOUSANDS = re.compile(r"(?<![\w.,])\d{1,3}(?:,\d{3})+(?![\w,]|\.\d)")
DECIMAL = re.compile(r"(?<![\w.,])\d+\.\d+(?![\w.])")


def local_numbers(text, lang):
    """1,000 and 0.5 as the language writes them (1.000 and 0,5 in German). Versions
    (1.0.46), addresses (127.0.0.1) and numbers glued to words (v1.5) stay as they are."""
    if lang not in SEPARATORS:
        return text
    group, dec = SEPARATORS[lang]
    text = DECIMAL.sub(lambda m: m.group(0).replace(".", "\0"), text)
    text = THOUSANDS.sub(lambda m: m.group(0).replace(",", group), text)
    return text.replace("\0", dec)


def plain(s):
    return html.unescape(re.sub(r"<[^>]+>", "", s)).strip()


# ---- catalogs -------------------------------------------------------------------------------

def load(lang):
    """All of a language's translations, one dict."""
    d = {}
    for f in sorted((LOCALES / lang).glob("*.json")):
        if f.stem == "js" or f.stem.startswith("_"):
            continue
        for k, v in json.loads(f.read_text(encoding="utf-8-sig")).items():
            if not k.startswith("_"):
                d[k] = v
    return d


def check_translation(src, tr):
    """None if `tr` may replace `src`, else why not."""
    if not isinstance(tr, str) or not tr.strip():
        return "empty"
    want = sorted(PH.findall(src))
    got = sorted(PH.findall(tr))
    if want != got:
        return "placeholders differ"
    rest = PH.sub("", tr)
    if "<" in rest or ">" in rest:
        return "markup outside placeholders"
    depth = []
    for m in PH.finditer(tr):
        if m.group(1):
            depth.append(m.group(1))
        elif m.group(2):
            if not depth or depth.pop() != m.group(2):
                return "placeholders not nested"
    return "placeholders not nested" if depth else None


def check_attr(src, tr):
    if not isinstance(tr, str) or not tr.strip():
        return "empty"
    if any(c in tr for c in '<>"'):
        return 'markup or " in an attribute'
    return None


# ---- localized pages ------------------------------------------------------------------------

def page_paths(pages):
    return {p["path"] for p in pages.values()}


def local_path(lang, path):
    if lang == "en":
        return path
    return f"/{lang}" + ("/" if path == "/" else path)


def rewrite_links(doc, lang, paths):
    """Links to pages of this site point to the same language; the language menu's do not."""
    def sub(m):
        href = m.group(2)
        base, sep, frag = href.partition("#")
        base, qs, q = base.partition("?")
        if base in paths:
            return m.group(1) + local_path(lang, base) + qs + q + sep + frag + m.group(3)
        return m.group(0)

    def tag(m):
        t = m.group(0)
        return t if "data-lang=" in t else re.sub(r'''(\bhref=")(/[^"]*)(")''', sub, t, count=1)
    return re.sub(r"<a\b[^>]*>", tag, doc)


def localize(doc, lang, tr, report, page_key, pages):
    """The English page `doc` in `lang`, using translations `tr` (key -> text)."""
    root, segs, attrs = segments(doc)
    tag_raw = {}
    reps = []
    seg_spans = [(s.start, s.end) for s in segs]

    def inside(pos):
        return any(a <= pos < b for a, b in seg_spans)

    by_tag = {}
    for node, name, s, e, v in attrs:
        t = tr.get(key(norm(v)))
        bad = check_attr(v, t) if t is not None else "missing"
        if bad:
            report.append((page_key, "attr", norm(v), bad))
            continue
        t = local_numbers(t, lang)
        by_tag.setdefault(node.start, []).append((s - node.start, e - node.start, t))
        if not inside(node.start):
            reps.append((s, e, t))
    for start, lst in by_tag.items():
        n = next(a[0] for a in attrs if a[0].start == start)
        raw = n.raw
        for s, e, t in sorted(lst, reverse=True):
            raw = raw[:s] + t + raw[e:]
        tag_raw[start] = raw

    def open_raw(n):
        return tag_raw.get(n.start, n.raw)

    def close_raw(n):
        return doc[n.close_start:n.end] if n.close_start < n.end else ""

    def render(seg, text):
        out = []
        pos = 0
        for m in PH.finditer(text):
            out.append(text[pos:m.start()])
            i = int(m.group(1) or m.group(2) or m.group(3))
            kind, n = seg.parts[i]
            if m.group(3):
                out.append(doc[n.start:n.end] if n.kind == "raw" or n.tag in VOID else open_raw(n) + doc[n.open_end:n.end])
            elif m.group(1):
                out.append(open_raw(n))
            else:
                out.append(close_raw(n))
            pos = m.end()
        out.append(text[pos:])
        return "".join(out)

    plain_map = {}
    for s in segs:
        t = tr.get(s.key)
        bad = check_translation(s.src, t) if t is not None else "missing"
        if bad:
            report.append((page_key, "text", s.src, bad))
            t = s.src
        else:
            t = local_numbers(t, lang)
        reps.append((s.start, s.end, render(s, t)))
        plain_map[plain(s.src)] = plain(render(s, t)) if not bad else None
    reps.sort(key=lambda r: r[0])
    out, pos = [], 0
    for s, e, t in reps:
        if s < pos:
            continue
        out.append(doc[pos:s])
        out.append(t)
        pos = e
    out.append(doc[pos:])
    doc = "".join(out)

    url = "https://genoryn.io"

    def ld(m):
        data = json.loads(m.group(1))

        def walk(v, k=None):
            if isinstance(v, dict):
                return {kk: walk(vv, kk) for kk, vv in v.items()}
            if isinstance(v, list):
                return [walk(x, k) for x in v]
            if isinstance(v, str):
                if k in LD_KEYS and LETTERS.search(v):
                    t = plain_map.get(v) or (local_numbers(tr[key(v)], lang) if isinstance(tr.get(key(v)), str) else None)
                    if t and not check_attr(v, t.replace('"', "'")):
                        return t
                    report.append((page_key, "ld", v, "missing"))
                    return v
                if k == "inLanguage":
                    return INFO[lang]["lang"]
                if k in ("item", "url", "mainEntityOfPage") and v.startswith(url + "/") and "#" not in v:
                    p = v[len(url):]
                    if p in page_paths(pages):
                        return url + local_path(lang, p)
            return v
        return '<script type="application/ld+json">' + json.dumps(walk(data), ensure_ascii=False, separators=(",", ":")) + "</script>"

    doc = re.sub(r'<script type="application/ld\+json">(.*?)</script>', ld, doc, flags=re.S)
    doc = re.sub(r"<html\b[^>]*>", lambda m: m.group(0).replace('lang="en"', f'lang="{INFO[lang]["lang"]}"'), doc, count=1)
    return rewrite_links(doc, lang, page_paths(pages))


# ---- extraction -----------------------------------------------------------------------------

def extract(dist, pages, snippets, js_strings):
    """locales/en/*.json from the built English pages."""
    common = {}
    attr_keys = set()
    for snip in snippets:
        _, segs, attrs = segments(snip)
        for s in segs:
            common[s.key] = s.src
        for _, _, _, _, v in attrs:
            common[key(norm(v))] = norm(v)
            attr_keys.add(key(norm(v)))
    files = {"common": dict(common), "seo": {}}
    seen = set(common)
    for pk, p in pages.items():
        doc = (dist / p["file"]).read_text(encoding="utf-8")
        _, segs, attrs = segments(doc)
        cat = {}
        found = set()
        for s in segs:
            found.add(plain(s.src))
            if s.key not in seen:
                cat[s.key] = s.src
                seen.add(s.key)
        for _, _, _, _, v in attrs:
            k = key(norm(v))
            found.add(plain(v))
            attr_keys.add(k)
            if k not in seen:
                cat[k] = norm(v)
                seen.add(k)
        for v in ld_strings(doc):
            if v not in found and key(v) not in seen:
                files["seo"][key(v)] = v
                seen.add(key(v))
        files[pk] = cat
    files["js"] = js_strings
    out = LOCALES / "en"
    out.mkdir(parents=True, exist_ok=True)
    for old in out.glob("*.json"):
        if old.stem not in files and not old.stem.startswith("_"):
            old.unlink()
    for name, cat in files.items():
        (out / f"{name}.json").write_text(json.dumps(cat, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    # Attribute texts (and JSON-LD strings) may not contain " < >; the checker needs to know which.
    attr_keys |= set(files["seo"])
    (out / "_attrs.json").write_text(json.dumps(sorted(attr_keys), indent=0) + "\n", encoding="utf-8")
    return files


STR = r'"((?:[^"\\]|\\.)*)"'
JS_T = re.compile(r"\bT[HE]?\(\s*" + STR)
JS_TN = re.compile(r"\bTn\([^,()]*(?:\([^()]*\)[^,()]*)*,\s*" + STR + r"\s*,\s*" + STR)
VARS = re.compile(r"\{\w+\}")


def js_strings(files):
    """Every T("...") and Tn(n, "one", "other") text of the scripts: a string, or {"one", "other"}."""
    out = {}
    for f in files:
        text = f.read_text(encoding="utf-8")
        for m in JS_T.finditer(text):
            s = json.loads('"' + m.group(1) + '"')
            out.setdefault(key(s), s)
        for m in JS_TN.finditer(text):
            one, other = (json.loads('"' + g + '"') for g in m.groups())
            out.setdefault(key(one + "|" + other), {"one": one, "other": other})
    return out


def check_js(src, tr):
    """None if a JavaScript translation may be used. Scripts put texts into HTML, so a
    translation may hold < > & only where the English does."""
    def markup(s, e):
        return any(c in s and c not in e for c in "<>&")
    if isinstance(src, dict):
        if not isinstance(tr, dict) or not tr.get("other"):
            return "a plural needs at least an \"other\" form"
        want = set(VARS.findall(src["other"])) | set(VARS.findall(src["one"]))
        for form, s in tr.items():
            if form not in ("zero", "one", "two", "few", "many", "other") or not isinstance(s, str):
                return f"unknown plural form {form}"
            if not set(VARS.findall(s)) <= want:
                return "unknown {value} in a plural"
            if markup(s, src["one"] + src["other"]):
                return "markup in a translation"
        return None
    if not isinstance(tr, str) or not tr.strip():
        return "empty"
    if sorted(VARS.findall(tr)) != sorted(VARS.findall(src)):
        return "{values} differ"
    if markup(tr, src):
        return "markup in a translation"
    return None


def js_catalog(lang, en_js):
    """The JavaScript catalog of a language: English text (or "one|other") -> translation."""
    f = LOCALES / lang / "js.json"
    tr = json.loads(f.read_text(encoding="utf-8-sig")) if f.exists() else {}
    out = {}
    for k, src in en_js.items():
        t = tr.get(k)
        if t is not None and not check_js(src, t):
            t = {f: local_numbers(s, lang) for f, s in t.items()} if isinstance(t, dict) else local_numbers(t, lang)
            out[src["one"] + "|" + src["other"] if isinstance(src, dict) else src] = t
    return out


if __name__ == "__main__":
    if sys.argv[1:] == ["extract"]:
        import build
        build.extract()
    else:
        print(__doc__)
