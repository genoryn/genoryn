"""Checks the translation catalogs (and, with --dist, the built localized pages).

    python website/i18n_check.py                     every language, every catalog
    python website/i18n_check.py --lang de --files docs faq
    python website/i18n_check.py --strict --dist     CI: missing translations are errors too

Errors: invalid JSON, keys that are not in English, placeholders or {values} that differ,
markup outside placeholders, names that must stay as written, broken localized pages or links.
Warnings: untranslated text, numbers that differ, unusually long translations.
"""
import argparse
import json
import pathlib
import re
import sys

import i18n

ROOT = pathlib.Path(__file__).resolve().parent
KEEP = ["Genoryn", "GNR", "KryptoHash", "zk-STARK", "ML-KEM", "Quick Swap", "Zarazi Sokani"]
REVIEW = re.compile(r"recovery phrase|password|never (share|type|enter|give)|scam|steal|investment advice|promise of value|private key", re.I)
CJK = {"zh-cn", "ja", "ko"}
NUM = re.compile(r"\d[\d,.]*\d|\d")


def strip_ph(s):
    return i18n.PH.sub("", s)


def check_lang(lang, names, strict, root=None, fallback=None):
    """`root`: another catalog tree (the wallet's); `fallback`: translations used when the tree
    has none (the site's, for the same English texts)."""
    errors, warnings, review = [], [], 0
    root = root or i18n.LOCALES
    fallback = fallback or {}
    en_dir, dir_ = root / "en", root / lang
    attrs = set(json.loads((en_dir / "_attrs.json").read_text(encoding="utf-8"))) if (en_dir / "_attrs.json").exists() else set()
    total = done = 0
    for f in sorted(en_dir.glob("*.json")):
        if f.stem.startswith("_") or (names and f.stem not in names):
            continue
        en = json.loads(f.read_text(encoding="utf-8"))
        tf = dir_ / f.name
        try:
            tr = json.loads(tf.read_text(encoding="utf-8-sig")) if tf.exists() else {}
        except json.JSONDecodeError as e:
            errors.append(f"{lang}/{f.name}: invalid JSON: {e}")
            continue
        if tf.exists() and tr.get("_review") not in ("machine", "human"):
            errors.append(f'{lang}/{f.name}: "_review" must be "machine" or "human"')
        for k in tr:
            if not k.startswith("_") and k not in en:
                errors.append(f"{lang}/{f.name}: key {k} is not in English (stale or invented)")
        for k, src in en.items():
            total += 1
            t = tr.get(k, fallback.get(k))
            where = f"{lang}/{f.name} {k}"
            if t is None:
                if strict:
                    errors.append(f"{where}: missing")
                continue
            done += 1
            if f.stem in ("js", "rust"):
                bad = i18n.check_js(src, t)
                if bad:
                    errors.append(f"{where}: {bad}")
                continue
            bad = i18n.check_attr(src, t) if k in attrs else i18n.check_translation(src, t)
            if bad:
                errors.append(f"{where}: {bad}: {t[:90]!r}")
                continue
            if any(ch in t for ch in "\ufffd\u0000") or re.search(r"[\u0001-\u0008\u000b\u000c\u000e-\u001f]", t):
                errors.append(f"{where}: control or replacement characters")
            for name in KEEP:
                if name in src and name not in t:
                    errors.append(f"{where}: {name!r} must stay as written")
            s_plain, t_plain = strip_ph(src), strip_ph(t)
            if t_plain == s_plain and len(re.findall(r"[A-Za-z]{3,}", s_plain)) >= 4:
                warnings.append(f"{where}: not translated: {s_plain[:70]!r}")
            nums = lambda s: sorted(n.replace(",", "").replace(".", "") for n in NUM.findall(re.sub(r"&#?\w+;", "", s)))
            if nums(s_plain) != nums(t_plain):
                warnings.append(f"{where}: numbers differ: {s_plain[:60]!r}")
            if lang not in CJK and len(t_plain) > 2.6 * len(s_plain) + 20:
                warnings.append(f"{where}: much longer than English")
            if REVIEW.search(s_plain):
                review += 1
    return total, done, errors, warnings, review


def check_dist(langs):
    import seo
    dist = ROOT / "dist"
    errors = []
    files = {p["file"] for p in seo.PAGES.values()}
    for lang in langs:
        for f in files:
            page = dist / lang / f
            if not page.exists():
                errors.append(f"dist/{lang}/{f}: missing (run build.py)")
                continue
            doc = page.read_text(encoding="utf-8")
            if not re.search(r'<html\b[^>]*\blang="' + re.escape(i18n.INFO[lang]["lang"]) + '"', doc):
                errors.append(f"dist/{lang}/{f}: wrong lang attribute")
            for href in re.findall(r'<a\b[^>]*\bhref="(/[^"#?]*)', doc):
                if href.startswith(f"/{lang}/"):
                    rest = href[len(lang) + 2:]
                    target = dist / lang / ((rest or "index") + ("" if rest.endswith(".html") else ".html"))
                    if not target.exists():
                        errors.append(f"dist/{lang}/{f}: link to {href} has no page")
    return errors


