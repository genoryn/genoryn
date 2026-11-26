# Design system

One token file, `design/assets/tokens.css`, styles the website (`website/`), the desktop
wallet (`apps/krypton-wallet/src/ui.html`) and Genoryn Talks. `fx.css` holds the surfaces,
the page background, the controls, the charts' styles and the search panel; `fx.js` draws the
background layer, toggles the theme, reveals sections on scroll and moves a soft light with
the cursor; `cmdk.js` is the search panel (Ctrl+K); `fx-head.html` picks the theme and the
effect level before first paint.

## Obsidian (1.0.55)

The design is called Obsidian: **solid layers divided by hairlines**, in the manner of
Linear, Vercel and Raycast. The palette did not change; what changed is how it is used.

* **Layers.** The page is black. A card is an opaque layer one step lighter
  (`--bg-card #0a0a0a`) with a 1 px hairline (`--line`); what sits inside a card is a tint
  (`--bg-inset`, `--bg-raised`); what floats over the page (menus, dialogs, tips, the search
  panel) is the lightest layer (`--bg-pop`) with a shadow (`--shadow-pop`). Nothing is
  translucent glass any more, so text never sits on a moving background.
* **Corners.** 6 px for keys and chips, 8 for controls, 12 for cards, 16 for panels, 20 for
  the few large bands (`--radius-xs` to `--radius-xl`). Pills stay round.
* **Controls.** One height (`--ctl`, 40 px; 44 on touch screens): buttons, fields and
  segmented switches line up. A primary button is the theme's ink (white on black, black on
  white).
* **Background.** A grey mesh at the top of the page (`--mesh`), a faint grid over it
  (`--grid-line`) and a little grain, all drawn by CSS. No background image is loaded.
* **Type.** Headlines in Space Grotesk, tight (-0.03 to -0.045 em), running from white to
  grey (`--ink-gradient`); numbers large; labels in sentence case, not in capitals.
* **Light.** On pointer devices a soft light follows the cursor across a card and along its
  edge (`.glass` reads `--mx` and `--my`, set by `fx.js` once a frame for the card under the
  pointer only).
* **Search.** Ctrl+K (Cmd+K on a Mac) opens a panel. On the website it finds a page by a few
  letters and opens a block, a transaction or an address in the explorer. In the wallet it
  only moves between the tabs and runs what the top bar's buttons run; it never sends, swaps
  or signs, and it stays shut while the wallet is locked. What is typed stays in the page.

## Mode: monochrome (the GENORYN brand)

Surfaces and chrome are true greys (R = G = B). Dark: `--bg-app #000`, cards `#0a0a0a`,
floats `#101010`, lines `rgba(255,255,255,.08)`, text `#fff / #a3a3a3 / #7a7a7a`. Light
mirrors it (page `#fafafa`, cards `#fff`, text `#000 / #575757 / #7a7a7a`).
Fonts: Space Grotesk (display), Inter (body), JetBrains Mono (numbers), self-hosted, OFL.

## Preserved colours (exact values)

| Token | Value | Meaning |
|---|---|---|
| `--status-success` | `#34d399` (line `#164e3b`) | verified, mining, incoming funds |
| `--status-warning` | `#fbbf24` (line `#5a4512`) | syncing, pending, warnings |
| `--status-error` | `#f87171` | wallet errors, outgoing amounts |
| `--status-error-site` | `#e57373` | website errors (v1 colour) |
| `--danger-bg` / `--danger-text` | `#3a1520` / `#fecaca` | destructive buttons |
| `--brand-tile` / `--brand-ink` | `#0b0b0b` / `#f2f2f2` | GENORYN mark |
| `--brand-tile-line` | `#2a2a2a` (light `#d6d6d6`) | mark border |
| `--brand-gray` | `#9a9a9a` | brand secondary grey |

No hue or lightness was changed. On light surfaces, status colours sit on a dark backing pill
(`--status-backing #141414`) instead of being darkened.

## Charts

`charts.js` draws line, area and bar charts as SVG, without a library. Since 1.0.55 a chart
can keep a scale it is given (`range`): a value far outside it is drawn at the edge, hollow
and dashed, and the tip still shows its real figure. A point may be marked soft (small and
grey; the line passes it by), a bar may be an empty slot (a day with nothing in it), a
missing value breaks the line, the time axis may cover a period wider than the data
(`span`), and a few events may be set out one after another instead of by their time
(`ordinal`). The trade history uses all of these.

## The author's portrait

`website/assets/author/` holds an illustration of Zarazi Sokani: a figure in a black hood,
the face lost in shadow, only a beard showing. It is a generated illustration, strictly
greyscale, and shows no real person. Sizes: the portrait (864 and 480 px wide, and a JPEG)
and a square around the hood for small uses (320, 128 and 64 px), lifted a little so that it
reads at 24 to 40 px.

## Performance rules

* `backdrop-filter` only on layers over moving content (the header once the page scrolls,
  the veil behind a dialog and behind the search panel).
* Weak devices (≤ 2 cores, ≤ 2 GB RAM, Save-Data, reduced transparency) get `data-fx="low"`:
  no blur, opaque layers, no moving light, a still background. Override with
  `localStorage.gk-fx`.
* Only opacity and transform animate. The background is CSS: nothing is fetched for it, and
  with reduced motion it does not move.

## Verification

* Every page of the website and every tab of the wallet, in 10 languages, at phone and
  desktop widths, dark and light: no horizontal overflow, no clipped or overlapping text
  (`site_i18n_visual`, `wallet_i18n_visual`, `nav_fit`, `nav_collide`).
* Computed colour of status text: `rgb(52, 211, 153)` in every theme and width.
* Testimonials were replaced by protocol principles: the project has no real endorsements
  yet and will not invent any.
