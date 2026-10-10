import { For } from 'solid-js';
import { GSign } from '~/components/Logo';
import { Icon } from '~/components/Icon';
import { PageHero, SectionHead } from '~/components/ui';

const COLORS: Array<[string, string, string]> = [
  ['Tile black', '#0B0B0B', 'Primary dark surface, the mark tile'],
  ['Ink white', '#F2F2F2', 'Sign on dark, light tile'],
  ['Secondary gray', '#9A9A9A', 'Secondary text and details'],
  ['Tile border (dark)', '#2A2A2A', 'Borders on dark surfaces'],
  ['Tile border (light)', '#D6D6D6', 'Borders on light surfaces'],
];

const FILES: Array<[string, string]> = [
  ['genoryn-mark-dark.svg', 'Primary mark — white sign on black tile'],
  ['genoryn-mark-light.svg', 'Black sign on light tile'],
  ['genoryn-sign-white.svg', 'Sign alone, for dark backgrounds'],
  ['genoryn-sign-black.svg', 'Sign alone, for light backgrounds'],
  ['genoryn-wordmark-white.svg', 'Sign + wordmark, dark headers'],
  ['genoryn-wordmark-black.svg', 'Sign + wordmark, light headers'],
  ['genoryn-banner.svg', 'Social banner 1280 × 320'],
  ['genoryn-mark-dark-1024.png', 'Raster mark 1024 px'],
  ['genoryn-mark-light-1024.png', 'Raster light mark 1024 px'],
  ['genoryn-sign-white-1024.png', 'Transparent sign, white'],
  ['genoryn-sign-black-1024.png', 'Transparent sign, black'],
  ['genoryn.ico', 'Windows icon 16–256 px'],
];

export default function Brand() {
  return (
    <>
      <PageHero eyebrow="Brand" title={<>The sign of finite money.</>} lead="A bold geometric G crossed by two short vertical strokes — a currency sign in the tradition of ₿. It stands for GNR, a supply that is finite by protocol. The identity is strictly monochrome." />
      <section class="section tight">
        <div class="container">
          <div class="grid cols-2">
            <div class="logo-tile dark"><GSign size={150} /></div>
            <div class="logo-tile light"><GSign size={150} /></div>
          </div>
          <SectionHead eyebrow="Palette" title={<>Black, white and gray.</>} />
          <div class="grid cols-4" style={{ 'grid-template-columns': 'repeat(auto-fit, minmax(180px, 1fr))' }}>
            <For each={COLORS}>
              {([name, hex, use]) => (
                <div class="swatch">
                  <div class="chip" style={{ background: hex }} />
                  <div class="meta">
                    <b>{name}</b>
                    <code>{hex}</code>
                    <div class="muted" style={{ 'font-size': '12.5px', 'margin-top': '4px' }}>{use}</div>
                  </div>
                </div>
              )}
            </For>
          </div>
          <SectionHead eyebrow="Typography (genoryn.io)" title={<>Four voices.</>} />
          <div class="grid cols-2">
            <div class="card"><div style={{ 'font-family': 'var(--font-display)', 'font-size': '34px', 'font-weight': 700, 'text-transform': 'uppercase', 'letter-spacing': '-0.03em' }}>Unbounded</div><p class="muted">Display — hero statements and section titles.</p></div>
            <div class="card"><div style={{ 'font-family': 'var(--font-sans)', 'font-size': '34px', 'font-weight': 600 }}>Inter</div><p class="muted">Interface and body text — the wallet's own typeface.</p></div>
            <div class="card"><div style={{ 'font-family': 'var(--font-mono)', 'font-size': '30px' }}>JetBrains Mono</div><p class="muted">Hashes, amounts, heights and code.</p></div>
            <div class="card"><div style={{ 'font-family': 'var(--font-serif)', 'font-size': '38px' }}>Instrument <em>Serif</em></div><p class="muted">Editorial pages: Learn, Protocol, Security, About.</p></div>
          </div>
        </div>
      </section>
      <section class="section">
        <div class="container">
          <SectionHead eyebrow="Downloads" title={<>Assets</>} lead="Released with the project under MIT OR Apache-2.0. Using them must not suggest an official endorsement of a fork, exchange listing or token sale." />
          <div class="table-wrap">
            <table class="data">
              <tbody>
                <For each={FILES}>
                  {([file, desc]) => (
                    <tr>
                      <td class="mono strong" style={{ 'font-size': '13px' }}>{file}</td>
                      <td class="muted">{desc}</td>
                      <td class="num">
                        <a class="btn sm" href={`/brand/${file}`} download="">
                          <Icon name="download" size={14} />
                        </a>
                      </td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </div>
          <div class="grid cols-2 mt-4">
            <div class="card">
              <h3>Do</h3>
              <ul class="muted" style={{ 'padding-left': '18px', 'line-height': 1.9 }}>
                <li>Keep clear space of at least a quarter of the mark's width on every side.</li>
                <li>Use the tile below 24 px; the bare sign needs at least 12 px.</li>
                <li>Place the white sign only on dark backgrounds and the black sign only on light ones.</li>
                <li>Write the unit after amounts: the sign, then <code>142.57 GNR</code>.</li>
              </ul>
            </div>
            <div class="card">
              <h3>Don't</h3>
              <ul class="muted" style={{ 'padding-left': '18px', 'line-height': 1.9 }}>
                <li>Recolor the sign — no gradients, no brand colors.</li>
                <li>Outline, rotate, stretch or add effects to the sign.</li>
                <li>Imply endorsement of forks, listings or sales.</li>
              </ul>
            </div>
          </div>
        </div>
      </section>
    </>
  );
}
