/** Exact port of genoryn-core/src/emission.rs (GENORYN mainnet 1.0 monetary policy). All arithmetic is BigInt. */
import { GORI, MAX_SUPPLY } from './gori';
import { BLOCKS_PER_YEAR, EARLY_BLOCKS, EARLY_REWARD_GNR, HALVING_INTERVAL, NORMAL_START, TREASURY_EMISSION_BLOCKS, TREASURY_END, TREASURY_LOCK_HEIGHT } from './params';

export const EARLY_REWARD = BigInt(EARLY_REWARD_GNR) * GORI;
export const EARLY_BUDGET = BigInt(EARLY_BLOCKS) * EARLY_REWARD;
export const TREASURY_BUDGET = 210_000n * GORI;
export const NORMAL_BUDGET = MAX_SUPPLY - EARLY_BUDGET - TREASURY_BUDGET;
export const INITIAL_NORMAL_REWARD = NORMAL_BUDGET / (2n * BigInt(HALVING_INTERVAL));
export const U64_MAX = 18_446_744_073_709_551_615n;

const EARLY = BigInt(EARLY_BLOCKS);
const START = BigInt(NORMAL_START);
const T_END = BigInt(TREASURY_END);
const T_BLOCKS = BigInt(TREASURY_EMISSION_BLOCKS);
const INTERVAL = BigInt(HALVING_INTERVAL);

export interface Reward {
  normal: bigint;
  early: bigint;
  treasury: bigint;
  miner: bigint;
  total: bigint;
}

const H = (h: number | bigint) => BigInt(h);
const satSub = (a: bigint, b: bigint) => (a > b ? a - b : 0n);
const min = (a: bigint, b: bigint) => (a < b ? a : b);

/** Halving epoch of the normal reward; 0 before and during the first period. */
export function epoch(height: number | bigint): bigint {
  return satSub(H(height), START) / INTERVAL;
}

export function reward(height: number | bigint): Reward {
  const h = H(height);
  if (h === 0n) return { normal: 0n, early: 0n, treasury: 0n, miner: 0n, total: 0n };
  if (h <= EARLY) return { normal: 0n, early: EARLY_REWARD, treasury: 0n, miner: EARLY_REWARD, total: EARLY_REWARD };
  const e = epoch(h);
  const normal = e >= 128n ? 0n : INITIAL_NORMAL_REWARD >> e;
  const treasury = h <= T_END ? GORI : 0n;
  return { normal, early: 0n, treasury, miner: normal, total: normal + treasury };
}

export function normalIssued(height: number | bigint): bigint {
  let blocks = satSub(H(height), EARLY);
  let subsidy = INITIAL_NORMAL_REWARD;
  let issued = 0n;
  while (blocks > 0n && subsidy > 0n) {
    const n = min(blocks, INTERVAL);
    issued += subsidy * n;
    blocks -= n;
    subsidy >>= 1n;
  }
  return min(issued, NORMAL_BUDGET);
}

export function earlyIssued(height: number | bigint): bigint {
  return min(H(height), EARLY) * EARLY_REWARD;
}

export function treasuryIssued(height: number | bigint): bigint {
  return min(satSub(H(height), EARLY), T_BLOCKS) * GORI;
}

export function issued(height: number | bigint): bigint {
  return normalIssued(height) + earlyIssued(height) + treasuryIssued(height);
}

/** Ten equal 21,000 GNR tranches; the first becomes usable at year 10, the last at year 19. */
export function treasuryUnlockHeight(issuanceHeight: number | bigint): number | null {
  const i = H(issuanceHeight);
  if (i < START || i > T_END) return null;
  const tranche = (i - START) / (T_BLOCKS / 10n);
  return Number(BigInt(TREASURY_LOCK_HEIGHT) + tranche * BigInt(BLOCKS_PER_YEAR));
}

export function treasuryVested(height: number | bigint): bigint {
  const h = H(height);
  const lock = BigInt(TREASURY_LOCK_HEIGHT);
  if (h < lock) return 0n;
  let tranches = (h - lock) / BigInt(BLOCKS_PER_YEAR) + 1n;
  if (tranches > 10n) tranches = 10n;
  return tranches * (TREASURY_BUDGET / 10n);
}

export const LIFETIME_ROUNDING_REMAINDER = MAX_SUPPLY - issued(U64_MAX);
/** Lifetime issuance minus the burned mainnet treasury share. */
export const MAX_CIRCULATING = issued(U64_MAX) - TREASURY_BUDGET;

export interface Supply {
  denomination: 'gori';
  gori_per_gnr: string;
  max_supply: string;
  issued_supply: string;
  remaining_supply: string;
  normal_mining_issued: string;
  early_issued: string;
  treasury_issued: string;
  treasury_vested: string;
  current_block_reward: string;
  current_miner_reward: string;
  current_normal_reward: string;
  current_early_reward: string;
  current_treasury_reward: string;
  lifetime_rounding_remainder: string;
  height: number;
  halving_epoch: number;
  next_halving_height: number | null;
  early_blocks: number;
}

export function supply(height: number | bigint): Supply {
  const h = H(height);
  const r = reward(h);
  const iss = issued(h);
  const e = epoch(h);
  const next = (e + 1n) * INTERVAL + START;
  return {
    denomination: 'gori',
    gori_per_gnr: GORI.toString(),
    max_supply: MAX_SUPPLY.toString(),
    issued_supply: iss.toString(),
    remaining_supply: (MAX_SUPPLY - iss).toString(),
    normal_mining_issued: normalIssued(h).toString(),
    early_issued: earlyIssued(h).toString(),
    treasury_issued: treasuryIssued(h).toString(),
    treasury_vested: treasuryVested(h).toString(),
    current_block_reward: r.total.toString(),
    current_miner_reward: r.miner.toString(),
    current_normal_reward: r.normal.toString(),
    current_early_reward: r.early.toString(),
    current_treasury_reward: r.treasury.toString(),
    lifetime_rounding_remainder: LIFETIME_ROUNDING_REMAINDER.toString(),
    height: Number(h),
    halving_epoch: Number(e),
    next_halving_height: next > U64_MAX ? null : Number(next),
    early_blocks: EARLY_BLOCKS,
  };
}

/** Sampled issuance curve for charts: [year, issued gori]. */
export function emissionCurve(years = 40, step = 1): Array<{ year: number; height: number; issued: bigint; reward: bigint }> {
  const out = [];
  for (let y = 0; y <= years; y += step) {
    const height = y * BLOCKS_PER_YEAR;
    out.push({ year: y, height, issued: issued(height), reward: reward(Math.max(1, height)).total });
  }
  return out;
}
