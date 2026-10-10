//! Incremental wallet sync over an already validated chain.
//!
//! The wallet keeps an encrypted cache of its notes, history and mining rewards
//! and only decrypts blocks added since the last sync. The blocks come either
//! from the user's own validating node (Full mode) or from the locally verified
//! chain of the light wallet (Quick mode). Forks are detected by block hash and
//! rolled back to the common ancestor. Mining rewards stay in the history after
//! a reorganization: one replaced by a different block at its height is shown as
//! replaced, never silently deleted, and never counted in any balance.
use crate::{
    keys::Wallet,
    scan::{HistoryEntry, WalletSummary},
    sealed,
};
use anyhow::{Result, ensure};
use genoryn_core::{block::Block, codec::Hash, emission, names, params::Network};
use orchard::keys::{FullViewingKey, IncomingViewingKey, OutgoingViewingKey, Scope};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const CACHE_VERSION: u16 = 2;
const CACHE_DOMAIN: &[u8] = b"GENORYN/scan-cache/v2";
/// Recent block hashes kept for locating a fork point after a reorganization.
pub const REORG_WINDOW: usize = 128;
const MAX_CACHE_BYTES: u64 = 64 * 1024 * 1024;
/// A reward this young is shown as just mined: the highest reorganization risk.
pub const JUST_MINED_CONFIRMATIONS: u64 = 6;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedNote {
    pub value: u64,
    pub nullifier: Hash,
    pub txid: Hash,
    pub action: u32,
    pub height: u64,
    pub unlock_height: u64,
    pub spent_height: Option<u64>,
    /// The transaction that spent this note, for matching the outbox.
    pub spent_txid: Option<Hash>,
    pub memo: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedEntry {
    pub txid: Hash,
    pub direction: String,
    pub amount_atomic: u128,
    pub height: u64,
    pub timestamp: u64,
    pub memo: String,
}
/// Where a recorded mining reward stands on the current chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RewardState {
    /// Its block is on the chain this wallet follows.
    Canonical,
    /// The chain was rolled back below its height and has not yet reached that
    /// height again: not counted, not yet known to be replaced.
    Unconfirmed,
    /// A different block now occupies its height on the chain this wallet follows.
    Replaced,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MinedReward {
    pub txid: Hash,
    pub action: u32,
    pub height: u64,
    pub block_hash: Hash,
    pub amount_atomic: u64,
    pub timestamp: u64,
    pub unlock_height: u64,
    pub state: RewardState,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanCache {
    pub version: u16,
    pub network: Network,
    pub account: u32,
    pub height: u64,
    /// Blocks below this height are validated but not trial-decrypted: the wallet
    /// cannot have received anything before it existed. 0 scans everything.
    pub birthday: u64,
    /// `(height, hash)` of the most recent blocks, oldest first.
    pub recent: Vec<(u64, Hash)>,
    pub notes: Vec<CachedNote>,
    /// Chronological; summaries list the newest first.
    pub history: Vec<CachedEntry>,
    /// Every reward this wallet has seen, including replaced ones.
    pub mined: Vec<MinedReward>,
}

/// Keys derived once per sync rather than once per block.
pub struct ScanKeys {
    fvk: FullViewingKey,
    ivks: [IncomingViewingKey; 2],
    ovk: OutgoingViewingKey,
}
impl ScanKeys {
    pub fn new(wallet: &Wallet) -> Result<Self> {
        let fvk = wallet.viewing_key()?;
        Ok(Self {
            ivks: [fvk.to_ivk(Scope::External), fvk.to_ivk(Scope::Internal)],
            ovk: fvk.to_ovk(Scope::External),
            fvk,
        })
    }
}

/// The height at which a note created by a transaction of `kind` becomes spendable,
/// matching `ChainState::apply_body`.
fn unlock_height(network: Network, kind: u8, height: u64) -> u64 {
    match kind {
        1 => height.saturating_add(network.maturity()),
        2 => emission::treasury_unlock_height(height).unwrap_or(u64::MAX),
        _ => height,
    }
}
fn decode_memo(bytes: &[u8; 512]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.iter().position(|b| *b == 0).unwrap_or(512)])
        .into_owned()
}

