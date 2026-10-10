import { createMemo, createSignal, For } from 'solid-js';
import { A } from '@solidjs/router';
import { FAQ, REPO_URL } from '@genoryn/shared';
import { Icon } from '~/components/Icon';
import { ExtLink, PageHero, SectionHead } from '~/components/ui';

const TOPICS = [
  { id: 'all', label: 'All' },
  { id: 'basics', label: 'Basics' },
  { id: 'wallet', label: 'Wallet' },
  { id: 'mining', label: 'Mining' },
  { id: 'privacy', label: 'Privacy' },
  { id: 'network', label: 'Network' },
] as const;

export default function Support() {
  const [topic, setTopic] = createSignal<string>('all');
  const [q, setQ] = createSignal('');
  const items = createMemo(() => {
    const s = q().toLowerCase().trim();
    return FAQ.filter((f) => (topic() === 'all' || f.topic === topic()) && (!s || f.q.toLowerCase().includes(s) || f.a.toLowerCase().includes(s)));
  });
  return (
    <>
      <PageHero eyebrow="support.genoryn.org" title={<>How can we help?</>} lead="Answers to common questions about wallets, recovery, mining, privacy and the network." origin="Community-supported open-source project" />
      <section class="section tight">
        <div class="container narrow">
          <div class="search">
            <Icon name="search" size={18} class="lead-icon" />
            <input class="input" placeholder="Search questions…" value={q()} onInput={(e) => setQ(e.currentTarget.value)} aria-label="Search FAQ" />
          </div>
          <div class="tabs mt-2">
            <For each={TOPICS}>
              {(t) => (
                <button type="button" class={topic() === t.id ? 'active' : ''} onClick={() => setTopic(t.id)}>
                  {t.label}
                </button>
              )}
            </For>
          </div>
          <div class="mt-3">
            <For each={items()} fallback={<div class="empty">No matching questions.</div>}>
              {(f) => (
                <details class="faq">
                  <summary>{f.q}</summary>
                  <div class="answer">{f.a}</div>
                </details>
              )}
            </For>
          </div>
        </div>
      </section>
      <section class="section">
        <div class="container">
          <SectionHead eyebrow="Still stuck?" title={<>Get help</>} />
          <div class="grid cols-3">
            <ExtLink href={`${REPO_URL}/issues`} class="card hover spot">
              <div class="icon"><Icon name="github" size={20} /></div>
              <h3>Report a bug</h3>
              <p class="muted">Open an issue on GitHub with your platform, version and the steps to reproduce. Never include seeds, passwords or wallet files.</p>
            </ExtLink>
            <ExtLink href={`${REPO_URL}/blob/main/SECURITY.md`} class="card hover spot">
              <div class="icon"><Icon name="shield" size={20} /></div>
              <h3>Security issue</h3>
              <p class="muted">Report vulnerabilities privately following the security policy. Please do not disclose them publicly first.</p>
            </ExtLink>
            <A href="/docs" class="card hover spot">
              <div class="icon"><Icon name="book" size={20} /></div>
              <h3>Read the docs</h3>
              <p class="muted">Quickstart, recovery, CLI reference and node configuration for Windows and Linux.</p>
            </A>
          </div>
          <div class="callout warn mt-4">
            <Icon name="alert" size={18} style={{ 'flex-shrink': 0, 'margin-top': '2px' }} />
            <div>
              <b>Nobody from GENORYN will ever ask for your recovery phrase or password.</b> There is no official support chat, token sale, airdrop or exchange listing. Anyone claiming otherwise is a scammer.
            </div>
          </div>
        </div>
      </section>
    </>
  );
}
