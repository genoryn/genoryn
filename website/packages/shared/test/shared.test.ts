import { describe, expect, it } from 'vitest';
import {
  EARLY_BUDGET,
  INITIAL_NORMAL_REWARD,
  LIFETIME_ROUNDING_REMAINDER,
  MAX_CIRCULATING,
  NORMAL_BUDGET,
  buildPaymentUri,
  canonicalUsername,
  earlyIssued,
  emissionCurve,
  epoch,
  formatGnr,
  issued,
  normalIssued,
  reward,
  treasuryIssued,
  goriToGnr,
  parseGnr,
  parsePaymentUri,
  supply,
  treasuryUnlockHeight,
  validateAddress,
} from '../src';

const ADDRESS = 'gnrd1nqynarfyv0mz5cyst7cg23y6w8aur2vcncv4wgtfeu5xcp2jd6kvna59f534f0wymxdrc0my2nm';

const GORI = 1_000_000_000_000n;
const U64_MAX = 18_446_744_073_709_551_615n;

describe('emission (exact port of genoryn-core emission.rs, mainnet 1.0)', () => {
  it('budgets and first normal subsidy', () => {
    expect(EARLY_BUDGET).toBe(500_000n * GORI);
    expect(NORMAL_BUDGET).toBe(20_290_000n * GORI);
    expect(INITIAL_NORMAL_REWARD).toBe(4_825_437_595_129n);
  });
  it('height 0 issues nothing', () => {
    expect(reward(0)).toEqual({ normal: 0n, early: 0n, treasury: 0n, miner: 0n, total: 0n });
    const s = supply(0);
    expect(s.issued_supply).toBe('0');
    expect(s.early_issued).toBe('0');
    expect(s.current_block_reward).toBe('0');
    expect(s.early_blocks).toBe(500);
  });
  it('blocks 1–500 pay exactly 1,000 GNR, nothing else', () => {
    for (const h of [1, 499, 500]) {
      expect(reward(h)).toEqual({ normal: 0n, early: 1_000n * GORI, treasury: 0n, miner: 1_000n * GORI, total: 1_000n * GORI });
    }
    expect(issued(1)).toBe(1_000n * GORI);
    expect(issued(499)).toBe(499_000n * GORI);
    expect(issued(500)).toBe(500_000_000_000_000_000n);
    expect(treasuryIssued(500)).toBe(0n);
    expect(normalIssued(500)).toBe(0n);
    expect(epoch(500)).toBe(0n);
  });
  it('block 501 starts the normal reward and the treasury', () => {
    const r = reward(501);
    expect(r.early).toBe(0n);
    expect(r.normal).toBe(4_825_437_595_129n);
    expect(r.miner).toBe(4_825_437_595_129n);
    expect(r.treasury).toBe(GORI);
    expect(r.total).toBe(5_825_437_595_129n);
    expect(issued(501)).toBe(500_005_825_437_595_129n);
    const s = supply(501);
    expect(s.issued_supply).toBe('500005825437595129');
    expect(s.early_issued).toBe('500000000000000000');
    expect(s.normal_mining_issued).toBe('4825437595129');
    expect(s.treasury_issued).toBe('1000000000000');
    expect(s.current_miner_reward).toBe('4825437595129');
    expect(s.current_early_reward).toBe('0');
    expect(s.halving_epoch).toBe(0);
    expect(s.next_halving_height).toBe(2_102_901);
  });
  it('halving boundary 2,102,900 / 2,102,901', () => {
    expect(epoch(2_102_900)).toBe(0n);
    expect(reward(2_102_900).normal).toBe(INITIAL_NORMAL_REWARD);
    expect(epoch(2_102_901)).toBe(1n);
    expect(reward(2_102_901).normal).toBe(INITIAL_NORMAL_REWARD >> 1n);
    expect(supply(2_102_901).next_halving_height).toBe(2_102_900 + 2_102_401);
    expect(normalIssued(2_102_900)).toBe(INITIAL_NORMAL_REWARD * 2_102_400n);
  });
  it('treasury boundary 210,500 / 210,501', () => {
    expect(reward(210_500).treasury).toBe(GORI);
    expect(reward(210_501).treasury).toBe(0n);
    expect(treasuryIssued(210_500)).toBe(210_000n * GORI);
    expect(treasuryIssued(210_501)).toBe(210_000n * GORI);
    expect(treasuryIssued(U64_MAX)).toBe(210_000n * GORI);
  });
  it('u64::MAX lifetime totals and rounding remainder', () => {
    expect(normalIssued(U64_MAX)).toBe(20_289_999_999_950_064_000n);
    expect(earlyIssued(U64_MAX)).toBe(500_000n * GORI);
    expect(issued(U64_MAX)).toBe(20_999_999_999_950_064_000n);
    expect(LIFETIME_ROUNDING_REMAINDER).toBe(49_936_000n);
    expect(supply(1).lifetime_rounding_remainder).toBe('49936000');
    expect(MAX_CIRCULATING).toBe(20_789_999_999_950_064_000n);
    expect(reward(U64_MAX).normal).toBe(0n);
    expect(supply(U64_MAX).next_halving_height).toBeNull();
  });
  it('supply fields match the node /v1/supply contract', () => {
    expect(Object.keys(supply(1234)).sort()).toEqual(
      [
        'denomination', 'gori_per_gnr', 'max_supply', 'issued_supply', 'remaining_supply', 'normal_mining_issued', 'early_issued',
        'treasury_issued', 'treasury_vested', 'current_block_reward', 'current_miner_reward', 'current_normal_reward',
        'current_early_reward', 'current_treasury_reward', 'lifetime_rounding_remainder', 'height', 'halving_epoch',
        'next_halving_height', 'early_blocks',
      ].sort(),
    );
  });
  it('treasury tranches', () => {
    expect(treasuryUnlockHeight(500)).toBeNull();
    expect(treasuryUnlockHeight(501)).toBe(5_256_000);
    expect(treasuryUnlockHeight(21_500)).toBe(5_256_000);
    expect(treasuryUnlockHeight(21_501)).toBe(5_256_000 + 525_600);
    expect(treasuryUnlockHeight(210_500)).toBe(5_256_000 + 9 * 525_600);
    expect(treasuryUnlockHeight(210_501)).toBeNull();
  });
  it('emission curve stays below the cap', () => {
    const curve = emissionCurve(40, 1);
    expect(curve[0]!.issued).toBe(0n);
    expect(curve.at(-1)!.issued <= 21_000_000n * GORI).toBe(true);
  });
});

