//! Exact PPLNS: each block's reward is divided by the verified work in the
//! last N units of work before the block was found.
//!
//! `participant = distributable × participant_work / window_work`, computed
//! with integers only:
//!
//! - `fee = floor(reward × fee_bps / 10 000)`, `distributable = reward − fee`.
//! - The window is the newest work rows whose sum reaches
//!   `N = window_factor × block difficulty`; the oldest row in it counts only
//!   the part that fits. Before the pool has done N work, the window is all of
//!   it.
//! - Each credit is `floor(distributable × work × 2^32 / window_work)` in units
//!   of 2^-32 gori, so a contribution worth less than one gori still counts and
//!   accumulates until it can be paid.
//! - What floor division leaves over (`dust`, less than 2^-32 gori per miner
//!   per block) is recorded separately; credits plus dust equal
//!   `distributable` exactly, so rounding neither creates nor silently loses
//!   money.
use crate::{FRACTION_BITS, ONE_GORI};
use anyhow::{Result, ensure};
use serde::Serialize;
use std::collections::BTreeMap;

pub type MinerId = i64;

/// The window size for a block of `difficulty`: `factor × difficulty` units of
/// work (expected hashes), bounded to 64 bits.
pub fn window_target(difficulty: u64, factor: u32) -> u64 {
    difficulty.saturating_mul(u64::from(factor.max(1)))
}

