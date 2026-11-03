//! Proportional mining rewards.
//!
//! Every block's subsidy and fees flow into a consensus-held reward pool. Each
//! header that meets the share target (1/[`SHARE_RATIO`] of the block work) is a
//! *share*; the block that includes it credits its miner a fixed fraction of the
//! pool. The found block counts as a share of its own miner too. A miner with 1 %
//! of the hashrate therefore finds about 1 % of the shares and earns about 1 % of
//! the rewards, without luck deciding who takes a whole block.
//!
//! A share's credit depends only on the pool and its own weight, never on which
//! other shares the block includes, so leaving out other miners' shares gains a
//! block finder nothing; a left-out share stays includable for [`SHARE_WINDOW`]
//! blocks. Credits accrue per miner and are paid as one note
//! `ChainParams::payout_interval` blocks after the first credit, which keeps notes few and
//! large. Payout notes are built by consensus from public data (owner, value,
//! height, index), carry no proof, and are spent like any other note: spends stay
//! private because nullifiers require the owner's secret nullifier key.

use crate::codec::Writer;
use crate::params::*;
use crate::types::BlockHeader;
use crate::{CoreError, Hash32, Result};
use krypton_crypto::field::{Digest, digest_from_bytes, digest_from_uniform, digest_to_bytes};
use krypton_crypto::hash::{tagged, tagged_parts};
use std::collections::BTreeMap;

/// Unpaid credit of one miner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Account {
    pub amount: u64,
    /// Height of the first credit since the last payout.
    pub since: u64,
}

impl Account {
    pub fn to_bytes(&self) -> [u8; 16] {
        let mut b = [0u8; 16];
        b[..8].copy_from_slice(&self.amount.to_le_bytes());
        b[8..].copy_from_slice(&self.since.to_le_bytes());
        b
    }
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        let b: &[u8; 16] = b.try_into().ok()?;
        Some(Self {
            amount: u64::from_le_bytes(b[..8].try_into().ok()?),
            since: u64::from_le_bytes(b[8..].try_into().ok()?),
        })
    }
    /// First height at which this account is paid out.
    pub fn due_height(&self, payout_interval: u64) -> u64 {
        self.since.saturating_add(payout_interval)
    }
}

/// A reward note created by consensus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Payout {
    #[serde(with = "hex::serde")]
    pub owner: Hash32,
    pub value: u64,
}

/// Note randomness of payout `index` made at `height` (public by design).
pub fn payout_rcm(height: u64, index: u32, owner: &Hash32) -> Digest {
    digest_from_uniform(&tagged_parts(
        "Krypton/payout/rcm",
        &[&height.to_le_bytes(), &index.to_le_bytes(), owner],
    ))
}

/// Note commitment of payout `index` made at `height`.
pub fn payout_cm(height: u64, index: u32, p: &Payout) -> Result<Hash32> {
    let owner = digest_from_bytes(&p.owner)
        .map_err(|_| CoreError::Invalid("non-canonical payout owner"))?;
    let rcm = payout_rcm(height, index, &p.owner);
    Ok(digest_to_bytes(&krypton_crypto::rescue::note_commitment(
        &owner, p.value, &rcm,
    )))
}

pub fn payout_digest(payouts: &[Payout]) -> Hash32 {
    let mut w = Writer::default();
    w.u32(payouts.len() as u32);
    for p in payouts {
        w.raw(&p.owner).u64(p.value);
    }
    tagged("Krypton/payouts", &w.0)
}

pub fn share_root(shares: &[BlockHeader]) -> Hash32 {
    let mut w = Writer::default();
    w.u32(shares.len() as u32);
    for s in shares {
        w.raw(&s.hash());
    }
    tagged("Krypton/shares", &w.0)
}

/// Difficulty a share must meet when blocks need `block_difficulty`.
pub fn share_difficulty(block_difficulty: u128) -> u128 {
    (block_difficulty / SHARE_RATIO).max(1)
}

/// `a * b / c` without overflow (b and c are scaled down together when large;
/// the relative error stays below 2^-62).
pub fn mul_div(a: u64, mut b: u128, mut c: u128) -> u128 {
    if c == 0 {
        return 0;
    }
    while b >= 1 << 63 || c >= 1 << 63 {
        b >>= 1;
        c >>= 1;
    }
    if c == 0 {
        return 0;
    }
    a as u128 * b / c
}

