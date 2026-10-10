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
    /// P2P message prefix. GENORYN 1.0 networks use `GNR` + `0x10 + id`, so
    /// nodes of the retired pre-1.0 networks (`GNR` + id) are refused at once.
    pub fn magic(self) -> [u8; 4] {
        [0x47, 0x4e, 0x52, 0x10 + self.id()]
    }
    pub fn chain_id(self) -> String {
        format!("genoryn-{}-1.0", self.name())
    }
    /// P2P port; admin RPC is +1, public RPC +2, onion target +3. Mainnet 1.0
    /// moved from 29610 so it never meets the retired network on a shared host.
    pub fn base_port(self) -> u16 {
        match self {
            Self::Devnet => 29410,
            Self::Testnet => 29510,
            Self::Mainnet => 29710,
        }
    }
    /// Directory name for this network's node and wallet data, distinct from the
    /// retired pre-1.0 networks' directories (whose databases this node refuses).
    pub fn data_dir_name(self) -> String {
        format!("{}-1.0", self.name())
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
    /// Commits the chain identity, supply, proof-of-work and shielded protocols
    /// and the early-emission rule (500 blocks of 1,000 GNR) to the genesis hash.
    pub fn genesis_preimage(self) -> String {
        format!(
            "{}|{}|GNR|21000000|1000000000000|RandomX-v1|Ironwood-v3|early:500x1000|{}",
            self.chain_id(),
            GENESIS_MESSAGE,
            match self {
                Self::Devnet => "2026-10-10-devnet-1.0",
                Self::Testnet => "2026-10-10-testnet-1.0",
                Self::Mainnet => "2026-10-10-mainnet-1.0",
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
    /// Difficulty of the first blocks, before the retarget window fills. Mainnet
    /// 1.0 starts near 60 seconds of the launch miner's measured RandomX speed
    /// (about 3,100 hashes per second), so its first blocks cannot be mined in
    /// moments; the LWMA retarget follows the real hashrate from block 12 on.
    pub fn initial_difficulty(self) -> u64 {
        match self {
            Self::Devnet => 1,
            Self::Testnet => 100,
            Self::Mainnet => 150_000,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// The retired pre-1.0 mainnet, for isolation tests.
    const RETIRED_MAINNET_GENESIS: &str =
        "c5fc87d996c7fd9dc69ff0cdf96183c65e1be5c9293748b33c31cd8fb6e1d708";

    #[test]
    fn network_separation_and_finalized_mainnet() {
        assert_ne!(Network::Devnet.genesis(), Network::Testnet.genesis());
        assert_ne!(Network::Mainnet.genesis(), Network::Devnet.genesis());
        assert_ne!(Network::Devnet.magic(), Network::Mainnet.magic());
        assert!(Network::Mainnet.ensure_runnable().is_ok());
        assert!(Network::Mainnet.genesis_block().finalized);
        assert_eq!(
            Network::Mainnet.genesis_preimage(),
            "genoryn-mainnet-1.0|Privacy is not a feature. Privacy is the default.|GNR|21000000|1000000000000|RandomX-v1|Ironwood-v3|early:500x1000|2026-10-10-mainnet-1.0"
        );
        assert_eq!(Network::Mainnet.chain_id(), "genoryn-mainnet-1.0");
        assert_eq!(Network::Mainnet.maturity(), 60);
        assert_eq!(Network::Mainnet.initial_difficulty(), 150_000);
        assert_eq!(Network::Mainnet.data_dir_name(), "mainnet-1.0");
    }

    #[test]
    fn mainnet_1_0_is_isolated_from_the_retired_network() {
        assert_ne!(
            hex::encode(Network::Mainnet.genesis()),
            RETIRED_MAINNET_GENESIS
        );
        assert_ne!(Network::Mainnet.chain_id(), "genoryn-mainnet-v1");
        assert_ne!(Network::Mainnet.magic(), [0x47, 0x4e, 0x52, 3]);
        assert_eq!(Network::Mainnet.magic(), [0x47, 0x4e, 0x52, 0x13]);
        assert_ne!(Network::Mainnet.base_port(), 29610);
        assert_ne!(Network::Mainnet.data_dir_name(), "mainnet");
    }
}
