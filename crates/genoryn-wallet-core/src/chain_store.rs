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

#[cfg(test)]
mod tests {
    use super::*;
    use genoryn_core::params::Network;

    #[test]
    fn the_state_snapshot_resumes_exactly_and_falls_back_to_a_replay() {
        let block =
            Block::decode(include_bytes!("../../../tests/vectors/devnet-block-1.bin")).unwrap();
        let blocks = vec![block];
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("w.gnr");
        let wallet = Wallet::create(Network::Devnet).unwrap();
        let replayed = replay_chain(Network::Devnet, &blocks, Verification::Stored).unwrap();
        // No snapshot yet: a replay.
        let first = replay_from_snapshot(&wallet, &path, Network::Devnet, &blocks).unwrap();
        assert_eq!(first.best_hash, replayed.best_hash);
        save_state(&wallet, &path, &replayed).unwrap();
        let resumed = replay_from_snapshot(&wallet, &path, Network::Devnet, &blocks).unwrap();
        assert_eq!(resumed.best_hash, replayed.best_hash);
        assert_eq!(resumed.notes_root(), replayed.notes_root());
        assert_eq!(resumed.nullifiers_root(), replayed.nullifiers_root());
        assert_eq!(resumed.next_difficulty(), replayed.next_difficulty());
        assert!(
            !String::from_utf8_lossy(&std::fs::read(state_path(&path)).unwrap())
                .contains(&hex::encode(replayed.best_hash)),
            "the snapshot is sealed"
        );
        // A snapshot ahead of the kept chain (a reorganization, a shorter
        // chain) or of another wallet is ignored, never trusted.
        let empty = replay_from_snapshot(&wallet, &path, Network::Devnet, &[]).unwrap();
        assert_eq!(empty.height, 0);
        let other = Wallet::create(Network::Devnet).unwrap();
        assert_eq!(
            replay_from_snapshot(&other, &path, Network::Devnet, &blocks)
                .unwrap()
                .best_hash,
            replayed.best_hash,
            "a foreign snapshot costs a replay, nothing else"
        );
        // A snapshot from another program version is replayed again.
        sealed::save(
            &wallet,
            &state_path(&path),
            STATE_DOMAIN,
            STATE_VERSION,
            &SealedState {
                software: "0.0.0".into(),
                snapshot: replayed.snapshot(),
            },
        )
        .unwrap();
        assert_eq!(
            replay_from_snapshot(&wallet, &path, Network::Devnet, &blocks)
                .unwrap()
                .best_hash,
            replayed.best_hash
        );
    }
}

const STATE_VERSION: u16 = 1;
const STATE_DOMAIN: &[u8] = b"GENORYN/verified-chain-state/v1";
const MAX_STATE_BYTES: u64 = 1024 * 1024 * 1024;

/// The validated state at the tip of the verified chain, written by the
/// program version that validated it.
#[derive(Serialize, Deserialize)]
struct SealedState {
    software: String,
    snapshot: genoryn_core::state::StateSnapshot,
}

/// `wallet.gnr` keeps the state of its verified chain in `wallet.chainstate`.
pub fn state_path(wallet_path: &Path) -> PathBuf {
    wallet_path.with_extension("chainstate")
}

/// Keeps `state`, the validated state at the tip of `blocks`.
pub fn save_state(wallet: &Wallet, wallet_path: &Path, state: &ChainState) -> Result<()> {
    sealed::save(
        wallet,
        &state_path(wallet_path),
        STATE_DOMAIN,
        STATE_VERSION,
        &SealedState {
            software: env!("CARGO_PKG_VERSION").to_owned(),
            snapshot: state.snapshot(),
        },
    )
}

/// The validated state of `blocks`: from the kept snapshot when it was made
/// by this program version at a block still in `blocks`, applying only the
/// newer blocks (already validated, `Verification::Stored`); otherwise by
/// replaying every block. A missing, foreign, outdated or damaged snapshot
/// only costs the replay.
pub fn replay_from_snapshot(
    wallet: &Wallet,
    wallet_path: &Path,
    network: genoryn_core::params::Network,
    blocks: &[Block],
) -> Result<ChainState> {
    let restored = sealed::load::<SealedState>(
        wallet,
        &state_path(wallet_path),
        STATE_DOMAIN,
        STATE_VERSION,
        MAX_STATE_BYTES,
    )
    .ok()
    .flatten()
    .filter(|sealed| sealed.software == env!("CARGO_PKG_VERSION"))
    .and_then(|sealed| ChainState::from_snapshot(sealed.snapshot).ok())
    .filter(|state| {
        state.network == network
            && state.height <= blocks.len() as u64
            && match state.height {
                0 => true,
                height => blocks[height as usize - 1].hash().ok() == Some(state.best_hash),
            }
    });
    let Some(mut state) = restored else {
        return replay_chain(network, blocks, Verification::Stored);
    };
    let mut cache = genoryn_core::state::ProofCache::default();
    for block in &blocks[state.height as usize..] {
        state.apply_block_in_place(
            block,
            genoryn_core::chain::unix_time(),
            &mut cache,
            Verification::Stored,
        )?;
    }
    Ok(state)
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
