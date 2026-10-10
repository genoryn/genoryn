import { createSignal } from 'solid-js';
import { isServer } from 'solid-js/web';

export type Theme = 'dark' | 'light';

const initial = (): Theme => (isServer ? 'dark' : ((document.documentElement.getAttribute('data-theme') as Theme) ?? 'dark'));
const [theme, setThemeSignal] = createSignal<Theme>(initial());
export { theme };

export function setTheme(t: Theme) {
  setThemeSignal(t);
  if (isServer) return;
  document.documentElement.setAttribute('data-theme', t);
  try {
    localStorage.setItem('genoryn-theme', t);
  } catch {
    /* private mode */
  }
  for (const m of document.querySelectorAll('meta[name="theme-color"]')) m.setAttribute('content', t === 'dark' ? '#0B0B0B' : '#FAFAFA');
}

export function toggleTheme() {
  const next: Theme = theme() === 'dark' ? 'light' : 'dark';
  const doc = document as Document & { startViewTransition?: (cb: () => void) => void };
  if (doc.startViewTransition && !window.matchMedia('(prefers-reduced-motion: reduce)').matches) doc.startViewTransition(() => setTheme(next));
  else setTheme(next);
}

export function syncThemeFromDom() {
  if (!isServer) setThemeSignal(initial());
}
