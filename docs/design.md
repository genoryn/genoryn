# Design system

One token file, `design/assets/tokens.css`, styles both the website (`website/`) and the
desktop wallet (`apps/krypton-wallet/src/ui.html`). `fx.css` holds glass surfaces, ambient
background, neon accents and motion; `fx.js` runs the background cross-fade, theme toggle
and scroll reveal; `fx-head.html` picks the theme and the effect level before first paint.

## Mode: monochrome (the GENORYN brand)

Surfaces and chrome are true greys (R = G = B). Dark: `--bg-app #000`, cards
`rgba(255,255,255,.034)`, floats `rgba(0,0,0,.74)`, lines `rgba(255,255,255,.085)`, text
`#fff / #a3a3a3 / #7a7a7a`. Light mirrors it (cards `rgba(255,255,255,.6)`, text `#000 / #575757 / #7a7a7a`).
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

No hue or lightness was changed. On light glass, status colours sit on a dark backing pill
(`--status-backing #141414`) instead of being darkened.

## Performance rules

* `backdrop-filter` only on floating layers (navbar, toast, drawer, modal backdrop).
* Weak devices (≤ 2 cores, ≤ 2 GB RAM, Save-Data, reduced transparency) get `data-fx="low"`:
  no blur, opaque floats, no cross-fades, no drift. Override with `localStorage.gk-fx`.
* Only opacity and transform animate; background images are 1280×720 WebP under 10 KB, never
  inlined into JavaScript; the background never changes while the tab is hidden, during a
  transfer proof or miner start, or with reduced motion.

## Verification (latest run)

* 72 views (6 site routes + 5 wallet tabs × 390/768/1440 px × dark/light): zero horizontal overflow.
* Computed colour of status text: `rgb(52, 211, 153)` in every theme and width.
* Lighthouse mobile, landing: Performance 100, Accessibility 100, Best Practices 100, SEO 100
  (LCP 1.2 s, CLS 0.003, TBT 0 ms).
* Testimonials were replaced by protocol principles: the project has no real endorsements
  yet and will not invent any.
