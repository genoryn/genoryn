//! Incremental wallet sync against the user's own validating node.
//!
//! A full scan downloads and re-validates the whole chain on every refresh. When the
//! wallet talks to its own loopback node, which already validated every block, this
//! module instead keeps an encrypted cache of the wallet's notes and history and only
//! decrypts blocks added since the last sync. Forks are detected by block hash and
//! rolled back to the common ancestor; payments still use the full validated scan.
use crate::{
    keys::Wallet,
    scan::{HistoryEntry, WalletSummary},
};
use anyhow::{Context, Result, ensure};
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305,
    aead::{Aead, Payload},
};
use genoryn_core::{
    block::Block,
    codec::{self, Hash},
    emission, names,
    params::Network,
};
use orchard::keys::{FullViewingKey, IncomingViewingKey, OutgoingViewingKey, Scope};
use rand08::RngCore;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const CACHE_VERSION: u16 = 1;
/// Recent block hashes kept for locating a fork point after a reorganization.
pub const REORG_WINDOW: usize = 128;
const MAX_CACHE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedNote {
    pub value: u64,
    pub nullifier: Hash,
    pub txid: Hash,
    pub action: u32,
    pub height: u64,
    pub unlock_height: u64,
    pub spent_height: Option<u64>,
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanCache {
    pub version: u16,
    pub network: Network,
    pub account: u32,
    pub height: u64,
    /// `(height, hash)` of the most recent blocks, oldest first.
    pub recent: Vec<(u64, Hash)>,
    pub notes: Vec<CachedNote>,
    /// Chronological; summaries list the newest first.
    pub history: Vec<CachedEntry>,
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

impl ScanCache {
    pub fn new(network: Network, account: u32) -> Self {
        Self {
            version: CACHE_VERSION,
            network,
            account,
            height: 0,
            recent: Vec::new(),
            notes: Vec::new(),
            history: Vec::new(),
        }
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
        for tx in block.body.all_transactions() {
            for action in &tx.bundle.actions {
                for note in self.notes.iter_mut().filter(|note| {
                    note.spent_height.is_none() && note.nullifier == action.nullifier
                }) {
                    note.spent_height = Some(height);
                }
            }
            let bundle = tx.bundle.to_bundle()?;
            let txid = tx.id()?;
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
                self.notes.push(CachedNote {
                    value: note.value().inner(),
                    nullifier: note.nullifier(&keys.fvk).to_bytes(),
                    txid,
                    action: *action as u32,
                    height,
                    unlock_height: unlock_height(self.network, tx.context.kind, height),
                    spent_height: None,
                    memo,
                });
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
        self.height = height;
        self.recent.push((height, block.hash()?));
        if self.recent.len() > REORG_WINDOW {
            self.recent.remove(0);
        }
        Ok(())
    }
    /// Forgets everything above `height`, the last block shared with the node's chain.
    pub fn rollback_to(&mut self, height: u64) {
        self.notes.retain(|note| note.height <= height);
        for note in &mut self.notes {
            if note.spent_height.is_some_and(|spent| spent > height) {
                note.spent_height = None;
            }
        }
        self.history.retain(|entry| entry.height <= height);
        self.recent.retain(|(at, _)| *at <= height);
        self.height = height;
    }
    /// Whether a rollback to `height` can still be represented; otherwise rescan.
    pub fn can_roll_back_to(&self, height: u64) -> bool {
        height == 0 || self.recent.iter().any(|(at, _)| *at == height)
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
        cache.rollback_to(0);
        assert_eq!(cache.tip_hash(), Network::Devnet.genesis());
        assert!(cache.notes.is_empty() && cache.history.is_empty());
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
            unlock_height(Network::Devnet, 2, 50),
            emission::treasury_unlock_height(50).unwrap()
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

#[derive(Serialize, Deserialize)]
struct Envelope {
    version: u16,
    network: Network,
    account: u32,
    nonce: [u8; 24],
    ciphertext: Vec<u8>,
}

/// `wallet.gnr` keeps its scan cache in `wallet.scan` beside it.
pub fn cache_path(wallet_path: &Path) -> PathBuf {
    wallet_path.with_extension("scan")
}
fn aad(network: Network, account: u32, nonce: &[u8; 24]) -> Result<Vec<u8>> {
    codec::encode(&(b"GENORYN/scan-cache/v1".as_slice(), network, account, nonce))
}
/// A missing, foreign or undecryptable cache yields a fresh one (a full rescan).
pub fn load_cache(wallet: &Wallet, path: &Path) -> ScanCache {
    let fresh = || ScanCache::new(wallet.network, wallet.account);
    let read = || -> Result<ScanCache> {
        ensure!(
            std::fs::metadata(path)?.len() <= MAX_CACHE_BYTES,
            "scan cache too large"
        );
        let envelope: Envelope = serde_json::from_slice(&std::fs::read(path)?)?;
        ensure!(
            envelope.version == CACHE_VERSION
                && envelope.network == wallet.network
                && envelope.account == wallet.account,
            "scan cache belongs to another wallet or version"
        );
        let key = wallet.scan_cache_key()?;
        let cipher =
            XChaCha20Poly1305::new_from_slice(&*key).map_err(|_| anyhow::anyhow!("cipher init"))?;
        let plaintext = zeroize::Zeroizing::new(
            cipher
                .decrypt(
                    (&envelope.nonce).into(),
                    Payload {
                        msg: &envelope.ciphertext,
                        aad: &aad(envelope.network, envelope.account, &envelope.nonce)?,
                    },
                )
                .map_err(|_| anyhow::anyhow!("scan cache does not belong to this wallet"))?,
        );
        let cache: ScanCache = codec::decode(&plaintext, MAX_CACHE_BYTES as usize)?;
        ensure!(
            cache.version == CACHE_VERSION
                && cache.network == wallet.network
                && cache.account == wallet.account,
            "scan cache content mismatch"
        );
        Ok(cache)
    };
    read().unwrap_or_else(|_| fresh())
}
pub fn save_cache(wallet: &Wallet, path: &Path, cache: &ScanCache) -> Result<()> {
    let mut nonce = [0u8; 24];
    rand08::rngs::OsRng.fill_bytes(&mut nonce);
    let key = wallet.scan_cache_key()?;
    let cipher =
        XChaCha20Poly1305::new_from_slice(&*key).map_err(|_| anyhow::anyhow!("cipher init"))?;
    let plaintext = zeroize::Zeroizing::new(codec::encode(cache)?);
    let ciphertext = cipher
        .encrypt(
            (&nonce).into(),
            Payload {
                msg: &plaintext,
                aad: &aad(wallet.network, wallet.account, &nonce)?,
            },
        )
        .map_err(|_| anyhow::anyhow!("scan cache encryption failed"))?;
    let envelope = Envelope {
        version: CACHE_VERSION,
        network: wallet.network,
        account: wallet.account,
        nonce,
        ciphertext,
    };
    let temporary = path.with_extension("scan.tmp");
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        std::io::Write::write_all(&mut file, &serde_json::to_vec(&envelope)?)?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, path).context("could not store the scan cache")?;
    Ok(())
}
