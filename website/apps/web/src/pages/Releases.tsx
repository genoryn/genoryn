import { For, Show } from 'solid-js';
import { REPO_URL, formatBytes } from '@genoryn/shared';
import { Icon } from '~/components/Icon';
import { CopyButton, ExtLink, PageHero } from '~/components/ui';
import { useReleases } from '~/lib/releases';

/** Minimal, safe markdown: headings, bullets, bold, code and links rendered as text nodes. */
function Notes(props: { body: string }) {
  const lines = () => props.body.replace(/\r/g, '').split('\n');
  const inline = (s: string) => {
    const parts = s.split(/(\*\*[^*]+\*\*|`[^`]+`|\[[^\]]+\]\([^)]+\))/g);
    return parts.map((p) => {
      if (p.startsWith('**') && p.endsWith('**')) return <b class="strong">{p.slice(2, -2)}</b>;
      if (p.startsWith('`') && p.endsWith('`')) return <code>{p.slice(1, -1)}</code>;
      const m = p.match(/^\[([^\]]+)\]\(([^)]+)\)$/);
      if (m) {
        const href = /^https:\/\//.test(m[2]!) ? m[2]! : `${REPO_URL}/blob/main/${m[2]!.replace(/^\.?\//, '')}`;
        return <ExtLink href={href}>{m[1]}</ExtLink>;
      }
      return p;
    });
  };
  return (
    <div class="prose" style={{ 'font-size': '15px' }}>
      <For each={lines()}>
        {(l) => {
          if (/^###\s/.test(l)) return <h3>{l.replace(/^###\s/, '')}</h3>;
          if (/^##?\s/.test(l)) return <h3>{l.replace(/^##?\s/, '')}</h3>;
          if (/^\s*[-*]\s/.test(l)) return <p style={{ 'padding-left': '18px', 'text-indent': '-14px', margin: '0 0 6px' }}>• {inline(l.replace(/^\s*[-*]\s/, ''))}</p>;
          if (!l.trim()) return null;
          return <p>{inline(l)}</p>;
        }}
      </For>
    </div>
  );
}

export default function Releases() {
  const [data] = useReleases();
  return (
    <>
      <PageHero eyebrow="releases.genoryn.dev" title={<>Releases</>} lead="Every published GENORYN version, with notes, platforms and SHA-256 checksums — read live from the GitHub repository. Release 1.0.0 launched Mainnet 1.0; releases 0.4–0.6 belong to the retired network genoryn-mainnet-v1 and cannot connect to it." origin="Source: github.com/genoryn/genoryn/releases" />
      <section class="section tight">
        <div class="container narrow">
          <Show when={data()} fallback={<div class="empty">Loading releases from GitHub…</div>}>
            <Show when={data()!.releases.length} fallback={<div class="empty">No releases found. <ExtLink href={`${REPO_URL}/releases`}>Open GitHub</ExtLink></div>}>
              <For each={data()!.releases}>
                {(r, i) => (
                  <article class="card mb-3">
                    <div class="row between">
                      <div class="row">
                        <h2 style={{ margin: 0, 'font-family': 'var(--font-display)', 'font-size': '26px', 'letter-spacing': '-0.03em' }}>{r.name}</h2>
                        <Show when={i() === 0}>
                          <span class="badge solid">latest</span>
                        </Show>
                        <Show when={r.prerelease}>
                          <span class="badge">pre-release</span>
                        </Show>
                        <Show when={/^v?0\./.test(r.tag)}>
                          <span class="badge">retired network</span>
                        </Show>
                      </div>
                      <span class="mono muted" style={{ 'font-size': '13px' }}>{new Date(r.published_at).toISOString().slice(0, 10)} · {r.tag}</span>
                    </div>
                    <div class="mt-3">
                      <Notes body={r.body} />
                    </div>
                    <div class="table-wrap mt-3">
                      <table class="data">
                        <thead>
                          <tr>
                            <th>Package</th>
                            <th class="num">Size</th>
                            <th>SHA-256</th>
                            <th />
                          </tr>
                        </thead>
                        <tbody>
                          <For each={r.assets}>
                            {(a) => (
                              <tr>
                                <td class="mono strong" style={{ 'font-size': '13px' }}>{a.name}</td>
                                <td class="num mono">{formatBytes(a.size)}</td>
                                <td class="mono faint" style={{ 'font-size': '11.5px' }}>
                                  <Show when={a.sha256} fallback="—">
                                    <span class="row" style={{ gap: '6px', 'flex-wrap': 'nowrap' }}>
                                      {a.sha256!.slice(0, 16)}… <CopyButton text={a.sha256!} />
                                    </span>
                                  </Show>
                                </td>
                                <td class="num">
                                  <a class="btn sm" href={a.url} rel="noopener">
                                    <Icon name="download" size={14} />
                                  </a>
                                </td>
                              </tr>
                            )}
                          </For>
                        </tbody>
                      </table>
                    </div>
                    <div class="row mt-2">
                      <ExtLink href={r.url} class="btn sm ghost">
                        View on GitHub <Icon name="arrowUpRight" size={14} />
                      </ExtLink>
                    </div>
                  </article>
                )}
              </For>
            </Show>
          </Show>
        </div>
      </section>
    </>
  );
}
