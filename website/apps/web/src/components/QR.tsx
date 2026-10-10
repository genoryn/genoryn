import { createEffect, createSignal, Show } from 'solid-js';

/** Monochrome QR rendered locally (no network). */
export function QR(props: { value: string }) {
  const [svg, setSvg] = createSignal<string>('');
  createEffect(() => {
    const v = props.value;
    if (!v) return setSvg('');
    void import('qrcode').then((q) =>
      q.toString(v, { type: 'svg', errorCorrectionLevel: 'M', margin: 1, color: { dark: '#0B0B0B', light: '#FFFFFF' } }).then(setSvg),
    );
  });
  return (
    <Show when={svg()} fallback={<div class="qr-box" style={{ width: '280px', height: '280px' }} />}>
      <div class="qr-box" innerHTML={svg()} role="img" aria-label="QR code" />
    </Show>
  );
}
