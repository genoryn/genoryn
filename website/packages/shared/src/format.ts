export function shortHash(h: string, head = 10, tail = 8): string {
  if (!h || h.length <= head + tail + 1) return h;
  return `${h.slice(0, head)}…${h.slice(-tail)}`;
}

export function formatHashrate(hs: number | null | undefined): string {
  if (hs === null || hs === undefined || !Number.isFinite(hs)) return '—';
  const units = ['H/s', 'kH/s', 'MH/s', 'GH/s', 'TH/s', 'PH/s'];
  let v = hs;
  let i = 0;
  while (v >= 1000 && i < units.length - 1) {
    v /= 1000;
    i++;
  }
  return `${v.toFixed(v < 10 ? 2 : v < 100 ? 1 : 0)} ${units[i]}`;
}

export function formatBytes(n: number | null | undefined): string {
  if (n === null || n === undefined || !Number.isFinite(n)) return '—';
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(i === 0 ? 0 : v < 10 ? 2 : 1)} ${units[i]}`;
}

export function formatDuration(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined || !Number.isFinite(seconds)) return '—';
  const s = Math.max(0, Math.round(seconds));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${s % 60}s`;
  const h = Math.floor(m / 60);
  if (h < 48) return `${h}h ${m % 60}m`;
  const d = Math.floor(h / 24);
  if (d < 730) return `${d}d ${h % 24}h`;
  return `${(d / 365.25).toFixed(1)} years`;
}

export function timeAgo(unixSeconds: number, now = Date.now() / 1000): string {
  const diff = now - unixSeconds;
  if (diff < -5) return 'in the future';
  if (diff < 5) return 'just now';
  return `${formatDuration(diff)} ago`;
}

export function formatNumber(n: number | bigint | string, digits = 0): string {
  if (typeof n === 'string') n = Number(n);
  if (typeof n === 'bigint') return n.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ',');
  return n.toLocaleString('en-US', { maximumFractionDigits: digits, minimumFractionDigits: 0 });
}

export function utc(unixSeconds: number): string {
  return new Date(unixSeconds * 1000).toISOString().replace('T', ' ').replace(/\.\d+Z$/, ' UTC');
}

export const isHex64 = (s: string) => /^[0-9a-f]{64}$/i.test(s);
