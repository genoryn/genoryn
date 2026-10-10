//! Durable outgoing transactions.
//!
//! A payment is signed locally, its exact bytes and transaction id are sealed
//! into the outbox and synced to disk, and only then handed to a node. If the
//! node, Tor, the network or this computer fails before an acknowledgement, the
//! same bytes are relayed again later: a lost answer never produces a second,
//! different spend. The notes a pending payment spends stay reserved until the
//! chain itself shows the outcome (confirmed, spent by something else, or
//! expired), never because a node was unreachable or answered with an error.
use crate::{keys::Wallet, sealed, sync::ScanCache};
use anyhow::{Result, ensure};
use genoryn_core::{
    codec::{self, Hash},
    privacy::ShieldedTx,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

const OUTBOX_VERSION: u16 = 1;
const OUTBOX_DOMAIN: &[u8] = b"GENORYN/outbox/v1";
const MAX_OUTBOX_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 64;
/// Blocks past a transaction's expiry before its notes are released, so a
/// short reorganization that still includes it cannot surprise the wallet.
pub const EXPIRY_MARGIN: u64 = 10;
/// Blocks a confirmation must be buried before the entry is dropped.
pub const FORGET_AFTER: u64 = 720;
/// Base pause between relay attempts of an unacknowledged payment.
pub const RETRY_SECONDS: u64 = 15;
/// Pause between re-announcements of an acknowledged, unconfirmed payment.
pub const RESUBMIT_SECONDS: u64 = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutboxStatus {
    /// Signed and stored; no node has acknowledged it yet.
    Pending,
    /// A node acknowledged it with the matching transaction id. Not a payment
    /// confirmation: only a block is.
    Submitted,
    /// Included in the chain this wallet follows at `confirmed_height`.
    Confirmed,
    /// Past its expiry without being included; its notes are free again.
    Expired,
    /// Its notes were spent by a different transaction.
    Conflicted,
    /// It was confirmed, then a reorganization removed it; relayed again.
    Reorged,
}
impl OutboxStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Submitted => "Submitted",
            Self::Confirmed => "Confirmed",
            Self::Expired => "Expired",
            Self::Conflicted => "Conflicted",
            Self::Reorged => "Reorged",
        }
    }
    /// Whether the entry still holds its notes and should be relayed.
    pub fn is_open(self) -> bool {
        matches!(self, Self::Pending | Self::Submitted | Self::Reorged)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboxEntry {
    pub txid: Hash,
    /// Exact signed transaction bytes (`codec` encoding).
    pub tx_bytes: Vec<u8>,
    pub nullifiers: Vec<Hash>,
    pub expires: u64,
    pub created: u64,
    pub recipient: String,
    pub amount_atomic: u64,
    pub fee_atomic: u64,
    pub status: OutboxStatus,
    pub confirmed_height: Option<u64>,
    pub attempts: u32,
    /// Unix time of the last relay attempt.
    pub last_attempt: u64,
    pub last_error: Option<String>,
}
impl OutboxEntry {
    /// Whether to relay again at unix time `now`: soon while no node has
    /// acknowledged it, rarely once one has (in case that node dropped it).
    pub fn due(&self, now: u64) -> bool {
        let wait = match self.status {
            OutboxStatus::Pending | OutboxStatus::Reorged => {
                RETRY_SECONDS.saturating_mul(u64::from(self.attempts.min(10)))
            }
            OutboxStatus::Submitted => RESUBMIT_SECONDS,
            _ => return false,
        };
        now.saturating_sub(self.last_attempt) >= wait
    }
    pub fn new(
        tx: &ShieldedTx,
        recipient: String,
        amount_atomic: u64,
        fee_atomic: u64,
        created: u64,
    ) -> Result<Self> {
        Ok(Self {
            txid: tx.id()?,
            tx_bytes: codec::encode(tx)?,
            nullifiers: tx.bundle.actions.iter().map(|a| a.nullifier).collect(),
            expires: tx.context.expires,
            created,
            recipient,
            amount_atomic,
            fee_atomic,
            status: OutboxStatus::Pending,
            confirmed_height: None,
            attempts: 0,
            last_attempt: 0,
            last_error: None,
        })
    }
    /// The stored transaction, re-checked against its recorded id.
    pub fn transaction(&self) -> Result<ShieldedTx> {
        let tx: ShieldedTx = codec::decode(&self.tx_bytes, codec::MAX_TX_BYTES)?;
        ensure!(tx.id()? == self.txid, "outbox entry does not match its id");
        Ok(tx)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outbox {
    pub entries: Vec<OutboxEntry>,
}

#[derive(Clone, Debug, Serialize)]
pub struct OutboxSummary {
    pub txid: String,
    pub status: OutboxStatus,
    pub status_label: &'static str,
    pub recipient: String,
    pub amount_atomic: String,
    pub fee_atomic: String,
    pub created: u64,
    pub expires: u64,
    pub confirmed_height: Option<u64>,
    pub attempts: u32,
    pub last_error: Option<String>,
}

impl Outbox {
    /// Nullifiers reserved by payments whose outcome the chain has not shown.
    pub fn reserved(&self) -> BTreeSet<Hash> {
        self.entries
            .iter()
            .filter(|entry| entry.status.is_open())
            .flat_map(|entry| entry.nullifiers.iter().copied())
            .collect()
    }
    /// Adds a newly signed payment. Refuses one that reuses a reserved note.
    pub fn add(&mut self, entry: OutboxEntry) -> Result<()> {
        let reserved = self.reserved();
        ensure!(
            !entry.nullifiers.iter().any(|n| reserved.contains(n)),
            "a pending payment already spends one of these coins"
        );
        ensure!(
            !self.entries.iter().any(|e| e.txid == entry.txid),
            "payment already stored"
        );
        ensure!(
            self.entries.iter().filter(|e| e.status.is_open()).count() < MAX_ENTRIES,
            "too many unconfirmed payments; wait for a block"
        );
        self.entries.push(entry);
        Ok(())
    }
    /// Updates every entry from the validated chain summarised by `cache`.
    /// Returns whether anything changed.
    pub fn reconcile(&mut self, cache: &ScanCache) -> bool {
        let mut changed = false;
        let tip = cache.height;
        for entry in &mut self.entries {
            let spends: Vec<Option<(u64, Hash)>> = entry
                .nullifiers
                .iter()
                .map(|nullifier| {
                    cache
                        .notes
                        .iter()
                        .find(|note| note.nullifier == *nullifier)
                        .and_then(|note| note.spent_height.zip(note.spent_txid))
                })
                .collect();
            let ours = spends
                .iter()
                .filter_map(|spend| *spend)
                .filter(|(_, txid)| *txid == entry.txid)
                .map(|(height, _)| height)
                .max();
            let other = spends
                .iter()
                .filter_map(|spend| *spend)
                .any(|(_, txid)| txid != entry.txid);
            let next = if let Some(height) = ours {
                entry.confirmed_height = Some(height);
                OutboxStatus::Confirmed
            } else if other {
                OutboxStatus::Conflicted
            } else if entry.status == OutboxStatus::Confirmed {
                entry.confirmed_height = None;
                OutboxStatus::Reorged
            } else if tip >= entry.expires.saturating_add(EXPIRY_MARGIN) {
                OutboxStatus::Expired
            } else {
                entry.status
            };
            if next != entry.status {
                entry.status = next;
                changed = true;
            }
        }
        let before = self.entries.len();
        self.entries.retain(|entry| match entry.status {
            OutboxStatus::Confirmed => entry
                .confirmed_height
                .is_none_or(|height| tip < height.saturating_add(FORGET_AFTER)),
            OutboxStatus::Expired | OutboxStatus::Conflicted => {
                tip < entry.expires.saturating_add(FORGET_AFTER)
            }
            _ => true,
        });
        changed || before != self.entries.len()
    }
    /// Entries to hand to a node now (again).
    pub fn to_relay(&self, now: u64) -> Vec<usize> {
        (0..self.entries.len())
            .filter(|i| self.entries[*i].due(now))
            .collect()
    }
    /// Records a node's answer at unix time `now`. An id that differs from ours
    /// is an error and changes nothing; failures never release reserved notes.
    pub fn record_relay(&mut self, index: usize, now: u64, answer: Result<String, String>) {
        let entry = &mut self.entries[index];
        entry.attempts = entry.attempts.saturating_add(1);
        entry.last_attempt = now;
        match answer {
            Ok(txid) if txid == hex::encode(entry.txid) => {
                entry.last_error = None;
                if matches!(entry.status, OutboxStatus::Pending | OutboxStatus::Reorged) {
                    entry.status = OutboxStatus::Submitted;
                }
            }
            Ok(txid) => {
                entry.last_error = Some(format!(
                    "the node answered with a different transaction id ({}); the payment stays pending",
                    txid.chars().take(64).collect::<String>()
                ));
            }
            Err(error) => entry.last_error = Some(error.chars().take(300).collect()),
        }
    }
    pub fn summary(&self) -> Vec<OutboxSummary> {
        self.entries
            .iter()
            .rev()
            .map(|entry| OutboxSummary {
                txid: hex::encode(entry.txid),
                status: entry.status,
                status_label: entry.status.label(),
                recipient: entry.recipient.clone(),
                amount_atomic: entry.amount_atomic.to_string(),
                fee_atomic: entry.fee_atomic.to_string(),
                created: entry.created,
                expires: entry.expires,
                confirmed_height: entry.confirmed_height,
                attempts: entry.attempts,
                last_error: entry.last_error.clone(),
            })
            .collect()
    }
}

/// A node that already holds the transaction answers with an error; that is
/// an acknowledgement of these exact bytes, not a failure.
pub fn is_already_known(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("duplicate mempool transaction")
        || error.contains("already in mempool")
        || error.contains("already known")
}

/// `wallet.gnr` keeps its outbox in `wallet.outbox` beside it.
pub fn outbox_path(wallet_path: &Path) -> PathBuf {
    wallet_path.with_extension("outbox")
}
/// A missing outbox is empty. A damaged or foreign one is an error: silently
/// starting empty would release reservations and could lead to a double spend.
pub fn load(wallet: &Wallet, path: &Path) -> Result<Outbox> {
    Ok(sealed::load(
        wallet,
        path,
        OUTBOX_DOMAIN,
        OUTBOX_VERSION,
        MAX_OUTBOX_BYTES,
    )?
    .unwrap_or_default())
}
pub fn save(wallet: &Wallet, path: &Path, outbox: &Outbox) -> Result<()> {
    sealed::save(wallet, path, OUTBOX_DOMAIN, OUTBOX_VERSION, outbox)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::CachedNote;
    use genoryn_core::params::Network;

    fn entry(txid: u8, nullifiers: &[u8], expires: u64) -> OutboxEntry {
        OutboxEntry {
            txid: [txid; 32],
            tx_bytes: vec![],
            nullifiers: nullifiers.iter().map(|n| [*n; 32]).collect(),
            expires,
            created: 0,
            recipient: "gnr1test".into(),
            amount_atomic: 5,
            fee_atomic: 1,
            status: OutboxStatus::Pending,
            confirmed_height: None,
            attempts: 0,
            last_attempt: 0,
            last_error: None,
        }
    }
    fn cache(tip: u64, spends: &[(u8, Option<(u64, u8)>)]) -> ScanCache {
        let mut cache = ScanCache::new(Network::Devnet, 0);
        cache.height = tip;
        for (nullifier, spent) in spends {
            cache.notes.push(CachedNote {
                value: 10,
                nullifier: [*nullifier; 32],
                txid: [0; 32],
                action: 0,
                height: 1,
                unlock_height: 1,
                spent_height: spent.map(|(h, _)| h),
                spent_txid: spent.map(|(_, t)| [t; 32]),
                memo: String::new(),
            });
        }
        cache
    }

    #[test]
    fn reserved_notes_cannot_be_spent_twice() {
        let mut outbox = Outbox::default();
        outbox.add(entry(1, &[7, 8], 100)).unwrap();
        assert!(outbox.add(entry(2, &[8], 100)).is_err());
        assert!(outbox.add(entry(1, &[9], 100)).is_err(), "same id");
        outbox.add(entry(3, &[9], 100)).unwrap();
        assert_eq!(outbox.reserved().len(), 3);
    }

    #[test]
    fn lost_acknowledgements_and_errors_never_release_reservations() {
        let mut outbox = Outbox::default();
        outbox.add(entry(1, &[7], 100)).unwrap();
        assert_eq!(outbox.to_relay(1000), vec![0], "relayed at once");
        outbox.record_relay(0, 1000, Err("connection reset".into()));
        assert!(outbox.to_relay(1005).is_empty(), "a short pause first");
        outbox.record_relay(0, 1020, Err("timeout".into()));
        assert_eq!(outbox.entries[0].status, OutboxStatus::Pending);
        assert_eq!(outbox.entries[0].attempts, 2);
        assert!(outbox.reserved().contains(&[7; 32]));
        assert_eq!(outbox.to_relay(1100), vec![0], "relayed again, same bytes");
    }

    #[test]
    fn a_mismatched_transaction_id_is_not_an_acknowledgement() {
        let mut outbox = Outbox::default();
        outbox.add(entry(1, &[7], 100)).unwrap();
        outbox.record_relay(0, 10, Ok(hex::encode([2u8; 32])));
        assert_eq!(outbox.entries[0].status, OutboxStatus::Pending);
        assert!(
            outbox.entries[0]
                .last_error
                .as_ref()
                .unwrap()
                .contains("different")
        );
        outbox.record_relay(0, 20, Ok(hex::encode([1u8; 32])));
        assert_eq!(outbox.entries[0].status, OutboxStatus::Submitted);
        assert!(
            outbox.to_relay(100).is_empty(),
            "acknowledged: rarely again"
        );
        assert_eq!(outbox.to_relay(20 + RESUBMIT_SECONDS), vec![0]);
    }

    #[test]
    fn chain_evidence_drives_every_final_state() {
        let mut outbox = Outbox::default();
        outbox.add(entry(1, &[7], 100)).unwrap();
        outbox.add(entry(2, &[8], 100)).unwrap();
        outbox.add(entry(3, &[9], 100)).unwrap();
        let state = cache(50, &[(7, Some((40, 1))), (8, Some((41, 99))), (9, None)]);
        assert!(outbox.reconcile(&state));
        assert_eq!(outbox.entries[0].status, OutboxStatus::Confirmed);
        assert_eq!(outbox.entries[0].confirmed_height, Some(40));
        assert_eq!(outbox.entries[1].status, OutboxStatus::Conflicted);
        assert_eq!(outbox.entries[2].status, OutboxStatus::Pending);
        // Before expiry plus margin the notes stay reserved.
        assert!(!outbox.reconcile(&cache(
            109,
            &[(7, Some((40, 1))), (8, Some((41, 99))), (9, None)]
        )));
        assert_eq!(outbox.entries[2].status, OutboxStatus::Pending);
        outbox.reconcile(&cache(
            110,
            &[(7, Some((40, 1))), (8, Some((41, 99))), (9, None)],
        ));
        assert_eq!(outbox.entries[2].status, OutboxStatus::Expired);
        assert!(!outbox.reserved().contains(&[9; 32]));
    }

    #[test]
    fn a_reorganization_reopens_a_confirmed_payment() {
        let mut outbox = Outbox::default();
        outbox.add(entry(1, &[7], 100)).unwrap();
        outbox.reconcile(&cache(50, &[(7, Some((40, 1)))]));
        assert_eq!(outbox.entries[0].status, OutboxStatus::Confirmed);
        outbox.reconcile(&cache(39, &[(7, None)]));
        assert_eq!(outbox.entries[0].status, OutboxStatus::Reorged);
        assert!(outbox.reserved().contains(&[7; 32]));
        assert_eq!(outbox.to_relay(1_000_000), vec![0]);
    }

    #[test]
    fn outbox_survives_restart_and_a_damaged_file_is_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let path = outbox_path(&directory.path().join("w.gnr"));
        let wallet = Wallet::create(Network::Devnet).unwrap();
        assert_eq!(load(&wallet, &path).unwrap(), Outbox::default());
        let mut outbox = Outbox::default();
        outbox.add(entry(1, &[7], 100)).unwrap();
        save(&wallet, &path, &outbox).unwrap();
        assert_eq!(load(&wallet, &path).unwrap(), outbox);
        std::fs::write(&path, b"{}").unwrap();
        assert!(load(&wallet, &path).is_err(), "never silently empty");
    }

    #[test]
    fn duplicate_relay_answers_count_as_acknowledgements() {
        assert!(is_already_known(
            "node rejected request: {\"error\":\"duplicate mempool transaction\"}"
        ));
        assert!(!is_already_known("nullifier already spent"));
        assert!(!is_already_known("invalid proof"));
    }
}
