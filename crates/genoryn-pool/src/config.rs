//! Pool policy: activation height, fee, PPLNS window and payouts.
use crate::vardiff::VarDiffConfig;
use anyhow::{Result, bail, ensure};
use genoryn_core::params::Network;
use serde::{Deserialize, Serialize};

/// The requested first mainnet height whose pool-found blocks are shared.
/// A pool service setting, not a consensus rule: solo mining is unaffected.
pub const MAINNET_ACTIVATION_HEIGHT: u64 = 1150;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolPolicy {
    pub network: Network,
    /// Blocks below this height are never shared (and the pool does not open
    /// for shares before the next block can reach it).
    pub activation_height: u64,
    /// Pool fee in basis points of each block's coinbase (100 = 1%).
    pub fee_bps: u16,
    /// PPLNS window: this many times the found block's difficulty.
    pub window_factor: u32,
    pub vardiff: VarDiffConfig,
    /// Smallest payout in gori; smaller balances keep accumulating.
    pub payout_threshold: u64,
    /// Recipients per payout transaction (at most 15: 16 actions with change).
    pub payout_batch: usize,
    /// Network fee per payout transaction, paid by the pool operator.
    pub payout_fee: u64,
    /// Blocks after maturity during which a matured block is still re-checked.
    pub reorg_watch: u64,
}
impl PoolPolicy {
    pub fn defaults(network: Network) -> Self {
        Self {
            network,
            activation_height: match network {
                Network::Mainnet => MAINNET_ACTIVATION_HEIGHT,
                _ => 0,
            },
            fee_bps: 100,
            window_factor: 2,
            vardiff: VarDiffConfig::default(),
            payout_threshold: 100_000_000_000,
            payout_batch: 15,
            payout_fee: genoryn_core::params::MIN_FEE,
            reorg_watch: 720,
        }
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(self.fee_bps <= 5_000, "a pool fee above 50% is refused");
        ensure!(
            (1..=16).contains(&self.window_factor),
            "PPLNS window factor must be 1-16"
        );
        ensure!(
            (1..=15).contains(&self.payout_batch),
            "a payout transaction pays 1-15 recipients"
        );
        ensure!(
            self.payout_fee >= genoryn_core::params::MIN_FEE,
            "payout fee below the consensus minimum"
        );
        ensure!(
            self.payout_threshold > self.payout_fee,
            "the payout threshold must exceed the network fee"
        );
        self.vardiff.validate()
    }
    /// Whether a block at `height` may be shared.
    pub fn eligible(&self, height: u64) -> bool {
        height >= self.activation_height
    }
    /// Refuses to start a new mainnet pool whose activation height has
    /// already passed: rewards are never backdated, so a new height must be
    /// chosen explicitly. A pool that already keeps accounting state goes on.
    pub fn check_activation(&self, tip: u64, has_state: bool, confirmed: bool) -> Result<()> {
        if self.network == Network::Mainnet && !has_state && tip >= self.activation_height {
            if !confirmed {
                bail!(
                    "pool activation height {} has already passed (the verified tip is {tip}); \
                     no earlier block is shared retroactively. Choose a future height and \
                     confirm it explicitly (--activation-height H --confirm-activation-height)",
                    self.activation_height
                );
            }
            bail!(
                "a confirmed activation height must lie in the future: {} <= tip {tip}",
                self.activation_height
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_requested_mainnet_height_is_never_backdated() {
        let policy = PoolPolicy::defaults(Network::Mainnet);
        assert_eq!(policy.activation_height, 1150);
        policy.validate().unwrap();
        assert!(policy.check_activation(1136, false, false).is_ok());
        let missed = policy.check_activation(1150, false, false).unwrap_err();
        assert!(missed.to_string().contains("already passed"), "{missed}");
        assert!(policy.check_activation(1300, false, true).is_err());
        let later = PoolPolicy {
            activation_height: 2_000,
            ..policy.clone()
        };
        assert!(later.check_activation(1300, false, true).is_ok());
        assert!(
            policy.check_activation(5_000, true, false).is_ok(),
            "existing state"
        );
        assert!(!policy.eligible(1149) && policy.eligible(1150));
        assert!(
            PoolPolicy::defaults(Network::Devnet)
                .check_activation(9, false, false)
                .is_ok()
        );
        assert!(
            PoolPolicy {
                payout_batch: 16,
                ..policy.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            PoolPolicy {
                fee_bps: 6_000,
                ..policy
            }
            .validate()
            .is_err()
        );
    }
}
