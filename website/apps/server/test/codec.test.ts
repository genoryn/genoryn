import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { MAINNET_GENESIS_HASH, NETWORKS, genesisPreimage } from '@genoryn/shared';
import { blockHash, blockSize, bodyHash, domainHash, hex, parseBlocksJson, txId } from '../src/codec';
import { GENESIS, MAGIC_MAINNET } from '../src/crawler';

const load = (n: number) => {
  const text = readFileSync(new URL(`./fixtures/block-${n}.json`, import.meta.url), 'utf8');
  return parseBlocksJson(text)[0]!;
};

describe('consensus codec', () => {
  const b5 = load(5);
  const b6 = load(6);

  it('header hash of block 5 equals block 6 previous', () => {
    expect(blockHash(b5.header)).toBe(hex(b6.header.previous));
    expect(blockHash(b5.header)).toBe('810aa1ec26a569521348508a1d519fd0d45a09d3fbaef0485b10a6736b0ce8df');
  });

  it('body hash matches the header commitment', () => {
    expect(bodyHash(b5.body)).toBe(hex(b5.header.body_hash));
    expect(bodyHash(b6.body)).toBe(hex(b6.header.body_hash));
  });

  it('derives deterministic transaction ids', () => {
    expect(txId(b5.body.miner)).toBe('7b92dab55616a2f350d045dfd37859e532fc7841b3dbecb59c419de6a59e8cb6');
    expect(txId(b5.body.miner)).not.toBe(txId(b5.body.treasury!));
  });

  it('keeps 64-bit integers exact', () => {
    const big = parseBlocksJson('[{"header":{"nonce":18446744073709551615},"body":{"miner":{"bundle":{"value_balance":-9223372036854775808}}}}]')[0] as unknown as {
      header: { nonce: string };
      body: { miner: { bundle: { value_balance: string } } };
    };
    expect(big.header.nonce).toBe('18446744073709551615');
    expect(big.body.miner.bundle.value_balance).toBe('-9223372036854775808');
  });

  it('computes canonical block size', () => {
    expect(blockSize(b5)).toBeGreaterThan(10_000);
    expect(blockSize(b5)).toBeLessThan(2_000_000);
  });
});

describe('mainnet 1.0 identity', () => {
  it('genesis hash is the domain hash of the canonical preimage', () => {
    for (const n of Object.values(NETWORKS)) {
      expect(genesisPreimage(n)).toContain('|early:500x1000|');
    }
    expect(domainHash('GENORYN/genesis/v1', Buffer.from(genesisPreimage(NETWORKS.mainnet), 'utf8')).toString('hex')).toBe(MAINNET_GENESIS_HASH);
  });
  it('crawler speaks the mainnet 1.0 handshake', () => {
    expect(MAGIC_MAINNET.toString('hex')).toBe('474e5213');
    expect(GENESIS.toString('hex')).toBe('2b27dda9c2adcf9d61a58efc43a40213adc7fb2addfdae210f3357d72adf6b2e');
  });
});

describe('retired pre-1.0 mainnet codec (encoding unchanged in 1.0)', () => {
  const load = (n: number) => parseBlocksJson(readFileSync(new URL(`./fixtures/mainnet-block-${n}.json`, import.meta.url), 'utf8'))[0]!;
  it('chains real mainnet blocks and verifies body commitments', () => {
    const b5 = load(5);
    const b6 = load(6);
    expect(b5.header.network).toBe('Mainnet');
    expect(blockHash(b5.header)).toBe(hex(b6.header.previous));
    expect(bodyHash(b5.body)).toBe(hex(b5.header.body_hash));
    expect(b5.body.treasury).not.toBeNull();
  });
});