/// Display status of a mining reward at chain tip `tip`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum RewardStatus {
    JustMined,
    Maturing,
    Mature,
    /// Rolled back, waiting for the chain to reach its height again.
    Unconfirmed,
    Replaced,
}
impl RewardStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::JustMined => "Just mined",
            Self::Maturing => "Maturing",
            Self::Mature => "Mature",
            Self::Unconfirmed => "Verification pending",
            Self::Replaced => "Replaced",
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct MinedSummary {
    pub txid: String,
    pub action: u32,
    pub height: u64,
    pub block_hash: String,
    pub amount_atomic: String,
    pub timestamp: u64,
    pub unlock_height: u64,
    pub confirmations: u64,
    pub status: RewardStatus,
    pub status_label: &'static str,
    /// Fraction of the maturity period completed, 0 to 1000 (per mille).
    pub progress_permille: u64,
    pub spent: bool,
}
impl MinedReward {
    /// `confirmations` counts the reward's own block: at the tip it has one. It is
    /// mature (spendable) once the tip reaches `unlock_height`, exactly as consensus.
    pub fn status(&self, tip: u64, maturity: u64) -> (RewardStatus, u64, u64) {
        match self.state {
            RewardState::Replaced => return (RewardStatus::Replaced, 0, 0),
            RewardState::Unconfirmed => return (RewardStatus::Unconfirmed, 0, 0),
            RewardState::Canonical => {}
        }
        if tip < self.height {
            return (RewardStatus::Unconfirmed, 0, 0);
        }
        let confirmations = tip - self.height + 1;
        let progress = if maturity == 0 {
            1000
        } else {
            ((tip - self.height).min(maturity) * 1000) / maturity
        };
        let status = if tip >= self.unlock_height {
            RewardStatus::Mature
        } else if confirmations <= JUST_MINED_CONFIRMATIONS {
            RewardStatus::JustMined
        } else {
            RewardStatus::Maturing
        };
        (status, confirmations, progress)
    }
}

