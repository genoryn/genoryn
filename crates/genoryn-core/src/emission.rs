//! Consensus monetary policy (GENORYN mainnet 1.0). Height zero issues nothing;
//! all arithmetic is integer. See docs/protocol/emission.md.
//!
//! * Blocks 1-500: an early subsidy of exactly 1,000 GNR per block, paid to the
//!   miner of the block and nothing else (no normal reward, no treasury).
//! * From block 501: the normal reward, halving every 2,102,400 blocks (four
//!   years of 60-second blocks), plus 1 GNR per block to the treasury for
//!   210,000 blocks. On mainnet the treasury output goes to a provably
//!   unspendable address, so it is burned.
//! * Lifetime issuance never exceeds 21,000,000 GNR; rounding dust from integer
//!   halving is never minted.
use serde::Serialize;

pub type AtomicGnr = u128;
pub const GORI: AtomicGnr = 1_000_000_000_000;
pub const MAX_SUPPLY: AtomicGnr = 21_000_000 * GORI;
/// Blocks paying the early subsidy, starting at height 1.
pub const EARLY_BLOCKS: u64 = 500;
/// The whole block subsidy of each early block.
pub const EARLY_REWARD: AtomicGnr = 1_000 * GORI;
pub const EARLY_BUDGET: AtomicGnr = EARLY_BLOCKS as u128 * EARLY_REWARD;
pub const TREASURY_BUDGET: AtomicGnr = 210_000 * GORI;
pub const NORMAL_BUDGET: AtomicGnr = MAX_SUPPLY - EARLY_BUDGET - TREASURY_BUDGET;
pub const TARGET_SECONDS: u64 = 60;
pub const BLOCKS_PER_YEAR: u64 = 365 * 24 * 60 * 60 / TARGET_SECONDS;
pub const HALVING_INTERVAL: u64 = 4 * BLOCKS_PER_YEAR;
/// First height paying the normal reward and the treasury.
pub const NORMAL_START: u64 = EARLY_BLOCKS + 1;
pub const TREASURY_EMISSION_BLOCKS: u64 = 210_000;
/// Last height paying the treasury.
pub const TREASURY_END: u64 = EARLY_BLOCKS + TREASURY_EMISSION_BLOCKS;
pub const TREASURY_LOCK_HEIGHT: u64 = 10 * BLOCKS_PER_YEAR;
/// Chosen so that the geometric series of halvings stays within `NORMAL_BUDGET`.
pub const INITIAL_NORMAL_REWARD: AtomicGnr = NORMAL_BUDGET / (2 * HALVING_INTERVAL as u128);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reward {
    pub normal: AtomicGnr,
    pub early: AtomicGnr,
    pub treasury: AtomicGnr,
}
impl Reward {
    pub fn miner(self) -> AtomicGnr {
        self.normal + self.early
    }
    pub fn total(self) -> AtomicGnr {
        self.miner() + self.treasury
    }
}

/// Halving epoch of the normal reward; 0 before and during the first period.
pub fn epoch(height: u64) -> u64 {
    height.saturating_sub(NORMAL_START) / HALVING_INTERVAL
}

pub fn reward(height: u64) -> Reward {
    if height == 0 {
        return Reward {
            normal: 0,
            early: 0,
            treasury: 0,
        };
    }
    if height <= EARLY_BLOCKS {
        return Reward {
            normal: 0,
            early: EARLY_REWARD,
            treasury: 0,
        };
    }
    Reward {
        normal: INITIAL_NORMAL_REWARD
            .checked_shr(epoch(height).try_into().unwrap_or(u32::MAX))
            .unwrap_or(0),
        early: 0,
        treasury: if height <= TREASURY_END { GORI } else { 0 },
    }
}

/// Saturating height arithmetic makes queries at u64::MAX well-defined.
pub fn normal_issued(height: u64) -> AtomicGnr {
    let mut blocks = height.saturating_sub(EARLY_BLOCKS);
    let mut subsidy = INITIAL_NORMAL_REWARD;
    let mut issued = 0;
    while blocks > 0 && subsidy > 0 {
        let n = blocks.min(HALVING_INTERVAL);
        issued += subsidy * u128::from(n);
        blocks -= n;
        subsidy >>= 1;
    }
    issued.min(NORMAL_BUDGET)
}
pub fn early_issued(height: u64) -> AtomicGnr {
    u128::from(height.min(EARLY_BLOCKS)) * EARLY_REWARD
}
pub fn treasury_issued(height: u64) -> AtomicGnr {
    u128::from(
        height
            .saturating_sub(EARLY_BLOCKS)
            .min(TREASURY_EMISSION_BLOCKS),
    ) * GORI
}
pub fn issued(height: u64) -> AtomicGnr {
    normal_issued(height) + early_issued(height) + treasury_issued(height)
}

