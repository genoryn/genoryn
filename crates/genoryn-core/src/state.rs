use crate::{
    block::{Block, Body, Header},
    codec::{self, Hash},
    difficulty, emission,
    params::{MIN_FEE, Network},
    privacy::{self, ShieldedTx},
};
use anyhow::{Context, Result, ensure};
use incrementalmerkletree::{Hashable, Level, frontier::Frontier};
use orchard::{
    Anchor,
    note::ExtractedNoteCommitment,
    tree::{MerkleHashOrchard, MerklePath},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub const MAX_FUTURE_SECONDS: u64 = 120;
const MEDIAN_WINDOW: usize = 11;

/// How much of a block is re-verified when it is applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verification {
    /// Every consensus rule, including RandomX work, Halo2 proofs and state roots.
    Full,
    /// A block this node already fully validated before storing it, whose bytes
    /// are bound to the stored hash chain. Work, proofs and the nullifier root
    /// were checked at admission; every state transition is still re-executed.
    Stored,
}

/// Expected difficulty for the child of the last header in `ancestors`.
pub fn expected_difficulty(network: Network, ancestors: &[Header]) -> u64 {
    let tail = &ancestors[ancestors.len().saturating_sub(difficulty::WINDOW + 1)..];
    difficulty::next(
        &tail
            .iter()
            .map(|h| (h.timestamp, h.difficulty))
            .collect::<Vec<_>>(),
        network.initial_difficulty(),
    )
}
pub fn ancestors_median_time(ancestors: &[Header]) -> u64 {
    let tail = &ancestors[ancestors.len().saturating_sub(MEDIAN_WINDOW)..];
    difficulty::median_time(&tail.iter().map(|h| h.timestamp).collect::<Vec<_>>())
}
/// Zero-based ancestor index whose hash seeds RandomX at `height`; `None` is genesis.
pub fn pow_seed_index(height: u64) -> Option<usize> {
    let seed_height = height.saturating_sub(65) / 2048 * 2048;
    (seed_height > 0).then(|| (seed_height - 1) as usize)
}
pub fn pow_seed(network: Network, seed_ancestor: Option<&Header>) -> Result<Hash> {
    let source = match seed_ancestor {
        Some(header) => header.hash()?,
        None => network.genesis(),
    };
    Ok(codec::hash(b"GENORYN/RandomX/v1", &source))
}
/// Header rules that depend only on ancestor headers. `ancestors` must end at the
/// parent and contain at least its last `difficulty::WINDOW + 1` headers (or all).
pub fn check_header_context(
    network: Network,
    ancestors: &[Header],
    seed_ancestor: Option<&Header>,
    header: &Header,
    now: u64,
) -> Result<()> {
    ensure!(
        header.timestamp > ancestors_median_time(ancestors)
            && header.timestamp <= now.saturating_add(MAX_FUTURE_SECONDS),
        "invalid block timestamp"
    );
    ensure!(
        header.difficulty == expected_difficulty(network, ancestors)
            && header.pow_seed == pow_seed(network, seed_ancestor)?,
        "incorrect difficulty or RandomX seed"
    );
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NoteLocation {
    pub cmx: Hash,
    pub txid: Hash,
    pub action: u32,
    pub created_height: u64,
    pub unlock_height: u64,
}

#[derive(Clone)]
pub struct ChainState {
    pub network: Network,
    pub height: u64,
    pub best_hash: Hash,
    pub work: u128,
    pub headers: Vec<Header>,
    pub nullifiers: BTreeSet<Hash>,
    pub anchors: BTreeMap<Hash, u64>,
    pub mature: Vec<NoteLocation>,
    pub pending: BTreeMap<u64, Vec<NoteLocation>>,
    pub tree: Frontier<MerkleHashOrchard, 32>,
    /// On-chain usernames: derived state that never affects block validity.
    pub names: BTreeMap<String, crate::names::NameEntry>,
}
/// Successfully verified transaction ids. The id commits to every encoded byte,
/// including proof and signatures. Oldest entries leave first, so a full cache
/// never forces the whole mempool to be re-proven at once.
#[derive(Clone, Default)]
pub struct ProofCache {
    verified: BTreeSet<Hash>,
    order: VecDeque<Hash>,
}
const PROOF_CACHE_ENTRIES: usize = 4096;
impl ProofCache {
    pub fn verify(&mut self, tx: &ShieldedTx) -> Result<()> {
        let id = tx.id()?;
        if self.verified.contains(&id) {
            return Ok(());
        }
        tx.verify_crypto()?;
        while self.order.len() >= PROOF_CACHE_ENTRIES {
            if let Some(oldest) = self.order.pop_front() {
                self.verified.remove(&oldest);
            }
        }
        self.verified.insert(id);
        self.order.push_back(id);
        Ok(())
    }
}
impl ChainState {
    pub fn new(network: Network) -> Self {
        let tree = Frontier::empty();
        let mut anchors = BTreeMap::new();
        anchors.insert(Anchor::empty_tree().to_bytes(), 0);
        Self {
            network,
            height: 0,
            best_hash: network.genesis(),
            work: 0,
            headers: vec![],
            nullifiers: BTreeSet::new(),
            anchors,
            mature: vec![],
            pending: BTreeMap::new(),
            tree,
            names: BTreeMap::new(),
        }
    }
    pub fn notes_root(&self) -> Hash {
        Anchor::from(self.tree.root()).to_bytes()
    }
    /// Streams `codec::hash(domain, concat(sorted nullifiers))` without copying the set.
    pub fn nullifiers_root(&self) -> Hash {
        let domain = b"GENORYN/nullifiers/v1";
        let mut digest = Sha256::new();
        digest.update((domain.len() as u64).to_le_bytes());
        digest.update(domain);
        digest.update(((self.nullifiers.len() * 32) as u64).to_le_bytes());
        for nullifier in &self.nullifiers {
            digest.update(nullifier);
        }
        digest.finalize().into()
    }
    pub fn next_difficulty(&self) -> u64 {
        expected_difficulty(self.network, &self.headers)
    }
    pub fn median_time(&self) -> u64 {
        ancestors_median_time(&self.headers)
    }
    pub fn next_pow_seed(&self) -> Hash {
        pow_seed(
            self.network,
            pow_seed_index(self.height + 1).map(|i| &self.headers[i]),
        )
        .expect("header encoding cannot fail")
    }
    fn mature_at(&mut self, height: u64) -> Result<()> {
        let heights: Vec<_> = self.pending.range(..=height).map(|(h, _)| *h).collect();
        for h in heights {
            for note in self.pending.remove(&h).unwrap_or_default() {
                let cmx = Option::<ExtractedNoteCommitment>::from(
                    ExtractedNoteCommitment::from_bytes(&note.cmx),
                )
                .context("corrupt commitment")?;
                ensure!(
                    self.tree.append(MerkleHashOrchard::from_cmx(&cmx)),
                    "commitment tree exhausted"
                );
                self.mature.push(note);
            }
        }
        Ok(())
    }
    fn register_outputs(&mut self, tx: &ShieldedTx, height: u64, unlock: u64) -> Result<()> {
        let id = tx.id()?;
        for (i, a) in tx.bundle.actions.iter().enumerate() {
            ensure!(
                self.nullifiers.insert(a.nullifier),
                "duplicate or already spent nullifier"
            );
            self.pending.entry(unlock).or_default().push(NoteLocation {
                cmx: a.cmx,
                txid: id,
                action: i as u32,
                created_height: height,
                unlock_height: unlock,
            });
        }
        Ok(())
    }
    pub fn validate_transfer(
        &self,
        tx: &ShieldedTx,
        at_height: u64,
        cache: &mut ProofCache,
    ) -> Result<()> {
        self.check_transfer(tx, at_height, Some(cache))
    }
    /// `cache: None` skips proof and signature verification for stored blocks only.
    fn check_transfer(
        &self,
        tx: &ShieldedTx,
        at_height: u64,
        cache: Option<&mut ProofCache>,
    ) -> Result<()> {
        let c = &tx.context;
        ensure!(
            c.network == self.network && c.kind == 0 && c.height == 0,
            "transaction network/type mismatch"
        );
        ensure!(
            c.expires >= at_height && c.expires <= at_height.saturating_add(720),
            "transaction expired or expiry too far ahead"
        );
        ensure!(
            tx.bundle.flags == 7,
            "transfer flags must enable private spends and outputs"
        );
        ensure!(
            tx.bundle.actions.len() >= 2,
            "private transfer needs at least two actions"
        );
        ensure!(
            tx.bundle.value_balance >= MIN_FEE as i64,
            "fee below minimum"
        );
        ensure!(
            self.anchors.contains_key(&tx.bundle.anchor),
            "unknown or expired commitment anchor"
        );
        let mut seen = BTreeSet::new();
        for action in &tx.bundle.actions {
            ensure!(
                !self.nullifiers.contains(&action.nullifier) && seen.insert(action.nullifier),
                "nullifier already spent or duplicate"
            );
        }
        match cache {
            Some(cache) => cache.verify(tx),
            None => Ok(()),
        }
    }
    pub fn apply_body(
        &mut self,
        height: u64,
        previous: Hash,
        body: &Body,
        cache: &mut ProofCache,
    ) -> Result<()> {
        self.apply_body_with(height, previous, body, Some(cache))
    }
    fn apply_body_with(
        &mut self,
        height: u64,
        previous: Hash,
        body: &Body,
        mut cache: Option<&mut ProofCache>,
    ) -> Result<()> {
        ensure!(
            height == self.height + 1 && previous == self.best_hash,
            "non-contiguous state transition"
        );
        self.mature_at(height)?;
        let mut fees = 0u128;
        for tx in &body.transactions {
            self.check_transfer(tx, height, cache.as_deref_mut())?;
            fees = fees
                .checked_add(tx.bundle.value_balance as u128)
                .context("fee overflow")?;
            self.register_outputs(tx, height, height)?;
        }
        let reward = emission::reward(height);
        let expected = reward
            .miner()
            .checked_add(fees)
            .context("miner reward overflow")?;
        ensure!(expected <= i64::MAX as u128, "miner reward unrepresentable");
        self.validate_mint(
            &body.miner,
            1,
            height,
            previous,
            expected,
            cache.as_deref_mut(),
        )?;
        self.register_outputs(&body.miner, height, height + self.network.maturity())?;
        match (&body.treasury, reward.treasury) {
            (Some(tx), amount) if amount > 0 => {
                self.validate_mint(tx, 2, height, previous, amount, cache)?;
                ensure!(
                    privacy::treasury_note(tx)?.value().inner() as u128 == amount,
                    "incorrect treasury note amount"
                );
                self.register_outputs(
                    tx,
                    height,
                    emission::treasury_unlock_height(height).context("treasury schedule")?,
                )?;
            }
            (None, 0) => (),
            _ => anyhow::bail!("treasury output does not match emission schedule"),
        }
        ensure!(
            emission::issued(height) <= emission::MAX_SUPPLY,
            "supply cap exceeded"
        );
        self.mature_at(height)?;
        self.anchors.insert(self.notes_root(), height);
        self.anchors.retain(|_, h| height.saturating_sub(*h) <= 120);
        // Names apply only once the whole block is valid, in transaction order.
        for tx in &body.transactions {
            for operation in crate::names::operations(self.network, tx) {
                let _ = crate::names::apply(&mut self.names, self.network, &operation, height);
            }
        }
        Ok(())
    }
    fn validate_mint(
        &self,
        tx: &ShieldedTx,
        kind: u8,
        height: u64,
        previous: Hash,
        amount: u128,
        cache: Option<&mut ProofCache>,
    ) -> Result<()> {
        let c = &tx.context;
        ensure!(
            c.network == self.network
                && c.kind == kind
                && c.height == height
                && c.nonce == previous
                && c.expires == 0,
            "incorrect coinbase context"
        );
        ensure!(
            tx.bundle.flags == 6 && tx.bundle.actions.len() == 1,
            "coinbase must disable spends and have one output"
        );
        ensure!(
            tx.bundle.value_balance == -(amount as i64),
            "incorrect coinbase issuance"
        );
        ensure!(
            tx.bundle.anchor == Anchor::empty_tree().to_bytes(),
            "coinbase anchor must be empty"
        );
        match cache {
            Some(cache) => cache.verify(tx),
            None => Ok(()),
        }
    }
    pub fn apply_block(&self, block: &Block, now: u64, cache: &mut ProofCache) -> Result<Self> {
        let mut next = self.clone();
        next.apply_block_in_place(block, now, cache, Verification::Full)?;
        Ok(next)
    }
    /// Applies `block` to this state. On error the state is partially updated and
    /// must be discarded; callers that need the old state apply to a clone.
    pub fn apply_block_in_place(
        &mut self,
        block: &Block,
        now: u64,
        cache: &mut ProofCache,
        verification: Verification,
    ) -> Result<()> {
        block.check_shape()?;
        let h = &block.header;
        ensure!(
            h.network == self.network
                && h.height == self.height + 1
                && h.previous == self.best_hash,
            "wrong chain/height/parent"
        );
        let seed_ancestor = pow_seed_index(h.height)
            .map(|i| self.headers.get(i).context("missing RandomX seed ancestor"))
            .transpose()?;
        check_header_context(self.network, &self.headers, seed_ancestor, h, now)?;
        let full = verification == Verification::Full;
        if full {
            crate::pow::verify(h.pow_seed, &h.bytes()?, h.difficulty)?;
        }
        self.apply_body_with(h.height, h.previous, &block.body, full.then_some(cache))?;
        // The nullifier root hashes the whole set; replaying stored blocks skips only it.
        ensure!(
            h.notes_root == self.notes_root()
                && (!full || h.nullifiers_root == self.nullifiers_root()),
            "state root mismatch"
        );
        self.height = h.height;
        self.best_hash = block.hash()?;
        self.work = self
            .work
            .checked_add(h.difficulty as u128)
            .context("chain work overflow")?;
        self.headers.push(h.clone());
        Ok(())
    }
    /// Local wallet Merkle witness. Never expose a per-address query to a server.
    pub fn witness(&self, position: usize) -> Result<MerklePath> {
        ensure!(
            position < self.mature.len() && position <= u32::MAX as usize,
            "note position out of bounds"
        );
        let mut nodes: Vec<_> = self
            .mature
            .iter()
            .map(|n| {
                Option::<MerkleHashOrchard>::from(MerkleHashOrchard::from_bytes(&n.cmx))
                    .context("invalid commitment")
            })
            .collect::<Result<_>>()?;
        let mut index = position;
        let mut siblings = Vec::with_capacity(32);
        for level in 0..32u8 {
            let empty = MerkleHashOrchard::empty_root(Level::from(level));
            siblings.push(nodes.get(index ^ 1).copied().unwrap_or(empty));
            nodes = nodes
                .chunks(2)
                .map(|pair| {
                    MerkleHashOrchard::combine(
                        Level::from(level),
                        &pair[0],
                        pair.get(1).unwrap_or(&empty),
                    )
                })
                .collect();
            index /= 2;
        }
        Ok(MerklePath::from_parts(
            position as u32,
            siblings
                .try_into()
                .map_err(|_| anyhow::anyhow!("witness depth"))?,
        ))
    }
}

#[cfg(test)]
mod maturity_tests {
    use super::*;

    #[test]
    #[ignore = "real block proof validation plus synthetic distant-height maturity boundaries"]
    fn treasury_is_absent_until_unlock_and_witness_then_matches() {
        let block =
            Block::decode(include_bytes!("../../../tests/vectors/devnet-block-1.bin")).unwrap();
        let mut state = ChainState::new(Network::Devnet)
            .apply_block(&block, block.header.timestamp, &mut ProofCache::default())
            .unwrap();
        let treasury = block.body.treasury.as_ref().unwrap().bundle.actions[0].cmx;
        let miner = block.body.miner.bundle.actions[0].cmx;
        assert!(state.mature.is_empty());
        state.mature_at(2).unwrap();
        assert!(state.mature.is_empty());
        state.mature_at(3).unwrap();
        assert_eq!(state.mature.len(), 1);
        assert_eq!(state.mature[0].cmx, miner);
        state.mature_at(emission::TREASURY_LOCK_HEIGHT - 1).unwrap();
        assert!(state.mature.iter().all(|n| n.cmx != treasury));
        state.mature_at(emission::TREASURY_LOCK_HEIGHT).unwrap();
        let index = state.mature.iter().position(|n| n.cmx == treasury).unwrap();
        let commitment =
            Option::<ExtractedNoteCommitment>::from(ExtractedNoteCommitment::from_bytes(&treasury))
                .unwrap();
        assert_eq!(
            state.witness(index).unwrap().root(commitment).to_bytes(),
            state.notes_root()
        );
    }
}

#[cfg(test)]
mod context_equivalence_tests {
    use super::*;

    fn header(height: u64, timestamp: u64, difficulty: u64) -> Header {
        Header {
            version: 1,
            network: Network::Devnet,
            height,
            previous: [height as u8; 32],
            timestamp,
            difficulty,
            pow_seed: [0; 32],
            nonce: height,
            body_hash: [0; 32],
            notes_root: [0; 32],
            nullifiers_root: [0; 32],
        }
    }

    #[test]
    fn streamed_nullifier_root_matches_the_consensus_formula() {
        let mut state = ChainState::new(Network::Devnet);
        for count in [0usize, 1, 2, 37] {
            while state.nullifiers.len() < count {
                state
                    .nullifiers
                    .insert(codec::hash(b"test", &state.nullifiers.len().to_le_bytes()));
            }
            let concatenated: Vec<u8> = state
                .nullifiers
                .iter()
                .flat_map(|n| n.iter().copied())
                .collect();
            assert_eq!(
                state.nullifiers_root(),
                codec::hash(b"GENORYN/nullifiers/v1", &concatenated)
            );
        }
    }

    #[test]
    fn tail_context_matches_whole_history_context() {
        let mut headers = Vec::new();
        let mut time = 1_000u64;
        for height in 1..=300u64 {
            let full_difficulty = difficulty::next(
                &headers
                    .iter()
                    .map(|h: &Header| (h.timestamp, h.difficulty))
                    .collect::<Vec<_>>(),
                Network::Devnet.initial_difficulty(),
            );
            let full_median =
                difficulty::median_time(&headers.iter().map(|h| h.timestamp).collect::<Vec<_>>());
            assert_eq!(
                expected_difficulty(Network::Devnet, &headers),
                full_difficulty
            );
            assert_eq!(ancestors_median_time(&headers), full_median);
            time += [1, 600, 60, 7, 3600, 59][height as usize % 6];
            headers.push(header(height, time, full_difficulty));
        }
        assert_eq!(pow_seed_index(2048 + 64), None);
        assert_eq!(pow_seed_index(2048 + 65), Some(2047));
        assert_eq!(pow_seed_index(4096 + 64), Some(2047));
        assert_eq!(pow_seed_index(4096 + 65), Some(4095));
    }
}