describe('gori math', () => {
  it('exact conversions', () => {
    expect(goriToGnr(7_920_566_971_080n)).toBe('7.92056697108');
    expect(goriToGnr('1000000000000')).toBe('1');
    expect(parseGnr('1.000000000001')).toBe(1_000_000_000_001n);
    expect(formatGnr(21_000_000n * 10n ** 12n, 2)).toBe('21,000,000');
    expect(() => parseGnr('1e3')).toThrow();
    expect(() => parseGnr('0.0000000000001')).toThrow();
  });
});

describe('addresses and URIs', () => {
  it('validates the real miner address', () => {
    const r = validateAddress(ADDRESS, 'devnet');
    expect(r.valid).toBe(true);
    expect(r.byteLength).toBe(43);
  });
  it('rejects a corrupted checksum and other networks', () => {
    expect(validateAddress(ADDRESS.slice(0, -1) + 'q').valid).toBe(false);
    expect(validateAddress(ADDRESS, 'testnet').valid).toBe(false);
    expect(validateAddress('bc1qxyz').valid).toBe(false);
  });
  it('payment URI roundtrip and canonical amounts', () => {
    const uri = buildPaymentUri('devnet', ADDRESS, '1.500');
    expect(uri).toBe(`genoryn:${ADDRESS}?v=1&network=devnet&amount=1.5`);
    expect(parsePaymentUri(uri, 'devnet').amountGori).toBe(1_500_000_000_000n);
    expect(() => parsePaymentUri(`genoryn:${ADDRESS}?v=1&network=devnet&amount=1.50`, 'devnet')).toThrow();
    expect(() => parsePaymentUri(`genoryn:${ADDRESS}?v=1&v=1&network=devnet`, 'devnet')).toThrow();
    expect(() => parsePaymentUri(`genoryn:${ADDRESS}?v=1&network=testnet`, 'devnet')).toThrow();
    expect(() => parsePaymentUri(`genoryn:${ADDRESS}?v=1&network=devnet&memo=hi`, 'devnet')).toThrow();
  });
  it('username canonicalization', () => {
    expect(canonicalUsername('@Alice_1')).toEqual({ ok: true, name: 'alice_1' });
    expect(canonicalUsername('1abc').ok).toBe(false);
    expect(canonicalUsername('ab').ok).toBe(false);
  });
});

