import { REPO, type Release, type ReleaseAsset } from '@genoryn/shared';
import { config } from './config';

interface GhAsset {
  name: string;
  browser_download_url: string;
  size: number;
  download_count: number;
}
interface GhRelease {
  tag_name: string;
  name: string | null;
  published_at: string;
  html_url: string;
  body: string | null;
  prerelease: boolean;
  draft: boolean;
  assets: GhAsset[];
}

const TTL = 10 * 60 * 1000;
let cache: { at: number; releases: Release[]; repo: Record<string, unknown> | null } | null = null;
let inflight: Promise<void> | null = null;
const shaCache = new Map<string, string | null>();

const headers = () => ({
  accept: 'application/vnd.github+json',
  'user-agent': 'genoryn.io',
  ...(config.githubToken ? { authorization: `Bearer ${config.githubToken}` } : {}),
});

function platform(name: string): ReleaseAsset['platform'] {
  const n = name.toLowerCase();
  if (n.includes('windows') || n.endsWith('.exe') || n.endsWith('.msi')) return 'windows';
  if (n.includes('linux') || n.endsWith('.appimage') || n.endsWith('.deb')) return 'linux';
  if (n.includes('mac') || n.includes('darwin') || n.endsWith('.dmg')) return 'macos';
  return 'other';
}

async function sha256For(asset: GhAsset, all: GhAsset[]): Promise<string | null> {
  const side = all.find((a) => a.name === `${asset.name}.sha256`);
  if (!side) return null;
  if (shaCache.has(side.browser_download_url)) return shaCache.get(side.browser_download_url)!;
  try {
    const res = await fetch(side.browser_download_url, { signal: AbortSignal.timeout(10_000), headers: { 'user-agent': 'genoryn.io' } });
    const text = (await res.text()).slice(0, 4096);
    const m = text.match(/\b[0-9a-f]{64}\b/i);
    const v = m ? m[0].toLowerCase() : null;
    shaCache.set(side.browser_download_url, v);
    return v;
  } catch {
    return null;
  }
}

async function refresh() {
  const [relRes, repoRes] = await Promise.all([
    fetch(`https://api.github.com/repos/${REPO}/releases?per_page=30`, { headers: headers(), signal: AbortSignal.timeout(15_000) }),
    fetch(`https://api.github.com/repos/${REPO}`, { headers: headers(), signal: AbortSignal.timeout(15_000) }),
  ]);
  if (!relRes.ok) throw new Error(`GitHub releases: HTTP ${relRes.status}`);
  const list = ((await relRes.json()) as GhRelease[]).filter((r) => !r.draft);
  const releases: Release[] = [];
  for (const r of list) {
    const binaries = r.assets.filter((a) => !a.name.endsWith('.sha256'));
    const assets: ReleaseAsset[] = [];
    for (const a of binaries) {
      assets.push({ name: a.name, url: a.browser_download_url, size: a.size, downloads: a.download_count, sha256: await sha256For(a, r.assets), platform: platform(a.name) });
    }
    releases.push({ tag: r.tag_name, name: r.name ?? r.tag_name, published_at: r.published_at, url: r.html_url, body: r.body ?? '', prerelease: r.prerelease, assets });
  }
  const repo = repoRes.ok ? ((await repoRes.json()) as Record<string, unknown>) : null;
  cache = {
    at: Date.now(),
    releases,
    repo: repo
      ? { stars: repo.stargazers_count, forks: repo.forks_count, watchers: repo.subscribers_count, open_issues: repo.open_issues_count, pushed_at: repo.pushed_at, license: (repo.license as { spdx_id?: string } | null)?.spdx_id ?? null, topics: repo.topics }
      : null,
  };
}

export async function getReleases(): Promise<{ releases: Release[]; repo: Record<string, unknown> | null; cached_at: number | null; error?: string }> {
  if (!cache || Date.now() - cache.at > TTL) {
    inflight ??= refresh().finally(() => (inflight = null));
    try {
      await inflight;
    } catch (e) {
      if (!cache) return { releases: [], repo: null, cached_at: null, error: (e as Error).message };
    }
  }
  return { releases: cache!.releases, repo: cache!.repo, cached_at: cache!.at };
}