impl ScanCache {
    pub fn new(network: Network, account: u32) -> Self {
        Self::with_birthday(network, account, 0)
    }
    pub fn with_birthday(network: Network, account: u32, birthday: u64) -> Self {
        Self {
            version: CACHE_VERSION,
            network,
            account,
            height: 0,
            birthday,
            recent: Vec::new(),
            notes: Vec::new(),
            history: Vec::new(),
            mined: Vec::new(),
        }
    }
    /// A rescan from genesis that keeps the birthday and the mining history; the
    /// rewards are re-confirmed or marked replaced as the rescan reaches them.
    pub fn restart(&self) -> Self {
        let mut fresh = Self::with_birthday(self.network, self.account, self.birthday);
        fresh.mined = self.mined.clone();
        for reward in &mut fresh.mined {
            if reward.state == RewardState::Canonical {
                reward.state = RewardState::Unconfirmed;
            }
        }
        fresh
    }
    pub fn tip_hash(&self) -> Hash {
        self.recent
            .last()
            .map_or_else(|| self.network.genesis(), |(_, hash)| *hash)
    }
    /// Adds the next block, which must extend the cached tip.
    pub fn apply_block(&mut self, keys: &ScanKeys, block: &Block) -> Result<()> {
        let header = &block.header;
        ensure!(
            header.network == self.network
                && header.height == self.height + 1
                && header.previous == self.tip_hash(),
            "block does not extend the scanned chain"
        );
        let height = header.height;
        let block_hash = block.hash()?;
        // A recorded reward at this height is canonical only if this is its block.
        for reward in self.mined.iter_mut().filter(|r| r.height == height) {
            reward.state = if reward.block_hash == block_hash {
                RewardState::Canonical
            } else {
                RewardState::Replaced
            };
        }
        if height >= self.birthday {
            self.scan_transactions(keys, block, block_hash)?;
        }
        self.height = height;
        self.recent.push((height, block_hash));
        if self.recent.len() > REORG_WINDOW {
            self.recent.remove(0);
        }
        Ok(())
    }
    fn scan_transactions(
        &mut self,
        keys: &ScanKeys,
        block: &Block,
        block_hash: Hash,
    ) -> Result<()> {
        let header = &block.header;
        let height = header.height;
        for tx in block.body.all_transactions() {
            let txid = tx.id()?;
            for action in &tx.bundle.actions {
                for note in self.notes.iter_mut().filter(|note| {
                    note.spent_height.is_none() && note.nullifier == action.nullifier
                }) {
                    note.spent_height = Some(height);
                    note.spent_txid = Some(txid);
                }
            }
            let bundle = tx.bundle.to_bundle()?;
            let incoming = bundle.decrypt_outputs_with_keys(&keys.ivks);
            let mut received = 0u128;
            let mut last_memo = None;
            for (action, _, note, _, memo) in &incoming {
                if note.value().inner() == 0 {
                    continue;
                }
                received += note.value().inner() as u128;
                let memo = decode_memo(memo);
                last_memo = Some(memo.clone());
                let unlock = unlock_height(self.network, tx.context.kind, height);
                self.notes.push(CachedNote {
                    value: note.value().inner(),
                    nullifier: note.nullifier(&keys.fvk).to_bytes(),
                    txid,
                    action: *action as u32,
                    height,
                    unlock_height: unlock,
                    spent_height: None,
                    spent_txid: None,
                    memo,
                });
                if tx.context.kind == 1 {
                    let action = *action as u32;
                    match self
                        .mined
                        .iter_mut()
                        .find(|r| r.txid == txid && r.action == action)
                    {
                        Some(reward) => reward.state = RewardState::Canonical,
                        None => self.mined.push(MinedReward {
                            txid,
                            action,
                            height,
                            block_hash,
                            amount_atomic: note.value().inner(),
                            timestamp: header.timestamp,
                            unlock_height: unlock,
                            state: RewardState::Canonical,
                        }),
                    }
                }
            }
            let outgoing = bundle.recover_outputs_with_ovks(std::slice::from_ref(&keys.ovk));
            let mut sent = 0u128;
            let mut sent_memo = String::new();
            let mut burned = 0u128;
            let mut burned_memo = String::new();
            for (_, _, note, recipient, memo) in outgoing {
                if note.value().inner() == 0 {
                    continue;
                }
                // Change is recognised by this bundle's own incoming decryption.
                let own = incoming.iter().any(|(_, _, mine, _, _)| {
                    orchard::note::ExtractedNoteCommitment::from(mine.commitment())
                        == orchard::note::ExtractedNoteCommitment::from(note.commitment())
                });
                if own {
                    continue;
                }
                if names::is_burn_address(self.network, &recipient) {
                    burned += note.value().inner() as u128;
                    burned_memo = crate::scan::name_memo(&memo);
                } else {
                    sent += note.value().inner() as u128;
                    sent_memo = decode_memo(&memo);
                }
            }
            let entry = |direction: &str, amount: u128, memo: String| CachedEntry {
                txid,
                direction: direction.into(),
                amount_atomic: amount,
                height,
                timestamp: header.timestamp,
                memo,
            };
            if burned > 0 {
                self.history.push(entry("name", burned, burned_memo));
            }
            if sent > 0 {
                self.history.push(entry("sent", sent, sent_memo));
            } else if received > 0 && burned == 0 {
                let direction = match tx.context.kind {
                    1 => "mined",
                    2 => "treasury",
                    _ => "received",
                };
                self.history
                    .push(entry(direction, received, last_memo.unwrap_or_default()));
            }
        }
        Ok(())
    }
    /// Forgets everything above `height`, the last block shared with the chain.
    /// Mining rewards above it stay recorded as unconfirmed until the chain
    /// reaches their height again and shows which block is there.
    pub fn rollback_to(&mut self, height: u64) {
        self.notes.retain(|note| note.height <= height);
        for note in &mut self.notes {
            if note.spent_height.is_some_and(|spent| spent > height) {
                note.spent_height = None;
                note.spent_txid = None;
            }
        }
        self.history.retain(|entry| entry.height <= height);
        self.recent.retain(|(at, _)| *at <= height);
        for reward in self.mined.iter_mut().filter(|r| r.height > height) {
            if reward.state == RewardState::Canonical {
                reward.state = RewardState::Unconfirmed;
            }
        }
        self.height = height;
    }
    /// Whether a rollback to `height` can still be represented; otherwise rescan.
    pub fn can_roll_back_to(&self, height: u64) -> bool {
        height == 0 || self.recent.iter().any(|(at, _)| *at == height)
    }
    /// The wallet's mining rewards, newest first, with status at the cached tip.
    pub fn mined_summary(&self) -> Vec<MinedSummary> {
        let maturity = self.network.maturity();
        let mut rewards: Vec<MinedSummary> = self
            .mined
            .iter()
            .map(|reward| {
                let (status, confirmations, progress) = reward.status(self.height, maturity);
                let spent = self.notes.iter().any(|note| {
                    note.txid == reward.txid
                        && note.action == reward.action
                        && note.spent_height.is_some()
                });
                MinedSummary {
                    txid: hex::encode(reward.txid),
                    action: reward.action,
                    height: reward.height,
                    block_hash: hex::encode(reward.block_hash),
                    amount_atomic: reward.amount_atomic.to_string(),
                    timestamp: reward.timestamp,
                    unlock_height: reward.unlock_height,
                    confirmations,
                    status,
                    status_label: status.label(),
                    progress_permille: progress,
                    spent,
                }
            })
            .collect();
        rewards.sort_by(|a, b| b.height.cmp(&a.height).then(a.txid.cmp(&b.txid)));
        rewards
    }
    pub fn summary(&self, wallet: &Wallet, confirmation_target: u64) -> Result<WalletSummary> {
        ensure!(
            (1..=10_000).contains(&confirmation_target),
            "confirmation target must be 1-10000"
        );
        let unspent = self.notes.iter().filter(|note| note.spent_height.is_none());
        let balance = unspent.clone().map(|note| note.value as u128).sum::<u128>();
        let available = unspent
            .filter(|note| note.unlock_height <= self.height)
            .map(|note| note.value as u128)
            .sum::<u128>();
        let history = self
            .history
            .iter()
            .rev()
            .map(|entry| {
                let confirmations = self.height - entry.height + 1;
                HistoryEntry {
                    txid: hex::encode(entry.txid),
                    direction: entry.direction.clone(),
                    amount_atomic: entry.amount_atomic.to_string(),
                    height: entry.height,
                    timestamp: entry.timestamp,
                    status: if confirmations < confirmation_target {
                        format!(
                            "{confirmations} of {confirmation_target} confirmations; reorg risk remains"
                        )
                    } else {
                        format!(
                            "{confirmation_target}-confirmation display policy reached; reorgs remain possible"
                        )
                    },
                    confirmations,
                    memo: entry.memo.clone(),
                }
            })
            .collect();
        Ok(WalletSummary {
            address: wallet.receiving_identity()?,
            balance_atomic: balance.to_string(),
            spendable_atomic: available.to_string(),
            pending_atomic: (balance - available).to_string(),
            height: self.height,
            history,
        })
    }
}

