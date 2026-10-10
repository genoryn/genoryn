//! The light wallet's locally verified chain.
//!
//! Quick mode downloads blocks from remote nodes and validates every one in
//! full (RandomX work, Halo 2 proofs, state roots) before keeping it here. The
//! file is sealed with a wallet-derived key, so blocks that were never
//! validated on this computer cannot be slipped in; on the next start they are
//! replayed with `Verification::Stored` instead of being downloaded and proven
//! again, and only blocks after the last verified one are fetched.
use crate::{keys::Wallet, scan::replay_chain, sealed};
use anyhow::Result;
use genoryn_core::{
    block::Block,
    state::{ChainState, Verification},
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const CHAIN_VERSION: u16 = 1;
const CHAIN_DOMAIN: &[u8] = b"GENORYN/verified-chain/v1";
const MAX_CHAIN_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct VerifiedChain {
    pub blocks: Vec<Block>,
}
impl VerifiedChain {
    /// The validated state at the tip. A store that no longer replays (for
    /// example after a consensus-rule fix) is discarded and fetched again.
    pub fn replay(&mut self, network: genoryn_core::params::Network) -> ChainState {
        match replay_chain(network, &self.blocks, Verification::Stored) {
            Ok(state) => state,
            Err(_) => {
                self.blocks.clear();
                ChainState::new(network)
            }
        }
    }
}

/// `wallet.gnr` keeps its verified chain in `wallet.chain` beside it.
pub fn chain_path(wallet_path: &Path) -> PathBuf {
    wallet_path.with_extension("chain")
}
/// A missing or unreadable store is empty: it only costs a fresh download.
pub fn load(wallet: &Wallet, path: &Path) -> VerifiedChain {
    sealed::load(wallet, path, CHAIN_DOMAIN, CHAIN_VERSION, MAX_CHAIN_BYTES)
        .ok()
        .flatten()
        .unwrap_or_default()
}
pub fn save(wallet: &Wallet, path: &Path, chain: &VerifiedChain) -> Result<()> {
    sealed::save(wallet, path, CHAIN_DOMAIN, CHAIN_VERSION, chain)
}
