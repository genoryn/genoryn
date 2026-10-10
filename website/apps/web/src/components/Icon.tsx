import type { JSX } from 'solid-js';

const P: Record<string, string> = {
  search: 'M11 4a7 7 0 1 1 0 14 7 7 0 0 1 0-14zm10 17-4.35-4.35',
  sun: 'M12 3v2m0 14v2m9-9h-2M5 12H3m15.36-6.36-1.42 1.42M7.06 16.94l-1.42 1.42m12.72 0-1.42-1.42M7.06 7.06 5.64 5.64M12 8a4 4 0 1 1 0 8 4 4 0 0 1 0-8z',
  moon: 'M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z',
  menu: 'M4 7h16M4 12h16M4 17h16',
  close: 'M6 6l12 12M18 6 6 18',
  copy: 'M9 9h10v10H9zM5 15V5h10',
  check: 'M5 12.5 10 17 19 7',
  arrow: 'M5 12h14m-6-6 6 6-6 6',
  arrowUpRight: 'M7 17 17 7M8 7h9v9',
  download: 'M12 4v11m-5-5 5 5 5-5M5 20h14',
  github: 'M9 19c-4 1.5-4-2-6-2.5m12 5v-3.5c0-1 .1-1.4-.5-2 2.8-.3 5.5-1.4 5.5-6a4.6 4.6 0 0 0-1.3-3.2 4.2 4.2 0 0 0-.1-3.2s-1.1-.3-3.5 1.3a12 12 0 0 0-6.2 0C6.5 2.8 5.4 3.1 5.4 3.1a4.2 4.2 0 0 0-.1 3.2A4.6 4.6 0 0 0 4 9.5c0 4.6 2.7 5.7 5.5 6-.6.6-.6 1.2-.5 2V21',
  shield: 'M12 3 4 6v6c0 5 3.4 8.3 8 9 4.6-.7 8-4 8-9V6l-8-3z',
  lock: 'M6 11h12v10H6zM8 11V7a4 4 0 1 1 8 0v4',
  eyeOff: 'M3 3l18 18M10.6 10.6a2 2 0 0 0 2.8 2.8M9.9 5.1A10 10 0 0 1 12 5c5 0 8.5 4.5 9.5 7-.4 1-1.2 2.4-2.4 3.7M6.2 6.2C4.3 7.6 3 9.6 2.5 12c1 2.5 4.5 7 9.5 7 1.7 0 3.2-.5 4.5-1.2',
  cpu: 'M8 8h8v8H8zM4 9h2m-2 6h2m12-6h2m-2 6h2M9 4v2m6-2v2M9 18v2m6-2v2M6 6h12v12H6z',
  cube: 'M12 2 3 7v10l9 5 9-5V7l-9-5zm0 0v10m0 0 9-5m-9 5-9-5',
  layers: 'm12 3 9 5-9 5-9-5 9-5zm-9 9 9 5 9-5M3 16l9 5 9-5',
  node: 'M4 6h16v5H4zM4 13h16v5H4zM7 8.5h.01M7 15.5h.01',
  globe: 'M12 3a9 9 0 1 1 0 18 9 9 0 0 1 0-18zm-9 9h18M12 3c2.5 2.5 3.5 5.5 3.5 9s-1 6.5-3.5 9c-2.5-2.5-3.5-5.5-3.5-9s1-6.5 3.5-9z',
  pulse: 'M3 12h4l3-8 4 16 3-8h4',
  infinity: 'M7 9a3 3 0 1 0 0 6c2 0 3-1.5 5-3s3-3 5-3a3 3 0 1 1 0 6c-2 0-3-1.5-5-3S9 9 7 9z',
  key: 'M14 10a4 4 0 1 0-5.6 3.7L3 19v2h3v-2h2v-2h2l1.4-1.4A4 4 0 0 0 14 10zm1-1h.01',
  hash: 'M5 9h14M5 15h14M10 4 8 20M16 4l-2 16',
  clock: 'M12 3a9 9 0 1 1 0 18 9 9 0 0 1 0-18zm0 4v5l3 2',
  coins: 'M9 7c0 1.66 2.69 3 6 3s6-1.34 6-3-2.69-3-6-3-6 1.34-6 3zm0 0v4c0 1.66 2.69 3 6 3s6-1.34 6-3V7M3 13c0 1.66 2.69 3 6 3m-6-3v4c0 1.66 2.69 3 6 3s6-1.34 6-3m-12-4c0-1.66 2.69-3 6-3',
  terminal: 'M4 5h16v14H4zm3 4 3 3-3 3m5 0h5',
  book: 'M4 5a2 2 0 0 1 2-2h13v16H6a2 2 0 0 0-2 2V5zm0 14a2 2 0 0 1 2-2h13',
  code: 'm8 8-4 4 4 4m8-8 4 4-4 4M14 4l-4 16',
  user: 'M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8zm-8 9a8 8 0 0 1 16 0',
  qr: 'M4 4h6v6H4zm10 0h6v6h-6zM4 14h6v6H4zm10 0h2v2h-2zm4 0h2v2h-2zm-4 4h2v2h-2zm4 0h2v2h-2z',
  help: 'M12 3a9 9 0 1 1 0 18 9 9 0 0 1 0-18zm-2.5 6.5A2.5 2.5 0 1 1 12 12v1.5m0 3.5h.01',
  zap: 'M13 3 4 14h7l-1 7 9-11h-7l1-7z',
  file: 'M6 3h9l4 4v14H6zM14 3v5h5',
  scale: 'M12 4v16M5 20h14M6 8l-3 6a3 3 0 0 0 6 0L6 8zm12 0-3 6a3 3 0 0 0 6 0l-3-6zM6 8h12',
  alert: 'M12 4 2 20h20L12 4zm0 6v4m0 3h.01',
  fingerprint: 'M12 11v3a8 8 0 0 1-1.5 4.7M8 11a4 4 0 0 1 8 0v1a12 12 0 0 1-1 4.8M5 9a7.5 7.5 0 0 1 14 2v1M4 15c.6-1 1-2 1-3m14.5 3a16 16 0 0 1-.5 3',
  tag: 'M3 12V3h9l9 9-9 9-9-9zm5-4h.01',
  refresh: 'M20 11a8 8 0 0 0-14.9-3.9M4 4v4h4m-4 5a8 8 0 0 0 14.9 3.9M20 20v-4h-4',
};

export function Icon(props: { name: keyof typeof P | string; size?: number; class?: string; stroke?: number; style?: JSX.CSSProperties }) {
  return (
    <svg
      class={props.class}
      style={props.style}
      width={props.size ?? 18}
      height={props.size ?? 18}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={props.stroke ?? 1.6}
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <path d={P[props.name] ?? P.hash} />
    </svg>
  );
}