describe('mainnet parameters', () => {
  it('validates gnr1 addresses and rejects reserved names', async () => {
    const { validateAddress, canonicalUsername, NETWORKS, genesisPreimage, nameRegistrationPriceGnr } = await import('../src');
    expect(validateAddress('gnr16km85nuxptk0dmmls9j0whtv2rpedtahldzwf06gv2gdpsye05rh95eh2hc49sf0vp8t5jxw7lz', 'mainnet').valid).toBe(true);
    expect(canonicalUsername('@Treasury').ok).toBe(false);
    expect(genesisPreimage(NETWORKS.mainnet)).toBe(
      'genoryn-mainnet-1.0|Privacy is not a feature. Privacy is the default.|GNR|21000000|1000000000000|RandomX-v1|Ironwood-v3|early:500x1000|2026-10-10-mainnet-1.0',
    );
    expect(nameRegistrationPriceGnr('max')).toBe(1000n);
    expect(nameRegistrationPriceGnr('maxim')).toBe(100n);
    expect(nameRegistrationPriceGnr('maximus')).toBe(10n);
  });
  it('mainnet 1.0 network identity', async () => {
    const { NETWORKS, genesisPreimage, MAINNET_GENESIS_HASH, NODE_VERSION, GENORYN_IO_ONION } = await import('../src');
    expect(NETWORKS.mainnet.chainId).toBe('genoryn-mainnet-1.0');
    expect(NETWORKS.testnet.chainId).toBe('genoryn-testnet-1.0');
    expect(NETWORKS.devnet.chainId).toBe('genoryn-devnet-1.0');
    expect(NETWORKS.mainnet.magic).toBe('0x474e5213');
    expect(NETWORKS.testnet.magic).toBe('0x474e5212');
    expect(NETWORKS.devnet.magic).toBe('0x474e5211');
    expect([NETWORKS.mainnet.p2pPort, NETWORKS.mainnet.adminRpcPort, NETWORKS.mainnet.publicRpcPort]).toEqual([29710, 29711, 29712]);
    expect([NETWORKS.testnet.p2pPort, NETWORKS.devnet.p2pPort]).toEqual([29510, 29410]);
    expect(NETWORKS.mainnet.maturity).toBe(60);
    expect(NETWORKS.devnet.maturity).toBe(2);
    expect(genesisPreimage(NETWORKS.testnet).endsWith('|early:500x1000|2026-10-10-testnet-1.0')).toBe(true);
    expect(genesisPreimage(NETWORKS.devnet).startsWith('genoryn-devnet-1.0|')).toBe(true);
    expect(MAINNET_GENESIS_HASH).toBe('2b27dda9c2adcf9d61a58efc43a40213adc7fb2addfdae210f3357d72adf6b2e');
    expect(NODE_VERSION).toBe('1.0.0');
    expect(GENORYN_IO_ONION).toBe('2a6gncfqgp56dgd73vbbicav4552lepqzk6rakltyuuz7hqmttq2diad.onion');
  });
});
