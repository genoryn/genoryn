import { A } from '@solidjs/router';
import { onMount } from 'solid-js';
import { SearchBar } from '~/components/SearchBar';
import { setTitle } from '~/lib/dom';

export default function NotFound() {
  onMount(() => setTitle('Page not found — GENORYN'));
  return (
    <section class="nf">
      <div class="container narrow">
        <div class="code-num">404</div>
        <h1 style={{ 'font-family': 'var(--font-display)', 'text-transform': 'uppercase', 'letter-spacing': '-0.03em' }}>Nothing here.</h1>
        <p class="lead" style={{ margin: '0 auto 32px' }}>
          This page doesn't exist — like a public balance on GENORYN. Try searching the chain instead.
        </p>
        <SearchBar />
        <div class="row mt-3" style={{ 'justify-content': 'center' }}>
          <A href="/" class="btn">Home</A>
          <A href="/explorer" class="btn">Explorer</A>
        </div>
      </div>
    </section>
  );
}