def maintain(a):
    """--prune, --todo and --merge, for the site's catalogs or (--wallet) the wallet's."""
    if a.wallet:
        import i18n_wallet
    root = i18n_wallet.LOCALES if a.wallet else i18n.LOCALES
    langs = a.lang or i18n.OTHER
    en = {f.stem: json.loads(f.read_text(encoding="utf-8")) for f in (root / "en").glob("*.json") if not f.stem.startswith("_")}
    for lang in langs:
        site = i18n_wallet.load(lang) if a.wallet else {}
        cats = {}
        for name in en:
            f = root / lang / f"{name}.json"
            cats[name] = json.loads(f.read_text(encoding="utf-8-sig")) if f.exists() else {"_review": "machine"}
        if a.merge:
            f = pathlib.Path(a.merge) / f"{lang}.json"
            if f.exists():
                done = json.loads(f.read_text(encoding="utf-8-sig"))
                for name, cat in done.items():
                    for k, v in cat.items():
                        if k in en.get(name, {}):
                            cats[name][k] = v
        if a.prune:
            for name, cat in cats.items():
                for k in [k for k in cat if not k.startswith("_") and k not in en[name]]:
                    del cat[k]
        todo = {}
        for name, cat in cats.items():
            miss = {k: v for k, v in en[name].items() if k not in cat and k not in site}
            if miss:
                todo[name] = miss
            # Keys in the English order, "_review" first.
            ordered = {"_review": cat.get("_review", "machine")}
            ordered.update({k: cat[k] for k in en[name] if k in cat})
            if len(ordered) > 1 or (root / lang / f"{name}.json").exists():
                (root / lang).mkdir(parents=True, exist_ok=True)
                (root / lang / f"{name}.json").write_text(json.dumps(ordered, ensure_ascii=False, indent=1) + "\n", encoding="utf-8", newline="\n")
        if a.todo:
            out = pathlib.Path(a.todo)
            out.mkdir(parents=True, exist_ok=True)
            (out / f"{lang}.json").write_text(json.dumps(todo, ensure_ascii=False, indent=1) + "\n", encoding="utf-8", newline="\n")
        print(f"{lang:6} {sum(len(v) for v in todo.values())} texts still to translate")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--lang", nargs="*")
    ap.add_argument("--files", nargs="*")
    ap.add_argument("--strict", action="store_true")
    ap.add_argument("--dist", action="store_true")
    ap.add_argument("--warnings", action="store_true", help="print every warning")
    ap.add_argument("--wallet", action="store_true", help="the wallet's catalogs (apps/krypton-wallet/locales)")
    ap.add_argument("--prune", action="store_true", help="remove keys the English no longer has")
    ap.add_argument("--todo", metavar="DIR", help="write <lang>.json with the English texts still missing")
    ap.add_argument("--merge", metavar="DIR", help="merge translated <lang>.json files (from --todo) into the catalogs")
    a = ap.parse_args()
    if a.prune or a.todo or a.merge:
        maintain(a)
        return
    langs = a.lang or [c for c in i18n.OTHER if (i18n.LOCALES / c).is_dir()]
    failed = False
    if a.wallet:
        import i18n_wallet
    for lang in langs:
        if a.wallet:
            site = {}
            for f in sorted((i18n.LOCALES / lang).glob("*.json")) if (i18n.LOCALES / lang).is_dir() else []:
                if not f.stem.startswith("_"):
                    site.update(json.loads(f.read_text(encoding="utf-8-sig")))
            total, done, errors, warnings, review = check_lang(lang, a.files, a.strict, i18n_wallet.LOCALES, site)
        else:
            total, done, errors, warnings, review = check_lang(lang, a.files, a.strict)
        print(f"{lang:6} {done}/{total} translated, {len(errors)} errors, {len(warnings)} warnings, {review} safety or legal texts to review by a person")
        for e in errors[:60]:
            print("  ERROR", e)
        if len(errors) > 60:
            print(f"  … {len(errors) - 60} more errors")
        for w in warnings if a.warnings else warnings[:8]:
            print("  warn ", w)
        failed |= bool(errors)
    if a.dist:
        errs = check_dist(langs)
        for e in errs[:40]:
            print("  ERROR", e)
        print(f"dist: {len(errs)} errors")
        failed |= bool(errs)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
