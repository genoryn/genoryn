import { onMount, Show } from 'solid-js';

let katexPromise: Promise<typeof import('katex')> | null = null;
const loadKatex = () => {
  katexPromise ??= Promise.all([import('katex'), import('katex/dist/katex.min.css')]).then(([k]) => k);
  return katexPromise;
};

/** TeX rendered with KaTeX on the client (lazy). Prerendered HTML keeps the TeX source for crawlers. */
export function MathTex(props: { tex: string; block?: boolean; caption?: string }) {
  let el!: HTMLSpanElement;
  onMount(async () => {
    const k = await loadKatex();
    try {
      k.default.render(props.tex, el, { displayMode: !!props.block, throwOnError: false, output: 'html' });
    } catch {
      /* leave TeX source visible */
    }
  });
  const node = <span ref={el} class="math-raw">{props.tex}</span>;
  return (
    <Show when={props.block} fallback={node}>
      <div class="math-block">
        {node}
        <Show when={props.caption}>
          <div class="math-caption">{props.caption}</div>
        </Show>
      </div>
    </Show>
  );
}