/// Ten equal 21,000 GNR tranches; first becomes usable at year 10, last at year 19.
/// Maturity is enforced by excluding locked commitments from spend anchors.
pub fn treasury_unlock_height(issuance_height: u64) -> Option<u64> {
    if !(NORMAL_START..=TREASURY_END).contains(&issuance_height) {
        return None;
    }
    let tranche = (issuance_height - NORMAL_START) / (TREASURY_EMISSION_BLOCKS / 10);
    Some(TREASURY_LOCK_HEIGHT + tranche * BLOCKS_PER_YEAR)
}
pub fn treasury_vested(height: u64) -> AtomicGnr {
    if height < TREASURY_LOCK_HEIGHT {
        return 0;
    }
    let tranches = ((height - TREASURY_LOCK_HEIGHT) / BLOCKS_PER_YEAR + 1).min(10);
    u128::from(tranches) * (TREASURY_BUDGET / 10)
}

#[derive(Serialize)]
pub struct Supply {
    pub denomination: &'static str,
    pub gori_per_gnr: String,
    pub max_supply: String,
    pub issued_supply: String,
    pub remaining_supply: String,
    pub normal_mining_issued: String,
    pub early_issued: String,
    pub treasury_issued: String,
    pub treasury_vested: String,
    pub current_block_reward: String,
    pub current_miner_reward: String,
    pub current_normal_reward: String,
    pub current_early_reward: String,
    pub current_treasury_reward: String,
    pub lifetime_rounding_remainder: String,
    pub height: u64,
    pub halving_epoch: u64,
    pub next_halving_height: Option<u64>,
    pub early_blocks: u64,
}
pub fn supply(height: u64) -> Supply {
    let r = reward(height);
    Supply {
        denomination: "gori",
        gori_per_gnr: GORI.to_string(),
        max_supply: MAX_SUPPLY.to_string(),
        issued_supply: issued(height).to_string(),
        remaining_supply: (MAX_SUPPLY - issued(height)).to_string(),
        normal_mining_issued: normal_issued(height).to_string(),
        early_issued: early_issued(height).to_string(),
        treasury_issued: treasury_issued(height).to_string(),
        treasury_vested: treasury_vested(height).to_string(),
        current_block_reward: r.total().to_string(),
        current_miner_reward: r.miner().to_string(),
        current_normal_reward: r.normal.to_string(),
        current_early_reward: r.early.to_string(),
        current_treasury_reward: r.treasury.to_string(),
        lifetime_rounding_remainder: (MAX_SUPPLY - issued(u64::MAX)).to_string(),
        height,
        halving_epoch: epoch(height),
        next_halving_height: epoch(height)
            .checked_add(1)
            .and_then(|e| e.checked_mul(HALVING_INTERVAL))
            .and_then(|h| h.checked_add(NORMAL_START)),
        early_blocks: EARLY_BLOCKS,
    }
}

