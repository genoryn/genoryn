use crate::codec::{Hash, hash};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

pub const GENESIS_MESSAGE: &str = "Privacy is not a feature. Privacy is the default.";
pub const PROTOCOL_VERSION: u16 = 1;
pub const MIN_FEE: u64 = 10_000_000;
pub const MAX_MONEY_PER_TX: u64 = i64::MAX as u64;
pub const MAX_TRANSACTIONS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Network {
    Devnet,
    Testnet,
    Mainnet,
}

#[derive(Debug, Serialize)]
pub struct GenesisBlock {
    pub version: u16,
    pub height: u64,
    pub network: String,
    pub chain_id: String,
    pub message: &'static str,
    pub currency: &'static str,
    pub max_supply_gori: String,
    pub issued_supply_gori: String,
    pub transaction_count: u64,
    pub proof_of_work: &'static str,
    pub shielded_protocol: &'static str,
    pub finalized: bool,
    pub canonical_preimage: String,
    pub hash: String,
}
impl Network {
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "devnet" => Ok(Self::Devnet),
            "testnet" => Ok(Self::Testnet),
            "mainnet" => Ok(Self::Mainnet),
            _ => bail!("unknown network"),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Devnet => "devnet",
            Self::Testnet => "testnet",
            Self::Mainnet => "mainnet",
        }
    }
    pub fn id(self) -> u8 {
        match self {
            Self::Devnet => 1,
            Self::Testnet => 2,
            Self::Mainnet => 3,
        }
    }
    pub fn magic(self) -> [u8; 4] {
        [0x47, 0x4e, 0x52, self.id()]
    }
    pub fn chain_id(self) -> String {
        format!("genoryn-{}-v1", self.name())
    }
    pub fn base_port(self) -> u16 {
        match self {
            Self::Devnet => 29410,
            Self::Testnet => 29510,
            Self::Mainnet => 29610,
        }
    }
    pub fn hrp(self) -> &'static str {
        match self {
            Self::Devnet => "gnrd",
            Self::Testnet => "gnrt",
            Self::Mainnet => "gnr",
        }
    }
    pub fn genesis(self) -> Hash {
        hash(b"GENORYN/genesis/v1", self.genesis_preimage().as_bytes())
    }
    pub fn genesis_preimage(self) -> String {
        format!(
            "{}|{}|GNR|21000000|1000000000000|RandomX-v1|Ironwood-v3|{}",
            self.chain_id(),
            GENESIS_MESSAGE,
            match self {
                Self::Devnet => "2026-10-07-devnet",
                Self::Testnet => "2026-10-07-testnet",
                Self::Mainnet => "2026-10-10-mainnet",
            }
        )
    }
    /// Height zero has its own zero-issuance record, never an ordinary coinbase.
    pub fn genesis_block(self) -> GenesisBlock {
        GenesisBlock {
            version: 1,
            height: 0,
            network: self.name().into(),
            chain_id: self.chain_id(),
            message: GENESIS_MESSAGE,
            currency: "GNR",
            max_supply_gori: crate::emission::MAX_SUPPLY.to_string(),
            issued_supply_gori: "0".into(),
            transaction_count: 0,
            proof_of_work: "RandomX-v1",
            shielded_protocol: "Ironwood-v3",
            finalized: true,
            canonical_preimage: self.genesis_preimage(),
            hash: hex::encode(self.genesis()),
        }
    }
    /// Every network has a finalized genesis since 0.4.0. Kept as the single gate for
    /// future network retirement.
    pub fn ensure_runnable(self) -> Result<()> {
        Ok(())
    }
    pub fn maturity(self) -> u64 {
        if self == Self::Devnet { 2 } else { 60 }
    }
    pub fn initial_difficulty(self) -> u64 {
        if self == Self::Devnet { 1 } else { 100 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn network_separation_and_finalized_mainnet() {
        assert_ne!(Network::Devnet.genesis(), Network::Testnet.genesis());
        assert_ne!(Network::Mainnet.genesis(), Network::Devnet.genesis());
        assert_ne!(Network::Devnet.magic(), Network::Mainnet.magic());
        assert!(Network::Mainnet.ensure_runnable().is_ok());
        assert!(Network::Mainnet.genesis_block().finalized);
        assert_eq!(
            Network::Mainnet.genesis_preimage(),
            "genoryn-mainnet-v1|Privacy is not a feature. Privacy is the default.|GNR|21000000|1000000000000|RandomX-v1|Ironwood-v3|2026-10-10-mainnet"
        );
        assert_eq!(Network::Mainnet.maturity(), 60);
        assert_eq!(Network::Mainnet.initial_difficulty(), 100);
    }
}
