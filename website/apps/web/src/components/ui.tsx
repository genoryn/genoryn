import { createSignal, For, Show, type JSX, type ParentProps } from 'solid-js';
import { A } from '@solidjs/router';
import { formatGnr, shortHash, timeAgo, utc } from '@genoryn/shared';
import { Icon } from './Icon';
import { GSign } from './Logo';
import { copyText } from '~/lib/dom';
import { now, rawStats, stats, statsReceivedAt } from '~/lib/live';

/** Shown whenever the figures on screen are not live: the node is unreachable or the live stream stopped. */
export function StaleNote(props: { class?: string }) {
  const message = () => {
    const s = rawStats();
    if (!s) return null;
    if (s.stale) {
      if (!s.last_updated) return 'Node unreachable · no data has been received from it yet, so live figures are unavailable.';
      const tail = stats() ? 'Figures below are the last known values, not the live network state.' : 'Live figures are unavailable.';
      return `Last updated ${utc(s.last_updated)} (${timeAgo(s.last_updated, now())}) · node unreachable. ${tail}`;
    }
    const received = statsReceivedAt();
    if (received && now() - received > 60) return `Last updated ${timeAgo(received, now())} · live connection lost, reconnecting. Figures below are the last known values.`;
    return null;
  };
  return (
    <Show when={message()}>
      <Callout warn icon="alert" class={props.class}>
        <span style={{ 'font-size': '13.5px' }} role="status">
          {message()}
        </span>
      </Callout>
    </Show>
  );
}

export function CopyButton(props: { text: string; label?: string }) {
  const [done, setDone] = createSignal(false);
  return (
    <button
      type="button"
      class="copy-btn"
      aria-label={props.label ?? 'Copy to clipboard'}
      title={done() ? 'Copied' : 'Copy'}
      onClick={async (e) => {
        e.preventDefault();
        e.stopPropagation();
        if (await copyText(props.text)) {
          setDone(true);
          setTimeout(() => setDone(false), 1400);
        }
      }}
    >
      <Icon name={done() ? 'check' : 'copy'} size={13} />
    </button>
  );
}

export function Hash(props: { value: string; href?: string; short?: boolean; head?: number; tail?: number; copy?: boolean; class?: string }) {
  const text = () => (props.short === false ? props.value : shortHash(props.value, props.head ?? 10, props.tail ?? 8));
  return (
    <span class={`hash ${props.class ?? ''}`} title={props.value}>
      <Show when={props.href} fallback={<code class="mono">{text()}</code>}>
        <A href={props.href!} class="link mono">
          {text()}
        </A>
      </Show>
      <Show when={props.copy !== false && props.value}>
        <CopyButton text={props.value} />
      </Show>
    </span>
  );
}

export function Gnr(props: { gori: string | bigint | null | undefined; digits?: number; unit?: boolean; sign?: boolean; class?: string }) {
  return (
    <span class={`gnr-amount ${props.class ?? ''}`}>
      <Show when={props.sign !== false}>
        <GSign size={13} class="gnr-sign" />
      </Show>
      <span>{props.gori === null || props.gori === undefined ? '—' : formatGnr(props.gori, props.digits ?? 4)}</span>
      <Show when={props.unit !== false}>
        <span class="gnr-unit">GNR</span>
      </Show>
    </span>
  );
}

export function TimeAgo(props: { ts: number | null | undefined }) {
  return <span class="nowrap">{props.ts ? timeAgo(props.ts, now()) : '—'}</span>;
}

export function LiveDot(props: { on: boolean; err?: boolean }) {
  return <span class={`live-dot ${props.err ? 'err' : props.on ? '' : 'off'}`} aria-hidden="true" />;
}