/// Reward state a block is applied to.
pub trait RewardView {
    fn pool(&self) -> u64;
    fn account(&self, owner: &Hash32) -> Option<Account>;
    /// Accounts with `since <= max_since`, ordered by (since, owner), at most `limit`.
    fn due_accounts(&self, max_since: u64, limit: usize) -> Vec<(Hash32, Account)>;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RewardEffects {
    /// Pool balance after the block.
    pub pool: u64,
    /// New state of every account the block touched (None = removed after payout).
    pub accounts: Vec<(Hash32, Option<Account>)>,
    pub payouts: Vec<Payout>,
    /// Total credited to shares by this block.
    pub credited: u64,
}

/// Applies a block's rewards. `shares` lists (miner owner, weight) of every share the
/// block credits, the block's own header included; weights are share difficulties.
pub fn apply(
    view: &impl RewardView,
    height: u64,
    difficulty: u128,
    inflow: u64,
    shares: &[(Hash32, u128)],
    payout_interval: u64,
) -> Result<RewardEffects> {
    let pool = view
        .pool()
        .checked_add(inflow)
        .ok_or(CoreError::Invalid("reward pool overflow"))?;
    let difficulty = difficulty.max(1);
    let denom = (POOL_SPREAD * SHARE_RATIO).saturating_mul(difficulty);
    let cap = MAX_SHARE_WEIGHT.saturating_mul(difficulty);
    let mut raw: Vec<u128> = shares
        .iter()
        .map(|(_, w)| mul_div(pool, (*w).min(cap), denom))
        .collect();
    let total: u128 = raw.iter().sum();
    if total > pool as u128 {
        for r in raw.iter_mut() {
            *r = mul_div(pool, *r, total);
        }
    }
    let mut per_owner: BTreeMap<Hash32, u64> = BTreeMap::new();
    let mut credited: u64 = 0;
    for ((owner, _), c) in shares.iter().zip(&raw) {
        let c = (*c).min((pool - credited) as u128) as u64;
        if c == 0 {
            continue;
        }
        *per_owner.entry(*owner).or_default() += c;
        credited += c;
    }
    let mut state: BTreeMap<Hash32, Option<Account>> = BTreeMap::new();
    for (owner, c) in &per_owner {
        let mut a = view.account(owner).unwrap_or(Account {
            amount: 0,
            since: height,
        });
        a.amount = a
            .amount
            .checked_add(*c)
            .ok_or(CoreError::Invalid("account overflow"))?;
        state.insert(*owner, Some(a));
    }
    let mut payouts = Vec::new();
    if height >= payout_interval {
        for (owner, a) in view.due_accounts(height - payout_interval, MAX_PAYOUTS_PER_BLOCK) {
            let a = state.get(&owner).copied().flatten().unwrap_or(a);
            if a.amount > 0 {
                payouts.push(Payout {
                    owner,
                    value: a.amount,
                });
            }
            state.insert(owner, None);
        }
    }
    Ok(RewardEffects {
        pool: pool - credited,
        accounts: state.into_iter().collect(),
        payouts,
        credited,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emission::INITIAL_SUBSIDY;
    const IV: u64 = 360;

    #[derive(Default)]
    struct Mem {
        pool: u64,
        accounts: BTreeMap<Hash32, Account>,
    }
    impl RewardView for Mem {
        fn pool(&self) -> u64 {
            self.pool
        }
        fn account(&self, o: &Hash32) -> Option<Account> {
            self.accounts.get(o).copied()
        }
        fn due_accounts(&self, max_since: u64, limit: usize) -> Vec<(Hash32, Account)> {
            let mut v: Vec<_> = self
                .accounts
                .iter()
                .filter(|(_, a)| a.since <= max_since)
                .map(|(o, a)| (*o, *a))
                .collect();
            v.sort_by_key(|(o, a)| (a.since, *o));
            v.truncate(limit);
            v
        }
    }
    impl Mem {
        fn commit(&mut self, fx: &RewardEffects) {
            self.pool = fx.pool;
            for (o, a) in &fx.accounts {
                match a {
                    Some(a) => self.accounts.insert(*o, *a),
                    None => self.accounts.remove(o),
                };
            }
        }
    }

    /// Simulates miners with the given hashrate fractions; shares are drawn in exact
    /// proportion (SHARE_RATIO per block) to isolate the accounting from luck.
    fn simulate(fractions: &[u128], blocks: u64) -> (Vec<u64>, u64, u64) {
        let d = 1_000_000u128;
        let mut m = Mem::default();
        let mut paid = vec![0u64; fractions.len()];
        let mut minted = 0u64;
        let owners: Vec<Hash32> = (0..fractions.len()).map(|i| [i as u8 + 1; 32]).collect();
        let total: u128 = fractions.iter().sum();
        let mut carry = vec![0u128; fractions.len()];
        for h in 1..=blocks {
            let mut shares = Vec::new();
            for (i, f) in fractions.iter().enumerate() {
                carry[i] += f * SHARE_RATIO;
                while carry[i] >= total {
                    carry[i] -= total;
                    shares.push((owners[i], share_difficulty(d) * SHARE_RATIO));
                }
            }
            let inflow = crate::emission::subsidy(h);
            minted += inflow;
            let fx = apply(&m, h, d, inflow, &shares, IV).unwrap();
            for p in &fx.payouts {
                let i = owners.iter().position(|o| *o == p.owner).unwrap();
                paid[i] += p.value;
            }
            m.commit(&fx);
        }
        let unpaid = m.pool + m.accounts.values().map(|a| a.amount).sum::<u64>();
        (paid, minted, unpaid)
    }

    #[test]
    fn rewards_follow_hashrate_and_conserve_supply() {
        let fractions = [880u128, 100, 10, 10];
        let (paid, minted, unpaid) = simulate(&fractions, 3 * IV);
        let total_paid: u64 = paid.iter().sum();
        assert_eq!(total_paid + unpaid, minted, "no coin created or lost");
        for (i, f) in fractions.iter().enumerate() {
            let share = paid[i] as f64 / total_paid as f64;
            let want = *f as f64 / 1000.0;
            assert!((share - want).abs() < 0.004, "miner {i}: {share} vs {want}");
        }
        assert!(paid[2] > 0, "a 1% miner is paid too");
    }

    #[test]
    fn excluding_other_shares_does_not_raise_own_credit() {
        let m = Mem {
            pool: 40 * INITIAL_SUBSIDY,
            ..Default::default()
        };
        let d = 1_000_000u128;
        let me = ([9u8; 32], d);
        let others: Vec<(Hash32, u128)> = (0..31).map(|i| ([i as u8 + 20; 32], d)).collect();
        let alone = apply(&m, 10, d, 0, &[me], IV).unwrap();
        let mut all = vec![me];
        all.extend(others);
        let shared = apply(&m, 10, d, 0, &all, IV).unwrap();
        let mine = |fx: &RewardEffects| {
            fx.accounts
                .iter()
                .find(|(o, _)| *o == me.0)
                .unwrap()
                .1
                .unwrap()
                .amount
        };
        assert_eq!(mine(&alone), mine(&shared));
    }

    #[test]
    fn payouts_are_bounded_and_ordered() {
        let mut m = Mem {
            pool: 0,
            ..Default::default()
        };
        for i in 0..200u32 {
            let mut o = [0u8; 32];
            o[..4].copy_from_slice(&i.to_be_bytes());
            m.accounts.insert(
                o,
                Account {
                    amount: 1,
                    since: (i % 3) as u64,
                },
            );
        }
        let fx = apply(&m, IV + 5, 1000, 0, &[], IV).unwrap();
        assert_eq!(fx.payouts.len(), MAX_PAYOUTS_PER_BLOCK);
        assert!(fx.accounts.iter().all(|(_, a)| a.is_none()));
    }

    #[test]
    fn extreme_difficulties_do_not_overflow() {
        let m = Mem {
            pool: u64::MAX / 2,
            ..Default::default()
        };
        let d = u128::MAX / 64;
        let fx = apply(&m, 1, d, 0, &[([1; 32], d), ([2; 32], u128::MAX)], IV).unwrap();
        assert!(fx.credited <= u64::MAX / 2);
        assert_eq!(fx.pool + fx.credited, u64::MAX / 2);
    }

    #[test]
    fn payout_commitment_is_deterministic_and_canonical() {
        let p = Payout {
            owner: [0; 32],
            value: 5,
        };
        assert_eq!(payout_cm(7, 0, &p).unwrap(), payout_cm(7, 0, &p).unwrap());
        assert_ne!(payout_cm(7, 0, &p).unwrap(), payout_cm(7, 1, &p).unwrap());
        assert!(
            payout_cm(
                7,
                0,
                &Payout {
                    owner: [0xff; 32],
                    value: 5
                }
            )
            .is_err()
        );
    }
}