/// Sums newest-first `(miner, work)` rows until `target` work is reached. The
/// row that crosses the boundary counts only the part that fits.
pub fn collect_window(
    rows: impl IntoIterator<Item = (MinerId, u64)>,
    target: u64,
) -> (BTreeMap<MinerId, u64>, u64) {
    let mut window = BTreeMap::new();
    let mut total = 0u64;
    for (miner, work) in rows {
        if total >= target {
            break;
        }
        let counted = work.min(target - total);
        if counted == 0 {
            continue;
        }
        *window.entry(miner).or_insert(0) += counted;
        total += counted;
    }
    (window, total)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Credit {
    pub miner: MinerId,
    pub work: u64,
    /// In units of 2^-32 gori.
    pub fixed: u128,
}
impl Credit {
    pub fn gori(&self) -> u128 {
        self.fixed >> FRACTION_BITS
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Allocation {
    pub reward: u64,
    pub fee: u64,
    pub distributable: u64,
    pub window_work: u64,
    pub credits: Vec<Credit>,
    /// `distributable × 2^32 − Σ credits`, in units of 2^-32 gori.
    pub dust: u128,
}

/// Divides `reward` among `window` (per-miner work, from [`collect_window`]).
pub fn allocate(reward: u64, fee_bps: u16, window: &BTreeMap<MinerId, u64>) -> Result<Allocation> {
    ensure!(fee_bps <= 10_000, "pool fee above 100%");
    ensure!(
        reward <= i64::MAX as u64,
        "reward exceeds the coinbase limit"
    );
    let window_work = window
        .values()
        .try_fold(0u64, |sum, work| sum.checked_add(*work))
        .ok_or_else(|| anyhow::anyhow!("window work exceeds 64 bits"))?;
    ensure!(
        window_work > 0,
        "a found block always has work in its window"
    );
    let fee = (u128::from(reward) * u128::from(fee_bps) / 10_000) as u64;
    let distributable = reward - fee;
    let total = u128::from(window_work);
    let mut credits = Vec::with_capacity(window.len());
    let mut allocated = 0u128;
    for (&miner, &work) in window {
        // distributable < 2^63 and work < 2^64: the product fits in 128 bits,
        // and the remainder (< window_work < 2^64) times 2^32 does too.
        let product = u128::from(distributable) * u128::from(work);
        let whole = product / total;
        let fraction = ((product % total) << FRACTION_BITS) / total;
        let fixed = (whole << FRACTION_BITS) | fraction;
        allocated += fixed;
        credits.push(Credit { miner, work, fixed });
    }
    let budget = u128::from(distributable) * ONE_GORI;
    ensure!(allocated <= budget, "allocation exceeds the reward");
    Ok(Allocation {
        reward,
        fee,
        distributable,
        window_work,
        credits,
        dust: budget - allocated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const GNR: u64 = 1_000_000_000_000;

    #[test]
    fn rewards_follow_verified_work_not_accounts() {
        // Hashrates from 10 H/s to 100,000 H/s over the same period.
        let rates = [10u64, 100, 500, 1_000, 10_000, 100_000];
        let window: BTreeMap<_, _> = rates
            .iter()
            .enumerate()
            .map(|(i, rate)| (i as MinerId, rate * 600))
            .collect();
        let reward = 4_825_437_595_129;
        let allocation = allocate(reward, 100, &window).unwrap();
        assert_eq!(allocation.fee, reward / 100);
        let total: u64 = window.values().sum();
        for credit in &allocation.credits {
            let exact =
                u128::from(allocation.distributable) * u128::from(credit.work) / u128::from(total);
            assert_eq!(credit.gori(), exact, "floor of the exact share");
            assert!(credit.fixed > 0, "a 10 H/s miner earns a real share");
        }
        let weakest = &allocation.credits[0];
        let strongest = &allocation.credits[5];
        let ratio = strongest.fixed / weakest.fixed;
        assert!((9_999..=10_001).contains(&ratio), "{ratio}");
        // Splitting one miner's work across many addresses earns nothing extra.
        let mut split = window.clone();
        let work = split.remove(&5).unwrap();
        for part in 0..100 {
            split.insert(100 + part, work / 100);
        }
        let divided = allocate(reward, 100, &split).unwrap();
        let sum: u128 = divided
            .credits
            .iter()
            .filter(|c| c.miner >= 100)
            .map(|c| c.fixed)
            .sum();
        assert!(
            sum <= strongest.fixed && strongest.fixed - sum < 100,
            "only rounding"
        );
    }

    #[test]
    fn the_window_counts_the_newest_work_and_cuts_the_oldest_row() {
        let rows = [(1, 50), (2, 30), (1, 40), (3, 100)];
        let (window, total) = collect_window(rows, 100);
        assert_eq!(total, 100);
        assert_eq!(window.get(&1), Some(&70));
        assert_eq!(window.get(&2), Some(&30));
        assert_eq!(window.get(&3), None, "older than the last N");
        let (young, total) = collect_window([(1, 5), (2, 5)], 1_000);
        assert_eq!((total, young.len()), (10, 2), "a young pool counts all");
        assert_eq!(window_target(13_638_833, 2), 27_277_666);
        assert_eq!(window_target(u64::MAX, 4), u64::MAX);
    }

    #[test]
    fn tiny_contributions_are_kept_below_one_gori() {
        // A share worth about 0.0003 gori of this block.
        let window: BTreeMap<_, _> = [(1, 1), (2, 3_000_000)].into();
        let allocation = allocate(1_000, 0, &window).unwrap();
        let tiny = &allocation.credits[0];
        assert_eq!(tiny.gori(), 0);
        assert_eq!(
            tiny.fixed,
            1_000 * ONE_GORI / 3_000_001,
            "less than one gori, still counted"
        );
        // The resolution is 2^-32 gori: anything smaller is dust of the block.
        let window: BTreeMap<_, _> = [(1, 1), (2, 3_000_000_000_000)].into();
        assert_eq!(allocate(1, 0, &window).unwrap().credits[0].fixed, 0);
    }

    #[test]
    fn hostile_inputs_fail_closed() {
        assert!(allocate(GNR, 10_001, &[(1, 1)].into()).is_err());
        assert!(allocate(GNR, 0, &BTreeMap::new()).is_err());
        assert!(allocate(u64::MAX, 0, &[(1, 1)].into()).is_err());
        assert!(allocate(GNR, 0, &[(1, u64::MAX), (2, 1)].into()).is_err());
    }

    proptest! {
        #[test]
        fn credits_plus_dust_equal_the_distributable_reward_exactly(
            reward in 0u64..=i64::MAX as u64,
            fee_bps in 0u16..=10_000,
            works in proptest::collection::vec(1u64..=u32::MAX as u64 * 4, 1..64),
        ) {
            let window: BTreeMap<_, _> =
                works.iter().enumerate().map(|(i, w)| (i as MinerId, *w)).collect();
            let allocation = allocate(reward, fee_bps, &window).unwrap();
            let credited: u128 = allocation.credits.iter().map(|c| c.fixed).sum();
            prop_assert_eq!(credited + allocation.dust, u128::from(allocation.distributable) * ONE_GORI);
            prop_assert_eq!(allocation.fee + allocation.distributable, reward);
            // Rounding leaves less than 2^-32 gori per miner.
            prop_assert!(allocation.dust < window.len() as u128);
            // More work never earns less.
            for a in &allocation.credits {
                for b in &allocation.credits {
                    if a.work > b.work { prop_assert!(a.fixed >= b.fixed); }
                }
            }
        }
    }
}
