//! Wallet engine: one code path for light mode (remote nodes, everything verified
//! locally) and full-node mode (your own node as the source).

#![forbid(unsafe_code)]

pub mod file;
pub mod keys;
pub mod password;
pub mod proof;
pub mod swap;

use anyhow::{Context, Result, bail, ensure};
use keys::Keys;
use krypton_core::address::{
    Address, NotePlain, decrypt_note_unchecked, encrypt_note, memo_from_str, try_decrypt_note,
};
use krypton_core::reward;
use krypton_core::tree::Frontier;
use krypton_core::types::*;
use krypton_core::{Hash32, Network, emission};
use krypton_crypto::field::{Digest, digest_from_bytes, digest_from_uniform, digest_to_bytes};
use krypton_crypto::rescue;
use krypton_light::{CompactBlock, CompactVerifier};
use krypton_zk::{InputWitness, OutputWitness, SpendWitness};
use rand_core::RngCore;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// Where chain data comes from. Implemented by the HTTP node client; tests use a local store.
pub trait ChainSource {
    fn tip_height(&self) -> Result<u64>;
    fn header(&self, height: u64) -> Result<BlockHeader>;
    fn compact_blocks(&self, from: u64, count: u32) -> Result<Vec<CompactBlock>>;
    fn frontier(&self, height: u64) -> Result<Frontier>;
    fn witness(&self, pos: u64) -> Result<(Vec<Digest>, Digest)>;
    fn submit(&self, tx: &Transaction) -> Result<Hash32>;
    /// The tip proven by proof of work (FlyClient). `None` only for sources that are
    /// themselves trusted (a local full node's own store). `known` short-cuts an
    /// unchanged tip.
    fn verified_tip(
        &self,
        params: &krypton_core::ChainParams,
        pow: &krypton_pow::Verifier,
        known: Option<&BlockHeader>,
    ) -> Result<Option<BlockHeader>>;
    /// Whether `h` belongs to the chain committed by `tip`.
    fn header_in(&self, h: &BlockHeader, tip: &BlockHeader) -> Result<bool>;
    /// The total proof of work of the chain ending at the verified `tip`, as committed by
    /// its MMR. `None` where it is not known (trusted local sources).
    fn tip_work(&self, _tip: &BlockHeader) -> Result<Option<u128>> {
        Ok(None)
    }
}

impl ChainSource for krypton_light::client::NodeClient {
    fn verified_tip(
        &self,
        params: &krypton_core::ChainParams,
        pow: &krypton_pow::Verifier,
        known: Option<&BlockHeader>,
    ) -> Result<Option<BlockHeader>> {
        krypton_light::client::NodeClient::verified_tip(self, params, pow, known).map(Some)
    }
    fn header_in(&self, h: &BlockHeader, tip: &BlockHeader) -> Result<bool> {
        krypton_light::client::NodeClient::header_in(self, h, tip)
    }
    fn tip_work(&self, tip: &BlockHeader) -> Result<Option<u128>> {
        krypton_light::client::NodeClient::tip_work(self, tip).map(Some)
    }
    fn tip_height(&self) -> Result<u64> {
        Ok(self.info()?.height)
    }
    fn header(&self, height: u64) -> Result<BlockHeader> {
        krypton_light::client::NodeClient::header(self, height)
    }
    fn compact_blocks(&self, from: u64, count: u32) -> Result<Vec<CompactBlock>> {
        krypton_light::client::NodeClient::compact_blocks(self, from, count)
    }
    fn frontier(&self, height: u64) -> Result<Frontier> {
        krypton_light::client::NodeClient::frontier(self, height)
    }
    fn witness(&self, pos: u64) -> Result<(Vec<Digest>, Digest)> {
        self.subtree_witness(pos)
    }
    fn submit(&self, tx: &Transaction) -> Result<Hash32> {
        krypton_light::client::NodeClient::submit(self, tx)
    }
}

