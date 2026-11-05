//! Network parameters.

use crate::Hash32;
use krypton_pow::PowParams;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Network {
    Mainnet,
    Testnet,
    Devnet,
}

/// The fair-launch anchor: a Bitcoin block mined *after* the launch was announced.
/// Its hash is part of the genesis, which proves nobody mined before that moment.
#[derive(Clone, Copy, Debug)]
pub struct LaunchAnchor {
    pub bitcoin_height: u64,
    pub bitcoin_hash: &'static str,
}

#[derive(Clone, Debug)]
pub struct ChainParams {
    pub network: Network,
    pub name: &'static str,
    pub chain_id: &'static str,
    pub magic: [u8; 4],
    pub p2p_port: u16,
    pub rpc_port: u16,
    pub address_prefix: &'static str,
    pub genesis_time: u64,
    pub genesis_message: &'static str,
    pub launch_anchor: Option<LaunchAnchor>,
    pub initial_difficulty: u128,
    pub pow: PowParams,
    /// Blocks before a reward note becomes spendable.
    pub coinbase_maturity: u64,
    /// Credits accrue per miner and are paid as one note this many blocks after the first credit.
    pub payout_interval: u64,
    pub seeds: &'static [&'static str],
    /// A main-chain block this release vouches for. Light clients require it in the chain
    /// they are shown and refuse headers above it with far less difficulty, so a server
    /// cannot present a cheap low-difficulty chain. Not a consensus rule.
    pub checkpoint: Option<Checkpoint>,
    /// First height at which lock spends (atomic swaps) are valid. `u64::MAX` until an
    /// activation height is announced: older nodes reject them, so this is a hard fork.
    pub lock_height: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct Checkpoint {
    pub height: u64,
    /// Block hash (hex).
    pub hash: &'static str,
    pub difficulty: u128,
}

impl Checkpoint {
    /// Lowest difficulty a light client accepts above the checkpoint (a 16-fold drop in
    /// hashrate after the release would need a new one).
    pub fn min_difficulty(&self) -> u128 {
        (self.difficulty / 16).max(1)
    }
}

pub const GENESIS_MESSAGE: &str =
    "Genoryn Krypton: money that outlives its makers. Quantum-resistant, private, unowned.";

impl Network {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "mainnet" => Some(Self::Mainnet),
            "testnet" => Some(Self::Testnet),
            "devnet" => Some(Self::Devnet),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mainnet => "mainnet",
            Self::Testnet => "testnet",
            Self::Devnet => "devnet",
        }
    }
    pub fn id(self) -> u8 {
        match self {
            Self::Mainnet => 1,
            Self::Testnet => 2,
            Self::Devnet => 3,
        }
    }
    pub fn from_id(id: u8) -> Option<Self> {
        match id {
            1 => Some(Self::Mainnet),
            2 => Some(Self::Testnet),
            3 => Some(Self::Devnet),
            _ => None,
        }
    }

    pub fn params(self) -> ChainParams {
        match self {
            // Mainnet genesis commits to Bitcoin block 974,438, the newest Bitcoin block
            // when the network launched: no Krypton block can predate it (docs/launch.md).
            Self::Mainnet => ChainParams {
                network: self,
                name: "Genoryn Krypton",
                chain_id: "genoryn-krypton-mainnet-1",
                magic: *b"GKM1",
                p2p_port: 30333,
                rpc_port: 30334,
                address_prefix: "gnr",
                genesis_time: 1_793_738_591,
                genesis_message: GENESIS_MESSAGE,
                launch_anchor: Some(LaunchAnchor {
                    bitcoin_height: 974_438,
                    bitcoin_hash: "00000000000000000001075275c9ea6c90ce99dd94214d855d2b726fb173bed9",
                }),
                initial_difficulty: 6_000_000_000,
                pow: PowParams::STANDARD,
                coinbase_maturity: 100,
                payout_interval: 360,
                seeds: &["2.24.162.163:29710", "seed.genoryn.io:30333"],
                checkpoint: Some(Checkpoint {
                    height: 1400,
                    hash: "8d650ee7c81cf920bd24ae4aaa3997bb7fcf4bba0e3ffa8205d388a75484a68d",
                    difficulty: 40_547_978_637,
                }),
                // Atomic swaps (lock spends) activate here: announced with 1.0.13, about a
                // day after its release, so wallets and nodes update first.
                lock_height: 3_300,
            },
            Self::Testnet => ChainParams {
                network: self,
                name: "Genoryn Krypton Testnet",
                chain_id: "genoryn-krypton-testnet-1",
                magic: *b"GKT1",
                p2p_port: 31333,
                rpc_port: 31334,
                address_prefix: "tgnr",
                genesis_time: 1_793_666_400,
                genesis_message: GENESIS_MESSAGE,
                launch_anchor: Some(LaunchAnchor {
                    bitcoin_height: 974_322,
                    bitcoin_hash: "000000000000000000012cce667f20784c18a1b55cd3e6ddb96964c9d16bb08f",
                }),
                initial_difficulty: 20_000_000,
                pow: PowParams::STANDARD,
                coinbase_maturity: 20,
                payout_interval: 360,
                seeds: &["2.24.162.163:29710", "seed.genoryn.io:31333"],
                checkpoint: None,
                lock_height: u64::MAX,
            },
            Self::Devnet => ChainParams {
                network: self,
                name: "Genoryn Krypton Devnet",
                chain_id: "genoryn-krypton-devnet-1",
                magic: *b"GKD1",
                p2p_port: 32333,
                rpc_port: 32334,
                address_prefix: "dgnr",
                genesis_time: 1_793_666_400,
                genesis_message: GENESIS_MESSAGE,
                launch_anchor: None,
                initial_difficulty: 200,
                pow: PowParams::DEVNET,
                coinbase_maturity: 2,
                payout_interval: 4,
                seeds: &[],
                checkpoint: None,
                lock_height: 0,
            },
        }
    }
}

