import { createSignal, For, onCleanup, onMount } from 'solid-js';

export function Toc(props: { items: Array<[string, string]>; title?: string }) {
  const [active, setActive] = createSignal(props.items[0]?.[0] ?? '');
  onMount(() => {
    const io = new IntersectionObserver(
      (entries) => {
        const visible = entries.filter((e) => e.isIntersecting).sort((a, b) => a.boundingClientRect.top - b.boundingClientRect.top);
        if (visible[0]) setActive(visible[0].target.id);
      },
      { rootMargin: '-20% 0px -70% 0px' },
    );
    props.items.forEach(([id]) => {
      const el = document.getElementById(id);
      if (el) io.observe(el);
    });
    onCleanup(() => io.disconnect());
  });
  return (
    <nav class="toc" aria-label="On this page">
      <h4>{props.title ?? 'On this page'}</h4>
      <For each={props.items}>
        {([id, label]) => (
          <a href={`#${id}`} class={active() === id ? 'active' : ''}>
            {label}
          </a>
        )}
      </For>
    </nav>
  );
}
