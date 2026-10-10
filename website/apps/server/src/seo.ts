import { FAQ, ROUTES, REPO_URL, SLOGAN, goriToGnr, routeMeta, shortHash } from '@genoryn/shared';
import type { BlockRow, Queries, TxRow } from './db';
import { config } from './config';

const esc = (s: string) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');

export interface PageMeta {
  title: string;
  description: string;
  path: string;
  status: number;
  jsonLd: object[];
  noindex?: boolean;
}

const organization = () => ({
  '@context': 'https://schema.org',
  '@type': 'Organization',
  name: 'GENORYN',
  url: config.siteUrl,
  logo: `${config.siteUrl}/brand/genoryn-mark-dark-512.png`,
  sameAs: [REPO_URL],
  slogan: SLOGAN,
});

export function resolveMeta(path: string, q: Queries): PageMeta {
  const clean = path.length > 1 ? path.replace(/\/+$/, '') : path;
  const r = routeMeta(clean);
  if (r) {
    const jsonLd: object[] = [];
    if (clean === '/') {
      jsonLd.push(organization(), {
        '@context': 'https://schema.org',
        '@type': 'WebSite',
        name: 'GENORYN',
        url: config.siteUrl,
        potentialAction: { '@type': 'SearchAction', target: `${config.siteUrl}/explorer/search?q={query}`, 'query-input': 'required name=query' },
      });
    }
    if (clean === '/download' || clean === '/wallet') {
      jsonLd.push({
        '@context': 'https://schema.org',
        '@type': 'SoftwareApplication',
        name: 'GENORYN Wallet',
        operatingSystem: 'Windows, Linux',
        applicationCategory: 'FinanceApplication',
        license: 'https://opensource.org/licenses/MIT',
        offers: { '@type': 'Offer', price: '0', priceCurrency: 'USD' },
        downloadUrl: `${REPO_URL}/releases/latest`,
      });
    }
    if (clean === '/support') {
      jsonLd.push({
        '@context': 'https://schema.org',
        '@type': 'FAQPage',
        mainEntity: FAQ.map((f) => ({ '@type': 'Question', name: f.q, acceptedAnswer: { '@type': 'Answer', text: f.a } })),
      });
    }
    return { title: r.title, description: r.description, path: clean, status: 200, jsonLd };
  }

  const block = clean.match(/^\/explorer\/block\/([0-9a-fA-F]{64}|\d{1,12})$/);
  if (block) {
    const id = block[1]!;
    const b = (/^\d+$/.test(id) ? q.blockByHeight.get(Number(id)) : q.blockByHash.get(id.toLowerCase())) as BlockRow | undefined;
    if (b)
      return {
        title: `Block #${b.height.toLocaleString('en-US')} — GENORYN Explorer`,
        description: `GENORYN ${b.network} block ${b.height}, hash ${shortHash(b.hash)}, mined ${new Date(b.timestamp * 1000).toUTCString()} with ${b.tx_count} transactions, difficulty ${b.difficulty} and ${goriToGnr(String(b.miner_reward), { maxFraction: 4 })} GNR miner reward.`,
        path: `/explorer/block/${b.height}`,
        status: 200,
        jsonLd: [],
      };
    return { title: 'Block not found — GENORYN Explorer', description: 'This block has not been indexed yet.', path: clean, status: 404, jsonLd: [], noindex: true };
  }
  const tx = clean.match(/^\/explorer\/tx\/([0-9a-fA-F]{64})$/);
  if (tx) {
    const t = q.tx.get(tx[1]!.toLowerCase()) as TxRow | undefined;
    if (t)
      return {
        title: `Transaction ${shortHash(t.txid, 8, 6)} — GENORYN Explorer`,
        description: `Shielded GENORYN transaction ${t.txid} in block ${t.height} with ${t.actions} actions. Amounts, senders and recipients are encrypted; only public fees, nullifiers and note commitments are visible.`,
        path: clean,
        status: 200,
        jsonLd: [],
      };
    return { title: 'Transaction not found — GENORYN Explorer', description: 'This transaction is unknown or not yet confirmed.', path: clean, status: 404, jsonLd: [], noindex: true };
  }
  if (clean === '/explorer/blocks')
    return { title: 'All blocks — GENORYN Explorer', description: 'Every block in the live GENORYN mainnet chain, newest first: height, hash, age, transactions, size, difficulty and miner reward. Updates in real time.', path: clean, status: 200, jsonLd: [] };
  if (/^\/explorer\/(search|nullifier\/[0-9a-fA-F]{64}|commitment\/[0-9a-fA-F]{64}|txs)$/.test(clean))
    return { title: 'GENORYN Explorer', description: routeMeta('/explorer')!.description, path: clean, status: 200, jsonLd: [], noindex: clean.includes('search') };
  return { title: 'Page not found — GENORYN', description: 'This page does not exist.', path: clean, status: 404, jsonLd: [], noindex: true };
}

export function headTags(m: PageMeta): string {
  const url = config.siteUrl + (m.path === '/' ? '/' : m.path);
  const image = `${config.siteUrl}/og.png`;
  return [
    `<title>${esc(m.title)}</title>`,
    `<meta name="description" content="${esc(m.description)}">`,
    `<link rel="canonical" href="${esc(url)}">`,
    m.noindex ? '<meta name="robots" content="noindex,follow">' : '<meta name="robots" content="index,follow,max-image-preview:large">',
    `<meta property="og:type" content="website">`,
    `<meta property="og:site_name" content="GENORYN">`,
    `<meta property="og:title" content="${esc(m.title)}">`,
    `<meta property="og:description" content="${esc(m.description)}">`,
    `<meta property="og:url" content="${esc(url)}">`,
    `<meta property="og:image" content="${image}">`,
    `<meta property="og:image:width" content="1200">`,
    `<meta property="og:image:height" content="630">`,
    `<meta name="twitter:card" content="summary_large_image">`,
    `<meta name="twitter:title" content="${esc(m.title)}">`,
    `<meta name="twitter:description" content="${esc(m.description)}">`,
    `<meta name="twitter:image" content="${image}">`,
    ...m.jsonLd.map((j) => `<script type="application/ld+json">${JSON.stringify(j).replace(/</g, '\\u003c')}</script>`),
  ].join('\n    ');
}

export function sitemap(q: Queries): string {
  const now = new Date().toISOString();
  const urls = ROUTES.map((r) => `<url><loc>${config.siteUrl}${r.path === '/' ? '/' : r.path}</loc><lastmod>${now}</lastmod><changefreq>${r.changefreq}</changefreq><priority>${r.priority.toFixed(2)}</priority></url>`);
  const heights = q.heightsForSitemap.all(500) as number[];
  for (const h of heights) urls.push(`<url><loc>${config.siteUrl}/explorer/block/${h}</loc><changefreq>monthly</changefreq><priority>0.30</priority></url>`);
  return `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n${urls.join('\n')}\n</urlset>\n`;
}

export const robots = () => `User-agent: *\nAllow: /\nDisallow: /api/\nDisallow: /explorer/search\n\nSitemap: ${config.siteUrl}/sitemap.xml\n`;