impl ChainParams {
    pub fn is_launched(&self) -> bool {
        self.network != Network::Mainnet || self.launch_anchor.is_some()
    }
    pub fn anchor_bytes(&self) -> Vec<u8> {
        match self.launch_anchor {
            Some(a) => {
                let mut v = a.bitcoin_height.to_le_bytes().to_vec();
                v.extend_from_slice(a.bitcoin_hash.as_bytes());
                v
            }
            None => Vec::new(),
        }
    }
    /// Commitment to every consensus parameter; changes produce a different chain.
    pub fn genesis_prev(&self) -> Hash32 {
        krypton_crypto::hash::tagged_parts(
            "Krypton/genesis/v1",
            &[
                self.chain_id.as_bytes(),
                self.genesis_message.as_bytes(),
                &self.anchor_bytes(),
                &crate::emission::INITIAL_SUBSIDY.to_le_bytes(),
                &crate::emission::HALVING_INTERVAL.to_le_bytes(),
                &crate::emission::MAX_SUPPLY.to_le_bytes(),
                &self.coinbase_maturity.to_le_bytes(),
                &self.payout_interval.to_le_bytes(),
                &SHARE_RATIO.to_le_bytes(),
                &SHARE_WINDOW.to_le_bytes(),
                &POOL_SPREAD.to_le_bytes(),
                &MAX_SHARE_WEIGHT.to_le_bytes(),
                b"KryptoHash-v1|StarkSpend-v1|X25519-MLKEM1024|LWMA-90|Shares-v1",
            ],
        )
    }
}

// Consensus limits shared by all networks.
pub const MAX_BLOCK_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_TX_BYTES: usize = 160 * 1024;
pub const MAX_PROOF_BYTES: usize = 150 * 1024;
pub const MEDIAN_TIME_WINDOW: usize = 11;
/// Maximum seconds a block timestamp may lie in the future.
pub const MAX_FUTURE_DRIFT: u64 = 300;
/// Transfers may anchor to the note tree root of any of the last N blocks.
pub const ANCHOR_WINDOW: u64 = 2_880;
/// Minimum relay fee per started kilobyte of transaction (policy, not consensus: any fee,
/// even zero, is valid in a block). A transfer is about 100 KB, so the floor is about
/// 0.00001 GNR; above it fees are a market, estimated from the mempool (`/v1/fee`).
pub const MIN_FEE_PER_KB: u64 = 10;
/// Size a wallet assumes for a transfer when it chooses a fee before proving it
/// (real transfers are about 100 KB; the margin keeps the fee above the floor).
pub const FEE_ESTIMATE_KB: u64 = 128;

// Proportional mining rewards (shares). See docs/spec/rewards.md.
/// A share needs 1/SHARE_RATIO of the block work: the network finds about this many
/// shares per block, so every miner earns in proportion to its hashrate.
pub const SHARE_RATIO: u128 = 32;
/// A share may be included up to this many blocks after the block it was mined on.
pub const SHARE_WINDOW: u64 = 32;
pub const MAX_SHARES_PER_BLOCK: usize = 256;
/// Each share is credited `pool * weight / (POOL_SPREAD * SHARE_RATIO * difficulty)`:
/// the reward pool drains over about POOL_SPREAD blocks, independent of who mines.
pub const POOL_SPREAD: u128 = 8;
/// A share weighs at most this many block difficulties (bounds old high-difficulty shares).
pub const MAX_SHARE_WEIGHT: u128 = 4;
pub const MAX_PAYOUTS_PER_BLOCK: usize = 64;
