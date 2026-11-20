# Translating genoryn.io and the wallet

The site is written in English; every other language is built from translation catalogs.
Languages: de, fr, es, pt (Brazilian), ru, tr, zh-cn (Simplified Chinese), ja, ko.

## Files

- `website/locales/en/<name>.json`: the English texts, one file per page (`common` is the
  navigation and footer, `seo` the search-engine texts, `js` the texts the scripts write).
  **Generated** by `python website/i18n.py extract` after `python website/build.py`. Never edit.
- `website/locales/<lang>/<name>.json`: the same keys with their translations, plus
  `"_review": "machine"` (machine translation, not yet checked by a person) or `"human"`.

```json
{
 "_review": "machine",
 "1f0c2a9b3d": "Starte das Mining",
 "77e01ac2f4": "Jede Überweisung ist <0>abgeschirmt</0>: <1/> zeigt nur …"
}
```

A key is a hash of the English text: when the English changes, the key changes, and the old
translation stops being used until someone translates the new text.

## Rules

1. **Placeholders.** `<0>…</0>` is a piece of markup with text inside (a link, bold text);
   `<3/>` is one without text (code, an icon, a line break). Keep every placeholder of the
   English text exactly once, with the same number; you may move them where your language
   needs them. Never add other HTML, and never write `<` or `>` yourself.
2. **Values.** In `js.json`, `{n}`, `{h}` and similar are replaced by numbers or words: keep each
   one. A plural is `{"one": "…", "other": "…"}` in English; give the forms your language uses
   (`one`, `few`, `many`, `other`, see [Unicode plural rules](https://www.unicode.org/cldr/charts/latest/supplemental/language_plural_rules.html));
   Chinese, Japanese and Korean need only `other`.
3. **Entities.** Keep `&amp;`, `&lt;`, `&gt;`, `&#x27;`, `&nbsp;` as they are.
4. **Never translate** what [the glossary](glossary.md) lists: names, cryptography, units,
   anything that is code, addresses, commands, recovery-phrase words. Write numbers as the
   English does (`1,000`, `0.5`); the build puts them in your language's style.
5. **Terms** come from [the glossary](glossary.md), the same in every file.
6. **Safety and legal texts** (recovery phrase, passwords, scams, "not investment advice") are
   translated exactly: never shortened, softened or left out.
7. Short labels (buttons, menus, table headers) stay short: the layout is the same in every language.

## Check

```sh
python website/build.py
python website/i18n_check.py --lang de          # errors must be 0
python website/i18n_check.py --lang de --warnings
```

Then open `website/dist/de/index.html` through any static server (`/de/...` paths).