export function Stat(props: { label: string; value: JSX.Element; sub?: JSX.Element; icon?: string; live?: boolean; class?: string }) {
  return (
    <div class={`stat ${props.class ?? ''}`}>
      <div class="label">
        <Show when={props.live !== undefined}>
          <LiveDot on={!!props.live} />
        </Show>
        <Show when={props.icon}>
          <Icon name={props.icon!} size={13} />
        </Show>
        {props.label}
      </div>
      <div class="value">{props.value}</div>
      <Show when={props.sub}>
        <div class="sub">{props.sub}</div>
      </Show>
    </div>
  );
}

export function Callout(props: ParentProps<{ warn?: boolean; icon?: string; class?: string }>) {
  return (
    <div class={`callout ${props.warn ? 'warn' : ''} ${props.class ?? ''}`}>
      <Icon name={props.icon ?? (props.warn ? 'alert' : 'shield')} size={18} style={{ 'flex-shrink': 0, 'margin-top': '2px' }} />
      <div>{props.children}</div>
    </div>
  );
}

export function CodeBlock(props: { code: string; title?: string; lang?: string }) {
  const lines = () =>
    props.code.split('\n').map((l) => {
      if (l.trimStart().startsWith('#')) return <span class="c">{l}</span>;
      if (l.startsWith('$ ')) return (<><span class="p">$ </span>{l.slice(2)}</>);
      return <>{l}</>;
    });
  const copyable = () =>
    props.code
      .split('\n')
      .filter((l) => !l.trimStart().startsWith('#'))
      .map((l) => (l.startsWith('$ ') ? l.slice(2) : l))
      .join('\n')
      .trim();
  return (
    <div class="code">
      <div class="code-head">
        <span>{props.title ?? props.lang ?? 'shell'}</span>
        <CopyButton text={copyable()} />
      </div>
      <pre>
        <code>
          <For each={lines()}>{(l, i) => (<>{l}{i() < lines().length - 1 ? '\n' : ''}</>)}</For>
        </code>
      </pre>
    </div>
  );
}

export function PageHero(props: ParentProps<{ eyebrow: string; title: JSX.Element; lead?: JSX.Element; origin?: string; actions?: JSX.Element }>) {
  return (
    <header class="page-hero">
      <div class="bg-grid" />
      <div class="bg-glow" style={{ top: '-600px', right: '-300px' }} />
      <div class="container" style={{ position: 'relative' }}>
        <div class="eyebrow">{props.eyebrow}</div>
        <h1>{props.title}</h1>
        <Show when={props.lead}>
          <p class="lead">{props.lead}</p>
        </Show>
        <Show when={props.actions}>
          <div class="row mt-3">{props.actions}</div>
        </Show>
        {props.children}
        <Show when={props.origin}>
          <div class="origin-tag">// {props.origin}</div>
        </Show>
      </div>
    </header>
  );
}

export function Kv(props: { rows: Array<[JSX.Element, JSX.Element]> }) {
  return (
    <div class="kv">
      <For each={props.rows}>
        {([k, v]) => (
          <>
            <div class="k">{k}</div>
            <div class="v">{v}</div>
          </>
        )}
      </For>
    </div>
  );
}

export function SectionHead(props: { eyebrow: string; title: JSX.Element; lead?: JSX.Element; center?: boolean }) {
  return (
    <div class={`section-head reveal ${props.center ? 'center' : ''}`}>
      <div class="eyebrow">{props.eyebrow}</div>
      <h2>{props.title}</h2>
      <Show when={props.lead}>
        <p class="lead" style={props.center ? { margin: '0 auto' } : undefined}>
          {props.lead}
        </p>
      </Show>
    </div>
  );
}

export function ExtLink(props: ParentProps<{ href: string; class?: string }>) {
  return (
    <a href={props.href} class={props.class} target="_blank" rel="noopener noreferrer">
      {props.children}
    </a>
  );
}

export function Feature(props: { icon: string; title: string; children: JSX.Element }) {
  return (
    <div class="card spot hover reveal">
      <div class="icon">
        <Icon name={props.icon} size={20} />
      </div>
      <h3>{props.title}</h3>
      <p class="muted">{props.children}</p>
    </div>
  );
}