/// Nodes keep note-tree frontier checkpoints every this many blocks.
const CHECKPOINT: u64 = 64;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OwnedNote {
    #[serde(with = "hex::serde")]
    pub cm: Hash32,
    pub value: u64,
    #[serde(with = "hex::serde")]
    pub rcm: [u8; 32],
    pub memo: String,
    pub height: u64,
    pub coinbase: bool,
    pub position: Option<u64>,
    pub spent_height: Option<u64>,
    /// Transaction id of a pending spend (unconfirmed).
    pub pending_spend: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub txid: String,
    pub height: Option<u64>,
    pub amount: i128,
    pub fee: u64,
    pub memo: String,
    pub to: Option<String>,
    pub time: u64,
    /// For our own sends: the change commitment that confirms the transaction.
    #[serde(default)]
    pub change_cm: Option<String>,
    /// For our own payments: what a payment proof discloses (never shared unless asked).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disclosure: Option<Disclosure>,
    /// For our own sends: the last height at which the transaction may be mined.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expiry: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Disclosure {
    pub output: u8,
    #[serde(with = "hex::serde")]
    pub key: Vec<u8>,
    #[serde(with = "hex::serde")]
    pub owner: Vec<u8>,
    #[serde(with = "hex::serde")]
    pub recipient: Vec<u8>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Checkpoint {
    #[serde(with = "hex::serde")]
    header: Vec<u8>,
    #[serde(with = "hex::serde")]
    frontier: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct State {
    mnemonic: String,
    passphrase: String,
    network: Network,
    birthday: u64,
    notes: Vec<OwnedNote>,
    history: Vec<HistoryEntry>,
    /// Verified chain position (header + frontier) for recent blocks, newest last.
    checkpoints: Vec<Checkpoint>,
    /// Genesis hash (hex) of the chain the notes and checkpoints belong to.
    #[serde(default)]
    genesis: Option<String>,
    /// Lock notes of atomic swaps this wallet funds or may claim.
    #[serde(default)]
    locks: Vec<swap::LockRecord>,
    /// Application records stored (encrypted) with the wallet, e.g. swaps in progress.
    #[serde(default)]
    app: std::collections::BTreeMap<String, serde_json::Value>,
    /// The most proof of work this wallet has verified for its chain (decimal), and that
    /// tip's hash: a node showing a chain with less work is refused, so a cheap fork
    /// cannot replace the chain the wallet already knows.
    #[serde(default)]
    best_work: Option<String>,
    #[serde(default)]
    best_tip: Option<String>,
}

pub struct Wallet {
    pub keys: Keys,
    state: State,
    path: PathBuf,
    file_key: file::FileKey,
    verifier: Option<CompactVerifier>,
    /// Proof-of-work verifier for light-client checks (epoch caches built on demand).
    pow: std::sync::OnceLock<krypton_pow::Verifier>,
    /// Last tip verified by proof of work.
    verified_tip: Option<BlockHeader>,
    /// Payments to us seen in the mempool, not in a block yet (not saved: they are
    /// shown as "incoming, pending" and become ordinary notes once mined).
    incoming: Vec<Incoming>,
}

/// A payment to this wallet waiting in the mempool.
#[derive(Debug, Clone, Serialize)]
pub struct Incoming {
    pub txid: String,
    #[serde(skip)]
    pub cm: Hash32,
    pub value: u64,
    pub memo: String,
    /// When this wallet first saw it (unix seconds).
    pub seen: u64,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Balance {
    pub spendable: u64,
    pub pending: u64,
    pub immature: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SyncReport {
    pub from: u64,
    pub to: u64,
    pub new_notes: usize,
    pub spent_notes: usize,
    pub reorged: u64,
    /// The source follows a different chain (a relaunch): chain-derived state was
    /// discarded and rebuilt from the birthday. Keys are unaffected.
    pub reset: bool,
}

const KEEP_CHECKPOINTS: usize = 120;

impl Wallet {
    /// Creates a new wallet file. `birthday` is the first height to scan from.
    pub fn create(
        path: &Path,
        password: &str,
        mnemonic: &str,
        passphrase: &str,
        network: Network,
        birthday: u64,
        kdf: krypton_crypto::kdf::Argon2Params,
    ) -> Result<Self> {
        ensure!(
            !path.exists(),
            "wallet file already exists: {}",
            path.display()
        );
        ensure!(
            password.chars().count() >= 8,
            "use a password of at least 8 characters"
        );
        let keys = Keys::from_mnemonic(mnemonic, passphrase, network, 0)?;
        let state = State {
            mnemonic: keys::normalize(mnemonic)?.to_string(),
            passphrase: passphrase.to_string(),
            network,
            birthday,
            notes: vec![],
            history: vec![],
            checkpoints: vec![],
            genesis: None,
            locks: vec![],
            app: Default::default(),
            best_work: None,
            best_tip: None,
        };
        let w = Self {
            keys,
            state,
            path: path.to_path_buf(),
            file_key: file::FileKey::new(password, kdf)?,
            verifier: None,
            pow: std::sync::OnceLock::new(),
            verified_tip: None,
            incoming: Vec::new(),
        };
        w.save()?;
        Ok(w)
    }

    pub fn open(path: &Path, password: &str) -> Result<Self> {
        let (file_key, pt) = file::FileKey::open(path, password)?;
        let state: State = serde_json::from_slice(&pt)?;
        let keys = Keys::from_mnemonic(&state.mnemonic, &state.passphrase, state.network, 0)?;
        let mut w = Self {
            keys,
            state,
            path: path.to_path_buf(),
            file_key,
            verifier: None,
            pow: std::sync::OnceLock::new(),
            verified_tip: None,
            incoming: Vec::new(),
        };
        // Checkpoints written by an older format or for an abandoned chain cannot be
        // trusted: drop the chain-derived state and rescan (keys are unaffected).
        match w.restore_verifier() {
            Ok(v) => w.verifier = v,
            Err(_) => w.reset_chain_state(),
        }
        Ok(w)
    }

    fn reset_chain_state(&mut self) {
        self.state.notes.clear();
        self.state.history.clear();
        self.state.checkpoints.clear();
        self.state.genesis = None;
        self.state.best_work = None;
        self.state.best_tip = None;
        self.verifier = None;
        self.lock_rollback(0, 0, true);
    }

    /// Re-encrypts the wallet file under a new password (fresh salt). The caller has
    /// checked the old password.
    pub fn change_password(
        &mut self,
        new: &str,
        kdf: krypton_crypto::kdf::Argon2Params,
    ) -> Result<()> {
        ensure!(
            new.chars().count() >= 8,
            "use a password of at least 8 characters"
        );
        self.file_key = file::FileKey::new(new, kdf)?;
        self.save()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn save(&self) -> Result<()> {
        let pt = Zeroizing::new(serde_json::to_vec(&self.state)?);
        self.file_key.save(&self.path, &pt)
    }

    /// Payment proof for one of our sends (see [`proof`]).
    /// Looks for payments to us among pending transactions. Wallets fetch the whole pool,
    /// so the node learns nothing about which transactions are theirs. `live` are the
    /// ids still in the pool: entries that left it or were mined are dropped.
    /// Returns how many incoming payments are new.
    pub fn scan_mempool(&mut self, txs: &[Transaction], live: &[Hash32]) -> usize {
        self.learn_secrets(txs);
        let owner = self.keys.spend.owner;
        let live: std::collections::HashSet<String> = live.iter().map(hex::encode).collect();
        let mut kept: Vec<Incoming> = std::mem::take(&mut self.incoming)
            .into_iter()
            .filter(|i| live.contains(&i.txid) && !self.state.notes.iter().any(|n| n.cm == i.cm))
            .collect();
        let mut new = 0;
        for tx in txs {
            let id = hex::encode(tx.txid());
            // Our own sends (and their change) are already in the history.
            if !live.contains(&id) || self.state.history.iter().any(|h| h.txid == id) {
                continue;
            }
            for o in &tx.outputs {
                if kept.iter().any(|i| i.cm == o.cm)
                    || self.state.notes.iter().any(|n| n.cm == o.cm)
                {
                    continue;
                }
                if let Some(note) = try_decrypt_note(&self.keys.kem, &owner, &o.cm, &o.ciphertext)
                    && note.value > 0
                {
                    kept.push(Incoming {
                        txid: id.clone(),
                        cm: o.cm,
                        value: note.value,
                        memo: note.memo_text(),
                        seen: krypton_core::now(),
                    });
                    new += 1;
                }
            }
        }
        self.incoming = kept;
        new
    }

    /// Payments to us waiting in the mempool (see [`Wallet::scan_mempool`]).
    pub fn incoming(&self) -> &[Incoming] {
        &self.incoming
    }

    pub fn payment_proof(&self, txid: &str) -> Result<String> {
        let h = self
            .state
            .history
            .iter()
            .find(|h| h.txid == txid)
            .context("no such transaction in this wallet")?;
        let d = h
            .disclosure
            .as_ref()
            .context("no payment proof for this entry (receipts and self-transfers have none)")?;
        let arr = |v: &[u8]| -> Result<Hash32> { v.try_into().context("corrupt disclosure") };
        Ok(proof::PaymentProof {
            network: self.network(),
            txid: arr(&hex::decode(txid)?)?,
            output: d.output,
            key: arr(&d.key)?,
            owner: arr(&d.owner)?,
            recipient: arr(&d.recipient)?,
        }
        .encode())
    }

    /// Checks a payment proof against the chain; confirms whether it paid this wallet.
    pub fn verify_payment(
        &self,
        src: &impl proof::ProofSource,
        proof_text: &str,
    ) -> Result<proof::PaymentReport> {
        let p = proof::PaymentProof::decode(proof_text)?;
        ensure!(
            p.network == self.network(),
            "proof is for {}",
            p.network.as_str()
        );
        proof::verify_for(&p, src, Some(&self.address()), self.pow())
    }

    /// A record the application keeps in the (encrypted) wallet file.
    pub fn app_data(&self, key: &str) -> Option<&serde_json::Value> {
        self.state.app.get(key)
    }

    pub fn set_app_data(&mut self, key: &str, v: serde_json::Value) -> Result<()> {
        self.state.app.insert(key.to_string(), v);
        self.save()
    }

    pub fn mnemonic(&self) -> Zeroizing<String> {
        Zeroizing::new(self.state.mnemonic.clone())
    }
    pub fn network(&self) -> Network {
        self.state.network
    }
    pub fn address(&self) -> Address {
        self.keys.address()
    }
    pub fn notes(&self) -> &[OwnedNote] {
        &self.state.notes
    }
    pub fn history(&self) -> &[HistoryEntry] {
        &self.state.history
    }
    pub fn synced_height(&self) -> Option<u64> {
        self.verifier.as_ref().map(|v| v.last.height)
    }

    fn restore_verifier(&self) -> Result<Option<CompactVerifier>> {
        let Some(c) = self.state.checkpoints.last() else {
            return Ok(None);
        };
        let header = BlockHeader::from_bytes(&c.header)?;
        let frontier = Frontier::from_bytes(&c.frontier).context("corrupt checkpoint")?;
        Ok(Some(CompactVerifier::new(header, frontier)?))
    }

    fn push_checkpoint(&mut self) {
        if let Some(v) = &self.verifier {
            self.state.checkpoints.push(Checkpoint {
                header: v.last.to_bytes(),
                frontier: v.frontier.to_bytes(),
            });
            let n = self.state.checkpoints.len();
            if n > KEEP_CHECKPOINTS {
                self.state.checkpoints.drain(..n - KEEP_CHECKPOINTS);
            }
        }
    }

    pub fn balance(&self) -> Balance {
        let mut b = Balance::default();
        for n in self.state.notes.iter().filter(|n| n.spent_height.is_none()) {
            b.total += n.value;
            if n.position.is_none() {
                b.immature += n.value;
            } else if n.pending_spend.is_some() {
                b.pending += n.value;
            } else {
                b.spendable += n.value;
            }
        }
        b
    }

    /// Proof-of-work verifier for this wallet's network.
    pub fn pow(&self) -> &krypton_pow::Verifier {
        self.pow.get_or_init(|| {
            let p = self.network().params();
            krypton_pow::Verifier::new(p.pow, krypton_core::consensus::genesis_block(&p).hash())
        })
    }

    /// The source's tip, proven by proof of work (None for a trusted local store).
    pub fn verify_tip(&mut self, src: &impl ChainSource) -> Result<Option<BlockHeader>> {
        let params = self.network().params();
        let known = self.verified_tip.clone();
        let t = src.verified_tip(&params, self.pow(), known.as_ref())?;
        self.verified_tip = t.clone();
        Ok(t)
    }

    /// Refuses a proven tip with less total work than the best this wallet has verified
    /// (an honest chain only gains work), and remembers it otherwise. Whether it is
    /// strictly heavier than the best before.
    fn check_work(&mut self, src: &impl ChainSource, t: &BlockHeader) -> Result<bool> {
        let hash = hex::encode(t.hash());
        if self.state.best_tip.as_deref() == Some(hash.as_str()) {
            return Ok(false);
        }
        let Some(work) = src.tip_work(t)? else {
            return Ok(false);
        };
        let best = self
            .state
            .best_work
            .as_deref()
            .and_then(|s| s.parse::<u128>().ok());
        if best.is_some_and(|b| work < b) {
            bail!(
                "this node shows a chain with less proof of work than one this wallet has already verified (it may be behind, or dishonest); try again later or use another node"
            );
        }
        self.state.best_work = Some(work.to_string());
        self.state.best_tip = Some(hash);
        Ok(best.is_none_or(|b| work > b))
    }

    /// Synchronizes with `src`. The tip is proven by proof of work (FlyClient); every
    /// batch of compact blocks must hash-link to it and is checked against its headers
    /// before any of it is applied.
    pub fn sync(
        &mut self,
        src: &impl ChainSource,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<SyncReport> {
        let params = self.network().params();
        let expected_genesis = krypton_core::consensus::genesis_block(&params).hash();
        ensure!(
            src.header(0)?.hash() == expected_genesis,
            "the node follows another chain than this wallet's network"
        );
        let vtip = self.verify_tip(src)?;
        let tip = match &vtip {
            Some(t) => t.height,
            None => src.tip_height()?,
        };
        let mut report = SyncReport::default();
        let genesis = hex::encode(expected_genesis);
        let foreign = match &self.state.genesis {
            Some(g) => *g != genesis,
            None => match self.state.checkpoints.first() {
                Some(c) => {
                    let h = BlockHeader::from_bytes(&c.header)?;
                    match &vtip {
                        // Only a proven chain may tell us ours is foreign.
                        Some(t) => h.height <= t.height && !src.header_in(&h, t)?,
                        None => h.height > tip || src.header(h.height)?.hash() != h.hash(),
                    }
                }
                None => false,
            },
        };
        if foreign {
            self.reset_chain_state();
            report.reset = true;
        }
        self.state.genesis = Some(genesis);
        if let Some(t) = &vtip {
            let heavier = self.check_work(src, t)?;
            // We followed a fork: the proven chain (at least as heavy) has another block at
            // our height, itself proven by the tip's MMR, or is strictly heavier and shorter.
            // A node that merely sends bad proofs proves nothing and changes nothing. Go
            // back to where the chains meet, or rescan if that is deeper than we keep.
            let fork = match self.verifier.as_ref().map(|v| v.last.clone()) {
                Some(last) if last.height > t.height => heavier,
                Some(last) if last.height == t.height => last.hash() != t.hash(),
                Some(last) => {
                    let theirs = src.header(last.height)?;
                    theirs.hash() != last.hash() && src.header_in(&theirs, t)?
                }
                None => false,
            };
            if fork {
                match self.rollback(src, vtip.as_ref()) {
                    Ok(n) => report.reorged += n,
                    Err(_) => {
                        self.reset_chain_state();
                        self.check_work(src, t)?;
                        report.reset = true;
                    }
                }
            }
        }
        if self.verifier.is_none() {
            let start = (self.state.birthday / CHECKPOINT) * CHECKPOINT;
            let start = start.min(tip / CHECKPOINT * CHECKPOINT);
            let header = src.header(start)?;
            if let Some(t) = &vtip {
                ensure!(
                    src.header_in(&header, t)?,
                    "starting header is not part of the proven chain"
                );
            }
            let frontier = src.frontier(start)?;
            self.verifier = Some(CompactVerifier::new(header, frontier)?);
            self.push_checkpoint();
        }
        report.from = self.verifier.as_ref().map(|v| v.last.height).unwrap_or(0);
        loop {
            let next = self.verifier.as_ref().expect("verifier").last.height + 1;
            if next > tip {
                break;
            }
            let count = (tip - next + 1).min(50) as u32;
            let mut blocks = src.compact_blocks(next, count)?;
            if blocks.is_empty() {
                break;
            }
            blocks.retain(|b| b.header.height <= tip);
            if blocks.is_empty() {
                break;
            }
            let last = self.verifier.as_ref().expect("verifier").last.hash();
            if let Some(t) = &vtip
                && blocks.first().is_some_and(|b| b.header.prev == last)
            {
                let linked = blocks.windows(2).all(|w| {
                    w[1].header.prev == w[0].hash() && w[1].header.height == w[0].header.height + 1
                });
                let end = &blocks.last().expect("non-empty").header;
                ensure!(
                    linked && src.header_in(end, t)?,
                    "blocks from the node are not part of the proven chain"
                );
            }
            for b in &blocks {
                let ok = self.verifier.as_ref().expect("verifier").last.hash() == b.header.prev;
                if !ok {
                    // Roll back only if the proven chain no longer contains our last block;
                    // a node sending blocks that merely fail to link must not undo history.
                    if let Some(t) = &vtip {
                        let last = self.verifier.as_ref().expect("verifier").last.clone();
                        ensure!(
                            last.height > t.height || !src.header_in(&last, t)?,
                            "the node sent blocks that do not extend the proven chain"
                        );
                    }
                    report.reorged += self.rollback(src, vtip.as_ref())?;
                    break;
                }
                self.verifier.as_mut().expect("verifier").verify(b)?;
                self.process_block(b, &mut report)?;
                self.push_checkpoint();
                progress(b.header.height, tip);
            }
        }
        report.to = self.verifier.as_ref().map(|v| v.last.height).unwrap_or(0);
        self.expire_pending(report.to);
        self.save()?;
        Ok(report)
    }

    /// Forgets our sends that can no longer be mined (past their expiry height without
    /// confirming) and makes the notes they would have spent spendable again.
    fn expire_pending(&mut self, synced: u64) {
        let dead = |h: &HistoryEntry| h.height.is_none() && h.expiry.is_some_and(|e| e < synced);
        let expired: Vec<String> = self
            .state
            .history
            .iter()
            .filter(|h| dead(h))
            .map(|h| h.txid.clone())
            .collect();
        if expired.is_empty() {
            return;
        }
        self.state.history.retain(|h| !dead(h));
        for n in self.state.notes.iter_mut() {
            if n.spent_height.is_none()
                && n.pending_spend
                    .as_ref()
                    .is_some_and(|t| expired.contains(t))
            {
                n.pending_spend = None;
            }
        }
        for l in self.state.locks.iter_mut() {
            if l.pending_spend
                .as_ref()
                .is_some_and(|t| expired.contains(t))
            {
                l.pending_spend = None;
            }
        }
    }

    /// Steps back until our checkpoint matches the source's chain.
    fn rollback(&mut self, src: &impl ChainSource, vtip: Option<&BlockHeader>) -> Result<u64> {
        let mut dropped = 0u64;
        loop {
            let Some(c) = self.state.checkpoints.last() else {
                bail!(
                    "reorganization deeper than the kept history; restore from the recovery phrase"
                )
            };
            let h = BlockHeader::from_bytes(&c.header)?;
            // The fork point is found against the proven chain, not the node's word.
            let still_in = match vtip {
                Some(t) => h.height <= t.height && src.header_in(&h, t)?,
                None => src.header(h.height)?.hash() == h.hash(),
            };
            if still_in {
                break;
            }
            self.state.checkpoints.pop();
            dropped += 1;
        }
        let cp = self.state.checkpoints.last().expect("checkpoint");
        let fork = BlockHeader::from_bytes(&cp.header)?.height;
        let tree_size = Frontier::from_bytes(&cp.frontier)
            .context("corrupt checkpoint")?
            .size;
        self.state.notes.retain(|n| n.height <= fork);
        for n in &mut self.state.notes {
            if n.spent_height.is_some_and(|s| s > fork) {
                n.spent_height = None;
            }
            // Positions assigned after the fork are recomputed while rescanning.
            if n.position.is_some_and(|p| p >= tree_size) {
                n.position = None;
            }
        }
        self.lock_rollback(fork, tree_size, false);
        // Receipts after the fork are found again while rescanning; own sends become pending.
        self.state.history.retain(|h| {
            let receipt = h.txid.starts_with("payout:") || h.txid.starts_with("note:");
            !(receipt && h.height.is_some_and(|x| x > fork))
        });
        for h in &mut self.state.history {
            if h.height.is_some_and(|x| x > fork) {
                h.height = None;
            }
        }
        self.verifier = self.restore_verifier()?;
        Ok(dropped)
    }

    fn process_block(&mut self, b: &CompactBlock, report: &mut SyncReport) -> Result<()> {
        let height = b.header.height;
        let owner = self.keys.spend.owner;
        let owner_bytes = digest_to_bytes(&owner);
        for (i, p) in b.payouts.iter().enumerate() {
            if p.owner != owner_bytes || p.value == 0 {
                continue;
            }
            let cm = reward::payout_cm(height, i as u32, p)?;
            if self.state.notes.iter().any(|n| n.cm == cm) {
                continue;
            }
            self.state.notes.push(OwnedNote {
                cm,
                value: p.value,
                rcm: digest_to_bytes(&reward::payout_rcm(height, i as u32, &p.owner)),
                memo: "mining reward".into(),
                height,
                coinbase: true,
                position: None,
                spent_height: None,
                pending_spend: None,
            });
            report.new_notes += 1;
            self.state.history.push(HistoryEntry {
                txid: format!("payout:{}", hex::encode(cm)),
                height: Some(height),
                amount: p.value as i128,
                fee: 0,
                memo: "mining reward".into(),
                to: None,
                time: b.header.timestamp,
                change_cm: None,
                disclosure: None,
                expiry: None,
            });
        }
        for o in &b.outputs {
            self.lock_mined(&o.cm, height);
            if self.state.notes.iter().any(|n| n.cm == o.cm) {
                continue;
            }
            let Some(note) = decrypt_note_unchecked(&self.keys.kem, &o.cm, &o.ciphertext) else {
                continue;
            };
            if digest_to_bytes(&rescue::note_commitment(&owner, note.value, &note.rcm)) != o.cm {
                // Not our note, but possibly a lock note we funded (recovery from the phrase).
                self.recover_lock(&note, &o.cm, height);
                continue;
            }
            {
                self.state.notes.push(OwnedNote {
                    cm: o.cm,
                    value: note.value,
                    rcm: digest_to_bytes(&note.rcm),
                    memo: note.memo_text(),
                    height,
                    coinbase: false,
                    position: None,
                    spent_height: None,
                    pending_spend: None,
                });
                let cm_hex = hex::encode(o.cm);
                let is_change = self
                    .state
                    .history
                    .iter()
                    .any(|h| h.change_cm.as_deref() == Some(cm_hex.as_str()));
                if note.value > 0 {
                    report.new_notes += 1;
                }
                if note.value > 0 && !is_change {
                    self.state.history.push(HistoryEntry {
                        txid: format!("note:{cm_hex}"),
                        height: Some(height),
                        amount: note.value as i128,
                        fee: 0,
                        memo: note.memo_text(),
                        to: None,
                        time: b.header.timestamp,
                        change_cm: None,
                        disclosure: None,
                        expiry: None,
                    });
                }
            }
        }
        for (i, cm) in b.appended()?.iter().enumerate() {
            if let Some(n) = self
                .state
                .notes
                .iter_mut()
                .find(|n| n.cm == *cm && n.position.is_none())
            {
                n.position = Some(b.leaf_start + i as u64);
            }
            self.lock_position(cm, b.leaf_start + i as u64);
        }
        report.spent_notes += self.lock_spent(&b.nullifiers, height);
        let nk = self.keys.spend.nk;
        for n in self
            .state
            .notes
            .iter_mut()
            .filter(|n| n.spent_height.is_none())
        {
            if let Some(p) = n.position {
                let nf = digest_to_bytes(&rescue::nullifier(&nk, p));
                if b.nullifiers.contains(&nf) {
                    n.spent_height = Some(height);
                    n.pending_spend = None;
                    report.spent_notes += 1;
                }
            }
        }
        for h in self.state.history.iter_mut().filter(|h| h.height.is_none()) {
            // Our own sends confirm when their change output appears.
            if b.outputs
                .iter()
                .any(|o| Some(hex::encode(o.cm)) == h.change_cm)
            {
                h.height = Some(height);
            }
        }
        Ok(())
    }

    /// Builds, proves and submits a shielded payment.
    pub fn send(
        &mut self,
        src: &impl ChainSource,
        to: &Address,
        amount: u64,
        fee: u64,
        memo: &str,
    ) -> Result<Hash32> {
        ensure!(
            to.network == self.network(),
            "recipient address is for {}",
            to.network.as_str()
        );
        ensure!(amount > 0, "amount must be positive");
        ensure!(
            (MIN_FEE..=MAX_FEE).contains(&fee),
            "the fee must be between {} and {} GNR",
            krypton_core::emission::format_amount(MIN_FEE),
            krypton_core::emission::format_amount(MAX_FEE)
        );
        ensure!(self.verifier.is_some(), "sync the wallet first");
        let need = amount.checked_add(fee).context("amount overflow")?;
        let chosen = self.select_notes(need)?;
        self.spend_notes(src, &chosen, to, amount, fee, memo)
    }

    /// One or two spendable notes covering `need`.
    fn select_notes(&self, need: u64) -> Result<Vec<OwnedNote>> {
        let mut spendable: Vec<&OwnedNote> = self
            .state
            .notes
            .iter()
            .filter(|n| {
                n.position.is_some()
                    && n.spent_height.is_none()
                    && n.pending_spend.is_none()
                    && n.value > 0
            })
            .collect();
        spendable.sort_by_key(|n| std::cmp::Reverse(n.value));
        Ok(
            if let Some(one) = spendable.iter().rev().find(|n| n.value >= need) {
                vec![(*one).clone()]
            } else if spendable.len() >= 2 && spendable[0].value + spendable[1].value >= need {
                vec![spendable[0].clone(), spendable[1].clone()]
            } else {
                let total: u64 = spendable.iter().map(|n| n.value).sum();
                if total >= need {
                    bail!(
                        "NEEDS_CONSOLIDATION: funds are split across many notes; a transaction spends at most two, so merge them first"
                    );
                }
                bail!(
                    "insufficient spendable funds: have {}, need {}",
                    emission::format_amount(total),
                    emission::format_amount(need)
                );
            },
        )
    }

    /// Input witnesses for `chosen` at one common root, which must be the note root of
    /// our verified tip; padded with a dummy input to two.
    fn input_witnesses(
        &self,
        src: &impl ChainSource,
        chosen: &[OwnedNote],
    ) -> Result<(Vec<InputWitness>, Digest)> {
        let verifier = self.verifier.as_ref().context("sync the wallet first")?;
        ensure!(
            !chosen.is_empty() && chosen.len() <= 2,
            "a transaction spends one or two notes"
        );
        let mut inputs = Vec::new();
        let mut root = None;
        for n in chosen {
            let pos = n.position.context("note not in the tree yet")?;
            let (path, r) = src.witness(pos)?;
            if let Some(prev) = root {
                ensure!(
                    prev == r,
                    "the chain advanced while fetching witnesses; try again"
                );
            }
            root = Some(r);
            inputs.push(InputWitness {
                sk: self.keys.spend.sk,
                value: n.value,
                rcm: digest_from_bytes(&n.rcm)?,
                pos,
                path,
                dummy: false,
                lock: false,
            });
        }
        let root = root.expect("root");
        ensure!(
            digest_to_bytes(&root) == verifier.last.note_root,
            "the node's tree root differs from our verified chain; sync and retry"
        );
        if inputs.len() == 1 {
            inputs.push(InputWitness::dummy());
        }
        Ok((inputs, root))
    }

    fn spend_notes(
        &mut self,
        src: &impl ChainSource,
        chosen: &[OwnedNote],
        to: &Address,
        amount: u64,
        fee: u64,
        memo: &str,
    ) -> Result<Hash32> {
        let tip = self
            .verifier
            .as_ref()
            .context("sync the wallet first")?
            .last
            .height;
        let need = amount.checked_add(fee).context("amount overflow")?;
        let (inputs, root) = self.input_witnesses(src, chosen)?;
        let in_sum: u64 = chosen.iter().map(|n| n.value).sum();
        let change = in_sum - need;
        let me = self.address();
        let mk_rcm = || {
            let mut b = [0u8; 32];
            rand_core::OsRng.fill_bytes(&mut b);
            digest_from_uniform(&b)
        };
        let pay = NotePlain {
            value: amount,
            rcm: mk_rcm(),
            memo: memo_from_str(memo),
        };
        let chg = NotePlain {
            value: change,
            rcm: mk_rcm(),
            memo: memo_from_str("change"),
        };
        let (cm0, ct0, pay_key) = krypton_core::address::encrypt_note_disclosable(to, &pay)?;
        let (cm1, ct1) = encrypt_note(&me, &chg)?;
        let mut tx = Transaction {
            version: TX_VERSION,
            kind: TxKind::Transfer,
            anchor: digest_to_bytes(&root),
            nullifiers: [
                HexHash(digest_to_bytes(&inputs[0].nullifier())),
                HexHash(digest_to_bytes(&inputs[1].nullifier())),
            ],
            outputs: [
                Output {
                    cm: cm0,
                    ciphertext: ct0,
                },
                Output {
                    cm: cm1,
                    ciphertext: ct1,
                },
            ],
            fee,
            mint: 0,
            expiry_height: tip + 120,
            lock: None,
            proof: vec![],
        };
        let w = SpendWitness {
            inputs: [inputs[0].clone(), inputs[1].clone()],
            outputs: [
                OutputWitness {
                    owner: to.owner,
                    value: amount,
                    rcm: pay.rcm,
                },
                OutputWitness {
                    owner: me.owner,
                    value: change,
                    rcm: chg.rcm,
                },
            ],
            root,
            fee,
            mint: 0,
            sighash: tx.sighash(),
        };
        let (_, proof) = krypton_zk::prove(&w).map_err(|e| anyhow::anyhow!("{e}"))?;
        tx.proof = proof;
        let txid = src.submit(&tx)?;
        let id = hex::encode(txid);
        for n in self.state.notes.iter_mut() {
            if chosen.iter().any(|c| c.cm == n.cm) {
                n.pending_spend = Some(id.clone());
            }
        }
        let to_self = to.owner == me.owner;
        self.state.history.push(HistoryEntry {
            txid: id,
            height: None,
            amount: if to_self {
                -(fee as i128)
            } else {
                -(amount as i128) - fee as i128
            },
            fee,
            memo: memo.to_string(),
            to: Some(to.short()),
            time: krypton_core::now(),
            change_cm: Some(hex::encode(cm1)),
            disclosure: (!to_self).then(|| Disclosure {
                output: 0,
                key: pay_key.to_vec(),
                owner: digest_to_bytes(&to.owner).to_vec(),
                recipient: to.short_id().to_vec(),
            }),
            expiry: Some(tx.expiry_height),
        });
        if to_self {
            // The payment output is ours too: do not report it as income.
            self.state.history.push(HistoryEntry {
                txid: format!("self:{}", hex::encode(cm0)),
                height: None,
                amount: 0,
                fee: 0,
                memo: "internal".into(),
                to: None,
                time: krypton_core::now(),
                change_cm: Some(hex::encode(cm0)),
                disclosure: None,
                expiry: Some(tx.expiry_height),
            });
        }
        self.save()?;
        Ok(txid)
    }

    /// One consolidation round: pairs spendable notes (largest first) and merges each
    /// pair in its own transaction, all in parallel. Returns the submitted txids.
    pub fn consolidate(
        &mut self,
        src: &impl ChainSource,
        fee: u64,
        max_txs: usize,
    ) -> Result<Vec<Hash32>> {
        let mut notes: Vec<OwnedNote> = self
            .state
            .notes
            .iter()
            .filter(|n| {
                n.position.is_some()
                    && n.spent_height.is_none()
                    && n.pending_spend.is_none()
                    && n.value > fee
            })
            .cloned()
            .collect();
        notes.sort_by_key(|n| std::cmp::Reverse(n.value));
        ensure!(notes.len() >= 2, "nothing to consolidate");
        let me = self.address();
        let mut ids = Vec::new();
        for pair in notes.chunks(2).take(max_txs) {
            if pair.len() < 2 {
                break;
            }
            ids.push(self.spend_notes(
                src,
                pair,
                &me,
                pair[0].value + pair[1].value - fee,
                fee,
                "consolidate",
            )?);
        }
        Ok(ids)
    }

    /// Sends, first merging notes in parallel rounds when the payment needs more than
    /// two notes. Waits for each round to confirm (one block per round).
    pub fn send_auto(
        &mut self,
        src: &impl ChainSource,
        to: &Address,
        amount: u64,
        fee: u64,
        memo: &str,
        mut status: impl FnMut(&str),
    ) -> Result<Hash32> {
        for round in 1..=16 {
            self.sync(src, |_, _| {})?;
            if !self.needs_consolidation(amount, fee) {
                return self.send(src, to, amount, fee, memo);
            }
            let ids = self.consolidate(src, fee, 64)?;
            status(&format!(
                "consolidation round {round}: merging notes in {} transactions, waiting for the next block",
                ids.len()
            ));
            let start = std::time::Instant::now();
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3));
                self.sync(src, |_, _| {})?;
                if self.balance().pending == 0 {
                    break;
                }
                if start.elapsed() > std::time::Duration::from_secs(1800) {
                    bail!("consolidation did not confirm within 30 minutes");
                }
            }
        }
        bail!("too many consolidation rounds")
    }

    /// Whether a payment of `amount + fee` needs consolidation first.
    pub fn needs_consolidation(&self, amount: u64, fee: u64) -> bool {
        let mut v: Vec<u64> = self
            .state
            .notes
            .iter()
            .filter(|n| {
                n.position.is_some() && n.spent_height.is_none() && n.pending_spend.is_none()
            })
            .map(|n| n.value)
            .collect();
        v.sort_unstable_by(|a, b| b.cmp(a));
        let need = amount.saturating_add(fee);
        let total: u64 = v.iter().sum();
        total >= need && v.iter().take(2).sum::<u64>() < need
    }

    /// Clears pending spends that never confirmed (expired or dropped).
    pub fn reset_pending(&mut self) -> Result<()> {
        for n in self.state.notes.iter_mut() {
            n.pending_spend = None;
        }
        for l in self.state.locks.iter_mut() {
            l.pending_spend = None;
        }
        self.save()
    }
}

/// Fee for one transfer at `per_kb` gori per KB of transaction (never below the relay
/// floor). Fees are a market: wallets take `per_kb` from the node's estimate.
pub fn transfer_fee(per_kb: u64) -> u64 {
    use krypton_core::params::{FEE_ESTIMATE_KB, MIN_FEE_PER_KB};
    per_kb.max(MIN_FEE_PER_KB).saturating_mul(FEE_ESTIMATE_KB)
}

/// The lowest fee a transfer can carry and still be relayed.
pub const MIN_FEE: u64 =
    krypton_core::params::MIN_FEE_PER_KB * krypton_core::params::FEE_ESTIMATE_KB;

/// Fee when no estimate is available: twice the floor (about 0.000026 GNR).
pub const DEFAULT_FEE: u64 = 2 * MIN_FEE;

/// A fee above this (0.1 GNR) is refused as a likely typo.
pub const MAX_FEE: u64 = 10_000_000;

#[cfg(test)]
mod tests;
