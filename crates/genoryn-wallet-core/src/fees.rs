//! Explicit wallet fee recommendations. These are policy, never finality forecasts.
use genoryn_core::params::MIN_FEE;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FeePriority {
    Economy,
    Standard,
    Priority,
}

#[derive(Debug, Serialize)]
pub struct FeeQuote {
    pub priority: FeePriority,
    pub fee_atomic: String,
    pub minimum_fee_atomic: String,
    pub basis: &'static str,
    pub confirmation_guaranteed: bool,
}

pub fn estimate_fee(priority: FeePriority) -> FeeQuote {
    let multiplier: u64 = match priority {
        FeePriority::Economy => 1,
        FeePriority::Standard => 2,
        FeePriority::Priority => 5,
    };
    FeeQuote {
        priority,
        fee_atomic: (MIN_FEE * multiplier).to_string(),
        minimum_fee_atomic: MIN_FEE.to_string(),
        basis: "Fixed devnet policy: 1x, 2x or 5x minimum. Miners prefer public fee per encoded byte; congestion and inclusion time are not predicted.",
        confirmation_guaranteed: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_are_exact_policy_values_and_never_claim_inclusion() {
        for (priority, multiplier) in [
            (FeePriority::Economy, 1),
            (FeePriority::Standard, 2),
            (FeePriority::Priority, 5),
        ] {
            let quote = estimate_fee(priority);
            assert_eq!(
                quote.fee_atomic.parse::<u64>().unwrap(),
                MIN_FEE * multiplier
            );
            assert!(!quote.confirmation_guaranteed);
            assert_eq!(quote.minimum_fee_atomic, MIN_FEE.to_string());
        }
        assert!(serde_json::from_str::<FeePriority>("\"instant_final\"").is_err());
    }
}
