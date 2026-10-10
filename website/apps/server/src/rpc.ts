import type { NameStatus, NodeInfo, PeerSummary } from '@genoryn/shared';
import { config } from './config';
import { parseBlocksJson, type Block } from './codec';

async function get(base: string, path: string, timeoutMs = 10_000): Promise<string> {
  const res = await fetch(base + path, { signal: AbortSignal.timeout(timeoutMs), headers: { accept: 'application/json' } });
  const text = await res.text();
  if (!res.ok) {
    let msg = text.slice(0, 200);
    try {
      msg = (JSON.parse(text) as { error?: string }).error ?? msg;
    } catch {
      /* plain-text error */
    }
    throw new RpcError(res.status, msg);
  }
  if (text.length > 16 * 1024 * 1024) throw new RpcError(502, 'RPC response too large');
  return text;
}

export class RpcError extends Error {
  constructor(public status: number, message: string) {
    super(message);
  }
}

export const rpc = {
  async info(): Promise<NodeInfo> {
    return JSON.parse(await get(config.publicRpc, '/v1/info', 5000)) as NodeInfo;
  },
  async genesis(): Promise<Record<string, unknown>> {
    return JSON.parse(await get(config.publicRpc, '/v1/genesis')) as Record<string, unknown>;
  },
  async supply(): Promise<Record<string, unknown>> {
    return JSON.parse(await get(config.publicRpc, '/v1/supply')) as Record<string, unknown>;
  },
  async block(height: number): Promise<{ block: Block; rawText: string } | null> {
    const text = await get(config.publicRpc, `/v1/blocks?from=${height}&limit=1`, 30_000);
    const blocks = parseBlocksJson(text);
    const block = blocks[0];
    if (!block) return null;
    return { block, rawText: text.slice(1, -1) };
  },
  async username(name: string): Promise<Record<string, unknown>> {
    return JSON.parse(await get(config.publicRpc, `/v1/usernames?username=${encodeURIComponent(name)}`)) as Record<string, unknown>;
  },
  async name(name: string): Promise<NameStatus> {
    return JSON.parse(await get(config.publicRpc, `/v1/names?name=${encodeURIComponent(name)}`)) as NameStatus;
  },
  async namesByOwner(owner: string): Promise<{ names: NameStatus['registered'][]; pending: string[] }> {
    return JSON.parse(await get(config.publicRpc, `/v1/names/owner?owner=${owner}`)) as { names: NameStatus['registered'][]; pending: string[] };
  },
  async peers(): Promise<PeerSummary[]> {
    return JSON.parse(await get(config.adminRpc, '/v1/peers', 5000)) as PeerSummary[];
  },
};