pub fn parse_gnr(text: &str) -> Result<AtomicGnr, &'static str> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if whole.is_empty()
        || !whole.bytes().all(|c| c.is_ascii_digit())
        || fraction.len() > 12
        || !fraction.bytes().all(|c| c.is_ascii_digit())
    {
        return Err("amount must be a nonnegative decimal with at most 12 fractional digits");
    }
    let whole = whole.parse::<u128>().map_err(|_| "amount overflow")?;
    let fract = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<u128>().map_err(|_| "invalid fraction")?
            * 10u128.pow((12 - fraction.len()) as u32)
    };
    let amount = whole
        .checked_mul(GORI)
        .and_then(|n| n.checked_add(fract))
        .ok_or("amount overflow")?;
    if amount > MAX_SUPPLY {
        return Err("amount exceeds maximum supply");
    }
    Ok(amount)
}
pub fn format_gnr(amount: AtomicGnr) -> String {
    format!("{}.{:012}", amount / GORI, amount % GORI)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn the_first_500_blocks_pay_exactly_1000_gnr_and_nothing_else() {
        assert_eq!(reward(0).total(), 0, "genesis issues nothing");
        for height in [1, 2, 250, 499, 500] {
            let r = reward(height);
            assert_eq!(r.miner(), 1_000 * GORI, "height {height}");
            assert_eq!(r.total(), 1_000 * GORI, "the whole subsidy, not a bonus");
            assert_eq!((r.normal, r.treasury), (0, 0));
        }
        assert_eq!(early_issued(500), 500_000 * GORI);
        assert_eq!(issued(500), 500_000 * GORI);
        assert_eq!(EARLY_BUDGET, 500_000 * GORI);
        // 500,000 / 21,000,000 = 2.380952...%
        assert_eq!(EARLY_BUDGET * 1_000_000 / MAX_SUPPLY, 23_809);
    }

    #[test]
    fn block_501_starts_the_normal_schedule_and_the_treasury() {
        let r = reward(501);
        assert_eq!(r.early, 0);
        assert_eq!(r.normal, INITIAL_NORMAL_REWARD);
        assert_eq!(r.treasury, GORI);
        assert_eq!(INITIAL_NORMAL_REWARD, 4_825_437_595_129);
        assert_eq!(format_gnr(r.miner()), "4.825437595129");
        assert_eq!(issued(501), 500_000 * GORI + INITIAL_NORMAL_REWARD + GORI);
        assert_eq!(epoch(500), 0);
        assert_eq!(epoch(501), 0);
        assert_eq!(epoch(NORMAL_START + HALVING_INTERVAL - 1), 0);
        assert_eq!(epoch(NORMAL_START + HALVING_INTERVAL), 1);
    }

    #[test]
    fn lifetime_exhaustive_by_constant_reward_interval() {
        // Every possible reward-changing epoch, including all zero-emission epochs.
        let mut total = 0;
        for e in 0..128u32 {
            total += INITIAL_NORMAL_REWARD.checked_shr(e).unwrap_or(0) * HALVING_INTERVAL as u128;
        }
        assert_eq!(total, normal_issued(u64::MAX));
        assert!(total <= NORMAL_BUDGET);
        assert_eq!(early_issued(u64::MAX), EARLY_BUDGET);
        assert_eq!(treasury_issued(u64::MAX), TREASURY_BUDGET);
        assert!(issued(u64::MAX) <= MAX_SUPPLY);
        assert_eq!(reward(u64::MAX).total(), 0);
        assert_eq!(HALVING_INTERVAL, 2_102_400);
        // Exact lifetime figures, documented in docs/protocol/emission.md.
        assert_eq!(normal_issued(u64::MAX), 20_289_999_999_950_064_000);
        assert_eq!(issued(u64::MAX), 20_999_999_999_950_064_000);
        assert_eq!(MAX_SUPPLY - issued(u64::MAX), 49_936_000);
    }

    #[test]
    fn all_halving_boundaries() {
        assert_eq!(reward(0).total(), 0);
        for e in 1..128u64 {
            let h = NORMAL_START - 1 + e * HALVING_INTERVAL;
            assert_eq!(reward(h + 1).normal, reward(h).normal / 2, "epoch {e}");
            assert_eq!(issued(h + 1) - issued(h), reward(h + 1).total());
        }
        assert_eq!(
            supply(501).next_halving_height,
            Some(NORMAL_START + HALVING_INTERVAL)
        );
    }

    #[test]
    fn treasury_boundaries_and_vesting() {
        assert_eq!(reward(500).treasury, 0);
        assert_eq!(reward(501).treasury, GORI);
        assert_eq!(reward(TREASURY_END).treasury, GORI);
        assert_eq!(reward(TREASURY_END + 1).treasury, 0);
        assert_eq!(treasury_unlock_height(500), None);
        assert_eq!(treasury_unlock_height(TREASURY_END + 1), None);
        assert_eq!(treasury_vested(TREASURY_LOCK_HEIGHT - 1), 0);
        let mut tranches = [0u128; 10];
        for h in NORMAL_START..=TREASURY_END {
            let unlock = treasury_unlock_height(h).unwrap();
            assert!(unlock >= TREASURY_LOCK_HEIGHT);
            tranches[((unlock - TREASURY_LOCK_HEIGHT) / BLOCKS_PER_YEAR) as usize] +=
                reward(h).treasury;
        }
        assert_eq!(tranches, [TREASURY_BUDGET / 10; 10]);
        for year in 0..10u64 {
            assert_eq!(
                treasury_vested(TREASURY_LOCK_HEIGHT + year * BLOCKS_PER_YEAR),
                (year as u128 + 1) * TREASURY_BUDGET / 10
            );
        }
        assert_eq!(treasury_vested(u64::MAX), TREASURY_BUDGET);
    }

    #[test]
    fn no_issuance_is_counted_twice() {
        for height in [
            0, 1, 499, 500, 501, 502, 210_500, 210_501, 2_102_900, 2_102_901,
        ] {
            let parts = normal_issued(height) + early_issued(height) + treasury_issued(height);
            assert_eq!(parts, issued(height));
            let summed: u128 = (1..=height.min(600)).map(|h| reward(h).total()).sum();
            if height <= 600 {
                assert_eq!(summed, issued(height), "height {height}");
            }
        }
    }

    #[test]
    fn decimal_exactness() {
        assert_eq!(parse_gnr("0.000000000001").unwrap(), 1);
        assert_eq!(parse_gnr("21000000").unwrap(), MAX_SUPPLY);
        for bad in [
            "-1",
            "NaN",
            "1e3",
            "1.0000000000001",
            "21000000.000000000001",
            "1.2.3",
            "",
        ] {
            assert!(parse_gnr(bad).is_err());
        }
    }
    proptest! {
        #[test]
        fn cumulative_consistency(h in 1..u64::MAX) {
            prop_assert!(issued(h) <= MAX_SUPPLY);
            prop_assert_eq!(issued(h)-issued(h-1), reward(h).total());
        }
        #[test]
        fn early_range_is_exact(h in 1..=EARLY_BLOCKS) {
            prop_assert_eq!(reward(h).total(), EARLY_REWARD);
            prop_assert_eq!(issued(h), u128::from(h) * EARLY_REWARD);
        }
        #[test]
        fn decimal_roundtrip(a in 0u128..MAX_SUPPLY) { prop_assert_eq!(parse_gnr(&format_gnr(a)), Ok(a)); }
    }
}
