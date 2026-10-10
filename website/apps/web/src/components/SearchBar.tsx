import { createSignal, Show } from 'solid-js';
import { useNavigate } from '@solidjs/router';
import type { SearchResult } from '@genoryn/shared';
import { api } from '~/lib/api';
import { Icon } from './Icon';

export function routeForResult(r: SearchResult): string {
  switch (r.type) {
    case 'block':
      return `/explorer/block/${r.height}`;
    case 'tx':
      return `/explorer/tx/${r.txid}`;
    case 'nullifier':
      return `/explorer/nullifier/${r.nullifier}`;
    case 'commitment':
      return `/explorer/commitment/${r.cmx}`;
    case 'username':
      return `/name?u=${encodeURIComponent(r.username)}`;
    case 'address':
      return `/explorer/search?q=${encodeURIComponent(r.address)}`;
    default:
      return `/explorer/search?q=${encodeURIComponent(r.query)}`;
  }
}

export function SearchBar(props: { big?: boolean; compact?: boolean; placeholder?: string; autofocus?: boolean; initial?: string; class?: string }) {
  const navigate = useNavigate();
  const [q, setQ] = createSignal(props.initial ?? '');
  const [busy, setBusy] = createSignal(false);
  let input!: HTMLInputElement;

  const submit = async (e: Event) => {
    e.preventDefault();
    const query = q().trim();
    if (!query || busy()) return;
    setBusy(true);
    try {
      const r = await api<SearchResult>(`/search?q=${encodeURIComponent(query)}`);
      navigate(routeForResult(r));
      if (props.compact) {
        setQ('');
        input.blur();
      }
    } catch {
      navigate(`/explorer/search?q=${encodeURIComponent(query)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <form class={`search ${props.big ? 'big' : ''} ${props.class ?? ''}`} role="search" onSubmit={submit}>
      <Icon name="search" size={props.big ? 20 : 16} class="lead-icon" />
      <label class="sr-only" for={props.compact ? 'q-header' : 'q-main'}>
        Search the GENORYN blockchain
      </label>
      <input
        ref={input}
        id={props.compact ? 'q-header' : 'q-main'}
        class={`input mono ${props.big ? 'big' : ''}`}
        type="search"
        autocomplete="off"
        spellcheck={false}
        autofocus={props.autofocus}
        placeholder={props.placeholder ?? 'Block height, hash, txid, nullifier, commitment, address or @name'}
        value={q()}
        onInput={(e) => setQ(e.currentTarget.value)}
        maxLength={200}
      />
      <Show when={!props.compact} fallback={<kbd>/</kbd>}>
        <button class={`btn primary ${props.big ? '' : 'sm'} submit`} type="submit" disabled={busy()} style={props.big ? { '--h': '50px' } : undefined}>
          {busy() ? 'Searching…' : 'Search'}
        </button>
      </Show>
    </form>
  );
}
