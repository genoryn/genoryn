import { onCleanup, onMount } from 'solid-js';
import { isServer } from 'solid-js/web';
import { ROUTES } from '@genoryn/shared';

let observer: IntersectionObserver | null = null;

/** Observe every `.reveal` element not yet shown. Call after route changes. */
export function scanReveals() {
  if (isServer) return;
  if (!('IntersectionObserver' in window)) {
    document.querySelectorAll('.reveal').forEach((el) => el.classList.add('in'));
    return;
  }
  observer ??= new IntersectionObserver(
    (entries) => {
      for (const e of entries) {
        if (e.isIntersecting) {
          e.target.classList.add('in');
          observer!.unobserve(e.target);
        }
      }
    },
    { rootMargin: '0px 0px -8% 0px', threshold: 0.08 },
  );
  document.querySelectorAll('.reveal:not(.in)').forEach((el) => observer!.observe(el));
}

export function setTitle(title: string) {
  if (!isServer) document.title = title;
}

export function titleForPath(path: string): string {
  return ROUTES.find((r) => r.path === path)?.title ?? 'GENORYN';
}

/** Cursor-following spotlight for `.card.spot`. */
export function useSpotlight() {
  onMount(() => {
    const handler = (e: PointerEvent) => {
      const card = (e.target as HTMLElement).closest?.('.card.spot') as HTMLElement | null;
      if (!card) return;
      const r = card.getBoundingClientRect();
      card.style.setProperty('--mx', `${e.clientX - r.left}px`);
      card.style.setProperty('--my', `${e.clientY - r.top}px`);
    };
    document.addEventListener('pointermove', handler, { passive: true });
    onCleanup(() => document.removeEventListener('pointermove', handler));
  });
}

export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

export function detectOs(): 'windows' | 'linux' | 'macos' | 'other' {
  if (isServer) return 'other';
  const ua = navigator.userAgent.toLowerCase();
  if (ua.includes('windows')) return 'windows';
  if (ua.includes('mac os') || ua.includes('macintosh')) return 'macos';
  if (ua.includes('linux') || ua.includes('x11')) return 'linux';
  return 'other';
}
