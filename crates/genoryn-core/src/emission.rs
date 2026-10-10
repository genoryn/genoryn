//! Consensus monetary policy. Height zero issues nothing; all arithmetic is integer.
use serde::Serialize;

pub type AtomicGnr = u128;
pub const GORI: AtomicGnr = 1_000_000_000_000;
pub const MAX_SUPPLY: AtomicGnr = 21_000_000 * GORI;
pub const BOOTSTRAP_BUDGET: AtomicGnr = 100_000 * GORI;
pub const TREASURY_BUDGET: AtomicGnr = 210_000 * GORI;
pub const NORMAL_BUDGET: AtomicGnr = MAX_SUPPLY - BOOTSTRAP_BUDGET - TREASURY_BUDGET;
pub const TARGET_SECONDS: u64 = 60;
pub const BLOCKS_PER_YEAR: u64 = 365 * 24 * 60 * 60 / TARGET_SECONDS;
pub const HALVING_INTERVAL: u64 = 4 * BLOCKS_PER_YEAR;
pub const BOOTSTRAP_BLOCKS: u64 = 50_000;
pub const TREASURY_EMISSION_BLOCKS: u64 = 210_000;
pub const TREASURY_LOCK_HEIGHT: u64 = 10 * BLOCKS_PER_YEAR;
pub const INITIAL_NORMAL_REWARD: AtomicGnr = NORMAL_BUDGET / (2 * HALVING_INTERVAL as u128);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reward {
    pub normal: AtomicGnr,
    pub bootstrap: AtomicGnr,
    pub treasury: AtomicGnr,
}
impl Reward {
    pub fn miner(self) -> AtomicGnr {
        self.normal + self.bootstrap
    }
    pub fn total(self) -> AtomicGnr {
        self.miner() + self.treasury
    }
}

pub fn epoch(height: u64) -> u64 {
    height.saturating_sub(1) / HALVING_INTERVAL
}

pub fn reward(height: u64) -> Reward {
    if height == 0 {
        return Reward {
            normal: 0,
            bootstrap: 0,
            treasury: 0,
        };
    }
    Reward {
        normal: INITIAL_NORMAL_REWARD
            .checked_shr(epoch(height).try_into().unwrap_or(u32::MAX))
            .unwrap_or(0),
        bootstrap: if height <= BOOTSTRAP_BLOCKS {
            2 * GORI
        } else {
            0
        },
        treasury: if height <= TREASURY_EMISSION_BLOCKS {
            GORI
        } else {
            0
        },
    }
}

/// Saturating height arithmetic makes queries at u64::MAX well-defined.
pub fn normal_issued(height: u64) -> AtomicGnr {
    let mut blocks = height;
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
pub fn bootstrap_issued(height: u64) -> AtomicGnr {
    u128::from(height.min(BOOTSTRAP_BLOCKS)) * 2 * GORI
}
pub fn treasury_issued(height: u64) -> AtomicGnr {
    u128::from(height.min(TREASURY_EMISSION_BLOCKS)) * GORI
}
pub fn issued(height: u64) -> AtomicGnr {
    normal_issued(height) + bootstrap_issued(height) + treasury_issued(height)
}

/// Ten equal 21,000 GNR tranches; first becomes usable at year 10, last at year 19.
/// Maturity is enforced by excluding locked commitments from spend anchors.
pub fn treasury_unlock_height(issuance_height: u64) -> Option<u64> {
    if !(1..=TREASURY_EMISSION_BLOCKS).contains(&issuance_height) {
        return None;
    }
    let tranche = (issuance_height - 1) / (TREASURY_EMISSION_BLOCKS / 10);
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
    pub bootstrap_issued: String,
    pub treasury_issued: String,
    pub treasury_vested: String,
    pub current_block_reward: String,
    pub current_miner_reward: String,
    pub current_normal_reward: String,
    pub current_bootstrap_reward: String,
    pub current_treasury_reward: String,
    pub lifetime_rounding_remainder: String,
    pub height: u64,
    pub halving_epoch: u64,
    pub next_halving_height: Option<u64>,
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
        bootstrap_issued: bootstrap_issued(height).to_string(),
        treasury_issued: treasury_issued(height).to_string(),
        treasury_vested: treasury_vested(height).to_string(),
        current_block_reward: r.total().to_string(),
        current_miner_reward: r.miner().to_string(),
        current_normal_reward: r.normal.to_string(),
        current_bootstrap_reward: r.bootstrap.to_string(),
        current_treasury_reward: r.treasury.to_string(),
        lifetime_rounding_remainder: (MAX_SUPPLY - issued(u64::MAX)).to_string(),
        height,
        halving_epoch: epoch(height),
        next_halving_height: epoch(height)
            .checked_add(1)
            .and_then(|e| e.checked_mul(HALVING_INTERVAL))
            .and_then(|h| h.checked_add(1)),
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
    fn lifetime_exhaustive_by_constant_reward_interval() {
        // Every possible reward-changing epoch, including all zero-emission epochs.
        let mut total = 0;
        for e in 0..128u32 {
            total += INITIAL_NORMAL_REWARD.checked_shr(e).unwrap_or(0) * HALVING_INTERVAL as u128;
        }
        assert_eq!(total, normal_issued(u64::MAX));
        assert!(total <= NORMAL_BUDGET);
        assert_eq!(bootstrap_issued(u64::MAX), BOOTSTRAP_BUDGET);
        assert_eq!(treasury_issued(u64::MAX), TREASURY_BUDGET);
        assert!(issued(u64::MAX) <= MAX_SUPPLY);
        assert_eq!(reward(u64::MAX).total(), 0);
        assert_eq!(HALVING_INTERVAL, 2_102_400);
    }
    #[test]
    fn all_halving_boundaries() {
        assert_eq!(reward(0).total(), 0);
        for e in 1..128u64 {
            let h = e * HALVING_INTERVAL;
            assert_eq!(reward(h + 1).normal, reward(h).normal / 2);
            assert_eq!(issued(h + 1) - issued(h), reward(h + 1).total());
        }
    }
    #[test]
    fn bootstrap_and_treasury_boundaries() {
        assert_eq!(reward(50_000).bootstrap, 2 * GORI);
        assert_eq!(reward(50_001).bootstrap, 0);
        assert_eq!(reward(210_000).treasury, GORI);
        assert_eq!(reward(210_001).treasury, 0);
        assert_eq!(treasury_vested(TREASURY_LOCK_HEIGHT - 1), 0);
        let mut tranches = [0u128; 10];
        for h in 1..=TREASURY_EMISSION_BLOCKS {
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
        fn decimal_roundtrip(a in 0u128..MAX_SUPPLY) { prop_assert_eq!(parse_gnr(&format_gnr(a)), Ok(a)); }
    }
}