/// `wallet.gnr` keeps its scan cache in `wallet.scan` beside it.
pub fn cache_path(wallet_path: &Path) -> PathBuf {
    wallet_path.with_extension("scan")
}
/// A missing, foreign or undecryptable cache yields a fresh one (a full rescan).
pub fn load_cache(wallet: &Wallet, path: &Path) -> ScanCache {
    sealed::load::<ScanCache>(wallet, path, CACHE_DOMAIN, CACHE_VERSION, MAX_CACHE_BYTES)
        .ok()
        .flatten()
        .filter(|cache| {
            cache.version == CACHE_VERSION
                && cache.network == wallet.network
                && cache.account == wallet.account
        })
        .unwrap_or_else(|| ScanCache::new(wallet.network, wallet.account))
}
pub fn save_cache(wallet: &Wallet, path: &Path, cache: &ScanCache) -> Result<()> {
    sealed::save(wallet, path, CACHE_DOMAIN, CACHE_VERSION, cache)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache_with_history() -> ScanCache {
        let mut cache = ScanCache::new(Network::Devnet, 0);
        for height in 1..=10u64 {
            cache.recent.push((height, [height as u8; 32]));
            cache.notes.push(CachedNote {
                value: 5,
                nullifier: [height as u8; 32],
                txid: [height as u8; 32],
                action: 0,
                height,
                unlock_height: height + 2,
                spent_height: (height == 3).then_some(9),
                spent_txid: (height == 3).then_some([99; 32]),
                memo: String::new(),
            });
            cache.history.push(CachedEntry {
                txid: [height as u8; 32],
                direction: "mined".into(),
                amount_atomic: 5,
                height,
                timestamp: height,
                memo: String::new(),
            });
            cache.mined.push(MinedReward {
                txid: [height as u8; 32],
                action: 0,
                height,
                block_hash: [height as u8; 32],
                amount_atomic: 5,
                timestamp: height,
                unlock_height: height + 2,
                state: RewardState::Canonical,
            });
        }
        cache.height = 10;
        cache
    }

    #[test]
    fn rollback_forgets_the_abandoned_branch_and_restores_its_spends() {
        let mut cache = cache_with_history();
        assert_eq!(cache.tip_hash(), [10; 32]);
        assert!(cache.can_roll_back_to(7) && cache.can_roll_back_to(0));
        cache.rollback_to(7);
        assert_eq!(cache.height, 7);
        assert_eq!(cache.tip_hash(), [7; 32]);
        assert_eq!(cache.notes.len(), 7);
        assert_eq!(cache.history.len(), 7);
        assert_eq!(
            cache.notes[2].spent_height, None,
            "a spend in an abandoned block is undone"
        );
        assert_eq!(cache.notes[2].spent_txid, None);
        cache.rollback_to(0);
        assert_eq!(cache.tip_hash(), Network::Devnet.genesis());
        assert!(cache.notes.is_empty() && cache.history.is_empty());
    }

    #[test]
    fn rolled_back_rewards_stay_recorded_but_never_count() {
        let mut cache = cache_with_history();
        let balance_before: u64 = cache.notes.iter().map(|n| n.value).sum();
        cache.rollback_to(7);
        assert_eq!(cache.mined.len(), 10, "history keeps every reward");
        let summary = cache.mined_summary();
        let unconfirmed: Vec<_> = summary
            .iter()
            .filter(|r| r.status == RewardStatus::Unconfirmed)
            .map(|r| r.height)
            .collect();
        assert_eq!(unconfirmed, vec![10, 9, 8]);
        let balance_after: u64 = cache.notes.iter().map(|n| n.value).sum();
        assert_eq!(
            balance_after,
            balance_before - 15,
            "removed from the balance"
        );
        assert!(
            !summary.iter().any(|r| r.status == RewardStatus::Replaced),
            "a rollback alone is not evidence of replacement"
        );
    }

    #[test]
    fn reward_status_follows_consensus_maturity_without_off_by_one() {
        let reward = MinedReward {
            txid: [1; 32],
            action: 0,
            height: 100,
            block_hash: [1; 32],
            amount_atomic: 1,
            timestamp: 0,
            unlock_height: 160,
            state: RewardState::Canonical,
        };
        assert_eq!(reward.status(100, 60), (RewardStatus::JustMined, 1, 0));
        assert_eq!(reward.status(105, 60).0, RewardStatus::JustMined);
        assert_eq!(reward.status(106, 60).0, RewardStatus::Maturing);
        assert_eq!(reward.status(159, 60), (RewardStatus::Maturing, 60, 983));
        assert_eq!(
            reward.status(160, 60),
            (RewardStatus::Mature, 61, 1000),
            "spendable exactly when the tip reaches the unlock height"
        );
        assert_eq!(reward.status(99, 60).0, RewardStatus::Unconfirmed);
        let replaced = MinedReward {
            state: RewardState::Replaced,
            ..reward
        };
        assert_eq!(replaced.status(500, 60).0, RewardStatus::Replaced);
    }

    #[test]
    fn restart_keeps_birthday_and_mining_history() {
        let mut cache = cache_with_history();
        cache.birthday = 4;
        let fresh = cache.restart();
        assert_eq!(fresh.height, 0);
        assert_eq!(fresh.birthday, 4);
        assert_eq!(fresh.mined.len(), 10);
        assert!(
            fresh
                .mined
                .iter()
                .all(|r| r.state == RewardState::Unconfirmed)
        );
        assert!(fresh.notes.is_empty());
    }

    #[test]
    fn forks_older_than_the_window_require_a_rescan() {
        let mut cache = cache_with_history();
        cache.recent.drain(..5);
        assert!(!cache.can_roll_back_to(3));
        assert!(cache.can_roll_back_to(6));
    }

    #[test]
    fn unlock_heights_match_consensus_maturity() {
        assert_eq!(unlock_height(Network::Devnet, 0, 50), 50);
        assert_eq!(
            unlock_height(Network::Devnet, 1, 50),
            50 + Network::Devnet.maturity()
        );
        assert_eq!(
            unlock_height(Network::Devnet, 2, 600),
            emission::treasury_unlock_height(600).unwrap()
        );
        assert_eq!(
            unlock_height(Network::Devnet, 2, 50),
            u64::MAX,
            "no treasury output exists before height 501"
        );
    }

    #[test]
    fn caches_are_encrypted_per_wallet_and_reject_other_wallets() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("w.scan");
        let wallet = Wallet::create(Network::Devnet).unwrap();
        let cache = cache_with_history();
        save_cache(&wallet, &path, &cache).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            !bytes.windows(5).any(|w| w == b"mined"),
            "history is not stored in plain text"
        );
        assert_eq!(load_cache(&wallet, &path), cache);
        let other = Wallet::create(Network::Devnet).unwrap();
        assert_eq!(
            load_cache(&other, &path),
            ScanCache::new(Network::Devnet, 0)
        );
        std::fs::write(&path, b"garbage").unwrap();
        assert_eq!(
            load_cache(&wallet, &path),
            ScanCache::new(Network::Devnet, 0)
        );
    }
}
