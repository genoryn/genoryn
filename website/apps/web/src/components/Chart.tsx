import { createMemo, createSignal, For, Show } from 'solid-js';

export interface Point {
  x: number;
  y: number;
  label?: string;
}

const W = 800;
const H = 220;
const PAD = { l: 8, r: 8, t: 14, b: 22 };

function niceFormat(v: number): string {
  const a = Math.abs(v);
  if (a >= 1e9) return `${(v / 1e9).toFixed(1)}B`;
  if (a >= 1e6) return `${(v / 1e6).toFixed(1)}M`;
  if (a >= 1e3) return `${(v / 1e3).toFixed(1)}k`;
  return Number.isInteger(v) ? String(v) : v.toFixed(1);
}

/** Monochrome area/line chart in pure SVG. */
export function LineChart(props: { points: Point[]; height?: number; format?: (v: number) => string; xFormat?: (x: number) => string; zeroBased?: boolean }) {
  const [hover, setHover] = createSignal<number | null>(null);
  let svg!: SVGSVGElement;
  const fmt = (v: number) => (props.format ?? niceFormat)(v);

  const geo = createMemo(() => {
    const pts = props.points;
    if (pts.length < 2) return null;
    const xs = pts.map((p) => p.x);
    const ys = pts.map((p) => p.y);
    const minX = Math.min(...xs);
    const maxX = Math.max(...xs);
    let minY = props.zeroBased ? 0 : Math.min(...ys);
    let maxY = Math.max(...ys);
    if (minY === maxY) {
      maxY += 1;
      minY = Math.max(0, minY - 1);
    }
    const pad = (maxY - minY) * 0.08;
    if (!props.zeroBased) minY -= pad;
    maxY += pad;
    const sx = (x: number) => PAD.l + ((x - minX) / (maxX - minX || 1)) * (W - PAD.l - PAD.r);
    const sy = (y: number) => PAD.t + (1 - (y - minY) / (maxY - minY)) * (H - PAD.t - PAD.b);
    const line = pts.map((p, i) => `${i ? 'L' : 'M'}${sx(p.x).toFixed(1)},${sy(p.y).toFixed(1)}`).join('');
    const area = `${line}L${sx(maxX).toFixed(1)},${H - PAD.b}L${sx(minX).toFixed(1)},${H - PAD.b}Z`;
    const ticks = [0, 0.5, 1].map((t) => minY + t * (maxY - minY));
    return { line, area, sx, sy, ticks, minX, maxX };
  });

  const onMove = (e: PointerEvent) => {
    const g = geo();
    if (!g) return;
    const r = svg.getBoundingClientRect();
    const x = ((e.clientX - r.left) / r.width) * W;
    let best = 0;
    let bestD = Infinity;
    props.points.forEach((p, i) => {
      const d = Math.abs(g.sx(p.x) - x);
      if (d < bestD) {
        bestD = d;
        best = i;
      }
    });
    setHover(best);
  };

  return (
    <div style={{ position: 'relative' }}>
      <Show when={geo()} fallback={<div class="empty" style={{ height: `${props.height ?? 220}px`, display: 'grid', 'place-items': 'center' }}>Collecting data…</div>}>
        {(g) => (
          <svg ref={svg} class="chart" viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" style={{ height: `${props.height ?? 220}px` }} onPointerMove={onMove} onPointerLeave={() => setHover(null)} role="img">
            <defs>
              <linearGradient id="chart-fade" x1="0" x2="0" y1="0" y2="1">
                <stop offset="0%" stop-color="currentColor" stop-opacity="0.22" />
                <stop offset="100%" stop-color="currentColor" stop-opacity="0" />
              </linearGradient>
            </defs>
            <For each={g().ticks}>{(t) => <line class="axis" x1={PAD.l} x2={W - PAD.r} y1={g().sy(t)} y2={g().sy(t)} stroke-dasharray="2 4" />}</For>
            <path class="area" d={g().area} style={{ color: 'var(--text-strong)' }} />
            <path class="line" d={g().line} />
            <Show when={hover() !== null}>
              <line class="axis" x1={g().sx(props.points[hover()!]!.x)} x2={g().sx(props.points[hover()!]!.x)} y1={PAD.t} y2={H - PAD.b} />
              <circle cx={g().sx(props.points[hover()!]!.x)} cy={g().sy(props.points[hover()!]!.y)} r="4" fill="var(--text-strong)" />
            </Show>
          </svg>
        )}
      </Show>
      <Show when={hover() !== null && geo()}>
        <div
          class="chart-tip"
          style={{
            left: `${(geo()!.sx(props.points[hover()!]!.x) / W) * 100}%`,
            top: `${(geo()!.sy(props.points[hover()!]!.y) / H) * (props.height ?? 220)}px`,
          }}
        >
          {props.points[hover()!]!.label ?? `#${props.points[hover()!]!.x}`} · {fmt(props.points[hover()!]!.y)}
        </div>
      </Show>
      <Show when={geo()}>
        <div class="row between faint mono" style={{ 'font-size': '11px', 'margin-top': '6px' }}>
          <span>{props.xFormat ? props.xFormat(geo()!.minX) : `#${geo()!.minX}`}</span>
          <span>
            min {fmt(Math.min(...props.points.map((p) => p.y)))} · max {fmt(Math.max(...props.points.map((p) => p.y)))}
          </span>
          <span>{props.xFormat ? props.xFormat(geo()!.maxX) : `#${geo()!.maxX}`}</span>
        </div>
      </Show>
    </div>
  );
}

export function BarChart(props: { points: Point[]; height?: number; format?: (v: number) => string }) {
  const [hover, setHover] = createSignal<number | null>(null);
  const fmt = (v: number) => (props.format ?? niceFormat)(v);
  const max = createMemo(() => Math.max(1, ...props.points.map((p) => p.y)));
  const bw = createMemo(() => (W - PAD.l - PAD.r) / Math.max(1, props.points.length));
  return (
    <div style={{ position: 'relative' }}>
      <svg class="chart" viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" style={{ height: `${props.height ?? 220}px` }} onPointerLeave={() => setHover(null)}>
        <line class="axis" x1={PAD.l} x2={W - PAD.r} y1={H - PAD.b} y2={H - PAD.b} />
        <For each={props.points}>
          {(p, i) => {
            const h = () => Math.max(p.y > 0 ? 2 : 0, (p.y / max()) * (H - PAD.t - PAD.b));
            return <rect class="bar" x={PAD.l + i() * bw() + bw() * 0.15} width={Math.max(1, bw() * 0.7)} y={H - PAD.b - h()} height={h()} rx="1.5" onPointerEnter={() => setHover(i())} />;
          }}
        </For>
      </svg>
      <Show when={hover() !== null}>
        <div class="chart-tip" style={{ left: `${((PAD.l + (hover()! + 0.5) * bw()) / W) * 100}%`, top: '24px' }}>
          {props.points[hover()!]!.label ?? `#${props.points[hover()!]!.x}`} · {fmt(props.points[hover()!]!.y)}
        </div>
      </Show>
    </div>
  );
}
