import { createEffect, createSignal, For, on, onCleanup, onMount, Show, type ParentProps } from 'solid-js';
import { A, useLocation } from '@solidjs/router';
import { isServer } from 'solid-js/web';
import { REPO_URL, ROUTES, SLOGAN, formatNumber } from '@genoryn/shared';
import { GSign } from './Logo';
import { Icon } from './Icon';
import { SearchBar } from './SearchBar';
import { LiveDot } from './ui';
import { theme, toggleTheme, syncThemeFromDom } from '~/lib/theme';
import { connected, rawStats, startLive, stats } from '~/lib/live';
import { scanReveals, setTitle, titleForPath, useSpotlight } from '~/lib/dom';

const SHORT: Record<string, string> = {
  '/wallet': 'Native full-node wallet',
  '/download': 'Latest release, verified',
  '/name': 'On-chain @usernames',
  '/pay': 'Payment requests & QR',
  '/support': 'FAQ and help',
  '/learn': 'How private money works',
  '/status': 'Live network health',
  '/docs': 'Quickstart, CLI, config',
  '/explorer': 'Live blocks & transactions',
  '/api': 'Public JSON API & RPC',
  '/nodes': 'Run a node or seed',
  '/protocol': 'Consensus & cryptography',
  '/mining': 'RandomX CPU mining',
  '/testnet': 'Test network parameters',
  '/mainnet': 'Live network parameters',
  '/devnet': 'Development lab network',
  '/releases': 'Version history',
  '/privacy': 'Built-in Tor & onion services',
  '/security': 'Threat model & disclosure',
};

const org = ROUTES.filter((r) => r.section === 'org');
const dev = ROUTES.filter((r) => r.section === 'dev');

function NavMenu(props: { label: string; items: typeof ROUTES }) {
  return (
    <div class="nav-item">
      <button class="nav-trigger" type="button" aria-haspopup="true">
        {props.label}
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
          <path d="M2 4l3 3 3-3" fill="none" stroke="currentColor" stroke-width="1.4" />
        </svg>
      </button>
      <div class="nav-menu" role="menu">
        <For each={props.items}>
          {(r) => (
            <A href={r.path} role="menuitem">
              <b>{r.nav}</b>
              <small>{SHORT[r.path]}</small>
              <span class="origin">{r.origin}</span>
            </A>
          )}
        </For>
      </div>
    </div>
  );
}

function Header(props: { onMenu: () => void; menuOpen: boolean }) {
  return (
    <header class="site-header">
      <div class="container">
        <A href="/" class="brand" aria-label="GENORYN home">
          <GSign size={30} />
          <span class="brand-name">GENORYN</span>
          <span class="brand-net">mainnet</span>
        </A>
        <nav class="nav" aria-label="Main">
          <A href="/explorer" class="nav-link" activeClass="active">
            Explorer
          </A>
          <NavMenu label="Network" items={org} />
          <NavMenu label="Developers" items={dev} />
          <A href="/mining" class="nav-link" activeClass="active">
            Mining
          </A>
          <A href="/learn" class="nav-link" activeClass="active">
            Learn
          </A>
        </nav>
        <div class="header-actions">
          <SearchBar compact class="header-search" placeholder="Search chain…" />
          <A href="/status" class="icon-btn" title={rawStats()?.stale ? 'genoryn.io node unreachable — showing last known data' : connected() ? 'Live: connected to the genoryn.io node' : 'Connecting…'} aria-label="Network status">
            <LiveDot on={connected() && !!rawStats()?.node_online} err={!!rawStats() && !rawStats()!.node_online} />
          </A>
          <button class="icon-btn" type="button" onClick={toggleTheme} aria-label={`Switch to ${theme() === 'dark' ? 'light' : 'dark'} theme`}>
            <Icon name={theme() === 'dark' ? 'sun' : 'moon'} size={17} />
          </button>
          <a class="icon-btn gh-btn" href={REPO_URL} target="_blank" rel="noopener noreferrer" aria-label="GENORYN on GitHub">
            <Icon name="github" size={17} />
          </a>
          <A href="/download" class="btn primary sm dl-btn" style={{ 'margin-left': '4px' }}>
            Download
          </A>
          <button class="icon-btn menu-btn" type="button" onClick={props.onMenu} aria-expanded={props.menuOpen} aria-label="Menu">
            <Icon name={props.menuOpen ? 'close' : 'menu'} size={18} />
          </button>
        </div>
      </div>
    </header>
  );
}

function MobileNav(props: { close: () => void }) {
  return (
    <div class="mobile-nav" onClick={(e) => (e.target as HTMLElement).closest('a') && props.close()}>
      <SearchBar placeholder="Search chain…" />
      <h4>Explore</h4>
      <A href="/">Home</A>
      <A href="/explorer">Explorer</A>
      <A href="/mining">Mining</A>
      <h4>Network</h4>
      <For each={org}>{(r) => <A href={r.path}>{r.nav}</A>}</For>
      <h4>Developers</h4>
      <For each={dev.filter((r) => r.path !== '/explorer' && r.path !== '/mining')}>{(r) => <A href={r.path}>{r.nav}</A>}</For>
      <h4>Project</h4>
      <A href="/about">About</A>
      <A href="/brand">Brand</A>
    </div>
  );
}

function Footer() {
  const s = () => stats();
  return (
    <footer class="site-footer">
      <div class="container">
        <div class="footer-grid">
          <div>
            <A href="/" class="brand" aria-label="GENORYN home">
              <GSign size={34} />
              <span class="brand-name">GENORYN</span>
            </A>
            <p class="muted mt-2" style={{ 'max-width': '340px', 'font-size': '14.5px' }}>
              Private by design. Finite by protocol. An independent, open-source proof-of-work network. Creator: unknown.
            </p>
            <div class="row mt-2" style={{ 'font-size': '13px' }}>
              <LiveDot on={!!s()?.node_online} err={!!rawStats()?.stale} />
              <span class="mono muted">
                <Show when={s()} fallback={rawStats()?.stale ? 'node unreachable' : 'connecting…'}>
                  {s()!.network} · block {formatNumber(s()!.height)}
                  {s()!.stale ? ' · last known · node unreachable' : ` · ${s()!.peers} peers`}
                </Show>
              </span>
            </div>
          </div>
          <div>
            <h4>Network</h4>
            <For each={org}>
              {(r) => (
                <A href={r.path}>
                  {r.nav}
                </A>
              )}
            </For>
          </div>
          <div>
            <h4>Developers</h4>
            <For each={dev}>{(r) => <A href={r.path}>{r.nav}</A>}</For>
          </div>
          <div>
            <h4>Project</h4>
            <A href="/about">About</A>
            <A href="/brand">Brand</A>
            <a href={REPO_URL} target="_blank" rel="noopener noreferrer">
              GitHub <span>↗</span>
            </a>
            <a href={`${REPO_URL}/releases/latest`} target="_blank" rel="noopener noreferrer">
              Latest release <span>↗</span>
            </a>
            <a href={`${REPO_URL}/blob/main/SECURITY.md`} target="_blank" rel="noopener noreferrer">
              Security policy <span>↗</span>
            </a>
            <a href="/sitemap.xml">Sitemap</a>
          </div>
        </div>
        <div class="footer-slogan" aria-label={SLOGAN}>
          Privacy is not a crime.
          <span>It's a right.</span>
        </div>
        <div class="footer-bottom">
          <span>MIT or Apache-2.0 · Not yet independently audited — mine or hold only what you can afford to lose · No trackers or cookies.</span>
          <span class="mono">genoryn.io</span>
        </div>
      </div>
    </footer>
  );
}

export default function Layout(props: ParentProps) {
  const location = useLocation();
  const [menu, setMenu] = createSignal(false);
  useSpotlight();

  onMount(() => {
    document.documentElement.classList.add('js');
    syncThemeFromDom();
    startLive();
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement;
      if (e.key === '/' && !['INPUT', 'TEXTAREA', 'SELECT'].includes(t.tagName)) {
        const input = document.getElementById('q-header') as HTMLInputElement | null;
        if (input && input.offsetParent) {
          e.preventDefault();
          input.focus();
        }
      }
      if (e.key === 'Escape') setMenu(false);
    };
    document.addEventListener('keydown', onKey);
    onCleanup(() => document.removeEventListener('keydown', onKey));
  });

  createEffect(
    on(
      () => location.pathname,
      (path, prev) => {
        if (isServer) return;
        setMenu(false);
        if (prev !== undefined && !location.hash) window.scrollTo({ top: 0, behavior: 'instant' as ScrollBehavior });
        if (!path.startsWith('/explorer/')) setTitle(titleForPath(path));
        requestAnimationFrame(() => requestAnimationFrame(scanReveals));
      },
    ),
  );

  return (
    <>
      <a class="skip-link" href="#content">
        Skip to content
      </a>
      <Header onMenu={() => setMenu(!menu())} menuOpen={menu()} />
      <Show when={menu()}>
        <MobileNav close={() => setMenu(false)} />
      </Show>
      <main id="content">
        {props.children}
      </main>
      <Footer />
    </>
  );
}
